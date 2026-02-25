use std::env;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::path::PathBuf;
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::time::{Duration, Instant};

use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use stasis::{
    publish_cli_args_to_env, restore_cli_args_env, run_jit_tests_in_directory,
    run_self_host_aot_cli, run_with_default_backend, run_with_real_backend,
    scenarios::brickout_revenge_v1_runner_config, RunnerConfig,
};
use stasis_runner::swap::contracts::TargetMode;

struct CliOptions {
    runner: RunnerConfig,
    emit_events_jsonl: bool,
    events_jsonl_file: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TestCliArgs {
    directory: PathBuf,
    watch: bool,
    watch_settle_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AotCliContractArgs {
    project_dir: PathBuf,
    output_exe: PathBuf,
    summary_file: Option<PathBuf>,
    entry_file: Option<PathBuf>,
    quality_gate: bool,
}

fn apply_brickout_revenge_v1_scenario(config: &mut RunnerConfig) {
    let previous = config.clone();
    let mut scenario = brickout_revenge_v1_runner_config(previous.max_ticks);
    // Keep explicit CLI/runtime policy flags stable regardless of arg order.
    scenario.tick_sleep_micros = previous.tick_sleep_micros;
    scenario.watch_directory = previous.watch_directory;
    scenario.target_mode = previous.target_mode;
    scenario.fail_compile = previous.fail_compile;
    scenario.disable_on_code_swap_hook = previous.disable_on_code_swap_hook;
    scenario.hook_failure_reason = previous.hook_failure_reason;
    scenario.swap_failure_reason = previous.swap_failure_reason;
    scenario.runtime_launch = previous.runtime_launch;
    scenario.aot_probe_loadability = previous.aot_probe_loadability;
    *config = scenario;
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
            "--runtime-launch" => {
                config.runtime_launch = true;
            }
            "--no-runtime-launch" => {
                config.runtime_launch = false;
            }
            "--aot-probe-load" => {
                config.aot_probe_loadability = true;
            }
            "--no-aot-probe-load" => {
                config.aot_probe_loadability = false;
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
                        apply_brickout_revenge_v1_scenario(&mut config);
                        config.runtime_launch = true;
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

fn parse_test_cli_args(args: &[String]) -> Result<TestCliArgs, String> {
    let mut directory: Option<PathBuf> = None;
    let mut watch = false;
    let mut watch_settle_ms: u64 = 150;
    let mut i: usize = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if arg == "--dir" {
            if i + 1 >= args.len() {
                return Err("missing value for --dir".to_string());
            }
            directory = Some(PathBuf::from(args[i + 1].clone()));
            i += 2;
            continue;
        }
        if arg == "--watch" {
            watch = true;
            i += 1;
            continue;
        }
        if arg == "--watch-settle-ms" {
            if i + 1 >= args.len() {
                return Err("missing value for --watch-settle-ms".to_string());
            }
            let parsed = args[i + 1].parse::<u64>().map_err(|error| {
                format!(
                    "invalid value for --watch-settle-ms '{}': {error}",
                    args[i + 1]
                )
            })?;
            watch_settle_ms = parsed.max(10);
            i += 2;
            continue;
        }
        i += 1;
    }
    let Some(directory) = directory else {
        return Err("missing required --dir <path>".to_string());
    };
    Ok(TestCliArgs {
        directory,
        watch,
        watch_settle_ms,
    })
}

fn try_run_test_subcommand() -> Option<i32> {
    let mut args = env::args().skip(1);
    let first = args.next()?;
    if first != "test" {
        return None;
    }
    let arg_list: Vec<String> = args.collect();
    let parsed = match parse_test_cli_args(&arg_list) {
        Ok(value) => value,
        Err(message) => {
            eprintln!("{message}");
            return Some(2);
        }
    };

    if parsed.watch {
        let exit = match run_test_watch_loop(&parsed) {
            Ok(code) => code,
            Err(message) => {
                eprintln!("{message}");
                1
            }
        };
        return Some(exit);
    }

    let exit = match run_test_dir_once(&parsed.directory) {
        Ok(code) => code,
        Err(message) => {
            eprintln!("{message}");
            1
        }
    };
    Some(exit)
}

fn run_test_dir_once(directory: &Path) -> Result<i32, String> {
    let started = Instant::now();
    let summary = run_jit_tests_in_directory(directory)?;
    println!("test_files_discovered={}", summary.files_discovered);
    println!("test_files_with_tests={}", summary.files_with_tests);
    println!("tests_discovered={}", summary.tests_discovered);
    println!("tests_run={}", summary.tests_run);
    println!("tests_passed={}", summary.tests_passed);
    println!("tests_failed={}", summary.tests_failed);
    for failure in &summary.failures {
        println!("test_failure={failure}");
    }
    println!("elapsed_ms={:.3}", started.elapsed().as_secs_f64() * 1000.0);
    Ok(if summary.tests_failed > 0 { 1 } else { 0 })
}

fn run_test_watch_loop(parsed: &TestCliArgs) -> Result<i32, String> {
    let first_exit = run_test_dir_once(&parsed.directory)?;
    println!("watch_mode=1");
    println!("watch_settle_ms={}", parsed.watch_settle_ms);
    println!("watch_last_exit={first_exit}");

    let (tx, rx) = channel::<notify::Result<Event>>();
    let mut watcher = RecommendedWatcher::new(
        move |res| {
            let _ = tx.send(res);
        },
        Config::default(),
    )
    .map_err(|error| format!("failed to create test watcher: {error}"))?;
    watcher
        .watch(&parsed.directory, RecursiveMode::Recursive)
        .map_err(|error| {
            format!(
                "failed to watch test directory '{}': {error}",
                parsed.directory.display()
            )
        })?;

    let settle = Duration::from_millis(parsed.watch_settle_ms);
    loop {
        let mut saw_stasis_change = false;
        while !saw_stasis_change {
            let event = rx
                .recv()
                .map_err(|_| "test watch channel disconnected".to_string())?;
            let Ok(event) = event else {
                continue;
            };
            if is_stasis_change_event(&event) {
                saw_stasis_change = true;
            }
        }

        let mut last_change_at = Instant::now();
        loop {
            let remaining = settle
                .checked_sub(last_change_at.elapsed())
                .unwrap_or(Duration::ZERO);
            if remaining.is_zero() {
                break;
            }
            match rx.recv_timeout(remaining) {
                Ok(Ok(event)) => {
                    if is_stasis_change_event(&event) {
                        last_change_at = Instant::now();
                    }
                }
                Ok(Err(_)) => {}
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => {
                    return Err("test watch channel disconnected".to_string());
                }
            }
        }

        println!("watch_change=stasis");
        let exit = run_test_dir_once(&parsed.directory)?;
        println!("watch_last_exit={exit}");
    }
}

fn is_stasis_change_event(event: &Event) -> bool {
    let is_change_kind = matches!(
        event.kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    );
    if !is_change_kind {
        return false;
    }
    event.paths.iter().any(|path| {
        path.extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("stasis"))
    })
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

fn parse_aot_cli_contract_args(args: &[String]) -> Result<AotCliContractArgs, String> {
    let mut project_dir: Option<PathBuf> = None;
    let mut output_exe: Option<PathBuf> = None;
    let mut summary_file: Option<PathBuf> = None;
    let mut entry_file: Option<PathBuf> = None;
    let mut quality_gate = false;
    let mut i: usize = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if arg == "--project-dir" {
            if i + 1 >= args.len() {
                return Err("missing value for --project-dir".to_string());
            }
            project_dir = Some(PathBuf::from(args[i + 1].clone()));
            i += 2;
            continue;
        }
        if arg == "--out" {
            if i + 1 >= args.len() {
                return Err("missing value for --out".to_string());
            }
            output_exe = Some(PathBuf::from(args[i + 1].clone()));
            i += 2;
            continue;
        }
        if arg == "--summary-file" {
            if i + 1 >= args.len() {
                return Err("missing value for --summary-file".to_string());
            }
            summary_file = Some(PathBuf::from(args[i + 1].clone()));
            i += 2;
            continue;
        }
        if arg == "--entry-file" {
            if i + 1 >= args.len() {
                return Err("missing value for --entry-file".to_string());
            }
            entry_file = Some(PathBuf::from(args[i + 1].clone()));
            i += 2;
            continue;
        }
        if arg == "--quality-gate" {
            quality_gate = true;
            i += 1;
            continue;
        }
        i += 1;
    }

    let Some(project_dir) = project_dir else {
        return Err("missing required --project-dir <path>".to_string());
    };
    let Some(output_exe) = output_exe else {
        return Err("missing required --out <path>".to_string());
    };
    Ok(AotCliContractArgs {
        project_dir,
        output_exe,
        summary_file,
        entry_file,
        quality_gate,
    })
}

fn try_run_aot_cli_subcommand() -> Option<i32> {
    let mut args = env::args().skip(1);
    let first = args.next()?;
    if first != "aot-cli" {
        return None;
    }
    // Host boundary for self-host AOT CLI:
    // parse process args, publish bridge env, call self-host compile entry.
    // Compile orchestration policy belongs in .stasis compiler code.
    let arg_list: Vec<String> = args.collect();
    let parsed = match parse_aot_cli_contract_args(&arg_list) {
        Ok(value) => value,
        Err(message) => {
            eprintln!("{message}");
            return Some(2);
        }
    };
    let old_summary_override = std::env::var("STASIS_AOT_SUMMARY_FILE").ok();
    let old_entry_override = std::env::var("STASIS_AOT_ENTRY_FILE").ok();
    let old_quality_gate = std::env::var("STASIS_AOT_QUALITY_GATE").ok();
    if let Some(path) = parsed.summary_file.as_ref() {
        if let Some(parent) = path.parent() {
            if let Err(error) = std::fs::create_dir_all(parent) {
                eprintln!(
                    "failed to create summary directory {}: {error}",
                    parent.display()
                );
                return Some(2);
            }
        }
        std::env::set_var("STASIS_AOT_SUMMARY_FILE", path);
    } else {
        std::env::remove_var("STASIS_AOT_SUMMARY_FILE");
    }
    if let Some(path) = parsed.entry_file.as_ref() {
        std::env::set_var("STASIS_AOT_ENTRY_FILE", path);
    } else {
        std::env::remove_var("STASIS_AOT_ENTRY_FILE");
    }
    if parsed.quality_gate {
        std::env::set_var("STASIS_AOT_QUALITY_GATE", "1");
    } else {
        std::env::remove_var("STASIS_AOT_QUALITY_GATE");
    }
    let mut bridge_args = vec![
        "--project-dir".to_string(),
        parsed.project_dir.display().to_string(),
        "--out".to_string(),
        parsed.output_exe.display().to_string(),
    ];
    if let Some(path) = parsed.summary_file.as_ref() {
        bridge_args.push("--summary-file".to_string());
        bridge_args.push(path.display().to_string());
    }
    if let Some(path) = parsed.entry_file.as_ref() {
        bridge_args.push("--entry-file".to_string());
        bridge_args.push(path.display().to_string());
    }
    if parsed.quality_gate {
        bridge_args.push("--quality-gate".to_string());
    }
    let bridge_snapshot = publish_cli_args_to_env(&bridge_args, parsed.summary_file.as_deref());

    let result = run_self_host_aot_cli(&parsed.project_dir, &parsed.output_exe);
    restore_cli_args_env(bridge_snapshot);
    if let Some(value) = old_summary_override {
        std::env::set_var("STASIS_AOT_SUMMARY_FILE", value);
    } else {
        std::env::remove_var("STASIS_AOT_SUMMARY_FILE");
    }
    if let Some(value) = old_entry_override {
        std::env::set_var("STASIS_AOT_ENTRY_FILE", value);
    } else {
        std::env::remove_var("STASIS_AOT_ENTRY_FILE");
    }
    if let Some(value) = old_quality_gate {
        std::env::set_var("STASIS_AOT_QUALITY_GATE", value);
    } else {
        std::env::remove_var("STASIS_AOT_QUALITY_GATE");
    }

    match result {
        Ok(summary) => {
            println!("aot_cli_source_file_count={}", summary.source_file_count);
            println!("aot_cli_output_exe={}", summary.linked_image_path.display());
            println!("aot_cli_entry_symbol={}", summary.entry_symbol);
            Some(0)
        }
        Err(message) => {
            eprintln!("{message}");
            Some(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_test_cli_args_accepts_required_dir_flag() {
        let args = vec!["--dir".to_string(), "tests/stasis".to_string()];
        let parsed = parse_test_cli_args(&args).expect("parse should succeed");
        assert_eq!(parsed.directory, PathBuf::from("tests/stasis"));
        assert!(!parsed.watch);
        assert_eq!(parsed.watch_settle_ms, 150);
    }

    #[test]
    fn parse_test_cli_args_rejects_missing_required_dir_flag() {
        let args = vec!["--ticks".to_string(), "10".to_string()];
        let error = parse_test_cli_args(&args).expect_err("parse should fail");
        assert!(error.contains("missing required --dir"));
    }

    #[test]
    fn parse_test_cli_args_accepts_watch_options() {
        let args = vec![
            "--dir".to_string(),
            "tests/stasis".to_string(),
            "--watch".to_string(),
            "--watch-settle-ms".to_string(),
            "250".to_string(),
        ];
        let parsed = parse_test_cli_args(&args).expect("parse should succeed");
        assert_eq!(parsed.directory, PathBuf::from("tests/stasis"));
        assert!(parsed.watch);
        assert_eq!(parsed.watch_settle_ms, 250);
    }

    #[test]
    fn parse_test_cli_args_rejects_invalid_watch_settle_value() {
        let args = vec![
            "--dir".to_string(),
            "tests/stasis".to_string(),
            "--watch-settle-ms".to_string(),
            "invalid".to_string(),
        ];
        let error = parse_test_cli_args(&args).expect_err("parse should fail");
        assert!(error.contains("invalid value for --watch-settle-ms"));
    }

    #[test]
    fn parse_aot_cli_contract_args_accepts_required_flags() {
        let args = vec![
            "--project-dir".to_string(),
            "proj".to_string(),
            "--out".to_string(),
            "bin.exe".to_string(),
        ];
        let parsed = parse_aot_cli_contract_args(&args).expect("parse should succeed");
        assert_eq!(parsed.project_dir, PathBuf::from("proj"));
        assert_eq!(parsed.output_exe, PathBuf::from("bin.exe"));
        assert_eq!(parsed.summary_file, None);
        assert_eq!(parsed.entry_file, None);
        assert!(!parsed.quality_gate);
    }

    #[test]
    fn parse_aot_cli_contract_args_accepts_summary_flag() {
        let args = vec![
            "--project-dir".to_string(),
            "proj".to_string(),
            "--out".to_string(),
            "bin.exe".to_string(),
            "--summary-file".to_string(),
            "summary.json".to_string(),
        ];
        let parsed = parse_aot_cli_contract_args(&args).expect("parse should succeed");
        assert_eq!(parsed.summary_file, Some(PathBuf::from("summary.json")));
    }

    #[test]
    fn parse_aot_cli_contract_args_accepts_entry_file_flag() {
        let args = vec![
            "--project-dir".to_string(),
            "proj".to_string(),
            "--out".to_string(),
            "bin.exe".to_string(),
            "--entry-file".to_string(),
            "brickout_revenge_v1.stasis".to_string(),
        ];
        let parsed = parse_aot_cli_contract_args(&args).expect("parse should succeed");
        assert_eq!(
            parsed.entry_file,
            Some(PathBuf::from("brickout_revenge_v1.stasis"))
        );
        assert!(!parsed.quality_gate);
    }

    #[test]
    fn parse_aot_cli_contract_args_accepts_quality_gate_flag() {
        let args = vec![
            "--project-dir".to_string(),
            "proj".to_string(),
            "--out".to_string(),
            "bin.exe".to_string(),
            "--quality-gate".to_string(),
        ];
        let parsed = parse_aot_cli_contract_args(&args).expect("parse should succeed");
        assert!(parsed.quality_gate);
    }

    #[test]
    fn parse_aot_cli_contract_args_rejects_missing_required_flags() {
        let args = vec!["--project-dir".to_string(), "proj".to_string()];
        let error = parse_aot_cli_contract_args(&args).expect_err("parse should fail");
        assert!(error.contains("missing required --out"));
    }

    #[test]
    fn scenario_apply_preserves_preparsed_runtime_flags() {
        let mut config = RunnerConfig::default();
        config.max_ticks = 777;
        config.tick_sleep_micros = 333;
        config.watch_directory = Some(PathBuf::from("samples"));
        config.target_mode = TargetMode::AotProd;
        config.fail_compile = true;
        config.disable_on_code_swap_hook = true;
        config.hook_failure_reason = Some("hook failed".to_string());
        config.swap_failure_reason = Some("swap failed".to_string());

        apply_brickout_revenge_v1_scenario(&mut config);

        assert_eq!(config.max_ticks, 777);
        assert_eq!(config.tick_sleep_micros, 333);
        assert_eq!(config.watch_directory, Some(PathBuf::from("samples")));
        assert_eq!(config.target_mode, TargetMode::AotProd);
        assert!(config.fail_compile);
        assert!(config.disable_on_code_swap_hook);
        assert_eq!(config.hook_failure_reason.as_deref(), Some("hook failed"));
        assert_eq!(config.swap_failure_reason.as_deref(), Some("swap failed"));
        assert_eq!(
            config.inject_file_change,
            Some(PathBuf::from(
                "samples/brickout_revenge/brickout_revenge_v1.stasis"
            ))
        );
    }

    #[test]
    fn aot_cli_host_glue_stays_out_of_compile_orchestration() {
        let source = include_str!("main.rs");
        let forbidden_collect = ["collect_stasis_files_recursive", "("].concat();
        let forbidden_compile = ["compile_changed_files", "("].concat();
        assert!(
            !source.contains(&forbidden_collect),
            "aot-cli host should not enumerate project sources directly"
        );
        assert!(
            !source.contains(&forbidden_compile),
            "aot-cli host should not run compiler frontend directly"
        );
        assert!(
            source.contains("run_self_host_aot_cli("),
            "aot-cli host should delegate through self-host entrypoint"
        );
    }
}

fn main() {
    if let Some(exit) = try_run_test_subcommand() {
        std::process::exit(exit);
    }
    if let Some(exit) = try_run_aot_cli_subcommand() {
        std::process::exit(exit);
    }

    let options = parse_args();
    let is_brickout_profile = options.runner.inject_file_change.as_ref()
        == Some(&PathBuf::from(
            "samples/brickout_revenge/brickout_revenge_v1.stasis",
        ));
    let use_simulated = options.runner.fail_compile
        || options.runner.hook_failure_reason.is_some()
        || options.runner.swap_failure_reason.is_some();
    let summary = if use_simulated {
        run_with_default_backend(options.runner)
    } else {
        run_with_real_backend(options.runner)
    };
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
    if let Some(compile_ms) = summary.last_compile_duration_ms {
        println!("last_compile_duration_ms={compile_ms}");
    }
    if let Some(commit_ms) = summary.last_commit_duration_ms {
        println!("last_commit_duration_ms={commit_ms}");
    }
    println!("has_in_flight_work={}", summary.has_in_flight_work);
    println!("runtime_launches={}", summary.runtime_launches);
    println!(
        "runtime_launch_failures={}",
        summary.runtime_launch_failures
    );
    println!(
        "aot_linked_image_activations={}",
        summary.aot_linked_image_activations
    );
    if let Some(path) = summary.active_aot_linked_image_path.as_ref() {
        println!("active_aot_linked_image_path={}", path.display());
    }
    if let Some(size) = summary.active_aot_linked_image_size_bytes {
        println!("active_aot_linked_image_size_bytes={size}");
    }
    if let Some(generation) = summary.active_aot_linked_image_generation {
        println!("active_aot_linked_image_generation={generation}");
    }
    println!(
        "retired_aot_linked_images={}",
        summary.retired_aot_linked_images
    );
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
    for reason in &summary.runtime_launch_failure_reasons {
        println!("runtime_launch_failure_reason={reason}");
    }

    std::process::exit(exit_code);
}
