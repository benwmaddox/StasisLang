use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use stasis_compiler::IncrementalCompilerHost;

const DEFAULT_FUNCTION_COUNTS: [usize; 1] = [1000];
const DEFAULT_CHUNK_SIZE: usize = 500;
const DEFAULT_SEED: u64 = 1337;
const DEFAULT_COLD_SAMPLES: usize = 1;
const DEFAULT_INCREMENTAL_SAMPLES: usize = 1;

#[derive(Debug, Clone)]
struct BenchConfig {
    function_counts: Vec<usize>,
    chunk_size: usize,
    seed: u64,
    cold_samples: usize,
    incremental_samples: usize,
}

#[derive(Debug, Clone)]
struct ScenarioResult {
    function_count: usize,
    file_count: usize,
    chunk_size: usize,
    seed: u64,
    cold_ms_p50: f64,
    cold_ms_p95: f64,
    incremental_ms_p50: f64,
    incremental_ms_p95: f64,
}

#[derive(Debug, Clone)]
struct FileLayout {
    path: PathBuf,
    start_fn: usize,
    end_fn: usize,
    has_main: bool,
}

fn default_addend(seed: u64, function_index: usize) -> i32 {
    let mixed = seed ^ ((function_index as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    ((mixed % 7) as i32) + 1
}

fn non_default_incremental_addend(default_value: i32, sample: usize) -> i32 {
    let (first, second) = match default_value {
        1 => (2, 3),
        2 => (1, 3),
        _ => (1, 2),
    };
    if sample % 2 == 0 {
        first
    } else {
        second
    }
}

fn render_function_line(function_index: usize, addend: i32) -> String {
    if function_index == 0 {
        return format!("function fn_0(): i32 {{ return {addend}; }}\n");
    }
    format!(
        "function fn_{function_index}(): i32 {{ return fn_{}() + {addend}; }}\n",
        function_index - 1
    )
}

fn render_file_source(
    layout: &FileLayout,
    function_count: usize,
    seed: u64,
    target_edit: Option<(usize, i32)>,
) -> String {
    let mut out = String::new();
    for function_index in layout.start_fn..layout.end_fn {
        let addend = if let Some((target_index, target_value)) = target_edit {
            if target_index == function_index {
                target_value
            } else {
                default_addend(seed, function_index)
            }
        } else {
            default_addend(seed, function_index)
        };
        out.push_str(&render_function_line(function_index, addend));
    }
    if layout.has_main {
        out.push_str(&format!(
            "function main(): i32 {{ return fn_{}(); }}\n",
            function_count.saturating_sub(1)
        ));
    }
    out
}

fn write_project_fixture(
    temp_root: &Path,
    function_count: usize,
    chunk_size: usize,
    seed: u64,
) -> Result<Vec<FileLayout>, String> {
    if function_count == 0 {
        return Err("function_count must be > 0".to_string());
    }
    if chunk_size == 0 {
        return Err("chunk_size must be > 0".to_string());
    }

    fs::create_dir_all(temp_root).map_err(|error| {
        format!(
            "failed creating fixture dir {}: {error}",
            temp_root.display()
        )
    })?;

    let mut layouts = Vec::new();
    let mut start_fn = 0usize;
    let mut file_index = 0usize;
    while start_fn < function_count {
        let end_fn = std::cmp::min(start_fn + chunk_size, function_count);
        let has_main = end_fn == function_count;
        let path = temp_root.join(format!("bench_{file_index:03}.stasis"));
        let layout = FileLayout {
            path: path.clone(),
            start_fn,
            end_fn,
            has_main,
        };
        let source = render_file_source(&layout, function_count, seed, None);
        fs::write(&path, source)
            .map_err(|error| format!("failed writing fixture {}: {error}", path.display()))?;
        layouts.push(layout);
        start_fn = end_fn;
        file_index += 1;
    }

    Ok(layouts)
}

fn percentile_ms(samples: &[Duration], percentile: usize) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let mut sorted = samples.to_vec();
    sorted.sort();
    let rank = ((percentile as f64 / 100.0) * (sorted.len() as f64 - 1.0)).round() as usize;
    let clamped = std::cmp::min(rank, sorted.len() - 1);
    sorted[clamped].as_secs_f64() * 1000.0
}

fn timed_compile(
    host: &mut IncrementalCompilerHost,
    paths: &[PathBuf],
) -> Result<Duration, String> {
    let start = Instant::now();
    let output = host.compile_changed_files(paths)?;
    let elapsed = start.elapsed();
    if output.status != 0 {
        return Err(format!(
            "compile failed with status {} and {} errors",
            output.status,
            output.errors.len()
        ));
    }
    Ok(elapsed)
}

fn run_scenario(
    function_count: usize,
    chunk_size: usize,
    seed: u64,
    cold_samples: usize,
    incremental_samples: usize,
) -> Result<ScenarioResult, String> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("clock error: {error}"))?
        .as_nanos();
    let temp_root = env::temp_dir().join(format!("stasis_compiler_bench_{function_count}_{stamp}"));
    let layouts = write_project_fixture(&temp_root, function_count, chunk_size, seed)?;
    let all_paths: Vec<PathBuf> = layouts.iter().map(|layout| layout.path.clone()).collect();

    let target_function = std::cmp::max(1, function_count / 2);
    let target_layout_index = layouts
        .iter()
        .position(|layout| target_function >= layout.start_fn && target_function < layout.end_fn)
        .ok_or_else(|| {
            format!("failed finding target function {target_function} in file layouts")
        })?;
    let target_layout = layouts[target_layout_index].clone();
    let target_default_addend = default_addend(seed, target_function);

    let mut cold_times = Vec::new();
    for _ in 0..cold_samples {
        let mut host = IncrementalCompilerHost::new();
        cold_times.push(timed_compile(&mut host, &all_paths)?);
    }

    let mut host = IncrementalCompilerHost::new();
    timed_compile(&mut host, &all_paths)?;

    let mut incremental_times = Vec::new();
    for sample in 0..incremental_samples {
        let override_value = non_default_incremental_addend(target_default_addend, sample);
        let source = render_file_source(
            &target_layout,
            function_count,
            seed,
            Some((target_function, override_value)),
        );
        fs::write(&target_layout.path, source).map_err(|error| {
            format!(
                "failed writing incremental fixture {}: {error}",
                target_layout.path.display()
            )
        })?;
        incremental_times.push(timed_compile(
            &mut host,
            std::slice::from_ref(&target_layout.path),
        )?);
    }

    fs::remove_dir_all(&temp_root).ok();

    Ok(ScenarioResult {
        function_count,
        file_count: layouts.len(),
        chunk_size,
        seed,
        cold_ms_p50: percentile_ms(&cold_times, 50),
        cold_ms_p95: percentile_ms(&cold_times, 95),
        incremental_ms_p50: percentile_ms(&incremental_times, 50),
        incremental_ms_p95: percentile_ms(&incremental_times, 95),
    })
}

fn parse_usize_csv(value: &str) -> Result<Vec<usize>, String> {
    let mut out = Vec::new();
    for item in value.split(',') {
        let trimmed = item.trim();
        if trimmed.is_empty() {
            continue;
        }
        let parsed: usize = trimmed
            .parse()
            .map_err(|error| format!("invalid usize value '{trimmed}': {error}"))?;
        out.push(parsed);
    }
    if out.is_empty() {
        return Err("expected at least one numeric value".to_string());
    }
    Ok(out)
}

fn parse_args() -> Result<BenchConfig, String> {
    let mut config = BenchConfig {
        function_counts: DEFAULT_FUNCTION_COUNTS.to_vec(),
        chunk_size: DEFAULT_CHUNK_SIZE,
        seed: DEFAULT_SEED,
        cold_samples: DEFAULT_COLD_SAMPLES,
        incremental_samples: DEFAULT_INCREMENTAL_SAMPLES,
    };

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--functions" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--functions requires a value".to_string())?;
                config.function_counts = parse_usize_csv(&value)?;
            }
            "--chunk-size" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--chunk-size requires a value".to_string())?;
                config.chunk_size = value
                    .parse()
                    .map_err(|error| format!("invalid --chunk-size value: {error}"))?;
            }
            "--seed" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--seed requires a value".to_string())?;
                config.seed = value
                    .parse()
                    .map_err(|error| format!("invalid --seed value: {error}"))?;
            }
            "--cold-samples" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--cold-samples requires a value".to_string())?;
                config.cold_samples = value
                    .parse()
                    .map_err(|error| format!("invalid --cold-samples value: {error}"))?;
            }
            "--incremental-samples" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--incremental-samples requires a value".to_string())?;
                config.incremental_samples = value
                    .parse()
                    .map_err(|error| format!("invalid --incremental-samples value: {error}"))?;
            }
            "--help" | "-h" => {
                println!("stasis compiler benchmark");
                println!("  --functions <csv>            default: 1000");
                println!("  --chunk-size <n>             default: 500");
                println!("  --seed <u64>                 default: 1337");
                println!("  --cold-samples <n>           default: 1");
                println!("  --incremental-samples <n>    default: 1");
                std::process::exit(0);
            }
            _ => return Err(format!("unknown argument: {arg}")),
        }
    }

    if config.function_counts.iter().any(|count| *count == 0) {
        return Err("--functions entries must be > 0".to_string());
    }
    if config.chunk_size == 0 {
        return Err("--chunk-size must be > 0".to_string());
    }
    if config.cold_samples == 0 {
        return Err("--cold-samples must be > 0".to_string());
    }
    if config.incremental_samples == 0 {
        return Err("--incremental-samples must be > 0".to_string());
    }

    Ok(config)
}

fn main() {
    let config = match parse_args() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("error: {error}");
            std::process::exit(2);
        }
    };

    println!("STASIS_COMPILER_BENCH_V1");
    println!(
        "config functions={:?} chunk_size={} seed={} cold_samples={} incremental_samples={}",
        config.function_counts,
        config.chunk_size,
        config.seed,
        config.cold_samples,
        config.incremental_samples
    );

    for function_count in &config.function_counts {
        let result = match run_scenario(
            *function_count,
            config.chunk_size,
            config.seed,
            config.cold_samples,
            config.incremental_samples,
        ) {
            Ok(result) => result,
            Err(error) => {
                eprintln!("error: scenario {} failed: {error}", function_count);
                std::process::exit(1);
            }
        };

        println!(
            "BENCH_RESULT {{\"functions\":{},\"files\":{},\"chunk_size\":{},\"seed\":{},\"cold_ms_p50\":{:.3},\"cold_ms_p95\":{:.3},\"incremental_ms_p50\":{:.3},\"incremental_ms_p95\":{:.3}}}",
            result.function_count,
            result.file_count,
            result.chunk_size,
            result.seed,
            result.cold_ms_p50,
            result.cold_ms_p95,
            result.incremental_ms_p50,
            result.incremental_ms_p95
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{default_addend, non_default_incremental_addend, parse_usize_csv, percentile_ms};
    use std::time::Duration;

    #[test]
    fn default_addend_is_deterministic() {
        assert_eq!(default_addend(1337, 42), default_addend(1337, 42));
        assert_ne!(default_addend(1337, 42), default_addend(1338, 42));
    }

    #[test]
    fn parse_usize_csv_works() {
        let parsed = parse_usize_csv("1000, 5000,32").expect("parse csv");
        assert_eq!(parsed, vec![1000, 5000, 32]);
    }

    #[test]
    fn percentile_ms_orders_correctly() {
        let samples = vec![
            Duration::from_millis(10),
            Duration::from_millis(20),
            Duration::from_millis(30),
            Duration::from_millis(40),
        ];
        assert_eq!(percentile_ms(&samples, 50), 30.0);
        assert_eq!(percentile_ms(&samples, 95), 40.0);
    }

    #[test]
    fn non_default_incremental_addend_differs_from_default() {
        for default in [1, 2, 3] {
            let even = non_default_incremental_addend(default, 0);
            let odd = non_default_incremental_addend(default, 1);
            assert_ne!(even, default);
            assert_ne!(odd, default);
            assert_ne!(even, odd);
        }
    }
}
