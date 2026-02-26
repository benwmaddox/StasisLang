use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use stasis_compiler::backend::jit::JitProcess;
use stasis_compiler::IncrementalCompilerHost;

const DEFAULT_FUNCTION_COUNTS: [usize; 1] = [1000];
const DEFAULT_CHUNK_SIZE: usize = 500;
const DEFAULT_SEED: u64 = 1337;
const DEFAULT_COLD_SAMPLES: usize = 1;
const DEFAULT_INCREMENTAL_SAMPLES: usize = 1;
const DEFAULT_MODE: BenchMode = BenchMode::Jit;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BenchMode {
    Analysis,
    Jit,
}

impl BenchMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Analysis => "analysis",
            Self::Jit => "jit",
        }
    }
}

#[derive(Debug, Clone)]
struct BenchConfig {
    mode: BenchMode,
    function_counts: Vec<usize>,
    chunk_size: usize,
    seed: u64,
    cold_samples: usize,
    incremental_samples: usize,
}

#[derive(Debug, Clone)]
struct ScenarioResult {
    mode: BenchMode,
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

fn is_export_function(function_index: usize, function_count: usize) -> bool {
    if function_count < 10 {
        return true;
    }
    function_index % 10 != 0
}

fn select_export_target(function_count: usize) -> usize {
    for index in (0..function_count).rev() {
        if is_export_function(index, function_count) {
            return index;
        }
    }
    0
}

fn render_function_line(function_index: usize, addend: i32, is_export: bool) -> String {
    let scale = (function_index % 5) as i32 + 2;
    let bias = (function_index % 3) as i32 - 1;
    let keyword = if is_export { "export " } else { "" };
    format!(
        "{keyword}function fn_{function_index}(): i32 {{ return ({addend} * {scale}) + {bias}; }}\n"
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
        out.push_str(&render_function_line(
            function_index,
            addend,
            is_export_function(function_index, function_count),
        ));
    }
    if layout.has_main {
        let target_function = select_export_target(function_count);
        out.push_str(&format!(
            "function main(): i32 {{ return fn_{}(); }}\n",
            target_function
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

fn compile_error_message(error: stasis_compiler::compiler::CompileError) -> String {
    match error {
        stasis_compiler::compiler::CompileError::Frontend(message)
        | stasis_compiler::compiler::CompileError::Backend(message)
        | stasis_compiler::compiler::CompileError::Invariant(message) => message,
    }
}

fn timed_compile_jit(
    process: &mut JitProcess,
    expected_functions: usize,
) -> Result<Duration, String> {
    let start = Instant::now();
    process.compile().map_err(compile_error_message)?;
    let elapsed = start.elapsed();
    if process.artifacts().len() != expected_functions {
        return Err(format!(
            "jit compile artifact count mismatch: expected {expected_functions}, got {}",
            process.artifacts().len()
        ));
    }
    if process
        .artifacts()
        .iter()
        .any(|artifact| artifact.code_ptr == 0)
    {
        return Err("jit compile produced zero code pointer artifact".to_string());
    }
    Ok(elapsed)
}

fn run_scenario_analysis(
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

    // Edit the function referenced by main so incremental timing reflects one changed reachable
    // function without chain-dependent ripple.
    let target_function = select_export_target(function_count);
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
        mode: BenchMode::Analysis,
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

fn run_scenario_jit(
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
    let temp_root = env::temp_dir().join(format!(
        "stasis_compiler_bench_jit_{function_count}_{stamp}"
    ));
    let layouts = write_project_fixture(&temp_root, function_count, chunk_size, seed)?;
    let expected_functions = function_count + 1; // generated fns + main

    let mut source_by_path: Vec<(PathBuf, String)> = Vec::with_capacity(layouts.len());
    for layout in &layouts {
        let source = fs::read_to_string(&layout.path).map_err(|error| {
            format!(
                "failed reading generated fixture {}: {error}",
                layout.path.display()
            )
        })?;
        source_by_path.push((layout.path.clone(), source));
    }

    let target_function = select_export_target(function_count);
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
        let mut process = JitProcess::new();
        for (path, source) in &source_by_path {
            process.upsert_file(path.to_string_lossy().to_string(), source.clone());
        }
        cold_times.push(timed_compile_jit(&mut process, expected_functions)?);
    }

    let mut process = JitProcess::new();
    for (path, source) in &source_by_path {
        process.upsert_file(path.to_string_lossy().to_string(), source.clone());
    }
    timed_compile_jit(&mut process, expected_functions)?;

    let mut incremental_times = Vec::new();
    for sample in 0..incremental_samples {
        let override_value = non_default_incremental_addend(target_default_addend, sample);
        let updated_source = render_file_source(
            &target_layout,
            function_count,
            seed,
            Some((target_function, override_value)),
        );
        fs::write(&target_layout.path, &updated_source).map_err(|error| {
            format!(
                "failed writing incremental fixture {}: {error}",
                target_layout.path.display()
            )
        })?;
        source_by_path[target_layout_index].1 = updated_source.clone();
        process.upsert_file(
            target_layout.path.to_string_lossy().to_string(),
            updated_source,
        );
        incremental_times.push(timed_compile_jit(&mut process, expected_functions)?);
    }

    fs::remove_dir_all(&temp_root).ok();

    Ok(ScenarioResult {
        mode: BenchMode::Jit,
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

fn run_scenario(
    mode: BenchMode,
    function_count: usize,
    chunk_size: usize,
    seed: u64,
    cold_samples: usize,
    incremental_samples: usize,
) -> Result<ScenarioResult, String> {
    match mode {
        BenchMode::Analysis => run_scenario_analysis(
            function_count,
            chunk_size,
            seed,
            cold_samples,
            incremental_samples,
        ),
        BenchMode::Jit => run_scenario_jit(
            function_count,
            chunk_size,
            seed,
            cold_samples,
            incremental_samples,
        ),
    }
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
        mode: DEFAULT_MODE,
        function_counts: DEFAULT_FUNCTION_COUNTS.to_vec(),
        chunk_size: DEFAULT_CHUNK_SIZE,
        seed: DEFAULT_SEED,
        cold_samples: DEFAULT_COLD_SAMPLES,
        incremental_samples: DEFAULT_INCREMENTAL_SAMPLES,
    };

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--mode" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--mode requires a value".to_string())?;
                config.mode = match value.as_str() {
                    "analysis" => BenchMode::Analysis,
                    "jit" => BenchMode::Jit,
                    _ => {
                        return Err(format!(
                            "invalid --mode value '{value}' (expected: analysis|jit)"
                        ))
                    }
                };
            }
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
                println!("  --mode <analysis|jit>        default: jit");
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
        "config mode={} functions={:?} chunk_size={} seed={} cold_samples={} incremental_samples={}",
        config.mode.as_str(),
        config.function_counts,
        config.chunk_size,
        config.seed,
        config.cold_samples,
        config.incremental_samples
    );

    for function_count in &config.function_counts {
        let result = match run_scenario(
            config.mode,
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
            "BENCH_RESULT {{\"mode\":\"{}\",\"functions\":{},\"files\":{},\"chunk_size\":{},\"seed\":{},\"cold_ms_p50\":{:.3},\"cold_ms_p95\":{:.3},\"incremental_ms_p50\":{:.3},\"incremental_ms_p95\":{:.3}}}",
            result.mode.as_str(),
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
    use super::{
        default_addend, is_export_function, non_default_incremental_addend, parse_usize_csv,
        percentile_ms, select_export_target,
    };
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

    #[test]
    fn export_ratio_is_ninety_ten_for_thousand_and_five_thousand() {
        for function_count in [1000usize, 5000usize] {
            let exported = (0..function_count)
                .filter(|index| is_export_function(*index, function_count))
                .count();
            let non_exported = function_count - exported;
            assert_eq!(exported * 10, function_count * 9);
            assert_eq!(non_exported * 10, function_count);
        }
    }

    #[test]
    fn selected_incremental_target_is_exported() {
        for function_count in [1usize, 10usize, 1000usize, 5000usize] {
            let target = select_export_target(function_count);
            assert!(is_export_function(target, function_count));
            assert!(target < function_count);
        }
    }
}
