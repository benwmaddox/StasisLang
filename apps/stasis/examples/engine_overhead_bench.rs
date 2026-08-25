use std::env;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use stasis::{run_with_real_backend, RunnerConfig};
use stasis_runner::swap::contracts::TargetMode;

const DEFAULT_SAMPLES: usize = 3;
const DEFAULT_TICKS: u32 = 240;
const DEFAULT_TICK_SLEEP_US: u64 = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModeArg {
    Jit,
    Aot,
    Both,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BenchMode {
    Jit,
    Aot,
}

impl BenchMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Jit => "jit",
            Self::Aot => "aot",
        }
    }

    fn target_mode(self) -> TargetMode {
        match self {
            Self::Jit => TargetMode::JitDev,
            Self::Aot => TargetMode::AotProd,
        }
    }
}

#[derive(Debug, Clone)]
struct BenchConfig {
    mode: ModeArg,
    samples: usize,
    ticks: u32,
    tick_sleep_us: u64,
}

#[derive(Debug, Clone, Copy)]
struct SampleMetrics {
    total: Duration,
    compile_ms: f64,
    commit_ms: f64,
    runtime_overhead_ms: f64,
    load: Option<Duration>,
}

#[derive(Debug, Clone, Copy)]
struct AggregatedMetrics {
    total_p50: f64,
    total_p95: f64,
    compile_p50: f64,
    compile_p95: f64,
    commit_p50: f64,
    commit_p95: f64,
    runtime_overhead_p50: f64,
    runtime_overhead_p95: f64,
    load_p50: Option<f64>,
    load_p95: Option<f64>,
}

fn parse_mode(value: &str) -> Result<ModeArg, String> {
    match value {
        "jit" => Ok(ModeArg::Jit),
        "aot" => Ok(ModeArg::Aot),
        "both" => Ok(ModeArg::Both),
        _ => Err(format!("invalid --mode '{value}' (expected jit|aot|both)")),
    }
}

fn parse_args() -> Result<BenchConfig, String> {
    let mut config = BenchConfig {
        mode: ModeArg::Both,
        samples: DEFAULT_SAMPLES,
        ticks: DEFAULT_TICKS,
        tick_sleep_us: DEFAULT_TICK_SLEEP_US,
    };
    let args: Vec<String> = env::args().skip(1).collect();
    let mut index = 0usize;
    while index < args.len() {
        match args[index].as_str() {
            "--mode" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err("--mode requires a value".to_string());
                };
                config.mode = parse_mode(value)?;
            }
            "--samples" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err("--samples requires a value".to_string());
                };
                config.samples = value
                    .parse::<usize>()
                    .map_err(|error| format!("invalid --samples '{value}': {error}"))?;
            }
            "--ticks" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err("--ticks requires a value".to_string());
                };
                config.ticks = value
                    .parse::<u32>()
                    .map_err(|error| format!("invalid --ticks '{value}': {error}"))?;
            }
            "--tick-sleep-us" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err("--tick-sleep-us requires a value".to_string());
                };
                config.tick_sleep_us = value
                    .parse::<u64>()
                    .map_err(|error| format!("invalid --tick-sleep-us '{value}': {error}"))?;
            }
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            unknown => return Err(format!("unknown argument '{unknown}'")),
        }
        index += 1;
    }

    if config.samples == 0 {
        return Err("--samples must be > 0".to_string());
    }
    if config.ticks == 0 {
        return Err("--ticks must be > 0".to_string());
    }
    Ok(config)
}

fn print_help() {
    println!("stasis engine overhead benchmark");
    println!("  --mode <jit|aot|both>      default: both");
    println!("  --samples <usize>          default: 3");
    println!("  --ticks <u32>              default: 240");
    println!("  --tick-sleep-us <u64>      default: 0");
}

fn render_fixture_source() -> &'static str {
    "function tick(): i32 { return 0; }\nfunction render(): i32 { return 0; }\nfunction on_code_swap(): void { return; }\n"
}

fn write_fixture(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create fixture directory {}: {error}",
                parent.display()
            )
        })?;
    }
    fs::write(path, render_fixture_source())
        .map_err(|error| format!("failed to write fixture {}: {error}", path.display()))
}

fn run_sample(mode: BenchMode, ticks: u32, tick_sleep_us: u64) -> Result<SampleMetrics, String> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("clock error: {error}"))?
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "stasis_engine_overhead_{}_{}_{}",
        mode.as_str(),
        std::process::id(),
        stamp
    ));
    let source_path = temp_root.join("engine_bench.stasis");
    write_fixture(&source_path)?;

    let config = RunnerConfig {
        max_ticks: ticks,
        tick_sleep_micros: tick_sleep_us,
        window: None,
        inject_file_change: Some(source_path.clone()),
        watch_directory: None,
        target_mode: mode.target_mode(),
        fail_compile: false,
        disable_on_code_swap_hook: false,
        hook_failure_reason: None,
        swap_failure_reason: None,
        runtime_launch: false,
        aot_probe_loadability: false,
        host_set_profile: None,
        host_set_registry_file: None,
    };

    let start = Instant::now();
    let summary = run_with_real_backend(config);
    let total = start.elapsed();

    let _ = fs::remove_dir_all(&temp_root);

    if summary.compile_successes == 0 || summary.compile_failures > 0 {
        return Err(format!(
            "expected successful compile in {} mode, got successes={} failures={} diagnostics={:?}",
            mode.as_str(),
            summary.compile_successes,
            summary.compile_failures,
            summary.compile_diagnostics
        ));
    }
    if summary.swap_commit_successes == 0 || summary.swap_commit_failures > 0 {
        return Err(format!(
            "expected successful swap commit in {} mode, got successes={} failures={} reasons={:?}",
            mode.as_str(),
            summary.swap_commit_successes,
            summary.swap_commit_failures,
            summary.swap_failure_reasons
        ));
    }

    let compile_ms = summary.last_compile_duration_ms.unwrap_or(0) as f64;
    let commit_ms = summary.last_commit_duration_ms.unwrap_or(0) as f64;
    let total_ms = total.as_secs_f64() * 1000.0;
    let runtime_overhead_ms = (total_ms - compile_ms - commit_ms).max(0.0);

    let load = if mode == BenchMode::Aot {
        if let Some(path) = summary.active_aot_linked_image_path {
            let read_start = Instant::now();
            fs::read(&path).map_err(|error| {
                format!(
                    "failed to read AOT linked-image artifact {} for load timing: {error}",
                    path.display()
                )
            })?;
            Some(read_start.elapsed())
        } else {
            None
        }
    } else {
        None
    };

    Ok(SampleMetrics {
        total,
        compile_ms,
        commit_ms,
        runtime_overhead_ms,
        load,
    })
}

fn percentile_ms(samples: &[f64], percentile: usize) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let rank = ((percentile as f64 / 100.0) * (sorted.len() as f64 - 1.0)).round() as usize;
    sorted[rank.min(sorted.len() - 1)]
}

fn aggregate(samples: &[SampleMetrics]) -> AggregatedMetrics {
    let total_ms: Vec<f64> = samples
        .iter()
        .map(|sample| sample.total.as_secs_f64() * 1000.0)
        .collect();
    let compile_ms: Vec<f64> = samples.iter().map(|sample| sample.compile_ms).collect();
    let commit_ms: Vec<f64> = samples.iter().map(|sample| sample.commit_ms).collect();
    let runtime_overhead_ms: Vec<f64> = samples
        .iter()
        .map(|sample| sample.runtime_overhead_ms)
        .collect();
    let load_ms: Vec<f64> = samples
        .iter()
        .filter_map(|sample| sample.load.map(|duration| duration.as_secs_f64() * 1000.0))
        .collect();

    AggregatedMetrics {
        total_p50: percentile_ms(&total_ms, 50),
        total_p95: percentile_ms(&total_ms, 95),
        compile_p50: percentile_ms(&compile_ms, 50),
        compile_p95: percentile_ms(&compile_ms, 95),
        commit_p50: percentile_ms(&commit_ms, 50),
        commit_p95: percentile_ms(&commit_ms, 95),
        runtime_overhead_p50: percentile_ms(&runtime_overhead_ms, 50),
        runtime_overhead_p95: percentile_ms(&runtime_overhead_ms, 95),
        load_p50: (!load_ms.is_empty()).then(|| percentile_ms(&load_ms, 50)),
        load_p95: (!load_ms.is_empty()).then(|| percentile_ms(&load_ms, 95)),
    }
}

fn run_mode(mode: BenchMode, config: &BenchConfig) -> Result<AggregatedMetrics, String> {
    let mut samples = Vec::with_capacity(config.samples);
    for _ in 0..config.samples {
        samples.push(run_sample(mode, config.ticks, config.tick_sleep_us)?);
    }
    Ok(aggregate(&samples))
}

fn print_mode_result(mode: BenchMode, metrics: AggregatedMetrics) {
    print!(
        "result mode={} total_ms_p50={:.3} total_ms_p95={:.3} compile_ms_p50={:.3} compile_ms_p95={:.3} commit_ms_p50={:.3} commit_ms_p95={:.3} runtime_overhead_ms_p50={:.3} runtime_overhead_ms_p95={:.3}",
        mode.as_str(),
        metrics.total_p50,
        metrics.total_p95,
        metrics.compile_p50,
        metrics.compile_p95,
        metrics.commit_p50,
        metrics.commit_p95,
        metrics.runtime_overhead_p50,
        metrics.runtime_overhead_p95
    );
    if let (Some(load_p50), Some(load_p95)) = (metrics.load_p50, metrics.load_p95) {
        print!(
            " load_artifact_ms_p50={:.3} load_artifact_ms_p95={:.3}",
            load_p50, load_p95
        );
    }
    println!();
}

fn main() -> Result<(), String> {
    let config = parse_args()?;
    println!(
        "bench engine_overhead mode={:?} samples={} ticks={} tick_sleep_us={}",
        config.mode, config.samples, config.ticks, config.tick_sleep_us
    );

    match config.mode {
        ModeArg::Jit => {
            let metrics = run_mode(BenchMode::Jit, &config)?;
            print_mode_result(BenchMode::Jit, metrics);
        }
        ModeArg::Aot => {
            let metrics = run_mode(BenchMode::Aot, &config)?;
            print_mode_result(BenchMode::Aot, metrics);
        }
        ModeArg::Both => {
            let jit = run_mode(BenchMode::Jit, &config)?;
            print_mode_result(BenchMode::Jit, jit);
            let aot = run_mode(BenchMode::Aot, &config)?;
            print_mode_result(BenchMode::Aot, aot);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_mode_accepts_valid_values() {
        assert_eq!(parse_mode("jit").expect("jit"), ModeArg::Jit);
        assert_eq!(parse_mode("aot").expect("aot"), ModeArg::Aot);
        assert_eq!(parse_mode("both").expect("both"), ModeArg::Both);
    }

    #[test]
    fn percentile_ms_returns_expected_value() {
        let samples = vec![1.0, 3.0, 2.0, 8.0];
        assert_eq!(percentile_ms(&samples, 50), 3.0);
        assert_eq!(percentile_ms(&samples, 95), 8.0);
    }
}
