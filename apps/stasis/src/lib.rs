#![forbid(unsafe_code)]

mod watch;

use stasis_runner::swap::contracts::{
    CodeGeneration, CompileRequest, CompileResult, CompileStatus, Diagnostic, DiagnosticSeverity,
    FileChangeEvent, FileChangeKind, FnId, FunctionPatch, FunctionPatchSet, LayoutHash, RequestId,
    SwapCommitResult, SwapCommitStatus, TextSource,
};
use stasis_runner::swap::pipeline::{CompilerBackend, DevHotSwapPipeline};
use std::path::PathBuf;
use std::thread;
use std::time::Duration;
use watch::WatchService;

#[derive(Debug, Clone)]
pub struct RunnerConfig {
    pub max_ticks: u32,
    pub tick_sleep_micros: u64,
    pub inject_file_change: Option<PathBuf>,
    pub watch_directory: Option<PathBuf>,
    pub fail_compile: bool,
    pub swap_failure_reason: Option<String>,
}

impl Default for RunnerConfig {
    fn default() -> Self {
        Self {
            max_ticks: 120,
            tick_sleep_micros: 0,
            inject_file_change: None,
            watch_directory: None,
            fail_compile: false,
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
    pub swap_commit_successes: u32,
    pub swap_commit_failures: u32,
    pub swap_failure_reasons: Vec<String>,
    pub last_swap_status: Option<SwapCommitStatus>,
    pub has_in_flight_work: bool,
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
            CompileResult::success(request.request_id, LayoutHash([1; 32]), patch_set)
        }
    };

    run_with_backend(config, backend)
}

pub fn run_with_backend<B: CompilerBackend>(config: RunnerConfig, backend: B) -> RunnerSummary {
    let mut watcher = config
        .watch_directory
        .as_deref()
        .and_then(|dir| WatchService::start(dir).ok());

    let mut pipeline = DevHotSwapPipeline::new(backend);
    let mut generation: u64 = 0;
    let mut swap_commit_successes: u32 = 0;
    let mut swap_commit_failures: u32 = 0;
    let mut swap_failure_reasons: Vec<String> = Vec::new();
    let mut compile_successes: u32 = 0;
    let mut compile_failures: u32 = 0;
    let mut compile_diagnostics: Vec<String> = Vec::new();
    let mut last_seen_compile_id: Option<RequestId> = None;
    let mut last_swap_status: Option<SwapCommitStatus> = None;
    let mut file_change_sent = false;
    let swap_failure_reason = config.swap_failure_reason.clone();

    let mut observe_results = |pipeline: &DevHotSwapPipeline| {
        if let Some(result) = pipeline.last_compile_result() {
            if last_seen_compile_id != Some(result.request_id) {
                last_seen_compile_id = Some(result.request_id);
                match result.status {
                    CompileStatus::Success => compile_successes += 1,
                    CompileStatus::Failed => {
                        compile_failures += 1;
                        if result.diagnostics.is_empty() {
                            compile_diagnostics
                                .push("compile failed with no diagnostics".to_string());
                        } else {
                            for diagnostic in &result.diagnostics {
                                compile_diagnostics.push(format_diagnostic(diagnostic));
                            }
                        }
                    }
                }
            }
        }

        if let Some(result) = pipeline.last_commit_result() {
            last_swap_status = Some(result.status.clone());
        }
    };

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
            if let Some(reason) = swap_failure_reason.as_ref() {
                swap_commit_failures += 1;
                swap_failure_reasons.push(reason.clone());
                SwapCommitResult::failed(request.request_id, reason.clone())
            } else {
                generation += 1;
                swap_commit_successes += 1;
                let swapped_ids = request
                    .fn_patch_set
                    .functions
                    .iter()
                    .map(|patch| patch.fn_id)
                    .collect();
                SwapCommitResult::success(
                    request.request_id,
                    swapped_ids,
                    CodeGeneration(generation),
                )
            }
        });

        pipeline.pump_coordinator();
        observe_results(&pipeline);
        thread::yield_now();
        if config.tick_sleep_micros > 0 {
            thread::sleep(Duration::from_micros(config.tick_sleep_micros));
        }
    }

    for _ in 0..500 {
        if !pipeline.has_in_flight_work() && pipeline.pending_commit_requests() == 0 {
            break;
        }

        pipeline.pump_coordinator();
        pipeline.process_commits_at_safe_point(|request| {
            if let Some(reason) = swap_failure_reason.as_ref() {
                swap_commit_failures += 1;
                swap_failure_reasons.push(reason.clone());
                SwapCommitResult::failed(request.request_id, reason.clone())
            } else {
                generation += 1;
                swap_commit_successes += 1;
                let swapped_ids = request
                    .fn_patch_set
                    .functions
                    .iter()
                    .map(|patch| patch.fn_id)
                    .collect();
                SwapCommitResult::success(
                    request.request_id,
                    swapped_ids,
                    CodeGeneration(generation),
                )
            }
        });
        pipeline.pump_coordinator();
        observe_results(&pipeline);
        thread::yield_now();
        if config.tick_sleep_micros > 0 {
            thread::sleep(Duration::from_micros(config.tick_sleep_micros));
        }
    }

    RunnerSummary {
        ticks_executed: config.max_ticks,
        compile_successes,
        compile_failures,
        compile_diagnostics,
        swap_commit_successes,
        swap_commit_failures,
        swap_failure_reasons,
        last_swap_status,
        has_in_flight_work: pipeline.has_in_flight_work(),
    }
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
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn runner_loop_compiles_and_commits_one_change() {
        let config = RunnerConfig {
            max_ticks: 200,
            tick_sleep_micros: 0,
            inject_file_change: Some(PathBuf::from(
                "samples/brickout_revenge/brickout_revenge_v1.stasis",
            )),
            watch_directory: None,
            fail_compile: false,
            swap_failure_reason: None,
        };

        let summary = run_with_default_backend(config);
        assert_eq!(summary.compile_successes, 1);
        assert_eq!(summary.compile_failures, 0);
        assert_eq!(summary.compile_diagnostics.len(), 0);
        assert_eq!(summary.swap_commit_successes, 1);
        assert_eq!(summary.swap_commit_failures, 0);
        assert_eq!(summary.swap_failure_reasons.len(), 0);
        assert_eq!(summary.last_swap_status, Some(SwapCommitStatus::Success));
        assert!(!summary.has_in_flight_work);
    }

    #[test]
    fn runner_loop_reports_compile_failure_and_skips_commit() {
        let config = RunnerConfig {
            max_ticks: 200,
            tick_sleep_micros: 0,
            inject_file_change: Some(PathBuf::from("samples/invalid.stasis")),
            watch_directory: None,
            fail_compile: true,
            swap_failure_reason: None,
        };

        let summary = run_with_default_backend(config);
        assert_eq!(summary.compile_successes, 0);
        assert_eq!(summary.compile_failures, 1);
        assert_eq!(summary.swap_commit_successes, 0);
        assert_eq!(summary.swap_commit_failures, 0);
        assert_eq!(summary.swap_failure_reasons.len(), 0);
        assert_eq!(summary.compile_diagnostics.len(), 1);
        assert!(summary.compile_diagnostics[0].contains("simulated compile failure"));
        assert_eq!(summary.last_swap_status, None);
        assert!(!summary.has_in_flight_work);
    }

    #[test]
    fn runner_loop_surfaces_swap_failure_reason() {
        let config = RunnerConfig {
            max_ticks: 200,
            tick_sleep_micros: 0,
            inject_file_change: Some(PathBuf::from("samples/swap_fail.stasis")),
            watch_directory: None,
            fail_compile: false,
            swap_failure_reason: Some("simulated swap rejection: layout mismatch".to_string()),
        };

        let summary = run_with_default_backend(config);
        assert_eq!(summary.compile_successes, 1);
        assert_eq!(summary.compile_failures, 0);
        assert_eq!(summary.swap_commit_successes, 0);
        assert_eq!(summary.swap_commit_failures, 1);
        assert_eq!(summary.last_swap_status, Some(SwapCommitStatus::Failed));
        assert_eq!(summary.swap_failure_reasons.len(), 1);
        assert!(summary.swap_failure_reasons[0].contains("layout mismatch"));
        assert!(!summary.has_in_flight_work);
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
            inject_file_change: None,
            watch_directory: Some(temp_root.clone()),
            fail_compile: false,
            swap_failure_reason: None,
        };

        let summary = run_with_default_backend(config);
        writer.join().expect("writer thread join");
        fs::remove_dir_all(&temp_root).expect("cleanup temp dir");

        assert!(summary.compile_successes >= 1);
        assert!(summary.swap_commit_successes >= 1);
        assert_eq!(summary.swap_commit_failures, 0);
        assert_eq!(summary.compile_failures, 0);
        assert_eq!(summary.last_swap_status, Some(SwapCommitStatus::Success));
        assert!(!summary.has_in_flight_work);
    }
}
