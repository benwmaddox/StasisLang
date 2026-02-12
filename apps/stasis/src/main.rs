use std::env;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::path::PathBuf;

use stasis::{
    run_with_default_backend, scenarios::brickout_revenge_v1_runner_config, RunnerConfig,
};
use stasis_runner::swap::contracts::TargetMode;

struct CliOptions {
    runner: RunnerConfig,
    emit_events_jsonl: bool,
    events_jsonl_file: Option<PathBuf>,
}

fn parse_args() -> CliOptions {
    let mut config = RunnerConfig::default();
    let mut emit_events_jsonl = false;
    let mut events_jsonl_file: Option<PathBuf> = None;
    let mut args = env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--ticks" => {
                if let Some(value) = args.next() {
                    if let Ok(parsed) = value.parse::<u32>() {
                        config.max_ticks = parsed;
                    }
                }
            }
            "--tick-sleep-us" => {
                if let Some(value) = args.next() {
                    if let Ok(parsed) = value.parse::<u64>() {
                        config.tick_sleep_micros = parsed;
                    }
                }
            }
            "--watch-file" => {
                if let Some(value) = args.next() {
                    config.inject_file_change = Some(PathBuf::from(value));
                }
            }
            "--watch-dir" => {
                if let Some(value) = args.next() {
                    config.watch_directory = Some(PathBuf::from(value));
                }
            }
            "--target-mode" => {
                if let Some(value) = args.next() {
                    if value.eq_ignore_ascii_case("jit") || value.eq_ignore_ascii_case("jit-dev") {
                        config.target_mode = TargetMode::JitDev;
                    } else if value.eq_ignore_ascii_case("aot")
                        || value.eq_ignore_ascii_case("aot-prod")
                    {
                        config.target_mode = TargetMode::AotProd;
                    }
                }
            }
            "--aot-prod" => {
                config.target_mode = TargetMode::AotProd;
            }
            "--fail-compile" => {
                config.fail_compile = true;
            }
            "--no-hook" => {
                config.disable_on_code_swap_hook = true;
            }
            "--fail-hook" => {
                config.hook_failure_reason = Some("simulated on_code_swap failure".to_string());
            }
            "--fail-hook-reason" => {
                if let Some(value) = args.next() {
                    config.hook_failure_reason = Some(value);
                }
            }
            "--fail-swap" => {
                config.swap_failure_reason = Some("simulated swap failure".to_string());
            }
            "--fail-swap-reason" => {
                if let Some(value) = args.next() {
                    config.swap_failure_reason = Some(value);
                }
            }
            "--events-jsonl" => {
                emit_events_jsonl = true;
            }
            "--events-jsonl-file" => {
                if let Some(value) = args.next() {
                    emit_events_jsonl = true;
                    events_jsonl_file = Some(PathBuf::from(value));
                }
            }
            "--scenario" => {
                if let Some(value) = args.next() {
                    if value == "brickout-revenge-v1" {
                        config = brickout_revenge_v1_runner_config(config.max_ticks);
                    }
                }
            }
            _ => {}
        }
    }

    CliOptions {
        runner: config,
        emit_events_jsonl,
        events_jsonl_file,
    }
}

fn write_events_jsonl(
    summary: &stasis::RunnerSummary,
    output_file: Option<&Path>,
) -> Result<(), String> {
    let mut writer: Box<dyn Write> = match output_file {
        Some(path) => {
            Box::new(BufWriter::new(File::create(path).map_err(|err| {
                format!("failed to create {}: {err}", path.display())
            })?))
        }
        None => Box::new(BufWriter::new(io::stdout().lock())),
    };

    for event in &summary.events {
        let line = serde_json::to_string(&event.with_schema_version())
            .map_err(|err| format!("failed to serialize event: {err}"))?;
        writeln!(writer, "{line}").map_err(|err| format!("failed to write event line: {err}"))?;
    }
    writer
        .flush()
        .map_err(|err| format!("failed to flush event output: {err}"))?;
    Ok(())
}

fn main() {
    let options = parse_args();
    let is_brickout_profile = options.runner.inject_file_change.as_ref()
        == Some(&PathBuf::from(
            "samples/brickout_revenge/brickout_revenge_v1.stasis",
        ));
    let summary = run_with_default_backend(options.runner);
    let exit_code = if summary.compile_failures > 0 || summary.swap_commit_failures > 0 {
        1
    } else {
        0
    };

    if options.emit_events_jsonl {
        if let Err(err) = write_events_jsonl(&summary, options.events_jsonl_file.as_deref()) {
            eprintln!("{err}");
            std::process::exit(2);
        }
        std::process::exit(exit_code);
    }

    println!("ticks_executed={}", summary.ticks_executed);
    println!("compile_successes={}", summary.compile_successes);
    println!("compile_failures={}", summary.compile_failures);
    println!("hook_runs={}", summary.hook_runs);
    println!("hook_failures={}", summary.hook_failures);
    println!("swap_commit_successes={}", summary.swap_commit_successes);
    println!("swap_commit_failures={}", summary.swap_commit_failures);
    println!(
        "swap_indicator_armed_count={}",
        summary.swap_indicator_armed_count
    );
    println!("swap_flash_peak_ticks={}", summary.swap_flash_peak_ticks);
    println!(
        "swap_flash_ticks_remaining={}",
        summary.swap_flash_ticks_remaining
    );
    println!("has_in_flight_work={}", summary.has_in_flight_work);
    if let Some(window) = summary.window {
        println!(
            "window_profile={}x{} vertical={}",
            window.width,
            window.height,
            window.is_vertical()
        );
    } else if is_brickout_profile {
        println!("window_profile=brickout_revenge_v1 <unset>");
    }
    for diagnostic in &summary.compile_diagnostics {
        println!("compile_diagnostic={diagnostic}");
    }
    for reason in &summary.swap_failure_reasons {
        println!("swap_failure_reason={reason}");
    }
    for reason in &summary.hook_failure_reasons {
        println!("hook_failure_reason={reason}");
    }

    std::process::exit(exit_code);
}
