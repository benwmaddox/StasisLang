use std::env;
use std::time::{Duration, Instant};

use stasis_compiler::backend::jit::JitProcess;

const DEFAULT_FUNCTION_COUNTS: [usize; 2] = [1000, 5000];
const DEFAULT_SEED: u64 = 1337;
const DEFAULT_COLD_SAMPLES: usize = 3;
const DEFAULT_INCREMENTAL_SAMPLES: usize = 5;
const WARMUP_SAMPLES: usize = 5;

#[derive(Debug, Clone)]
struct BenchConfig {
    function_counts: Vec<usize>,
    seed: u64,
    cold_samples: usize,
    incremental_samples: usize,
}

#[derive(Debug, Clone)]
struct ScenarioResult {
    function_count: usize,
    seed: u64,
    cold_ms_p50: f64,
    cold_ms_p95: f64,
    incremental_ms_p50: f64,
    incremental_ms_p95: f64,
    plan_ms_p50: f64,
    plan_ms_p95: f64,
    codegen_ms_p50: f64,
    codegen_ms_p95: f64,
    finalize_ms_p50: f64,
    finalize_ms_p95: f64,
    emitted_functions: usize,
}

fn default_value(seed: u64, function_index: usize) -> i32 {
    let mixed = seed ^ ((function_index as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    ((mixed % 101) as i32) - 50
}

fn non_default_incremental_value(default_value: i32, sample: usize) -> i32 {
    let alt = if default_value == i32::MAX {
        i32::MAX - 1
    } else {
        default_value + 1
    };
    if sample % 2 == 0 {
        alt
    } else {
        default_value
    }
}

fn render_function_line(function_index: usize, value: i32) -> String {
    format!("function fn_{function_index}(): i32 {{ return {value}; }}\n")
}

fn render_function_line_with_call(
    function_index: usize,
    callee_index: usize,
    value: i32,
) -> String {
    format!("function fn_{function_index}(): i32 {{ return fn_{callee_index}() + {value}; }}\n")
}

fn render_source(function_count: usize, seed: u64, target_edit: Option<(usize, i32)>) -> String {
    let mut out = String::new();
    for function_index in 0..function_count {
        let value = if let Some((target_index, target_value)) = target_edit {
            if target_index == function_index {
                target_value
            } else {
                default_value(seed, function_index)
            }
        } else {
            default_value(seed, function_index)
        };

        // Keep all functions reachable under reachability-gated emission (S10b+) by threading a
        // simple call chain through fn_0 -> fn_1 -> ... -> fn_{N-1}. Only the last function is a
        // literal return.
        if function_index + 1 < function_count {
            out.push_str(&render_function_line_with_call(
                function_index,
                function_index + 1,
                value,
            ));
        } else {
            out.push_str(&render_function_line(function_index, value));
        }
    }
    // Keep a `main` function so this fixture can be reused for broader runtime checks.
    out.push_str("function main(): i32 { return fn_0(); }\n");
    out
}

fn percentile_ms(samples: &[Duration], percentile: usize) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let mut sorted = samples.to_vec();
    sorted.sort();
    let rank = (percentile * sorted.len()).div_ceil(100).saturating_sub(1);
    sorted[rank.min(sorted.len() - 1)].as_secs_f64() * 1000.0
}

fn timed_compile(process: &mut JitProcess) -> Result<Duration, String> {
    let start = Instant::now();
    let report = process.compile().map_err(|error| format!("{error:?}"))?;
    if report.emit.emitted_functions == 0 {
        return Err("compile emitted zero functions unexpectedly".to_string());
    }
    Ok(start.elapsed())
}

fn run_scenario(
    function_count: usize,
    seed: u64,
    cold_samples: usize,
    incremental_samples: usize,
) -> Result<ScenarioResult, String> {
    if function_count == 0 {
        return Err("function_count must be > 0".to_string());
    }
    if cold_samples == 0 || incremental_samples == 0 {
        return Err("sample counts must be > 0".to_string());
    }

    let source = render_source(function_count, seed, None);
    let mut cold_times = Vec::with_capacity(cold_samples);
    for _ in 0..cold_samples {
        let mut process = JitProcess::new();
        process.upsert_file("bench.stasis", source.clone());
        cold_times.push(timed_compile(&mut process)?);
    }

    // Editing the top of the chain changes only fn_0 and its main caller. Editing the tail is the
    // intentionally broad counter-case and is covered by the correctness matrix.
    let target_function = 0;
    let default_target_value = default_value(seed, target_function);
    let mut process = JitProcess::new();
    process.upsert_file("bench.stasis", source);
    let initial = process.compile().map_err(|error| format!("{error:?}"))?;
    if initial.emit.emitted_functions == 0 {
        return Err("initial compile emitted zero functions unexpectedly".to_string());
    }

    let mut incremental_times = Vec::with_capacity(incremental_samples);
    let mut plan_times = Vec::with_capacity(incremental_samples);
    let mut codegen_times = Vec::with_capacity(incremental_samples);
    let mut finalize_times = Vec::with_capacity(incremental_samples);
    let mut edited = false;
    for sample in 0..(WARMUP_SAMPLES + incremental_samples) {
        edited = !edited;
        let replacement = if edited {
            non_default_incremental_value(default_target_value, 0)
        } else {
            default_target_value
        };
        let updated_source =
            render_source(function_count, seed, Some((target_function, replacement)));
        process.upsert_file("bench.stasis", updated_source);
        let start = Instant::now();
        let report = process.compile().map_err(|error| format!("{error:?}"))?;
        let elapsed = start.elapsed();
        let expected_patch_functions = 2;
        if report.emit.emitted_functions != expected_patch_functions {
            return Err(format!(
                "expected {expected_patch_functions} emitted functions for selective chain-root update, got {}",
                report.emit.emitted_functions,
            ));
        }
        let metadata = process
            .generation_metadata()
            .ok_or_else(|| "selective update metadata missing".to_string())?;
        if sample >= WARMUP_SAMPLES {
            plan_times.push(Duration::from_micros(metadata.plan_micros));
            codegen_times.push(Duration::from_micros(metadata.codegen_micros));
            finalize_times.push(Duration::from_micros(metadata.finalize_micros));
            incremental_times.push(elapsed);
        }
    }

    Ok(ScenarioResult {
        function_count,
        seed,
        cold_ms_p50: percentile_ms(&cold_times, 50),
        cold_ms_p95: percentile_ms(&cold_times, 95),
        incremental_ms_p50: percentile_ms(&incremental_times, 50),
        incremental_ms_p95: percentile_ms(&incremental_times, 95),
        plan_ms_p50: percentile_ms(&plan_times, 50),
        plan_ms_p95: percentile_ms(&plan_times, 95),
        codegen_ms_p50: percentile_ms(&codegen_times, 50),
        codegen_ms_p95: percentile_ms(&codegen_times, 95),
        finalize_ms_p50: percentile_ms(&finalize_times, 50),
        finalize_ms_p95: percentile_ms(&finalize_times, 95),
        emitted_functions: 2,
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
        seed: DEFAULT_SEED,
        cold_samples: DEFAULT_COLD_SAMPLES,
        incremental_samples: DEFAULT_INCREMENTAL_SAMPLES,
    };

    let args: Vec<String> = env::args().skip(1).collect();
    let mut index = 0usize;
    while index < args.len() {
        match args[index].as_str() {
            "--functions" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err("--functions requires a comma-separated value".to_string());
                };
                config.function_counts = parse_usize_csv(value)?;
            }
            "--seed" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err("--seed requires a value".to_string());
                };
                config.seed = value
                    .parse::<u64>()
                    .map_err(|error| format!("invalid --seed '{value}': {error}"))?;
            }
            "--cold-samples" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err("--cold-samples requires a value".to_string());
                };
                config.cold_samples = value
                    .parse::<usize>()
                    .map_err(|error| format!("invalid --cold-samples '{value}': {error}"))?;
            }
            "--incremental-samples" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err("--incremental-samples requires a value".to_string());
                };
                config.incremental_samples = value
                    .parse::<usize>()
                    .map_err(|error| format!("invalid --incremental-samples '{value}': {error}"))?;
            }
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            unknown => {
                return Err(format!("unknown argument '{unknown}'"));
            }
        }
        index += 1;
    }

    Ok(config)
}

fn print_help() {
    println!("Rust-native JIT benchmark");
    println!("  --functions <csv>           default: 1000,5000");
    println!("  --seed <u64>                default: 1337");
    println!("  --cold-samples <usize>      default: 3");
    println!("  --incremental-samples <usize> default: 5");
}

fn main() -> Result<(), String> {
    let config = parse_args()?;
    println!(
        "bench rust-native jit functions={:?} seed={} cold_samples={} warmups={} incremental_samples={}",
        config.function_counts,
        config.seed,
        config.cold_samples,
        WARMUP_SAMPLES,
        config.incremental_samples
    );

    for function_count in &config.function_counts {
        let result = run_scenario(
            *function_count,
            config.seed,
            config.cold_samples,
            config.incremental_samples,
        )?;
        println!(
            "result topology=chain_root functions={} seed={} reachable_functions={} emitted_functions={} reused_functions={} cold_ms_p50={:.3} cold_ms_p95={:.3} selective_update_ms_p50={:.3} selective_update_ms_p95={:.3} plan_ms_p50={:.3} plan_ms_p95={:.3} codegen_ms_p50={:.3} codegen_ms_p95={:.3} finalize_ms_p50={:.3} finalize_ms_p95={:.3}",
            result.function_count,
            result.seed,
            result.function_count + 1,
            result.emitted_functions,
            result.function_count + 1 - result.emitted_functions,
            result.cold_ms_p50,
            result.cold_ms_p95,
            result.incremental_ms_p50,
            result.incremental_ms_p95,
            result.plan_ms_p50,
            result.plan_ms_p95,
            result.codegen_ms_p50,
            result.codegen_ms_p95,
            result.finalize_ms_p50,
            result.finalize_ms_p95
        );
    }

    Ok(())
}
