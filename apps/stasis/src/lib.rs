#![forbid(unsafe_code)]

mod compiler_backend;
mod events;
pub mod scenarios;
mod watch;

pub use events::RunnerEvent;
pub use scenarios::WindowConfig;

use compiler_backend::IncrementalCompilerBackend;
use stasis_runner::swap::contracts::{
    CompileRequest, CompileResult, CompileStatus, Diagnostic, DiagnosticSeverity, FileChangeEvent,
    FileChangeKind, FnId, FunctionPatch, FunctionPatchSet, LayoutHash, RequestId, SwapCommitResult,
    SwapCommitStatus, TargetMode, TextSource,
};
use stasis_runner::swap::pipeline::{CompilerBackend, DevHotSwapPipeline};
use stasis_jit::FunctionPointerTable;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;
use watch::WatchService;

const SWAP_FLASH_TICKS_MAX: u32 = 180;

#[derive(Debug, Clone)]
pub struct RunnerConfig {
    pub max_ticks: u32,
    pub tick_sleep_micros: u64,
    pub window: Option<WindowConfig>,
    pub inject_file_change: Option<PathBuf>,
    pub watch_directory: Option<PathBuf>,
    pub target_mode: TargetMode,
    pub fail_compile: bool,
    pub disable_on_code_swap_hook: bool,
    pub hook_failure_reason: Option<String>,
    pub swap_failure_reason: Option<String>,
}

impl Default for RunnerConfig {
    fn default() -> Self {
        Self {
            max_ticks: 120,
            tick_sleep_micros: 0,
            window: None,
            inject_file_change: None,
            watch_directory: None,
            target_mode: TargetMode::JitDev,
            fail_compile: false,
            disable_on_code_swap_hook: false,
            hook_failure_reason: None,
            swap_failure_reason: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunnerSummary {
    pub ticks_executed: u32,
    pub compile_successes: u32,
    pub compile_failures: u32,
    pub compile_diagnostics: Vec<String>,
    pub hook_runs: u32,
    pub hook_failures: u32,
    pub hook_failure_reasons: Vec<String>,
    pub swap_commit_successes: u32,
    pub swap_commit_failures: u32,
    pub swap_failure_reasons: Vec<String>,
    pub swap_indicator_armed_count: u32,
    pub swap_flash_peak_ticks: u32,
    pub swap_flash_ticks_remaining: u32,
    pub window: Option<WindowConfig>,
    pub last_swap_status: Option<SwapCommitStatus>,
    pub has_in_flight_work: bool,
    pub events: Vec<RunnerEvent>,
}

pub fn run_with_default_backend(config: RunnerConfig) -> RunnerSummary {
    let backend = move |request: CompileRequest| -> CompileResult {
        if config.fail_compile {
            CompileResult::failed(
                request.request_id,
                vec![Diagnostic {
                    severity: DiagnosticSeverity::Error,
                    message: "simulated compile failure".to_string(),
                    path: request.changed_files.first().cloned(),
                    line: Some(1),
                    column: Some(1),
                }],
            )
        } else {
            let patch_set = FunctionPatchSet {
                functions: vec![FunctionPatch { fn_id: FnId(1) }],
            };
            let hook_symbol = if request.target_mode == TargetMode::JitDev {
                Some("on_code_swap".to_string())
            } else {
                None
            };
            CompileResult::success_with_hook_symbol(
                request.request_id,
                LayoutHash([1; 32]),
                patch_set,
                hook_symbol,
            )
        }
    };

    run_with_backend(config, backend)
}

pub fn run_with_real_backend(config: RunnerConfig) -> RunnerSummary {
    let backend = IncrementalCompilerBackend::new();
    run_with_backend(config, backend)
}

pub fn run_with_backend<B: CompilerBackend>(config: RunnerConfig, backend: B) -> RunnerSummary {
    let mut watcher = config
        .watch_directory
        .as_deref()
        .and_then(|dir| WatchService::start(dir).ok());
    let window = config.window;

    let mut pipeline = DevHotSwapPipeline::with_target_mode(backend, config.target_mode);
    let mut pointer_table = FunctionPointerTable::new();
    let mut hook_runs: u32 = 0;
    let mut hook_failures: u32 = 0;
    let mut hook_failure_reasons: Vec<String> = Vec::new();
    let mut swap_commit_successes: u32 = 0;
    let mut swap_commit_failures: u32 = 0;
    let mut swap_failure_reasons: Vec<String> = Vec::new();
    let mut swap_indicator_armed_count: u32 = 0;
    let mut swap_flash_peak_ticks: u32 = 0;
    let mut swap_flash_ticks_remaining: u32 = 0;
    let mut compile_successes: u32 = 0;
    let mut compile_failures: u32 = 0;
    let mut compile_diagnostics: Vec<String> = Vec::new();
    let mut last_seen_compile_id: Option<RequestId> = None;
    let mut last_seen_commit_id: Option<RequestId> = None;
    let mut last_swap_status: Option<SwapCommitStatus> = None;
    let mut events: Vec<RunnerEvent> = Vec::new();
    let mut file_change_sent = false;
    let disable_on_code_swap_hook = config.disable_on_code_swap_hook;
    let hook_failure_reason = config.hook_failure_reason.clone();
    let swap_failure_reason = config.swap_failure_reason.clone();

    for tick in 0..config.max_ticks {
        if !file_change_sent {
            if let Some(path) = &config.inject_file_change {
                let event = FileChangeEvent::new(
                    path.clone(),
                    u64::from(tick) + 1,
                    TextSource::FileWatcher,
                    FileChangeKind::Modified,
                );
                pipeline.submit_file_change(event);
                file_change_sent = true;
            }
        }

        if let Some(watch_service) = watcher.as_mut() {
            for event in watch_service.drain_stasis_changes() {
                pipeline.submit_file_change(event);
            }
        }

        pipeline.pump_coordinator();

        pipeline.process_commits_at_safe_point(|request| {
            if !disable_on_code_swap_hook {
                if let Some(hook_symbol) = request.hook_symbol.as_deref() {
                    hook_runs += 1;
                    if let Some(reason) = hook_failure_reason.as_ref() {
                        hook_failures += 1;
                        hook_failure_reasons.push(reason.clone());
                        let hook_error = format!("{hook_symbol} failed: {reason}");
                        events.push(RunnerEvent::HookResult {
                            request_id: request.request_id.0,
                            symbol: hook_symbol.to_string(),
                            status: "failed".to_string(),
                            error: Some(hook_error.clone()),
                        });
                        swap_commit_failures += 1;
                        swap_failure_reasons.push(hook_error.clone());
                        return SwapCommitResult::failed(request.request_id, hook_error);
                    }

                    events.push(RunnerEvent::HookResult {
                        request_id: request.request_id.0,
                        symbol: hook_symbol.to_string(),
                        status: "success".to_string(),
                        error: None,
                    });
                }
            }

            if let Some(reason) = swap_failure_reason.as_ref() {
                swap_commit_failures += 1;
                swap_failure_reasons.push(reason.clone());
                SwapCommitResult::failed(request.request_id, reason.clone())
            } else {
                swap_commit_successes += 1;
                let outcome = pointer_table.commit_patch_set(&request.fn_patch_set);
                SwapCommitResult::success(
                    request.request_id,
                    outcome.swapped_fn_ids,
                    outcome.new_generation,
                )
            }
        });

        pipeline.pump_coordinator();
        let new_commit = observe_pipeline_results(
            &pipeline,
            &mut last_seen_compile_id,
            &mut last_seen_commit_id,
            &mut compile_successes,
            &mut compile_failures,
            &mut compile_diagnostics,
            &mut last_swap_status,
            &mut events,
        );
        if let Some((request_id, status)) = new_commit {
            if status == SwapCommitStatus::Success {
                swap_indicator_armed_count += 1;
                swap_flash_ticks_remaining = SWAP_FLASH_TICKS_MAX;
                swap_flash_peak_ticks = swap_flash_peak_ticks.max(swap_flash_ticks_remaining);
                events.push(RunnerEvent::SwapIndicatorArmed {
                    request_id: request_id.0,
                    ticks: SWAP_FLASH_TICKS_MAX,
                });
            }
        }
        if swap_flash_ticks_remaining > 0 {
            swap_flash_ticks_remaining -= 1;
        }
        thread::yield_now();
        sleep_for_tick(config.tick_sleep_micros);
    }

    let drain_start = std::time::Instant::now();
    while drain_start.elapsed() < Duration::from_secs(30) {
        if !pipeline.has_in_flight_work() && pipeline.pending_commit_requests() == 0 {
            break;
        }

        pipeline.pump_coordinator();
        pipeline.process_commits_at_safe_point(|request| {
            if !disable_on_code_swap_hook {
                if let Some(hook_symbol) = request.hook_symbol.as_deref() {
                    hook_runs += 1;
                    if let Some(reason) = hook_failure_reason.as_ref() {
                        hook_failures += 1;
                        hook_failure_reasons.push(reason.clone());
                        let hook_error = format!("{hook_symbol} failed: {reason}");
                        events.push(RunnerEvent::HookResult {
                            request_id: request.request_id.0,
                            symbol: hook_symbol.to_string(),
                            status: "failed".to_string(),
                            error: Some(hook_error.clone()),
                        });
                        swap_commit_failures += 1;
                        swap_failure_reasons.push(hook_error.clone());
                        return SwapCommitResult::failed(request.request_id, hook_error);
                    }

                    events.push(RunnerEvent::HookResult {
                        request_id: request.request_id.0,
                        symbol: hook_symbol.to_string(),
                        status: "success".to_string(),
                        error: None,
                    });
                }
            }

            if let Some(reason) = swap_failure_reason.as_ref() {
                swap_commit_failures += 1;
                swap_failure_reasons.push(reason.clone());
                SwapCommitResult::failed(request.request_id, reason.clone())
            } else {
                swap_commit_successes += 1;
                let outcome = pointer_table.commit_patch_set(&request.fn_patch_set);
                SwapCommitResult::success(
                    request.request_id,
                    outcome.swapped_fn_ids,
                    outcome.new_generation,
                )
            }
        });
        pipeline.pump_coordinator();
        let new_commit = observe_pipeline_results(
            &pipeline,
            &mut last_seen_compile_id,
            &mut last_seen_commit_id,
            &mut compile_successes,
            &mut compile_failures,
            &mut compile_diagnostics,
            &mut last_swap_status,
            &mut events,
        );
        if let Some((request_id, status)) = new_commit {
            if status == SwapCommitStatus::Success {
                swap_indicator_armed_count += 1;
                swap_flash_ticks_remaining = SWAP_FLASH_TICKS_MAX;
                swap_flash_peak_ticks = swap_flash_peak_ticks.max(swap_flash_ticks_remaining);
                events.push(RunnerEvent::SwapIndicatorArmed {
                    request_id: request_id.0,
                    ticks: SWAP_FLASH_TICKS_MAX,
                });
            }
        }
        thread::yield_now();
        thread::sleep(Duration::from_millis(1));
    }

    let has_in_flight_work = pipeline.has_in_flight_work();
    events.push(RunnerEvent::Summary {
        ticks_executed: config.max_ticks,
        compile_successes,
        compile_failures,
        swap_commit_successes,
        swap_commit_failures,
        swap_indicator_armed_count,
        swap_flash_peak_ticks,
        swap_flash_ticks_remaining,
        window_width: window.map(|w| w.width),
        window_height: window.map(|w| w.height),
        has_in_flight_work,
    });

    RunnerSummary {
        ticks_executed: config.max_ticks,
        compile_successes,
        compile_failures,
        compile_diagnostics,
        hook_runs,
        hook_failures,
        hook_failure_reasons,
        swap_commit_successes,
        swap_commit_failures,
        swap_failure_reasons,
        swap_indicator_armed_count,
        swap_flash_peak_ticks,
        swap_flash_ticks_remaining,
        window,
        last_swap_status,
        has_in_flight_work,
        events,
    }
}

fn observe_pipeline_results(
    pipeline: &DevHotSwapPipeline,
    last_seen_compile_id: &mut Option<RequestId>,
    last_seen_commit_id: &mut Option<RequestId>,
    compile_successes: &mut u32,
    compile_failures: &mut u32,
    compile_diagnostics: &mut Vec<String>,
    last_swap_status: &mut Option<SwapCommitStatus>,
    events: &mut Vec<RunnerEvent>,
) -> Option<(RequestId, SwapCommitStatus)> {
    let mut new_commit: Option<(RequestId, SwapCommitStatus)> = None;
    if let Some(result) = pipeline.last_compile_result() {
        if *last_seen_compile_id != Some(result.request_id) {
            *last_seen_compile_id = Some(result.request_id);
            match result.status {
                CompileStatus::Success => {
                    *compile_successes += 1;
                    events.push(RunnerEvent::CompileResult {
                        request_id: result.request_id.0,
                        status: "success".to_string(),
                        diagnostics: Vec::new(),
                    });
                }
                CompileStatus::Failed => {
                    *compile_failures += 1;
                    let mut event_diagnostics = Vec::new();
                    if result.diagnostics.is_empty() {
                        let message = "compile failed with no diagnostics".to_string();
                        compile_diagnostics.push(message.clone());
                        event_diagnostics.push(message);
                    } else {
                        for diagnostic in &result.diagnostics {
                            let formatted = format_diagnostic(diagnostic);
                            compile_diagnostics.push(formatted.clone());
                            event_diagnostics.push(formatted);
                        }
                    }
                    events.push(RunnerEvent::CompileResult {
                        request_id: result.request_id.0,
                        status: "failed".to_string(),
                        diagnostics: event_diagnostics,
                    });
                }
            }
        }
    }

    if let Some(result) = pipeline.last_commit_result() {
        *last_swap_status = Some(result.status.clone());
        if *last_seen_commit_id != Some(result.request_id) {
            *last_seen_commit_id = Some(result.request_id);
            new_commit = Some((result.request_id, result.status.clone()));
            let status = match result.status {
                SwapCommitStatus::Success => "success",
                SwapCommitStatus::Failed => "failed",
            };
            let swapped_fn_ids = result.swapped_fn_ids.iter().map(|id| id.0).collect();
            let new_generation = result.new_generation.map(|generation| generation.0);
            events.push(RunnerEvent::SwapCommitResult {
                request_id: result.request_id.0,
                status: status.to_string(),
                swapped_fn_ids,
                new_generation,
                error: result.error.clone(),
            });
        }
    }
    new_commit
}

fn sleep_for_tick(tick_sleep_micros: u64) {
    let micros = if tick_sleep_micros > 0 {
        tick_sleep_micros
    } else {
        // Tiny default pause improves cross-thread determinism for test/runtime loops.
        50
    };
    thread::sleep(Duration::from_micros(micros));
}

fn format_diagnostic(diagnostic: &Diagnostic) -> String {
    let severity = match diagnostic.severity {
        DiagnosticSeverity::Error => "error",
        DiagnosticSeverity::Warning => "warning",
        DiagnosticSeverity::Note => "note",
    };

    let path_part = diagnostic
        .path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "<unknown>".to_string());

    let line = diagnostic.line.unwrap_or(0);
    let column = diagnostic.column.unwrap_or(0);
    format!(
        "{severity}:{path_part}:{line}:{column}: {}",
        diagnostic.message
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenarios::{brickout_revenge_v1_runner_config, BRICKOUT_REVENGE_V1_WINDOW};
    use std::fs;
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn runner_loop_compiles_and_commits_one_change() {
        let config = RunnerConfig {
            max_ticks: 200,
            tick_sleep_micros: 0,
            window: None,
            inject_file_change: Some(PathBuf::from(
                "samples/brickout_revenge/brickout_revenge_v1.stasis",
            )),
            watch_directory: None,
            target_mode: TargetMode::JitDev,
            fail_compile: false,
            disable_on_code_swap_hook: false,
            hook_failure_reason: None,
            swap_failure_reason: None,
        };

        let summary = run_with_default_backend(config);
        assert_eq!(summary.compile_successes, 1);
        assert_eq!(summary.compile_failures, 0);
        assert_eq!(summary.compile_diagnostics.len(), 0);
        assert_eq!(summary.hook_runs, 1);
        assert_eq!(summary.hook_failures, 0);
        assert_eq!(summary.hook_failure_reasons.len(), 0);
        assert_eq!(summary.swap_commit_successes, 1);
        assert_eq!(summary.swap_commit_failures, 0);
        assert_eq!(summary.swap_failure_reasons.len(), 0);
        assert_eq!(summary.swap_indicator_armed_count, 1);
        assert_eq!(summary.swap_flash_peak_ticks, SWAP_FLASH_TICKS_MAX);
        assert!(summary.swap_flash_ticks_remaining < SWAP_FLASH_TICKS_MAX);
        assert!(summary.window.is_none());
        assert_eq!(summary.last_swap_status, Some(SwapCommitStatus::Success));
        assert!(!summary.has_in_flight_work);
        assert_eq!(summary.events.len(), 5);
        assert!(matches!(
            summary.events[4],
            RunnerEvent::Summary {
                compile_successes: 1,
                compile_failures: 0,
                swap_commit_successes: 1,
                swap_commit_failures: 0,
                swap_indicator_armed_count: 1,
                swap_flash_peak_ticks: SWAP_FLASH_TICKS_MAX,
                swap_flash_ticks_remaining: _,
                window_width: None,
                window_height: None,
                ticks_executed: _,
                has_in_flight_work: false
            }
        ));
        assert!(summary.events.iter().any(|event| matches!(
            event,
            RunnerEvent::CompileResult {
                ref status,
                request_id: _,
                diagnostics: _
            } if status == "success"
        )));
        assert!(summary.events.iter().any(|event| matches!(
            event,
            RunnerEvent::HookResult {
                ref status,
                request_id: _,
                symbol: _,
                error: None
            } if status == "success"
        )));
        assert!(summary.events.iter().any(|event| matches!(
            event,
            RunnerEvent::SwapCommitResult {
                ref status,
                request_id: _,
                swapped_fn_ids: _,
                new_generation: _,
                error: _
            } if status == "success"
        )));
        assert!(summary.events.iter().any(|event| matches!(
            event,
            RunnerEvent::SwapIndicatorArmed {
                request_id: _,
                ticks: SWAP_FLASH_TICKS_MAX
            }
        )));
    }

    #[test]
    fn runner_loop_reports_compile_failure_and_skips_commit() {
        let config = RunnerConfig {
            max_ticks: 200,
            tick_sleep_micros: 0,
            window: None,
            inject_file_change: Some(PathBuf::from("samples/invalid.stasis")),
            watch_directory: None,
            target_mode: TargetMode::JitDev,
            fail_compile: true,
            disable_on_code_swap_hook: false,
            hook_failure_reason: None,
            swap_failure_reason: None,
        };

        let summary = run_with_default_backend(config);
        assert_eq!(summary.compile_successes, 0);
        assert_eq!(summary.compile_failures, 1);
        assert_eq!(summary.hook_runs, 0);
        assert_eq!(summary.hook_failures, 0);
        assert_eq!(summary.hook_failure_reasons.len(), 0);
        assert_eq!(summary.swap_commit_successes, 0);
        assert_eq!(summary.swap_commit_failures, 0);
        assert_eq!(summary.swap_failure_reasons.len(), 0);
        assert_eq!(summary.swap_indicator_armed_count, 0);
        assert_eq!(summary.swap_flash_peak_ticks, 0);
        assert_eq!(summary.swap_flash_ticks_remaining, 0);
        assert!(summary.window.is_none());
        assert_eq!(summary.compile_diagnostics.len(), 1);
        assert!(summary.compile_diagnostics[0].contains("simulated compile failure"));
        assert_eq!(summary.last_swap_status, None);
        assert!(!summary.has_in_flight_work);
        assert_eq!(summary.events.len(), 2);
        assert!(summary.events.iter().any(|event| matches!(
            event,
            RunnerEvent::CompileResult {
                ref status,
                request_id: _,
                ref diagnostics
            } if status == "failed" && !diagnostics.is_empty()
        )));
        assert!(matches!(
            summary.events[1],
            RunnerEvent::Summary {
                compile_successes: 0,
                compile_failures: 1,
                swap_commit_successes: 0,
                swap_commit_failures: 0,
                swap_indicator_armed_count: 0,
                swap_flash_peak_ticks: 0,
                swap_flash_ticks_remaining: 0,
                window_width: None,
                window_height: None,
                ticks_executed: _,
                has_in_flight_work: false
            }
        ));
    }

    #[test]
    fn runner_loop_surfaces_swap_failure_reason() {
        let config = RunnerConfig {
            max_ticks: 200,
            tick_sleep_micros: 0,
            window: None,
            inject_file_change: Some(PathBuf::from("samples/swap_fail.stasis")),
            watch_directory: None,
            target_mode: TargetMode::JitDev,
            fail_compile: false,
            disable_on_code_swap_hook: false,
            hook_failure_reason: None,
            swap_failure_reason: Some("simulated swap rejection: layout mismatch".to_string()),
        };

        let summary = run_with_default_backend(config);
        assert_eq!(summary.compile_successes, 1);
        assert_eq!(summary.compile_failures, 0);
        assert_eq!(summary.hook_runs, 1);
        assert_eq!(summary.hook_failures, 0);
        assert_eq!(summary.hook_failure_reasons.len(), 0);
        assert_eq!(summary.swap_commit_successes, 0);
        assert_eq!(summary.swap_commit_failures, 1);
        assert_eq!(summary.last_swap_status, Some(SwapCommitStatus::Failed));
        assert_eq!(summary.swap_failure_reasons.len(), 1);
        assert!(summary.swap_failure_reasons[0].contains("layout mismatch"));
        assert_eq!(summary.swap_indicator_armed_count, 0);
        assert_eq!(summary.swap_flash_peak_ticks, 0);
        assert_eq!(summary.swap_flash_ticks_remaining, 0);
        assert!(summary.window.is_none());
        assert!(!summary.has_in_flight_work);
        assert_eq!(summary.events.len(), 4);
        assert!(summary.events.iter().any(|event| matches!(
            event,
            RunnerEvent::SwapCommitResult {
                ref status,
                request_id: _,
                swapped_fn_ids: _,
                new_generation: None,
                ref error
            } if status == "failed" && error.as_deref() == Some("simulated swap rejection: layout mismatch")
        )));
    }

    #[test]
    fn runner_loop_hook_failure_aborts_swap_and_surfaces_error() {
        let config = RunnerConfig {
            max_ticks: 200,
            tick_sleep_micros: 0,
            window: None,
            inject_file_change: Some(PathBuf::from("samples/hook_fail.stasis")),
            watch_directory: None,
            target_mode: TargetMode::JitDev,
            fail_compile: false,
            disable_on_code_swap_hook: false,
            hook_failure_reason: Some("state invariant mismatch".to_string()),
            swap_failure_reason: None,
        };

        let summary = run_with_default_backend(config);
        assert_eq!(summary.compile_successes, 1);
        assert_eq!(summary.compile_failures, 0);
        assert_eq!(summary.hook_runs, 1);
        assert_eq!(summary.hook_failures, 1);
        assert_eq!(summary.hook_failure_reasons.len(), 1);
        assert!(summary.hook_failure_reasons[0].contains("state invariant mismatch"));
        assert_eq!(summary.swap_commit_successes, 0);
        assert_eq!(summary.swap_commit_failures, 1);
        assert_eq!(summary.last_swap_status, Some(SwapCommitStatus::Failed));
        assert_eq!(summary.swap_failure_reasons.len(), 1);
        assert!(summary.swap_failure_reasons[0].contains("on_code_swap failed"));
        assert_eq!(summary.swap_indicator_armed_count, 0);
        assert_eq!(summary.swap_flash_peak_ticks, 0);
        assert_eq!(summary.swap_flash_ticks_remaining, 0);
        assert!(summary.window.is_none());
        assert!(!summary.has_in_flight_work);
        assert_eq!(summary.events.len(), 4);
        assert!(summary.events.iter().any(|event| matches!(
            event,
            RunnerEvent::HookResult {
                ref status,
                request_id: _,
                symbol: _,
                ref error
            } if status == "failed" && error.as_ref().is_some_and(|e| e.contains("on_code_swap failed"))
        )));
    }

    #[test]
    fn watch_directory_change_triggers_compile_and_swap() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_watch_test_{}", stamp));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let watch_file = temp_root.join("game.stasis");
        fs::write(&watch_file, "function main(): i32 { return 0; }\n").expect("write initial file");

        let watch_file_for_thread = watch_file.clone();
        let writer = thread::spawn(move || {
            thread::sleep(Duration::from_millis(40));
            fs::write(
                &watch_file_for_thread,
                "function main(): i32 { return 1; }\n",
            )
            .expect("update watched file");
        });

        let config = RunnerConfig {
            max_ticks: 300,
            tick_sleep_micros: 1000,
            window: None,
            inject_file_change: None,
            watch_directory: Some(temp_root.clone()),
            target_mode: TargetMode::JitDev,
            fail_compile: false,
            disable_on_code_swap_hook: false,
            hook_failure_reason: None,
            swap_failure_reason: None,
        };

        let summary = run_with_default_backend(config);
        writer.join().expect("writer thread join");
        fs::remove_dir_all(&temp_root).expect("cleanup temp dir");

        assert!(summary.compile_successes >= 1);
        assert!(summary.hook_runs >= 1);
        assert_eq!(summary.hook_failures, 0);
        assert!(summary.swap_commit_successes >= 1);
        assert_eq!(summary.swap_commit_failures, 0);
        assert!(summary.swap_indicator_armed_count >= 1);
        assert_eq!(summary.swap_flash_peak_ticks, SWAP_FLASH_TICKS_MAX);
        assert!(summary.swap_flash_ticks_remaining <= SWAP_FLASH_TICKS_MAX);
        assert!(summary.window.is_none());
        assert_eq!(summary.compile_failures, 0);
        assert_eq!(summary.last_swap_status, Some(SwapCommitStatus::Success));
        assert!(!summary.has_in_flight_work);
        assert!(summary.events.len() >= 3);
    }

    #[test]
    fn runner_dispatches_aot_target_mode_when_configured() {
        use std::sync::{Arc, Mutex};

        let seen_modes: Arc<Mutex<Vec<TargetMode>>> = Arc::new(Mutex::new(Vec::new()));
        let seen_modes_capture = Arc::clone(&seen_modes);
        let backend = move |request: CompileRequest| -> CompileResult {
            seen_modes_capture
                .lock()
                .expect("poisoned")
                .push(request.target_mode);
            let patch_set = FunctionPatchSet {
                functions: vec![FunctionPatch { fn_id: FnId(1) }],
            };
            CompileResult::success(request.request_id, LayoutHash([2; 32]), patch_set)
        };

        let config = RunnerConfig {
            max_ticks: 200,
            tick_sleep_micros: 0,
            window: None,
            inject_file_change: Some(PathBuf::from("samples/prod_mode.stasis")),
            watch_directory: None,
            target_mode: TargetMode::AotProd,
            fail_compile: false,
            disable_on_code_swap_hook: false,
            hook_failure_reason: None,
            swap_failure_reason: None,
        };

        let summary = run_with_backend(config, backend);
        assert_eq!(summary.compile_successes, 1);
        assert_eq!(summary.compile_failures, 0);
        assert_eq!(summary.swap_commit_successes, 1);
        assert_eq!(summary.hook_runs, 0);

        let modes = seen_modes.lock().expect("poisoned");
        assert_eq!(modes.as_slice(), &[TargetMode::AotProd]);
    }

    #[test]
    fn brickout_revenge_profile_is_vertical() {
        assert!(BRICKOUT_REVENGE_V1_WINDOW.is_vertical());
    }

    #[test]
    fn brickout_revenge_profile_runs_incremental_swap_loop() {
        let config = brickout_revenge_v1_runner_config(200);
        let summary = run_with_default_backend(config);
        assert_eq!(summary.compile_successes, 1);
        assert_eq!(summary.compile_failures, 0);
        assert_eq!(summary.swap_commit_successes, 1);
        assert_eq!(summary.swap_commit_failures, 0);
        assert_eq!(summary.swap_indicator_armed_count, 1);
        assert_eq!(summary.window, Some(BRICKOUT_REVENGE_V1_WINDOW));
        assert!(summary.window.expect("window should exist").is_vertical());
        assert_eq!(summary.last_swap_status, Some(SwapCommitStatus::Success));
        assert!(!summary.has_in_flight_work);
    }

    #[cfg(windows)]
    #[test]
    fn real_backend_smoke_compiles_and_commits() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tests")
            .join("stasis")
            .join("run_main_returns_7.stasis");
        let config = RunnerConfig {
            max_ticks: 450,
            tick_sleep_micros: 0,
            window: None,
            inject_file_change: Some(fixture),
            watch_directory: None,
            target_mode: TargetMode::JitDev,
            fail_compile: false,
            disable_on_code_swap_hook: false,
            hook_failure_reason: None,
            swap_failure_reason: None,
        };

        let summary = run_with_real_backend(config);
        assert_eq!(summary.compile_successes, 1);
        assert_eq!(summary.compile_failures, 0);
        assert_eq!(summary.swap_commit_successes, 1);
        assert_eq!(summary.swap_commit_failures, 0);
        assert_eq!(summary.last_swap_status, Some(SwapCommitStatus::Success));
        assert!(!summary.has_in_flight_work);
    }
}
