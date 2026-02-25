#![forbid(unsafe_code)]

mod compiler_backend;
mod events;
mod runtime_exec;
pub mod scenarios;
mod self_host_runtime_bridge;
mod stasis_test_runner;
mod watch;

pub use compiler_backend::run_self_host_aot_cli;
pub use events::RunnerEvent;
pub use scenarios::WindowConfig;
pub use self_host_runtime_bridge::{
    publish_cli_args_to_env, publish_source_files_to_env, publish_staged_bridge_paths_to_env,
    restore_cli_args_env, restore_source_files_env, restore_staged_bridge_paths_env,
    stasis_process_env_lock,
};
pub use stasis_test_runner::{
    run_jit_tests_in_directory, run_jit_tests_in_directory_with_session, StasisTestRunSession,
    StasisTestRunSummary,
};

use compiler_backend::IncrementalCompilerBackend;
use runtime_exec::RuntimeLauncher;
use stasis_jit::FunctionPointerTable;
use stasis_runner::swap::contracts::{
    CompileRequest, CompileResult, CompileStatus, Diagnostic, DiagnosticSeverity, FileChangeEvent,
    FileChangeKind, FnId, FunctionPatch, FunctionPatchSet, JitCodePtrOverride, LayoutHash,
    RequestId, SwapCommitResult, SwapCommitStatus, TargetMode, TextSource,
};
use stasis_runner::swap::pipeline::{CompilerBackend, DevHotSwapPipeline};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;
use watch::WatchService;

const SWAP_FLASH_TICKS_MAX: u32 = 180;

#[derive(Debug, Clone, Default)]
struct PendingAotCompileMetadata {
    linked_image_path: Option<PathBuf>,
    linked_image_size_bytes: Option<u64>,
}

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
    pub runtime_launch: bool,
    pub aot_probe_loadability: bool,
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
            runtime_launch: true,
            aot_probe_loadability: false,
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
    pub last_compile_duration_ms: Option<u64>,
    pub last_commit_duration_ms: Option<u64>,
    pub window: Option<WindowConfig>,
    pub last_swap_status: Option<SwapCommitStatus>,
    pub has_in_flight_work: bool,
    pub events: Vec<RunnerEvent>,
    pub runtime_launches: u32,
    pub runtime_launch_failures: u32,
    pub runtime_launch_failure_reasons: Vec<String>,
    pub aot_linked_image_activations: u32,
    pub active_aot_linked_image_path: Option<PathBuf>,
    pub active_aot_linked_image_size_bytes: Option<u64>,
    pub active_aot_linked_image_generation: Option<u64>,
    pub retired_aot_linked_images: u32,
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

fn is_stasis_source_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("stasis"))
}

fn is_test_stasis_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".test.stasis"))
}

fn contains_entry_function(path: &Path) -> bool {
    let Ok(content) = fs::read_to_string(path) else {
        return false;
    };
    content.contains("function main(")
        || content.contains("function tick(")
        || content.contains("function @inline main(")
        || content.contains("function @inline tick(")
}

fn collect_stasis_sources_recursive(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.filter_map(|entry| entry.ok().map(|e| e.path())).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            collect_stasis_sources_recursive(&path, out);
        } else if is_stasis_source_file(&path) {
            out.push(path);
        }
    }
}

fn normalize_watch_path_for_compare(path: &Path) -> PathBuf {
    if path.exists() {
        if let Ok(canonical) = fs::canonicalize(path) {
            return canonical;
        }
    }
    if path.is_absolute() {
        return path.to_path_buf();
    }
    match std::env::current_dir() {
        Ok(cwd) => cwd.join(path),
        Err(_) => path.to_path_buf(),
    }
}

fn parse_watch_import_paths(source: &str) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for line in source.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("import ") {
            continue;
        }
        let Some(first_quote) = trimmed.find('"') else {
            continue;
        };
        let rest = &trimmed[first_quote + 1..];
        let Some(second_quote_rel) = rest.find('"') else {
            continue;
        };
        let candidate = &rest[..second_quote_rel];
        let path = PathBuf::from(candidate);
        if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("stasis"))
        {
            out.push(path);
        }
    }
    out
}

fn collect_watch_dependency_paths(root_source: &Path) -> Result<BTreeSet<PathBuf>, String> {
    if !root_source.exists() {
        return Err(format!(
            "watch root source does not exist: {}",
            root_source.display()
        ));
    }
    let mut out: BTreeSet<PathBuf> = BTreeSet::new();
    let mut queue: Vec<PathBuf> = vec![root_source.to_path_buf()];
    let mut visited: BTreeSet<PathBuf> = BTreeSet::new();
    while let Some(path) = queue.pop() {
        let normalized = normalize_watch_path_for_compare(&path);
        if !visited.insert(normalized.clone()) {
            continue;
        }
        out.insert(normalized.clone());
        let source = fs::read_to_string(&path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        let parent = path.parent().unwrap_or(Path::new("."));
        for import_path in parse_watch_import_paths(&source) {
            let candidate = if import_path.is_absolute() {
                import_path
            } else {
                parent.join(import_path)
            };
            let candidate_normalized = normalize_watch_path_for_compare(&candidate);
            out.insert(candidate_normalized.clone());
            if candidate.exists() {
                queue.push(candidate);
            }
        }
    }
    Ok(out)
}

fn should_submit_watch_event(
    event: &FileChangeEvent,
    root_source: Option<&Path>,
    dependency_paths: Option<&BTreeSet<PathBuf>>,
) -> bool {
    let Some(root_source) = root_source else {
        return true;
    };
    let Some(dependency_paths) = dependency_paths else {
        return true;
    };
    let normalized_event = normalize_watch_path_for_compare(&event.path);
    if normalized_event == normalize_watch_path_for_compare(root_source) {
        return true;
    }
    dependency_paths.contains(&normalized_event)
}

fn infer_watch_directory_entry_source(watch_directory: &Path) -> Option<PathBuf> {
    if !watch_directory.is_dir() {
        return None;
    }

    for preferred in [
        "brickout_revenge_v1.stasis",
        "main.stasis",
        "game.stasis",
        "app.stasis",
    ] {
        let candidate = watch_directory.join(preferred);
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    let mut sources: Vec<PathBuf> = Vec::new();
    collect_stasis_sources_recursive(watch_directory, &mut sources);
    sources.retain(|path| !is_test_stasis_file(path));
    if sources.is_empty() {
        return None;
    }
    if sources.len() == 1 {
        return Some(sources[0].clone());
    }

    let entry_candidates: Vec<PathBuf> = sources
        .iter()
        .filter(|path| contains_entry_function(path))
        .cloned()
        .collect();
    if !entry_candidates.is_empty() {
        return Some(entry_candidates[0].clone());
    }
    Some(sources[0].clone())
}

fn resolve_initial_source_file(config: &RunnerConfig) -> Option<PathBuf> {
    if let Some(explicit) = config.inject_file_change.as_ref() {
        return Some(explicit.clone());
    }
    let watch_directory = config.watch_directory.as_deref()?;
    infer_watch_directory_entry_source(watch_directory)
}

pub fn run_with_backend<B: CompilerBackend>(config: RunnerConfig, backend: B) -> RunnerSummary {
    let mut watcher = config
        .watch_directory
        .as_deref()
        .and_then(|dir| WatchService::start(dir).ok());
    let initial_source_file = resolve_initial_source_file(&config);
    let mut watch_dependency_paths = initial_source_file
        .as_deref()
        .and_then(|source| collect_watch_dependency_paths(source).ok());
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
    let mut last_compile_duration_ms: Option<u64> = None;
    let mut last_commit_duration_ms: Option<u64> = None;
    let mut last_seen_compile_id: Option<RequestId> = None;
    let mut last_seen_commit_id: Option<RequestId> = None;
    let mut last_swap_status: Option<SwapCommitStatus> = None;
    let mut events: Vec<RunnerEvent> = Vec::new();
    let mut file_change_sent = false;
    let hook_failure_reason = config.hook_failure_reason.clone();
    let swap_failure_reason = config.swap_failure_reason.clone();
    let mut pending_aot_metadata: BTreeMap<RequestId, PendingAotCompileMetadata> = BTreeMap::new();
    let mut pending_jit_code_ptr_overrides: BTreeMap<RequestId, Vec<JitCodePtrOverride>> =
        BTreeMap::new();
    let mut aot_linked_image_activations: u32 = 0;
    let mut active_aot_linked_image_path: Option<PathBuf> = None;
    let mut active_aot_linked_image_size_bytes: Option<u64> = None;
    let mut active_aot_linked_image_generation: Option<u64> = None;
    let mut retired_aot_linked_images: u32 = 0;

    let mut runtime_launcher = config
        .runtime_launch
        .then(|| initial_source_file.clone().map(RuntimeLauncher::new))
        .flatten();
    let mut runtime_launch_failures: u32 = 0;
    let mut runtime_launch_failure_reasons: Vec<String> = Vec::new();
    if config.runtime_launch && runtime_launcher.is_none() {
        runtime_launch_failures = 1;
        runtime_launch_failure_reasons.push(
            "runtime launch requested but no --watch-file source file is configured".to_string(),
        );
    }

    for tick in 0..config.max_ticks {
        if !file_change_sent {
            if let Some(path) = &initial_source_file {
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
            let mut refresh_dependency_graph = false;
            for event in watch_service.drain_stasis_changes() {
                if should_submit_watch_event(
                    &event,
                    initial_source_file.as_deref(),
                    watch_dependency_paths.as_ref(),
                ) {
                    refresh_dependency_graph = true;
                    pipeline.submit_file_change(event);
                }
            }
            if refresh_dependency_graph {
                if let Some(root_source) = initial_source_file.as_deref() {
                    if let Ok(next_graph) = collect_watch_dependency_paths(root_source) {
                        watch_dependency_paths = Some(next_graph);
                    }
                }
            }
        }

        pipeline.pump_coordinator();
        capture_pending_aot_compile_metadata(&pipeline, &mut pending_aot_metadata);
        capture_pending_jit_compile_metadata(&pipeline, &mut pending_jit_code_ptr_overrides);
        pipeline.process_commits_at_safe_point(|request| {
            apply_commit_request(
                request,
                &mut pointer_table,
                &config,
                &mut hook_runs,
                &mut hook_failures,
                &mut hook_failure_reasons,
                &mut swap_commit_successes,
                &mut swap_commit_failures,
                &mut swap_failure_reasons,
                &mut events,
                hook_failure_reason.as_ref(),
                swap_failure_reason.as_ref(),
                &pending_aot_metadata,
                &pending_jit_code_ptr_overrides,
            )
        });

        pipeline.pump_coordinator();
        let new_commit = observe_pipeline_results(
            &pipeline,
            &mut last_seen_compile_id,
            &mut last_seen_commit_id,
            &mut compile_successes,
            &mut compile_failures,
            &mut compile_diagnostics,
            &mut last_compile_duration_ms,
            &mut last_commit_duration_ms,
            &mut last_swap_status,
            &mut events,
        );
        if let Some((request_id, status)) = new_commit {
            let aot_metadata = pending_aot_metadata.remove(&request_id);
            pending_jit_code_ptr_overrides.remove(&request_id);
            if status == SwapCommitStatus::Success {
                swap_indicator_armed_count += 1;
                swap_flash_ticks_remaining = SWAP_FLASH_TICKS_MAX;
                swap_flash_peak_ticks = swap_flash_peak_ticks.max(swap_flash_ticks_remaining);
                events.push(RunnerEvent::SwapIndicatorArmed {
                    request_id: request_id.0,
                    ticks: SWAP_FLASH_TICKS_MAX,
                });
                if config.target_mode == TargetMode::AotProd {
                    if let Some(metadata) = aot_metadata {
                        if let Some(linked_path) = metadata.linked_image_path {
                            if active_aot_linked_image_path
                                .as_ref()
                                .is_some_and(|active| active != &linked_path)
                            {
                                retired_aot_linked_images += 1;
                            }
                            active_aot_linked_image_path = Some(linked_path);
                            active_aot_linked_image_size_bytes = metadata.linked_image_size_bytes;
                            active_aot_linked_image_generation = pipeline
                                .last_commit_result()
                                .and_then(|result| result.new_generation.map(|value| value.0));
                            aot_linked_image_activations += 1;
                        }
                    }
                }
                if config.runtime_launch {
                    if let Some(launcher) = runtime_launcher.as_mut() {
                        launcher.restart();
                    }
                }
            }
        } else if let Some(last_compile) = pipeline.last_compile_result() {
            if last_compile.status == CompileStatus::Failed {
                pending_aot_metadata.remove(&last_compile.request_id);
                pending_jit_code_ptr_overrides.remove(&last_compile.request_id);
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
        capture_pending_aot_compile_metadata(&pipeline, &mut pending_aot_metadata);
        capture_pending_jit_compile_metadata(&pipeline, &mut pending_jit_code_ptr_overrides);
        pipeline.process_commits_at_safe_point(|request| {
            apply_commit_request(
                request,
                &mut pointer_table,
                &config,
                &mut hook_runs,
                &mut hook_failures,
                &mut hook_failure_reasons,
                &mut swap_commit_successes,
                &mut swap_commit_failures,
                &mut swap_failure_reasons,
                &mut events,
                hook_failure_reason.as_ref(),
                swap_failure_reason.as_ref(),
                &pending_aot_metadata,
                &pending_jit_code_ptr_overrides,
            )
        });
        pipeline.pump_coordinator();
        let new_commit = observe_pipeline_results(
            &pipeline,
            &mut last_seen_compile_id,
            &mut last_seen_commit_id,
            &mut compile_successes,
            &mut compile_failures,
            &mut compile_diagnostics,
            &mut last_compile_duration_ms,
            &mut last_commit_duration_ms,
            &mut last_swap_status,
            &mut events,
        );
        if let Some((request_id, status)) = new_commit {
            let aot_metadata = pending_aot_metadata.remove(&request_id);
            pending_jit_code_ptr_overrides.remove(&request_id);
            if status == SwapCommitStatus::Success {
                swap_indicator_armed_count += 1;
                swap_flash_ticks_remaining = SWAP_FLASH_TICKS_MAX;
                swap_flash_peak_ticks = swap_flash_peak_ticks.max(swap_flash_ticks_remaining);
                events.push(RunnerEvent::SwapIndicatorArmed {
                    request_id: request_id.0,
                    ticks: SWAP_FLASH_TICKS_MAX,
                });
                if config.target_mode == TargetMode::AotProd {
                    if let Some(metadata) = aot_metadata {
                        if let Some(linked_path) = metadata.linked_image_path {
                            if active_aot_linked_image_path
                                .as_ref()
                                .is_some_and(|active| active != &linked_path)
                            {
                                retired_aot_linked_images += 1;
                            }
                            active_aot_linked_image_path = Some(linked_path);
                            active_aot_linked_image_size_bytes = metadata.linked_image_size_bytes;
                            active_aot_linked_image_generation = pipeline
                                .last_commit_result()
                                .and_then(|result| result.new_generation.map(|value| value.0));
                            aot_linked_image_activations += 1;
                        }
                    }
                }
                if config.runtime_launch {
                    if let Some(launcher) = runtime_launcher.as_mut() {
                        launcher.restart();
                    }
                }
            }
        } else if let Some(last_compile) = pipeline.last_compile_result() {
            if last_compile.status == CompileStatus::Failed {
                pending_aot_metadata.remove(&last_compile.request_id);
                pending_jit_code_ptr_overrides.remove(&last_compile.request_id);
            }
        }
        thread::yield_now();
        thread::sleep(Duration::from_millis(1));
    }

    let runtime_launches = runtime_launcher
        .as_ref()
        .map(|launcher| launcher.summary().launches)
        .unwrap_or(0);
    if let Some(launcher) = runtime_launcher.as_ref() {
        runtime_launch_failures += launcher.summary().failures;
        runtime_launch_failure_reasons.extend(launcher.summary().failure_reasons.iter().cloned());
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
        last_compile_duration_ms,
        last_commit_duration_ms,
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
        last_compile_duration_ms,
        last_commit_duration_ms,
        window,
        last_swap_status,
        has_in_flight_work,
        events,
        runtime_launches,
        runtime_launch_failures,
        runtime_launch_failure_reasons,
        aot_linked_image_activations,
        active_aot_linked_image_path,
        active_aot_linked_image_size_bytes,
        active_aot_linked_image_generation,
        retired_aot_linked_images,
    }
}

fn capture_pending_aot_compile_metadata(
    pipeline: &DevHotSwapPipeline,
    pending_aot_metadata: &mut BTreeMap<RequestId, PendingAotCompileMetadata>,
) {
    let Some(result) = pipeline.last_compile_result() else {
        return;
    };
    if result.status != CompileStatus::Success {
        return;
    }
    pending_aot_metadata
        .entry(result.request_id)
        .or_insert_with(|| PendingAotCompileMetadata {
            linked_image_path: result.aot_linked_image_path.clone(),
            linked_image_size_bytes: result.aot_linked_image_size_bytes,
        });
}

fn capture_pending_jit_compile_metadata(
    pipeline: &DevHotSwapPipeline,
    pending_jit_code_ptr_overrides: &mut BTreeMap<RequestId, Vec<JitCodePtrOverride>>,
) {
    let Some(result) = pipeline.last_compile_result() else {
        return;
    };
    if result.status != CompileStatus::Success {
        return;
    }
    let Some(overrides) = result.jit_code_ptr_overrides.clone() else {
        return;
    };
    pending_jit_code_ptr_overrides
        .entry(result.request_id)
        .or_insert(overrides);
}

#[allow(clippy::too_many_arguments)]
fn apply_commit_request(
    request: stasis_runner::swap::contracts::SwapCommitRequest,
    pointer_table: &mut FunctionPointerTable,
    config: &RunnerConfig,
    hook_runs: &mut u32,
    hook_failures: &mut u32,
    hook_failure_reasons: &mut Vec<String>,
    swap_commit_successes: &mut u32,
    swap_commit_failures: &mut u32,
    swap_failure_reasons: &mut Vec<String>,
    events: &mut Vec<RunnerEvent>,
    hook_failure_reason: Option<&String>,
    swap_failure_reason: Option<&String>,
    pending_aot_metadata: &BTreeMap<RequestId, PendingAotCompileMetadata>,
    pending_jit_code_ptr_overrides: &BTreeMap<RequestId, Vec<JitCodePtrOverride>>,
) -> SwapCommitResult {
    if !config.disable_on_code_swap_hook {
        if let Some(hook_symbol) = request.hook_symbol.as_deref() {
            *hook_runs += 1;
            if let Some(reason) = hook_failure_reason {
                *hook_failures += 1;
                hook_failure_reasons.push(reason.clone());
                let hook_error = format!("{hook_symbol} failed: {reason}");
                events.push(RunnerEvent::HookResult {
                    request_id: request.request_id.0,
                    symbol: hook_symbol.to_string(),
                    status: "failed".to_string(),
                    error: Some(hook_error.clone()),
                });
                *swap_commit_failures += 1;
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

    if let Some(reason) = swap_failure_reason {
        *swap_commit_failures += 1;
        swap_failure_reasons.push(reason.clone());
        return SwapCommitResult::failed(request.request_id, reason.clone());
    }

    if config.target_mode == TargetMode::AotProd && config.aot_probe_loadability {
        let Some(metadata) = pending_aot_metadata.get(&request.request_id) else {
            let message = format!(
                "AOT loadability probe failed for request {}: missing compile metadata",
                request.request_id.0
            );
            *swap_commit_failures += 1;
            swap_failure_reasons.push(message.clone());
            return SwapCommitResult::failed(request.request_id, message);
        };
        let Some(path) = metadata.linked_image_path.as_ref() else {
            let message = format!(
                "AOT loadability probe failed for request {}: missing linked image path",
                request.request_id.0
            );
            *swap_commit_failures += 1;
            swap_failure_reasons.push(message.clone());
            return SwapCommitResult::failed(request.request_id, message);
        };
        if let Err(message) = probe_aot_loadability(path) {
            *swap_commit_failures += 1;
            swap_failure_reasons.push(message.clone());
            return SwapCommitResult::failed(request.request_id, message);
        }
    }

    *swap_commit_successes += 1;
    let outcome = if config.target_mode == TargetMode::JitDev {
        if let Some(overrides) = pending_jit_code_ptr_overrides.get(&request.request_id) {
            pointer_table.commit_patch_set_with_overrides(&request.fn_patch_set, overrides)
        } else {
            pointer_table.commit_patch_set(&request.fn_patch_set)
        }
    } else {
        pointer_table.commit_patch_set(&request.fn_patch_set)
    };
    SwapCommitResult::success(
        request.request_id,
        outcome.swapped_fn_ids,
        outcome.new_generation,
    )
}

fn observe_pipeline_results(
    pipeline: &DevHotSwapPipeline,
    last_seen_compile_id: &mut Option<RequestId>,
    last_seen_commit_id: &mut Option<RequestId>,
    compile_successes: &mut u32,
    compile_failures: &mut u32,
    compile_diagnostics: &mut Vec<String>,
    last_compile_duration_ms: &mut Option<u64>,
    last_commit_duration_ms: &mut Option<u64>,
    last_swap_status: &mut Option<SwapCommitStatus>,
    events: &mut Vec<RunnerEvent>,
) -> Option<(RequestId, SwapCommitStatus)> {
    let mut new_commit: Option<(RequestId, SwapCommitStatus)> = None;
    if let Some(result) = pipeline.last_compile_result() {
        if *last_seen_compile_id != Some(result.request_id) {
            *last_seen_compile_id = Some(result.request_id);
            *last_compile_duration_ms = pipeline.last_compile_duration().map(duration_ms);
            match result.status {
                CompileStatus::Success => {
                    *compile_successes += 1;
                    events.push(RunnerEvent::CompileResult {
                        request_id: result.request_id.0,
                        status: "success".to_string(),
                        diagnostics: Vec::new(),
                        compile_duration_ms: *last_compile_duration_ms,
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
                        compile_duration_ms: *last_compile_duration_ms,
                    });
                }
            }
        }
    }

    if let Some(result) = pipeline.last_commit_result() {
        *last_swap_status = Some(result.status.clone());
        if *last_seen_commit_id != Some(result.request_id) {
            *last_seen_commit_id = Some(result.request_id);
            *last_commit_duration_ms = pipeline.last_commit_duration().map(duration_ms);
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
                commit_duration_ms: *last_commit_duration_ms,
            });
        }
    }
    new_commit
}

fn duration_ms(duration: Duration) -> u64 {
    let millis = duration.as_millis();
    u64::try_from(millis).unwrap_or(u64::MAX)
}

fn probe_aot_loadability(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Err(format!(
            "AOT loadability probe failed: linked image does not exist at {}",
            path.display()
        ));
    }
    #[cfg(windows)]
    {
        stasis_dynload::Library::load(path)
            .map(|_| ())
            .map_err(|error| {
                format!(
                    "AOT loadability probe failed for {}: {error}",
                    path.display()
                )
            })
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err("AOT loadability probe is currently supported on Windows only".to_string())
    }
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
    fn apply_commit_request_uses_jit_code_ptr_overrides_when_present() {
        let request_id = RequestId(44);
        let request = stasis_runner::swap::contracts::SwapCommitRequest::new(
            request_id,
            LayoutHash([7; 32]),
            FunctionPatchSet {
                functions: vec![FunctionPatch { fn_id: FnId(9) }],
            },
            None,
        );

        let mut pointer_table = FunctionPointerTable::new();
        let config = RunnerConfig::default();
        let mut hook_runs = 0u32;
        let mut hook_failures = 0u32;
        let mut hook_failure_reasons = Vec::new();
        let mut swap_commit_successes = 0u32;
        let mut swap_commit_failures = 0u32;
        let mut swap_failure_reasons = Vec::new();
        let mut events = Vec::new();
        let pending_aot_metadata: BTreeMap<RequestId, PendingAotCompileMetadata> = BTreeMap::new();
        let mut pending_jit_code_ptr_overrides: BTreeMap<RequestId, Vec<JitCodePtrOverride>> =
            BTreeMap::new();
        pending_jit_code_ptr_overrides.insert(
            request_id,
            vec![JitCodePtrOverride {
                fn_id: FnId(9),
                code_ptr: 0x9988,
            }],
        );

        let result = apply_commit_request(
            request,
            &mut pointer_table,
            &config,
            &mut hook_runs,
            &mut hook_failures,
            &mut hook_failure_reasons,
            &mut swap_commit_successes,
            &mut swap_commit_failures,
            &mut swap_failure_reasons,
            &mut events,
            None,
            None,
            &pending_aot_metadata,
            &pending_jit_code_ptr_overrides,
        );

        assert_eq!(result.status, SwapCommitStatus::Success);
        assert_eq!(swap_commit_successes, 1);
        assert_eq!(
            pointer_table.code_ptr(FnId(9)),
            Some(stasis_jit::CodePtr(0x9988))
        );
    }

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
            runtime_launch: false,
            aot_probe_loadability: false,
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
                has_in_flight_work: false,
                ..
            }
        ));
        assert!(summary.events.iter().any(|event| matches!(
            event,
            RunnerEvent::CompileResult {
                ref status,
                request_id: _,
                diagnostics: _,
                ..
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
                error: _,
                ..
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
            runtime_launch: false,
            aot_probe_loadability: false,
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
                ref diagnostics,
                ..
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
                has_in_flight_work: false,
                ..
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
            runtime_launch: false,
            aot_probe_loadability: false,
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
                ref error,
                ..
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
            runtime_launch: false,
            aot_probe_loadability: false,
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
            runtime_launch: false,
            aot_probe_loadability: false,
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
    fn watch_directory_dependency_change_triggers_recompile() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_watch_dep_change_{}", stamp));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let root_file = temp_root.join("game.stasis");
        let dep_file = temp_root.join("dep.stasis");
        fs::write(
            &root_file,
            "import \"./dep.stasis\";\nfunction main(): i32 { return dep(); }\n",
        )
        .expect("write root");
        fs::write(&dep_file, "function dep(): i32 { return 0; }\n").expect("write dep");

        let dep_file_for_thread = dep_file.clone();
        let writer = thread::spawn(move || {
            thread::sleep(Duration::from_millis(40));
            fs::write(&dep_file_for_thread, "function dep(): i32 { return 1; }\n")
                .expect("update dependency file");
        });

        let config = RunnerConfig {
            max_ticks: 300,
            tick_sleep_micros: 1000,
            window: None,
            inject_file_change: Some(root_file),
            watch_directory: Some(temp_root.clone()),
            target_mode: TargetMode::JitDev,
            fail_compile: false,
            disable_on_code_swap_hook: false,
            hook_failure_reason: None,
            swap_failure_reason: None,
            runtime_launch: false,
            aot_probe_loadability: false,
        };

        let summary = run_with_default_backend(config);
        writer.join().expect("writer thread join");
        fs::remove_dir_all(&temp_root).expect("cleanup temp dir");

        assert!(summary.compile_successes >= 2);
        assert!(summary.swap_commit_successes >= 2);
        assert_eq!(summary.compile_failures, 0);
    }

    #[test]
    fn watch_directory_ignores_non_dependency_changes() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        let temp_root =
            std::env::temp_dir().join(format!("stasis_watch_ignore_unrelated_{}", stamp));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let root_file = temp_root.join("game.stasis");
        let dep_file = temp_root.join("dep.stasis");
        let unrelated_file = temp_root.join("unrelated.stasis");
        fs::write(
            &root_file,
            "import \"./dep.stasis\";\nfunction main(): i32 { return dep(); }\n",
        )
        .expect("write root");
        fs::write(&dep_file, "function dep(): i32 { return 0; }\n").expect("write dep");
        fs::write(&unrelated_file, "function helper(): i32 { return 0; }\n")
            .expect("write unrelated");

        let unrelated_file_for_thread = unrelated_file.clone();
        let writer = thread::spawn(move || {
            thread::sleep(Duration::from_millis(40));
            fs::write(
                &unrelated_file_for_thread,
                "function helper(): i32 { return 1; }\n",
            )
            .expect("update unrelated file");
        });

        let config = RunnerConfig {
            max_ticks: 300,
            tick_sleep_micros: 1000,
            window: None,
            inject_file_change: Some(root_file),
            watch_directory: Some(temp_root.clone()),
            target_mode: TargetMode::JitDev,
            fail_compile: false,
            disable_on_code_swap_hook: false,
            hook_failure_reason: None,
            swap_failure_reason: None,
            runtime_launch: false,
            aot_probe_loadability: false,
        };

        let summary = run_with_default_backend(config);
        writer.join().expect("writer thread join");
        fs::remove_dir_all(&temp_root).expect("cleanup temp dir");

        assert_eq!(summary.compile_successes, 1);
        assert_eq!(summary.swap_commit_successes, 1);
        assert_eq!(summary.compile_failures, 0);
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
            runtime_launch: false,
            aot_probe_loadability: false,
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
    fn runtime_launch_requires_injected_source_file() {
        let config = RunnerConfig {
            max_ticks: 1,
            tick_sleep_micros: 0,
            window: None,
            inject_file_change: None,
            watch_directory: None,
            target_mode: TargetMode::JitDev,
            fail_compile: false,
            disable_on_code_swap_hook: false,
            hook_failure_reason: None,
            swap_failure_reason: None,
            runtime_launch: true,
            aot_probe_loadability: false,
        };

        let summary = run_with_default_backend(config);
        assert_eq!(summary.runtime_launches, 0);
        assert_eq!(summary.runtime_launch_failures, 1);
        assert!(summary
            .runtime_launch_failure_reasons
            .iter()
            .any(|reason| reason.contains("no --watch-file source file")));
    }

    #[test]
    fn resolve_initial_source_file_prefers_explicit_watch_file() {
        let config = RunnerConfig {
            max_ticks: 1,
            tick_sleep_micros: 0,
            window: None,
            inject_file_change: Some(PathBuf::from("samples/explicit.stasis")),
            watch_directory: Some(PathBuf::from("samples/brickout_revenge")),
            target_mode: TargetMode::JitDev,
            fail_compile: false,
            disable_on_code_swap_hook: false,
            hook_failure_reason: None,
            swap_failure_reason: None,
            runtime_launch: true,
            aot_probe_loadability: false,
        };

        let resolved = resolve_initial_source_file(&config).expect("resolved source file");
        assert_eq!(resolved, PathBuf::from("samples/explicit.stasis"));
    }

    #[test]
    fn resolve_initial_source_file_infers_brickout_entry_from_watch_dir() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_watch_entry_infer_{}", stamp));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        fs::write(temp_root.join("helper.stasis"), "function util(): i32 { return 1; }\n")
            .expect("write helper");
        fs::write(
            temp_root.join("brickout_revenge_v1.stasis"),
            "function tick(): i32 { return 0; }\nfunction render(): i32 { return 0; }\n",
        )
        .expect("write entry");

        let config = RunnerConfig {
            max_ticks: 1,
            tick_sleep_micros: 0,
            window: None,
            inject_file_change: None,
            watch_directory: Some(temp_root.clone()),
            target_mode: TargetMode::JitDev,
            fail_compile: false,
            disable_on_code_swap_hook: false,
            hook_failure_reason: None,
            swap_failure_reason: None,
            runtime_launch: true,
            aot_probe_loadability: false,
        };

        let resolved = resolve_initial_source_file(&config).expect("resolved source file");
        assert_eq!(resolved, temp_root.join("brickout_revenge_v1.stasis"));

        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn collect_watch_dependency_paths_includes_nested_imports() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_watch_dep_graph_{}", stamp));
        let sub_dir = temp_root.join("sub");
        fs::create_dir_all(&sub_dir).expect("create temp dirs");
        let root = temp_root.join("main.stasis");
        let dep = temp_root.join("dep.stasis");
        let dep2 = sub_dir.join("dep2.stasis");
        fs::write(
            &root,
            "import \"./dep.stasis\";\nfunction tick(): i32 { return dep(); }\n",
        )
        .expect("write root");
        fs::write(
            &dep,
            "import \"./sub/dep2.stasis\";\nfunction dep(): i32 { return dep2(); }\n",
        )
        .expect("write dep");
        fs::write(&dep2, "function dep2(): i32 { return 1; }\n").expect("write dep2");

        let graph = collect_watch_dependency_paths(&root).expect("dependency graph");
        assert!(graph.contains(&normalize_watch_path_for_compare(&root)));
        assert!(graph.contains(&normalize_watch_path_for_compare(&dep)));
        assert!(graph.contains(&normalize_watch_path_for_compare(&dep2)));

        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn should_submit_watch_event_filters_non_dependency_paths() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_watch_filter_{}", stamp));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let root = temp_root.join("root.stasis");
        let dep = temp_root.join("dep.stasis");
        let other = temp_root.join("other.stasis");
        fs::write(&root, "function tick(): i32 { return 0; }\n").expect("write root");
        fs::write(&dep, "function dep(): i32 { return 0; }\n").expect("write dep");
        fs::write(&other, "function other(): i32 { return 0; }\n").expect("write other");

        let mut dependency_paths = std::collections::BTreeSet::new();
        dependency_paths.insert(normalize_watch_path_for_compare(&root));
        dependency_paths.insert(normalize_watch_path_for_compare(&dep));

        let dep_event = FileChangeEvent::new(
            dep.clone(),
            1,
            TextSource::FileWatcher,
            FileChangeKind::Modified,
        );
        let other_event = FileChangeEvent::new(
            other.clone(),
            2,
            TextSource::FileWatcher,
            FileChangeKind::Modified,
        );
        assert!(should_submit_watch_event(
            &dep_event,
            Some(&root),
            Some(&dependency_paths)
        ));
        assert!(!should_submit_watch_event(
            &other_event,
            Some(&root),
            Some(&dependency_paths)
        ));

        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn aot_probe_loadability_rejects_missing_linked_image() {
        let missing_linked_image = std::env::temp_dir().join(format!(
            "stasis_missing_probe_{}.dll",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        if missing_linked_image.exists() {
            fs::remove_file(&missing_linked_image).ok();
        }

        let linked_image_for_backend = missing_linked_image.clone();
        let backend = move |request: CompileRequest| -> CompileResult {
            CompileResult::success_with_host_set_metadata(
                request.request_id,
                LayoutHash([3; 32]),
                FunctionPatchSet {
                    functions: vec![FunctionPatch { fn_id: FnId(1) }],
                },
                None,
                None,
                None,
                None,
                Some(linked_image_for_backend.clone()),
                Some(128),
                Some("abc".to_string()),
                None,
            )
        };

        let config = RunnerConfig {
            max_ticks: 200,
            tick_sleep_micros: 0,
            window: None,
            inject_file_change: Some(PathBuf::from("samples/probe_missing.stasis")),
            watch_directory: None,
            target_mode: TargetMode::AotProd,
            fail_compile: false,
            disable_on_code_swap_hook: false,
            hook_failure_reason: None,
            swap_failure_reason: None,
            runtime_launch: false,
            aot_probe_loadability: true,
        };

        let summary = run_with_backend(config, backend);
        assert_eq!(summary.compile_successes, 1);
        assert_eq!(summary.swap_commit_successes, 0);
        assert_eq!(summary.swap_commit_failures, 1);
        assert!(summary
            .swap_failure_reasons
            .iter()
            .any(|reason| reason.contains("AOT loadability probe failed")));
        assert_eq!(summary.aot_linked_image_activations, 0);
    }

    #[test]
    fn aot_activation_tracks_latest_linked_image_metadata() {
        let linked_image = std::env::temp_dir().join(format!(
            "stasis_activation_probe_{}.dll",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::write(&linked_image, "fake-linked-image").expect("write linked image fixture");

        let linked_image_for_backend = linked_image.clone();
        let backend = move |request: CompileRequest| -> CompileResult {
            CompileResult::success_with_host_set_metadata(
                request.request_id,
                LayoutHash([5; 32]),
                FunctionPatchSet {
                    functions: vec![FunctionPatch { fn_id: FnId(1) }],
                },
                None,
                None,
                None,
                None,
                Some(linked_image_for_backend.clone()),
                Some(17),
                Some("abcd".to_string()),
                None,
            )
        };

        let config = RunnerConfig {
            max_ticks: 200,
            tick_sleep_micros: 0,
            window: None,
            inject_file_change: Some(PathBuf::from("samples/prod_activation.stasis")),
            watch_directory: None,
            target_mode: TargetMode::AotProd,
            fail_compile: false,
            disable_on_code_swap_hook: false,
            hook_failure_reason: None,
            swap_failure_reason: None,
            runtime_launch: false,
            aot_probe_loadability: false,
        };

        let summary = run_with_backend(config, backend);
        fs::remove_file(&linked_image).ok();
        assert_eq!(summary.compile_successes, 1);
        assert_eq!(summary.swap_commit_successes, 1);
        assert_eq!(summary.aot_linked_image_activations, 1);
        assert_eq!(
            summary.active_aot_linked_image_path,
            Some(linked_image.clone())
        );
        assert_eq!(summary.active_aot_linked_image_size_bytes, Some(17));
        assert_eq!(summary.active_aot_linked_image_generation, Some(1));
        assert_eq!(summary.retired_aot_linked_images, 0);
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
    fn real_backend_smoke_compiles_and_commits_literal_main() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tests")
            .join("stasis")
            .join("rust_native_jit_smoke_main_returns_7.stasis");
        let config = RunnerConfig {
            // Real backend compile can take multiple seconds on busy CI/dev hosts.
            max_ticks: 7000,
            tick_sleep_micros: 1000,
            window: None,
            inject_file_change: Some(fixture),
            watch_directory: None,
            target_mode: TargetMode::JitDev,
            fail_compile: false,
            disable_on_code_swap_hook: false,
            hook_failure_reason: None,
            swap_failure_reason: None,
            runtime_launch: false,
            aot_probe_loadability: false,
        };

        let summary = run_with_real_backend(config);
        assert_eq!(summary.compile_successes, 1);
        assert_eq!(summary.compile_failures, 0);
        assert_eq!(summary.swap_commit_successes, 1);
        assert_eq!(summary.swap_commit_failures, 0);
        assert_eq!(summary.last_swap_status, Some(SwapCommitStatus::Success));
        assert!(!summary.has_in_flight_work);
    }

    #[cfg(windows)]
    #[test]
    fn real_backend_smoke_compiles_and_commits_binary_literal_main() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tests")
            .join("stasis")
            .join("rust_native_jit_smoke_main_returns_6_binary.stasis");
        let config = RunnerConfig {
            // Real backend compile can take multiple seconds on busy CI/dev hosts.
            max_ticks: 7000,
            tick_sleep_micros: 1000,
            window: None,
            inject_file_change: Some(fixture),
            watch_directory: None,
            target_mode: TargetMode::JitDev,
            fail_compile: false,
            disable_on_code_swap_hook: false,
            hook_failure_reason: None,
            swap_failure_reason: None,
            runtime_launch: false,
            aot_probe_loadability: false,
        };

        let summary = run_with_real_backend(config);
        assert_eq!(summary.compile_successes, 1);
        assert_eq!(summary.compile_failures, 0);
        assert_eq!(summary.swap_commit_successes, 1);
        assert_eq!(summary.swap_commit_failures, 0);
        assert_eq!(summary.last_swap_status, Some(SwapCommitStatus::Success));
        assert!(!summary.has_in_flight_work);
    }

    #[cfg(windows)]
    #[test]
    fn real_backend_smoke_compiles_and_commits_void_hook_and_literal_main() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tests")
            .join("stasis")
            .join("rust_native_jit_smoke_main_and_hook.stasis");
        let config = RunnerConfig {
            // Real backend compile can take multiple seconds on busy CI/dev hosts.
            max_ticks: 7000,
            tick_sleep_micros: 1000,
            window: None,
            inject_file_change: Some(fixture),
            watch_directory: None,
            target_mode: TargetMode::JitDev,
            fail_compile: false,
            disable_on_code_swap_hook: false,
            hook_failure_reason: None,
            swap_failure_reason: None,
            runtime_launch: false,
            aot_probe_loadability: false,
        };

        let summary = run_with_real_backend(config);
        assert_eq!(summary.compile_successes, 1);
        assert_eq!(summary.compile_failures, 0);
        assert_eq!(summary.swap_commit_successes, 1);
        assert_eq!(summary.swap_commit_failures, 0);
        assert_eq!(summary.last_swap_status, Some(SwapCommitStatus::Success));
        assert!(!summary.has_in_flight_work);
    }

    #[cfg(windows)]
    #[test]
    fn real_backend_smoke_compiles_and_commits_brickout_v1() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("samples")
            .join("brickout_revenge")
            .join("brickout_revenge_v1.stasis");
        let config = RunnerConfig {
            max_ticks: 7000,
            tick_sleep_micros: 1000,
            window: Some(BRICKOUT_REVENGE_V1_WINDOW),
            inject_file_change: Some(fixture),
            watch_directory: None,
            target_mode: TargetMode::JitDev,
            fail_compile: false,
            disable_on_code_swap_hook: false,
            hook_failure_reason: None,
            swap_failure_reason: None,
            runtime_launch: false,
            aot_probe_loadability: false,
        };

        let summary = run_with_real_backend(config);
        assert_eq!(summary.compile_successes, 1);
        assert_eq!(summary.compile_failures, 0);
        assert_eq!(summary.swap_commit_successes, 1);
        assert_eq!(summary.swap_commit_failures, 0);
        assert_eq!(summary.last_swap_status, Some(SwapCommitStatus::Success));
        assert_eq!(summary.window, Some(BRICKOUT_REVENGE_V1_WINDOW));
        assert!(!summary.has_in_flight_work);
    }
}
