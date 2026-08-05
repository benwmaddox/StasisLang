use super::{
    hex_sha256, image_extension, import_references, read_bounded_bytes, read_bounded_utf8,
    write_json, GauntletConfigV1, GauntletObserver, GauntletReference, GauntletRoleModel,
    GauntletRunPhase, GauntletRunStateV1, GAUNTLET_SCHEMA_VERSION, MAX_GOAL_BYTES,
    MAX_REFERENCE_BYTES,
};
use crate::toolchain_cli::live_tui;
use crate::toolchain_cli::{CommandResult, Workspace};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use stasis::{run_live_in_process, LiveRunConfig};
use stasis_ai::{AgentCompactionPolicy, AgentProfile, CodexExecProvider, ModelProvider};
use stasis_runner::live::{
    live_session, LiveCommand, LivePointerInput, LiveRequest, LiveResponse, LiveSessionClient,
};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const RUNS_PATH: &str = "build/gauntlet";
const RUN_STATE_NAME: &str = "run.json";
const EFFECTIVE_CONFIG_NAME: &str = "effective-config.json";
const EVENTS_NAME: &str = "events.jsonl";
const USAGE_NAME: &str = "usage.jsonl";
const DECISIONS_NAME: &str = "decisions.jsonl";
const REPORT_NAME: &str = "index.html";
const STOP_NAME: &str = "stop.request";
const QUALITY_BAR_NAME: &str = "quality-bar.json";
const MAX_CAPTURE_WAIT: Duration = Duration::from_secs(10);
const FINAL_ACCEPTANCES: u32 = 2;
const MAX_MEMORY_RECORDS: usize = 48;
const MAX_MEMORY_CHARS: usize = 32 * 1024;
const MAX_DECISION_FIELD_CHARS: usize = 2_000;

struct StopWatcher {
    done: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl StopWatcher {
    fn start(artifacts: &Path, canceled: Arc<AtomicBool>, wall_limit: Duration) -> Self {
        let done = Arc::new(AtomicBool::new(false));
        let worker_done = Arc::clone(&done);
        let stop_path = artifacts.join(STOP_NAME);
        let worker = thread::spawn(move || {
            let started = Instant::now();
            while !worker_done.load(Ordering::Acquire) {
                if stop_path.is_file() || started.elapsed() >= wall_limit {
                    canceled.store(true, Ordering::Release);
                    break;
                }
                thread::sleep(Duration::from_millis(100));
            }
        });
        Self {
            done,
            worker: Some(worker),
        }
    }
}

impl Drop for StopWatcher {
    fn drop(&mut self) {
        self.done.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrozenBar {
    schema_version: u32,
    goal_sha256: String,
    goal: String,
    workstreams: Vec<String>,
    hard_gates: Vec<String>,
    required_scenarios: Vec<Value>,
    references: Vec<GauntletReference>,
    web_sources: Vec<ScoutSource>,
    acceptance_score: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScoutResult {
    summary: String,
    sources: Vec<ScoutSource>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScoutSource {
    url: String,
    relevance: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LeadBootstrap {
    workstreams: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LeadDecision {
    done: bool,
    workstream: String,
    builder_prompt: String,
    rationale: String,
    next_step: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BlindCritique {
    preferred: String,
    score_a: u32,
    score_b: u32,
    largest_gap: String,
    summary: String,
}

pub(super) fn start(
    workspace: &Workspace,
    config_path: &Path,
    additional_references: &[PathBuf],
    discover_references: bool,
    observer: GauntletObserver,
    max_hours: Option<u32>,
    max_model_calls: Option<u32>,
) -> Result<CommandResult, String> {
    let config_path = if config_path.is_absolute() {
        config_path.to_path_buf()
    } else {
        workspace.root.join(config_path)
    };
    let source = fs::read_to_string(&config_path)
        .map_err(|error| format!("failed reading {}: {error}", config_path.display()))?;
    let mut config: GauntletConfigV1 = serde_json::from_str(&source)
        .map_err(|error| format!("invalid Gauntlet config {}: {error}", config_path.display()))?;
    config.validate()?;
    config.execution.observer = observer;
    config.quality_bar.allow_web_discovery |= discover_references;
    if let Some(hours) = max_hours {
        config.budget.wall_time_minutes = hours
            .checked_mul(60)
            .filter(|value| *value > 0)
            .ok_or_else(|| "Gauntlet max-hours must be greater than zero".to_string())?;
    }
    if let Some(calls) = max_model_calls {
        if calls == 0 {
            return Err("Gauntlet max-model-calls must be greater than zero".to_string());
        }
        config.budget.model_calls = calls;
    }
    config.validate()?;

    ensure_initial_commit(&workspace.root)?;
    require_clean_checkout(&workspace.root)?;
    let base_commit = git_stdout(&workspace.root, &["rev-parse", "HEAD"])?;
    let run_id = new_run_id(&base_commit);
    let branch = format!("stasis/gauntlet/{run_id}");
    let artifacts = run_artifacts(&workspace.root, &run_id);
    let worktree = workspace
        .root
        .join(RUNS_PATH)
        .join("worktrees")
        .join(&run_id);
    fs::create_dir_all(&artifacts)
        .map_err(|error| format!("failed creating Gauntlet run directory: {error}"))?;
    let mut references = config.quality_bar.references.clone();
    references.extend(import_references(&workspace.root, additional_references)?);
    config.quality_bar.references = freeze_references(&workspace.root, &run_id, references)?;
    config.validate()?;
    write_json(&artifacts.join(EFFECTIVE_CONFIG_NAME), &config)?;
    git_ok(
        &workspace.root,
        &[
            "worktree",
            "add",
            "-b",
            &branch,
            &worktree.to_string_lossy(),
            &base_commit,
        ],
    )?;
    let bootstrap_commit = ensure_worktree_ignores(&worktree)?;
    let now = unix_ms();
    let mut state = GauntletRunStateV1 {
        schema_version: GAUNTLET_SCHEMA_VERSION,
        run_id: run_id.clone(),
        phase: GauntletRunPhase::Created,
        project_root: worktree.to_string_lossy().to_string(),
        original_root: workspace.root.to_string_lossy().to_string(),
        branch,
        base_commit: base_commit.clone(),
        best_commit: Some(bootstrap_commit),
        current_workstream: None,
        model_calls: 0,
        accepted_candidates: 0,
        rejected_candidates: 0,
        consecutive_stalls: 0,
        started_unix_ms: now,
        updated_unix_ms: now,
        terminal_reason: None,
    };
    persist_state(&artifacts, &mut state)?;
    emit_event(
        &artifacts,
        "run_created",
        json!({"run_id": run_id, "branch": state.branch, "worktree": worktree}),
    )?;
    run_persistent(workspace, config, state, artifacts)
}

fn freeze_references(
    original_root: &Path,
    run_id: &str,
    references: Vec<GauntletReference>,
) -> Result<Vec<GauntletReference>, String> {
    let mut frozen_references = Vec::with_capacity(references.len());
    for (index, mut reference) in references.into_iter().enumerate() {
        let source = original_root.join(&reference.path);
        let bytes = read_bounded_bytes(&source, MAX_REFERENCE_BYTES, "Gauntlet reference")?;
        let actual_hash = hex_sha256(&bytes);
        if actual_hash != reference.sha256 {
            return Err(format!(
                "Gauntlet reference hash does not match {}",
                source.display()
            ));
        }
        let extension = image_extension(&source)?;
        let relative = PathBuf::from(RUNS_PATH)
            .join(&run_id)
            .join("references")
            .join(format!(
                "reference-{index}-{}.{}",
                &actual_hash[..12],
                extension
            ));
        let destination = original_root.join(&relative);
        let parent = destination
            .parent()
            .ok_or_else(|| "Gauntlet reference destination has no parent".to_string())?;
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed creating run reference directory: {error}"))?;
        fs::write(&destination, bytes)
            .map_err(|error| format!("failed freezing run reference: {error}"))?;
        reference.path = relative.to_string_lossy().replace('\\', "/");
        frozen_references.push(reference);
    }
    Ok(frozen_references)
}

pub(super) fn resume(
    workspace: &Workspace,
    run_id: &str,
    observer: GauntletObserver,
) -> Result<CommandResult, String> {
    validate_run_id(run_id)?;
    let artifacts = run_artifacts(&workspace.root, run_id);
    let mut state = load_state(&artifacts)?;
    if matches!(state.phase, GauntletRunPhase::Converged) {
        return status(workspace, run_id);
    }
    let mut config: GauntletConfigV1 = read_json(&artifacts.join(EFFECTIVE_CONFIG_NAME))?;
    config.execution.observer = observer;
    config.validate()?;
    write_json(&artifacts.join(EFFECTIVE_CONFIG_NAME), &config)?;
    let worktree = PathBuf::from(&state.project_root);
    if !worktree.join("stasis.json").is_file() {
        return Err(format!(
            "Gauntlet worktree is unavailable: {}",
            worktree.display()
        ));
    }
    rollback_candidate(
        &worktree,
        state.best_commit.as_deref().unwrap_or(&state.base_commit),
    )?;
    let stop = artifacts.join(STOP_NAME);
    if stop.exists() {
        fs::remove_file(&stop)
            .map_err(|error| format!("failed clearing prior stop request: {error}"))?;
    }
    state.phase = GauntletRunPhase::Building;
    state.terminal_reason = None;
    persist_state(&artifacts, &mut state)?;
    emit_event(&artifacts, "run_resumed", json!({}))?;
    run_persistent(workspace, config, state, artifacts)
}

pub(super) fn status(workspace: &Workspace, run_id: &str) -> Result<CommandResult, String> {
    validate_run_id(run_id)?;
    let artifacts = run_artifacts(&workspace.root, run_id);
    let state = load_state(&artifacts)?;
    Ok(CommandResult::success(
        format_status(&state, &artifacts),
        serde_json::to_value(&state).map_err(|error| error.to_string())?,
    ))
}

pub(super) fn stop(workspace: &Workspace, run_id: &str) -> Result<CommandResult, String> {
    validate_run_id(run_id)?;
    let artifacts = run_artifacts(&workspace.root, run_id);
    let state = load_state(&artifacts)?;
    fs::write(artifacts.join(STOP_NAME), format!("{}\n", unix_ms()))
        .map_err(|error| format!("failed writing Gauntlet stop request: {error}"))?;
    emit_event(&artifacts, "stop_requested", json!({"phase": state.phase}))?;
    Ok(CommandResult::success(
        format!("stop requested for Gauntlet {run_id}"),
        json!({"run_id": run_id, "stop_requested": true}),
    ))
}

pub(super) fn promote(
    workspace: &Workspace,
    run_id: &str,
    _json_output: bool,
) -> Result<CommandResult, String> {
    validate_run_id(run_id)?;
    require_clean_checkout_ignoring_runs(&workspace.root)?;
    let artifacts = run_artifacts(&workspace.root, run_id);
    let state = load_state(&artifacts)?;
    if state.accepted_candidates == 0 {
        return Err("Gauntlet has no accepted game candidate to promote".to_string());
    }
    let best = state
        .best_commit
        .as_deref()
        .ok_or_else(|| "Gauntlet has no accepted checkpoint to promote".to_string())?;
    let head = git_stdout(&workspace.root, &["rev-parse", "HEAD"])?;
    if head != state.base_commit {
        return Err(format!(
            "promotion requires the original checkout to remain at {}; current HEAD is {head}",
            state.base_commit
        ));
    }
    git_ok(&workspace.root, &["merge", "--ff-only", best])?;
    emit_event(&artifacts, "promoted", json!({"commit": best}))?;
    Ok(CommandResult::success(
        format!("promoted Gauntlet {run_id} checkpoint {best}"),
        json!({"run_id": run_id, "commit": best, "branch": state.branch}),
    ))
}

fn run_persistent(
    original: &Workspace,
    config: GauntletConfigV1,
    state: GauntletRunStateV1,
    artifacts: PathBuf,
) -> Result<CommandResult, String> {
    let project_root = PathBuf::from(&state.project_root);
    let manifest_source = fs::read_to_string(project_root.join("stasis.json"))
        .map_err(|error| format!("failed reading isolated project manifest: {error}"))?;
    let manifest: Value = serde_json::from_str(&manifest_source)
        .map_err(|error| format!("invalid isolated project manifest: {error}"))?;
    let entry = manifest
        .get("entry")
        .and_then(Value::as_str)
        .unwrap_or("src/main.stasis");
    let output = manifest
        .get("output")
        .and_then(Value::as_str)
        .unwrap_or("build");
    let name = manifest
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("Stasis Gauntlet");
    let entry_path = project_root.join(entry);
    let (client, server) = live_session(stasis_runner::live::DEFAULT_LIVE_QUEUE_CAPACITY);
    let controller_client = client.clone();
    let controller_artifacts = artifacts.clone();
    let controller = thread::spawn(move || {
        let result = controller_loop(
            controller_client.clone(),
            config,
            state,
            &controller_artifacts,
        );
        let _ = controller_client.submit(LiveRequest::new(u64::MAX, LiveCommand::Quit));
        result
    });
    let live_config = LiveRunConfig::new(
        project_root.clone(),
        PathBuf::from(entry),
        PathBuf::from(output),
    )
    .with_window_title(name);
    let runtime_result = run_live_in_process(
        &entry_path,
        Some(&project_root),
        16_000,
        None,
        server,
        live_config,
    );
    let controller_result = controller
        .join()
        .map_err(|_| "Gauntlet controller thread panicked".to_string())?;
    if let Err(error) = runtime_result {
        mark_failed_if_active(&artifacts, &format!("live runtime failed: {error}"))?;
        return Err(format!(
            "Gauntlet live runtime failed: {error}; run artifacts: {}",
            artifacts.display()
        ));
    }
    let state = match controller_result {
        Ok(state) => state,
        Err(error) => {
            mark_failed_if_active(&artifacts, &error)?;
            return Err(format!(
                "Gauntlet controller failed: {error}; run artifacts: {}",
                artifacts.display()
            ));
        }
    };
    let successful = matches!(state.phase, GauntletRunPhase::Converged);
    let human = format_status(&state, &artifacts);
    let data = json!({
        "run": state,
        "artifacts": artifacts,
        "original_root": original.root,
        "success": successful,
    });
    Ok(CommandResult {
        code: if successful { 0 } else { 2 },
        human,
        data,
    })
}

fn mark_failed_if_active(artifacts: &Path, reason: &str) -> Result<(), String> {
    let mut state = load_state(artifacts)?;
    if !matches!(
        state.phase,
        GauntletRunPhase::Converged
            | GauntletRunPhase::BudgetExhausted
            | GauntletRunPhase::Stalled
            | GauntletRunPhase::Canceled
            | GauntletRunPhase::Failed
    ) {
        finish(&mut state, artifacts, GauntletRunPhase::Failed, reason)?;
    }
    Ok(())
}

fn controller_loop(
    client: LiveSessionClient,
    config: GauntletConfigV1,
    mut state: GauntletRunStateV1,
    artifacts: &Path,
) -> Result<GauntletRunStateV1, String> {
    let started = Instant::now();
    let canceled = Arc::new(AtomicBool::new(false));
    let wall_limit = Duration::from_secs(u64::from(config.budget.wall_time_minutes) * 60);
    let elapsed_before = Duration::from_millis(unix_ms().saturating_sub(state.started_unix_ms));
    let _stop_watcher = StopWatcher::start(
        artifacts,
        Arc::clone(&canceled),
        wall_limit.saturating_sub(elapsed_before),
    );
    let project_root = PathBuf::from(&state.project_root);
    let goal_path = project_root.join(&config.goal_file);
    let goal = read_bounded_utf8(&goal_path, MAX_GOAL_BYTES, "Gauntlet goal")?;
    state.phase = GauntletRunPhase::DiscoveringBar;
    persist_state(artifacts, &mut state)?;
    let bar = if artifacts.join(QUALITY_BAR_NAME).is_file() {
        read_json(&artifacts.join(QUALITY_BAR_NAME))?
    } else {
        let bar = bootstrap_bar(&goal, &config, &mut state, artifacts, &canceled)?;
        persist_state(artifacts, &mut state)?;
        write_json(&artifacts.join(QUALITY_BAR_NAME), &bar)?;
        emit_event(
            artifacts,
            "quality_bar_frozen",
            serde_json::to_value(&bar).map_err(|e| e.to_string())?,
        )?;
        bar
    };
    let reference_images = resolve_reference_images(
        &bar.references,
        &project_root,
        Path::new(&state.original_root),
    );
    let scenario_pointer = logical_center(&project_root);
    request_live(&client, 1, LiveCommand::Pause)?;
    request_live(&client, 2, LiveCommand::ValidationSnapshot)?;
    let mut next_request = 3_u64;
    let (mut baseline, mut baseline_state) = capture_scenario(
        &client,
        &project_root,
        artifacts,
        "baseline",
        scenario_pointer,
        &mut next_request,
    )?;
    let mut largest_gap = latest_decision_next_step(artifacts).unwrap_or_else(|| {
        "Build the first complete playable version of the frozen brief.".to_string()
    });
    let mut final_acceptances = 0_u32;
    loop {
        if should_stop(artifacts) {
            finish(
                &mut state,
                artifacts,
                GauntletRunPhase::Canceled,
                "stop requested",
            )?;
            break;
        }
        if elapsed_before.saturating_add(started.elapsed()) >= wall_limit {
            finish(
                &mut state,
                artifacts,
                GauntletRunPhase::BudgetExhausted,
                "wall-time budget exhausted",
            )?;
            break;
        }
        if state.model_calls >= config.budget.model_calls {
            finish(
                &mut state,
                artifacts,
                GauntletRunPhase::BudgetExhausted,
                "model-call budget exhausted",
            )?;
            break;
        }
        if state.consecutive_stalls >= config.budget.stalled_candidates {
            finish(
                &mut state,
                artifacts,
                GauntletRunPhase::Stalled,
                "consecutive candidate limit reached",
            )?;
            break;
        }
        state.phase = GauntletRunPhase::Planning;
        persist_state(artifacts, &mut state)?;
        let mut model_calls = state.model_calls;
        let decision = lead_decision(
            &goal,
            &bar,
            &largest_gap,
            &state,
            &mut model_calls,
            artifacts,
            &config.models.lead,
            &canceled,
        )?;
        state.model_calls = model_calls;
        persist_state(artifacts, &mut state)?;
        emit_event(
            artifacts,
            "lead_decision",
            serde_json::to_value(&decision).map_err(|e| e.to_string())?,
        )?;
        append_decision(
            artifacts,
            "lead",
            "work_item_selected",
            &format!("Selected workstream {}", decision.workstream),
            &decision.rationale,
            &format!(
                "Accepted {} candidates and rejected {}; largest gap: {largest_gap}",
                state.accepted_candidates, state.rejected_candidates
            ),
            &decision.next_step,
        )?;
        if decision.done && final_acceptances >= FINAL_ACCEPTANCES {
            match run_final_gates(&project_root, artifacts) {
                Ok(()) => {
                    finish(
                        &mut state,
                        artifacts,
                        GauntletRunPhase::Converged,
                        "independent evaluations and release gates met the frozen bar",
                    )?;
                    break;
                }
                Err(error) => {
                    largest_gap = format!("Final release validation failed: {error}");
                    final_acceptances = 0;
                    state.consecutive_stalls = state.consecutive_stalls.saturating_add(1);
                    emit_event(
                        artifacts,
                        "final_validation_failed",
                        json!({"error": error}),
                    )?;
                    append_decision(
                        artifacts,
                        "controller",
                        "final_validation_failed",
                        "Release validation rejected convergence",
                        "The frozen completion bar requires every release gate to pass.",
                        &error,
                        &largest_gap,
                    )?;
                    persist_state(artifacts, &mut state)?;
                    continue;
                }
            }
        }
        if decision.builder_prompt.trim().is_empty() || decision.workstream.trim().is_empty() {
            return fail_state(&mut state, artifacts, "lead returned an empty work item");
        }
        state.phase = GauntletRunPhase::Building;
        state.current_workstream = Some(decision.workstream.clone());
        persist_state(artifacts, &mut state)?;
        let remaining_calls = config.budget.model_calls.saturating_sub(state.model_calls);
        if remaining_calls == 0 {
            finish(
                &mut state,
                artifacts,
                GauntletRunPhase::BudgetExhausted,
                "model-call budget exhausted before builder",
            )?;
            break;
        }
        let candidate_id = format!(
            "candidate-{:04}",
            state.accepted_candidates + state.rejected_candidates + 1
        );
        let memory = decision_memory_snapshot(artifacts)?;
        let prompt = format!(
            "Frozen game brief:\n{goal}\n\nWorkstream: {}\nTask: {}\nLargest evidenced gap: {largest_gap}\n\nDurable decision memory (explicit conclusions only):\n{memory}\n\nMake one coherent, end-to-end improvement. Preserve deterministic tick semantics. Add or update durable Stasis tests in the same atomic write when behavior changes. Use record_decision for consequential choices and finish after the tested write succeeds.",
            decision.workstream, decision.builder_prompt
        );
        let profile = AgentProfile {
            role: "Fresh Stasis Gauntlet builder".to_string(),
            instruction: "Use only the supplied Stasis live-workspace tools. Inspect relevant symbols and references, then make one contiguous atomic semantic edit batch. You may create bounded SVG, PNG, JSON/CSV, or procedural WAV assets; put one contiguous asset-tool group immediately before the related source writes in the same response. Use record_decision during exploration and after consequential tested choices to preserve concise conclusions, tradeoffs, evidence, and next steps for future agents; never record hidden chain-of-thought. The write must compile and run tests. Do not grade your own visual quality. Return done immediately after a successful tested write and decision record.".to_string(),
            max_turns: usize::try_from(
                remaining_calls.min(config.execution.builder_max_turns),
            )
            .unwrap_or(stasis_ai::DEFAULT_AGENT_TURNS),
            model: config.models.builder.model.clone(),
            reasoning_effort: config.models.builder.reasoning_effort.clone(),
            compaction: config.execution.compaction.enabled.then(|| AgentCompactionPolicy {
                max_request_bytes: config.execution.compaction.max_request_bytes,
                retain_recent_turns: config.execution.compaction.retain_recent_turns,
            }),
        };
        let outcome = live_tui::run_scripted_ai_profile(
            &client,
            &project_root,
            &prompt,
            profile,
            vec![baseline.clone()],
            false,
            true,
            true,
            Some(&artifacts.join(DECISIONS_NAME)),
            &canceled,
        );
        match outcome {
            Ok(outcome) => {
                state.model_calls = state.model_calls.saturating_add(outcome.model_calls);
                persist_state(artifacts, &mut state)?;
                append_usage_file(artifacts, &outcome.usage_trace)?;
                emit_event(
                    artifacts,
                    "builder_completed",
                    json!({
                        "candidate": candidate_id,
                        "summary": outcome.summary,
                        "trace": outcome.trace,
                        "usage": outcome.usage_trace,
                        "model_calls": outcome.model_calls,
                    }),
                )?;
                append_decision(
                    artifacts,
                    "builder",
                    "builder_completed",
                    &outcome.summary,
                    "The builder completed a compiler-tested atomic project transaction.",
                    &format!(
                        "candidate={candidate_id}; model_calls={}",
                        outcome.model_calls
                    ),
                    "Capture and independently evaluate the candidate.",
                )?;
            }
            Err(error) => {
                rollback_candidate(
                    &project_root,
                    state.best_commit.as_deref().unwrap_or(&state.base_commit),
                )?;
                reject(
                    &mut state,
                    artifacts,
                    &candidate_id,
                    &format!("builder failed: {error}"),
                )?;
                largest_gap = format!(
                    "The previous builder failed before producing a valid candidate: {error}"
                );
                continue;
            }
        }
        state.phase = GauntletRunPhase::Evaluating;
        persist_state(artifacts, &mut state)?;
        let (candidate, candidate_state) = capture_scenario(
            &client,
            &project_root,
            artifacts,
            &candidate_id,
            scenario_pointer,
            &mut next_request,
        )?;
        let candidate_changed = !git_stdout(&project_root, &["status", "--porcelain"])?
            .trim()
            .is_empty();
        if !candidate_changed {
            reject(
                &mut state,
                artifacts,
                &candidate_id,
                "builder produced no project change",
            )?;
            largest_gap = "The previous work item produced no source or asset change.".to_string();
            continue;
        }
        if state.model_calls >= config.budget.model_calls {
            rollback_candidate(
                &project_root,
                state.best_commit.as_deref().unwrap_or(&state.base_commit),
            )?;
            finish(
                &mut state,
                artifacts,
                GauntletRunPhase::BudgetExhausted,
                "model-call budget exhausted before visual critique",
            )?;
            break;
        }
        let candidate_is_a =
            hex_sha256(format!("{}:{candidate_id}", state.run_id).as_bytes()).as_bytes()[0] % 2
                == 0;
        let (a, b) = if candidate_is_a {
            (&candidate, &baseline)
        } else {
            (&baseline, &candidate)
        };
        let critique = blind_critic(
            &goal,
            &bar,
            a,
            b,
            &reference_images,
            &mut state.model_calls,
            artifacts,
            &config.models.visual_critic,
            &canceled,
        )?;
        persist_state(artifacts, &mut state)?;
        let preferred_candidate = match critique.preferred.as_str() {
            "a" => candidate_is_a,
            "b" => !candidate_is_a,
            _ => false,
        };
        let candidate_score = if candidate_is_a {
            critique.score_a
        } else {
            critique.score_b
        };
        if state.model_calls >= config.budget.model_calls {
            rollback_candidate(
                &project_root,
                state.best_commit.as_deref().unwrap_or(&state.base_commit),
            )?;
            finish(
                &mut state,
                artifacts,
                GauntletRunPhase::BudgetExhausted,
                "model-call budget exhausted before gameplay critique",
            )?;
            break;
        }
        let (state_a, state_b) = if candidate_is_a {
            (&candidate_state, &baseline_state)
        } else {
            (&baseline_state, &candidate_state)
        };
        let gameplay = gameplay_critic(
            &goal,
            &bar,
            state_a,
            state_b,
            &mut state.model_calls,
            artifacts,
            &config.models.gameplay_critic,
            &canceled,
        )?;
        persist_state(artifacts, &mut state)?;
        let gameplay_passes = match gameplay.preferred.as_str() {
            "a" => candidate_is_a,
            "b" => !candidate_is_a,
            "equivalent" => true,
            _ => false,
        };
        emit_event(
            artifacts,
            "critic_completed",
            json!({
                "candidate": candidate_id,
                "blind_mapping_sha256": hex_sha256(format!("{}:{}", state.run_id, candidate_is_a).as_bytes()),
                "critique": critique,
                "candidate_score": candidate_score,
                "preferred_candidate": preferred_candidate,
                "gameplay": gameplay,
                "gameplay_passes": gameplay_passes,
            }),
        )?;
        if preferred_candidate && gameplay_passes && candidate_score >= bar.acceptance_score {
            state.phase = GauntletRunPhase::Checkpointing;
            persist_state(artifacts, &mut state)?;
            let commit = checkpoint(&project_root, &candidate_id, &decision.workstream)?;
            state.best_commit = Some(commit.clone());
            state.accepted_candidates = state.accepted_candidates.saturating_add(1);
            state.consecutive_stalls = 0;
            final_acceptances = final_acceptances.saturating_add(1);
            baseline = candidate;
            baseline_state = candidate_state;
            largest_gap = critique.largest_gap;
            persist_state(artifacts, &mut state)?;
            emit_event(
                artifacts,
                "candidate_accepted",
                json!({"candidate": candidate_id, "commit": commit}),
            )?;
            append_decision(
                artifacts,
                "controller",
                "candidate_accepted",
                &format!("Accepted {candidate_id} for {}", decision.workstream),
                "The blind visual critic preferred the candidate and the gameplay critic found no regression.",
                &format!("commit={commit}; visual_score={candidate_score}"),
                &largest_gap,
            )?;
            generate_report_shell(artifacts, &state)?;
        } else {
            rollback_candidate(
                &project_root,
                state.best_commit.as_deref().unwrap_or(&state.base_commit),
            )?;
            largest_gap = critique.largest_gap;
            final_acceptances = 0;
            reject(
                &mut state,
                artifacts,
                &candidate_id,
                "blind critic did not prefer the candidate at the required score",
            )?;
            append_decision(
                artifacts,
                "critic",
                "largest_gap",
                &format!("Recorded the largest gap after rejecting {candidate_id}"),
                "The next builder should address the critic's evidenced gap rather than repeat the rejected approach.",
                &critique.summary,
                &largest_gap,
            )?;
            generate_report_shell(artifacts, &state)?;
        }
    }
    state.current_workstream = None;
    persist_state(artifacts, &mut state)?;
    generate_report(artifacts, &state, &bar)?;
    Ok(state)
}

fn bootstrap_bar(
    goal: &str,
    config: &GauntletConfigV1,
    state: &mut GauntletRunStateV1,
    artifacts: &Path,
    canceled: &AtomicBool,
) -> Result<FrozenBar, String> {
    let mut web_sources = Vec::new();
    if config.quality_bar.allow_web_discovery
        && config.budget.model_calls.saturating_sub(state.model_calls) >= 2
    {
        let mut provider = provider_for_role(&config.models.scout).with_web_search(true);
        let prompt = format!(
            "Act as a read-only visual reference scout for this 2D game brief. Find up to five reputable HTTPS pages whose visual or interaction patterns can form a concrete quality bar. Do not suggest copying protected assets. Return only the requested JSON.\n\n{goal}"
        );
        let scout: ScoutResult = provider.respond_structured(&prompt, &scout_schema(), canceled)?;
        append_provider_usage(artifacts, &mut provider)?;
        state.model_calls = state.model_calls.saturating_add(provider.call_count());
        persist_state(artifacts, state)?;
        web_sources = scout
            .sources
            .into_iter()
            .filter(|source| source.url.starts_with("https://"))
            .take(5)
            .collect();
        emit_event(
            artifacts,
            "reference_scout_completed",
            json!({"summary": scout.summary, "sources": web_sources}),
        )?;
    }
    let mut provider = provider_for_role(&config.models.lead);
    let prompt = format!(
        "Act as the lead for an autonomous Stasis 2D game build. Decompose this immutable brief into 4-10 independently improvable workstreams. Use short noun phrases. Return only the requested JSON.\n\n{goal}"
    );
    let bootstrap: LeadBootstrap =
        provider.respond_structured(&prompt, &bootstrap_schema(), canceled)?;
    append_provider_usage(artifacts, &mut provider)?;
    state.model_calls = state.model_calls.saturating_add(provider.call_count());
    persist_state(artifacts, state)?;
    let workstreams = bootstrap
        .workstreams
        .into_iter()
        .filter(|value| !value.trim().is_empty())
        .take(10)
        .collect::<Vec<_>>();
    if workstreams.is_empty() {
        return Err("Gauntlet lead produced no usable workstreams".to_string());
    }
    append_decision(
        artifacts,
        "lead",
        "quality_bar_bootstrap",
        "Decomposed the immutable brief into persistent workstreams",
        "Independent workstreams let builders improve one bounded concern while critics judge the integrated result.",
        &workstreams.join(", "),
        "Select the highest-value workstream from runtime evidence.",
    )?;
    Ok(FrozenBar {
        schema_version: 1,
        goal_sha256: hex_sha256(goal.as_bytes()),
        goal: goal.to_string(),
        workstreams,
        hard_gates: vec![
            "project compiles".to_string(),
            "all Stasis tests pass".to_string(),
            "a real framebuffer capture succeeds".to_string(),
            "deterministic tick semantics are preserved".to_string(),
        ],
        required_scenarios: config
            .quality_bar
            .required_scenarios
            .iter()
            .map(|scenario| json!({"id": scenario.id, "description": scenario.description}))
            .collect(),
        references: config.quality_bar.references.clone(),
        web_sources,
        acceptance_score: 65,
    })
}

fn provider_for_role(role: &GauntletRoleModel) -> CodexExecProvider {
    let mut provider = CodexExecProvider::default();
    if let Some(model) = role.model.as_deref() {
        provider = provider.with_model(model);
    }
    if let Some(reasoning_effort) = role.reasoning_effort.as_deref() {
        provider = provider.with_reasoning_effort(reasoning_effort);
    }
    provider
}

fn lead_decision(
    goal: &str,
    bar: &FrozenBar,
    largest_gap: &str,
    state: &GauntletRunStateV1,
    model_calls: &mut u32,
    artifacts: &Path,
    model: &GauntletRoleModel,
    canceled: &AtomicBool,
) -> Result<LeadDecision, String> {
    let mut provider = provider_for_role(model);
    let memory = decision_memory_snapshot(artifacts)?;
    let prompt = format!(
        "Act as the fresh lead for a Stasis Gauntlet. Choose exactly one highest-value next work item from the frozen workstreams. Set done=true only if the largest gap says the bar is fully met; otherwise done=false. Preserve a concise rationale and next step for future fresh agents; do not provide hidden chain-of-thought. Return only JSON.\n\nBrief:\n{goal}\n\nWorkstreams: {}\nAccepted: {} Rejected: {}\nLargest gap: {largest_gap}\n\nDurable decision memory (explicit conclusions only):\n{memory}",
        bar.workstreams.join(", "), state.accepted_candidates, state.rejected_candidates
    );
    let decision: LeadDecision = provider.respond_structured(&prompt, &lead_schema(), canceled)?;
    append_provider_usage(artifacts, &mut provider)?;
    *model_calls = model_calls.saturating_add(provider.call_count());
    if decision.rationale.trim().is_empty() || decision.next_step.trim().is_empty() {
        return Err("Gauntlet lead returned empty decision memory fields".to_string());
    }
    Ok(decision)
}

fn blind_critic(
    goal: &str,
    bar: &FrozenBar,
    image_a: &Path,
    image_b: &Path,
    references: &[PathBuf],
    model_calls: &mut u32,
    artifacts: &Path,
    model: &GauntletRoleModel,
    canceled: &AtomicBool,
) -> Result<BlindCritique, String> {
    let mut images = vec![image_a.to_path_buf(), image_b.to_path_buf()];
    images.extend(references.iter().take(5).cloned());
    let mut provider = provider_for_role(model).with_images(images);
    let prompt = format!(
        "You are a fresh read-only visual/gameplay critic. The first two attached images are anonymously labeled A then B; any later images are hashed quality references. You do not know which candidate is newer. Compare A and B against the frozen brief and references. Prefer A, B, or neither. Scores are integers 0-100. Identify one largest remaining gap. Do not discuss source code and return only JSON.\n\nBrief:\n{goal}\n\nWorkstreams: {}\nHard gates already passed: {}",
        bar.workstreams.join(", "), bar.hard_gates.join(", ")
    );
    let critique: BlindCritique =
        provider.respond_structured(&prompt, &critic_schema(), canceled)?;
    append_provider_usage(artifacts, &mut provider)?;
    *model_calls = model_calls.saturating_add(provider.call_count());
    if !matches!(
        critique.preferred.as_str(),
        "a" | "b" | "neither" | "equivalent"
    ) || critique.score_a > 100
        || critique.score_b > 100
        || critique.largest_gap.trim().is_empty()
    {
        return Err("critic returned an invalid preference, score, or gap".to_string());
    }
    Ok(critique)
}

fn resolve_reference_images(
    references: &[GauntletReference],
    project_root: &Path,
    original_root: &Path,
) -> Vec<PathBuf> {
    references
        .iter()
        .filter_map(|reference| {
            let configured = PathBuf::from(&reference.path);
            let candidates = if configured.is_absolute() {
                vec![configured]
            } else {
                vec![
                    project_root.join(&configured),
                    original_root.join(&configured),
                ]
            };
            candidates.into_iter().find(|path| {
                fs::read(path).is_ok_and(|bytes| hex_sha256(&bytes) == reference.sha256)
            })
        })
        .take(5)
        .collect()
}

fn gameplay_critic(
    goal: &str,
    bar: &FrozenBar,
    state_a: &Value,
    state_b: &Value,
    model_calls: &mut u32,
    artifacts: &Path,
    model: &GauntletRoleModel,
    canceled: &AtomicBool,
) -> Result<BlindCritique, String> {
    let mut provider = provider_for_role(model);
    let prompt = format!(
        "You are a fresh read-only gameplay critic. Two anonymous candidates A and B were run from the same runtime snapshot for the same deterministic ticks. Judge only behavioral regressions and evidence relative to the brief. Prefer a, b, equivalent, or neither. Equivalent is appropriate when a visual-only improvement leaves gameplay intact. Return only JSON.\n\nBrief:\n{goal}\n\nRequired scenarios: {}\n\nA evidence:\n{}\n\nB evidence:\n{}",
        serde_json::to_string(&bar.required_scenarios).map_err(|error| error.to_string())?,
        serde_json::to_string(state_a).map_err(|error| error.to_string())?,
        serde_json::to_string(state_b).map_err(|error| error.to_string())?,
    );
    let critique: BlindCritique =
        provider.respond_structured(&prompt, &critic_schema(), canceled)?;
    append_provider_usage(artifacts, &mut provider)?;
    *model_calls = model_calls.saturating_add(provider.call_count());
    if !matches!(
        critique.preferred.as_str(),
        "a" | "b" | "neither" | "equivalent"
    ) || critique.score_a > 100
        || critique.score_b > 100
    {
        return Err("gameplay critic returned an invalid result".to_string());
    }
    Ok(critique)
}

fn capture_scenario(
    client: &LiveSessionClient,
    project_root: &Path,
    artifacts: &Path,
    id: &str,
    pointer: Option<(i32, i32)>,
    request_id: &mut u64,
) -> Result<(PathBuf, Value), String> {
    request_live(client, *request_id, LiveCommand::ValidationRestore)?;
    *request_id = request_id.saturating_add(1);
    if let Some((x, y)) = pointer {
        request_live(
            client,
            *request_id,
            LiveCommand::SetInputState {
                pointers: vec![LivePointerInput {
                    id: 0,
                    x,
                    y,
                    is_down: true,
                    went_down: true,
                    went_up: false,
                }],
            },
        )?;
        *request_id = request_id.saturating_add(1);
        request_live(client, *request_id, LiveCommand::Step { ticks: 1 })?;
        *request_id = request_id.saturating_add(1);
        wait_for_steps(client, request_id)?;
        request_live(
            client,
            *request_id,
            LiveCommand::SetInputState {
                pointers: vec![LivePointerInput {
                    id: 0,
                    x,
                    y,
                    is_down: false,
                    went_down: false,
                    went_up: true,
                }],
            },
        )?;
        *request_id = request_id.saturating_add(1);
    }
    request_live(
        client,
        *request_id,
        LiveCommand::Step {
            ticks: if pointer.is_some() { 29 } else { 30 },
        },
    )?;
    *request_id = request_id.saturating_add(1);
    wait_for_steps(client, request_id)?;
    let frame = capture_frame(client, project_root, artifacts, id, request_id)?;
    let inspection = request_live(
        client,
        *request_id,
        LiveCommand::InspectAll {
            limit: 64,
            concise: true,
            every_ticks: None,
        },
    )?;
    *request_id = request_id.saturating_add(1);
    request_live(client, *request_id, LiveCommand::ValidationRestore)?;
    *request_id = request_id.saturating_add(1);
    request_live(
        client,
        *request_id,
        LiveCommand::SetInputState {
            pointers: Vec::new(),
        },
    )?;
    *request_id = request_id.saturating_add(1);
    Ok((frame, inspection.data.unwrap_or(Value::Null)))
}

fn logical_center(project_root: &Path) -> Option<(i32, i32)> {
    let source = fs::read_to_string(project_root.join("assets/manifest.json")).ok()?;
    let manifest: Value = serde_json::from_str(&source).ok()?;
    let width = manifest.pointer("/display/logical_width")?.as_i64()?;
    let height = manifest.pointer("/display/logical_height")?.as_i64()?;
    Some((
        i32::try_from(width / 2).ok()?,
        i32::try_from(height / 2).ok()?,
    ))
}

fn wait_for_steps(client: &LiveSessionClient, request_id: &mut u64) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        let response = request_live(client, *request_id, LiveCommand::Status)?;
        *request_id = request_id.saturating_add(1);
        let remaining = response
            .data
            .as_ref()
            .and_then(|data| data.get("step_remaining"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if remaining == 0 {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(10));
    }
    Err("deterministic Gauntlet scenario did not finish within 15 seconds".to_string())
}

fn capture_frame(
    client: &LiveSessionClient,
    project_root: &Path,
    artifacts: &Path,
    id: &str,
    request_id: &mut u64,
) -> Result<PathBuf, String> {
    let artifact = id.replace(
        |character: char| {
            !character.is_ascii_alphanumeric() && character != '-' && character != '_'
        },
        "_",
    );
    let scheduled = request_live(
        client,
        *request_id,
        LiveCommand::CaptureFrame {
            artifact: artifact.clone(),
        },
    )?;
    *request_id = request_id.saturating_add(1);
    let runtime_path = scheduled
        .data
        .as_ref()
        .and_then(|data| data.get("path"))
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .ok_or_else(|| "live runtime did not return a screenshot path".to_string())?;
    request_live(client, *request_id, LiveCommand::Step { ticks: 1 })?;
    *request_id = request_id.saturating_add(1);
    let deadline = Instant::now() + MAX_CAPTURE_WAIT;
    while Instant::now() < deadline {
        if runtime_path.is_file()
            && fs::metadata(&runtime_path).is_ok_and(|metadata| metadata.len() > 0)
        {
            let destination = artifacts.join("artifacts").join(id).join("frame.png");
            fs::create_dir_all(destination.parent().expect("capture parent")).map_err(|error| {
                format!("failed creating candidate artifact directory: {error}")
            })?;
            fs::copy(&runtime_path, &destination)
                .map_err(|error| format!("failed retaining runtime capture: {error}"))?;
            let bytes = fs::read(&destination)
                .map_err(|error| format!("failed hashing capture: {error}"))?;
            emit_event(
                artifacts,
                "frame_captured",
                json!({
                    "candidate": id,
                    "path": destination,
                    "sha256": hex_sha256(&bytes),
                    "project_root": project_root,
                }),
            )?;
            return Ok(destination);
        }
        thread::sleep(Duration::from_millis(25));
    }
    Err(format!(
        "runtime capture did not appear at {}",
        runtime_path.display()
    ))
}

fn request_live(
    client: &LiveSessionClient,
    request_id: u64,
    command: LiveCommand,
) -> Result<LiveResponse, String> {
    client.submit(LiveRequest::new(request_id, command))?;
    loop {
        let response = client.receive_timeout(Duration::from_secs(300))?;
        if response.request_id == request_id {
            if response.ok {
                return Ok(response);
            }
            return Err(response
                .error
                .unwrap_or_else(|| "live request failed".to_string()));
        }
    }
}

fn checkpoint(root: &Path, candidate: &str, workstream: &str) -> Result<String, String> {
    git_ok(root, &["add", "--all"])?;
    git_ok(
        root,
        &[
            "-c",
            "user.name=Stasis Gauntlet",
            "-c",
            "user.email=gauntlet@stasis.local",
            "commit",
            "--no-verify",
            "-m",
            &format!("feat: improve {workstream} ({candidate})"),
        ],
    )?;
    git_stdout(root, &["rev-parse", "HEAD"])
}

fn run_final_gates(project_root: &Path, artifacts: &Path) -> Result<(), String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("failed locating Stasis for final validation: {error}"))?;
    let commands: &[(&str, &[&str])] = &[
        ("format", &["format", "--check"]),
        ("check", &["check"]),
        ("test", &["test"]),
        ("build", &["build", "--mode", "release"]),
        (
            "package",
            &["package", "--target", "desktop", "--development-build"],
        ),
    ];
    let logs = artifacts.join("final-validation");
    fs::create_dir_all(&logs)
        .map_err(|error| format!("failed creating final validation logs: {error}"))?;
    for (name, args) in commands {
        if should_stop(artifacts) {
            return Err("stop requested during final validation".to_string());
        }
        let log_path = logs.join(format!("{name}.log"));
        let stdout = fs::File::create(&log_path)
            .map_err(|error| format!("failed creating {}: {error}", log_path.display()))?;
        let stderr = stdout
            .try_clone()
            .map_err(|error| format!("failed cloning final validation log: {error}"))?;
        let mut command = Command::new(&executable);
        command
            .arg("--workspace")
            .arg(project_root)
            .args(*args)
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr));
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut child = command
            .spawn()
            .map_err(|error| format!("failed starting final {name}: {error}"))?;
        let started = Instant::now();
        let status = loop {
            if should_stop(artifacts) || started.elapsed() >= Duration::from_secs(900) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "final {name} was canceled or exceeded 900 seconds; see {}",
                    log_path.display()
                ));
            }
            match child
                .try_wait()
                .map_err(|error| format!("failed waiting for final {name}: {error}"))?
            {
                Some(status) => break status,
                None => thread::sleep(Duration::from_millis(100)),
            }
        };
        if !status.success() {
            return Err(format!(
                "final {name} exited with {status}; see {}",
                log_path.display()
            ));
        }
        emit_event(artifacts, "final_gate_passed", json!({"gate": name}))?;
    }
    Ok(())
}

fn rollback_candidate(root: &Path, commit: &str) -> Result<(), String> {
    git_ok(
        root,
        &[
            "restore",
            "--source",
            commit,
            "--staged",
            "--worktree",
            "--",
            ".",
        ],
    )?;
    let untracked = git_stdout(root, &["ls-files", "--others", "--exclude-standard"])?;
    for relative in untracked.lines().filter(|line| !line.trim().is_empty()) {
        let path = root.join(relative);
        if path.is_file() {
            fs::remove_file(&path).map_err(|error| {
                format!("failed removing rejected file {}: {error}", path.display())
            })?;
        } else if path.is_dir() {
            fs::remove_dir_all(&path).map_err(|error| {
                format!(
                    "failed removing rejected directory {}: {error}",
                    path.display()
                )
            })?;
        }
    }
    Ok(())
}

fn reject(
    state: &mut GauntletRunStateV1,
    artifacts: &Path,
    candidate: &str,
    reason: &str,
) -> Result<(), String> {
    state.rejected_candidates = state.rejected_candidates.saturating_add(1);
    state.consecutive_stalls = state.consecutive_stalls.saturating_add(1);
    persist_state(artifacts, state)?;
    emit_event(
        artifacts,
        "candidate_rejected",
        json!({"candidate": candidate, "reason": reason}),
    )?;
    append_decision(
        artifacts,
        "controller",
        "candidate_rejected",
        &format!("Rejected {candidate}"),
        "A candidate must pass its atomic write and independent evidence gates before it can replace the accepted checkpoint.",
        reason,
        "Choose a smaller corrective work item that directly addresses the rejection evidence.",
    )?;
    generate_report_shell(artifacts, state)
}

fn finish(
    state: &mut GauntletRunStateV1,
    artifacts: &Path,
    phase: GauntletRunPhase,
    reason: &str,
) -> Result<(), String> {
    state.phase = phase;
    state.terminal_reason = Some(reason.to_string());
    emit_event(
        artifacts,
        "run_finished",
        json!({"phase": state.phase, "reason": reason}),
    )?;
    persist_state(artifacts, state)
}

fn fail_state<T>(
    state: &mut GauntletRunStateV1,
    artifacts: &Path,
    reason: &str,
) -> Result<T, String> {
    finish(state, artifacts, GauntletRunPhase::Failed, reason)?;
    Err(reason.to_string())
}

fn ensure_initial_commit(root: &Path) -> Result<(), String> {
    if git_stdout(root, &["rev-parse", "--verify", "HEAD"]).is_ok() {
        return Ok(());
    }
    git_ok(root, &["add", "--all"])?;
    git_ok(
        root,
        &[
            "-c",
            "user.name=Stasis Gauntlet",
            "-c",
            "user.email=gauntlet@stasis.local",
            "commit",
            "--no-verify",
            "-m",
            "feat: initialize Stasis game",
        ],
    )
}

fn ensure_worktree_ignores(root: &Path) -> Result<String, String> {
    let path = root.join(".gitignore");
    let mut source = fs::read_to_string(&path).unwrap_or_default();
    let mut changed = false;
    for pattern in ["build/", ".stasis_cache/"] {
        if !source.lines().any(|line| line.trim() == pattern) {
            if !source.is_empty() && !source.ends_with('\n') {
                source.push('\n');
            }
            source.push_str(pattern);
            source.push('\n');
            changed = true;
        }
    }
    if changed {
        fs::write(&path, source)
            .map_err(|error| format!("failed writing isolated Gauntlet ignores: {error}"))?;
        git_ok(root, &["add", ".gitignore"])?;
        git_ok(
            root,
            &[
                "-c",
                "user.name=Stasis Gauntlet",
                "-c",
                "user.email=gauntlet@stasis.local",
                "commit",
                "--no-verify",
                "-m",
                "chore: isolate Gauntlet artifacts",
            ],
        )?;
    }
    git_stdout(root, &["rev-parse", "HEAD"])
}

fn require_clean_checkout(root: &Path) -> Result<(), String> {
    git_ok(root, &["rev-parse", "--show-toplevel"])?;
    let status = git_stdout(root, &["status", "--porcelain", "--untracked-files=all"])?;
    if !status.trim().is_empty() {
        return Err("Gauntlet requires a clean original checkout; commit or stash tracked and untracked files first".to_string());
    }
    Ok(())
}

fn require_clean_checkout_ignoring_runs(root: &Path) -> Result<(), String> {
    let status = git_stdout(root, &["status", "--porcelain", "--untracked-files=all"])?;
    let unexpected = status
        .lines()
        .filter(|line| {
            let path = line.get(3..).unwrap_or(line).replace('\\', "/");
            !path.starts_with("build/gauntlet/")
        })
        .collect::<Vec<_>>();
    if !unexpected.is_empty() {
        return Err(format!(
            "promotion requires a clean original checkout; found {}",
            unexpected.join(", ")
        ));
    }
    Ok(())
}

fn git_ok(root: &Path, args: &[&str]) -> Result<(), String> {
    git_output(root, args).map(|_| ())
}

fn git_stdout(root: &Path, args: &[&str]) -> Result<String, String> {
    let output = git_output(root, args)?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn git_output(root: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|error| format!("failed to run git {}: {error}", args.join(" ")))?;
    if output.status.success() {
        Ok(output)
    } else {
        let diagnostic = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(format!(
            "git {} failed{}",
            args.join(" "),
            if diagnostic.is_empty() {
                String::new()
            } else {
                format!(": {diagnostic}")
            }
        ))
    }
}

fn persist_state(artifacts: &Path, state: &mut GauntletRunStateV1) -> Result<(), String> {
    state.updated_unix_ms = unix_ms();
    write_json(&artifacts.join(RUN_STATE_NAME), state)?;
    generate_report_shell(artifacts, state)
}

fn load_state(artifacts: &Path) -> Result<GauntletRunStateV1, String> {
    let state: GauntletRunStateV1 = read_json(&artifacts.join(RUN_STATE_NAME))?;
    if state.schema_version != GAUNTLET_SCHEMA_VERSION {
        return Err(format!(
            "unsupported persisted Gauntlet schema {}",
            state.schema_version
        ));
    }
    Ok(state)
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, String> {
    let source = fs::read_to_string(path)
        .map_err(|error| format!("failed reading {}: {error}", path.display()))?;
    serde_json::from_str(&source)
        .map_err(|error| format!("invalid JSON {}: {error}", path.display()))
}

fn append_decision(
    artifacts: &Path,
    role: &str,
    kind: &str,
    summary: &str,
    rationale: &str,
    evidence: &str,
    next_step: &str,
) -> Result<(), String> {
    let record = json!({
        "schema_version": 1,
        "unix_ms": unix_ms(),
        "audience": "lead_builder",
        "role": bounded_decision_text(role),
        "kind": bounded_decision_text(kind),
        "summary": bounded_decision_text(summary),
        "rationale": bounded_decision_text(rationale),
        "evidence": bounded_decision_text(evidence),
        "next_step": bounded_decision_text(next_step),
    });
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(artifacts.join(DECISIONS_NAME))
        .map_err(|error| format!("failed opening Gauntlet decision memory: {error}"))?;
    writeln!(
        file,
        "{}",
        serde_json::to_string(&record).map_err(|error| error.to_string())?
    )
    .map_err(|error| format!("failed writing Gauntlet decision memory: {error}"))?;
    file.flush()
        .map_err(|error| format!("failed flushing Gauntlet decision memory: {error}"))?;
    file.sync_data()
        .map_err(|error| format!("failed syncing Gauntlet decision memory: {error}"))
}

fn bounded_decision_text(value: &str) -> String {
    value.chars().take(MAX_DECISION_FIELD_CHARS).collect()
}

fn decision_memory_snapshot(artifacts: &Path) -> Result<String, String> {
    let records = read_decision_records(artifacts)?;
    let mut selected = Vec::new();
    let mut characters = 0_usize;
    for record in records.into_iter().rev() {
        if record.get("audience").and_then(Value::as_str) != Some("lead_builder") {
            continue;
        }
        let rendered = serde_json::to_string(&record).map_err(|error| error.to_string())?;
        let count = rendered.chars().count().saturating_add(1);
        if selected.len() >= MAX_MEMORY_RECORDS
            || characters.saturating_add(count) > MAX_MEMORY_CHARS
        {
            break;
        }
        characters = characters.saturating_add(count);
        selected.push(rendered);
    }
    selected.reverse();
    if selected.is_empty() {
        Ok("(no prior decisions recorded)".to_string())
    } else {
        Ok(selected.join("\n"))
    }
}

fn latest_decision_next_step(artifacts: &Path) -> Option<String> {
    read_decision_records(artifacts)
        .ok()?
        .into_iter()
        .rev()
        .find(|record| record.get("audience").and_then(Value::as_str) == Some("lead_builder"))?
        .get("next_step")?
        .as_str()
        .map(str::to_string)
}

fn read_decision_records(artifacts: &Path) -> Result<Vec<Value>, String> {
    let path = artifacts.join(DECISIONS_NAME);
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let metadata = fs::metadata(&path)
        .map_err(|error| format!("failed reading Gauntlet decision memory: {error}"))?;
    if metadata.len() > 8 * 1024 * 1024 {
        return Err("Gauntlet decision memory exceeds the 8 MiB safety limit".to_string());
    }
    let source = fs::read_to_string(&path)
        .map_err(|error| format!("failed reading Gauntlet decision memory: {error}"))?;
    let lines = source
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    let mut records = Vec::with_capacity(lines.len());
    for (index, line) in lines.iter().enumerate() {
        match serde_json::from_str::<Value>(line) {
            Ok(record) => records.push(record),
            Err(_) if index + 1 == lines.len() => break,
            Err(error) => return Err(format!("invalid Gauntlet decision record: {error}")),
        }
    }
    Ok(records)
}

fn emit_event(artifacts: &Path, kind: &str, data: Value) -> Result<(), String> {
    let event = json!({"schema_version": 1, "unix_ms": unix_ms(), "kind": kind, "data": data});
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(artifacts.join(EVENTS_NAME))
        .map_err(|error| format!("failed opening Gauntlet event stream: {error}"))?;
    writeln!(
        file,
        "{}",
        serde_json::to_string(&event).map_err(|error| error.to_string())?
    )
    .map_err(|error| format!("failed writing Gauntlet event: {error}"))?;
    let observer = read_json::<GauntletConfigV1>(&artifacts.join(EFFECTIVE_CONFIG_NAME))
        .ok()
        .map(|config| config.execution.observer)
        .unwrap_or(GauntletObserver::Auto);
    match observer {
        GauntletObserver::Jsonl => println!("{}", event),
        GauntletObserver::Auto | GauntletObserver::Tui => {
            let detail = match kind {
                "lead_decision" => event
                    .pointer("/data/workstream")
                    .and_then(Value::as_str)
                    .unwrap_or("planning"),
                "candidate_accepted" | "candidate_rejected" => event
                    .pointer("/data/candidate")
                    .and_then(Value::as_str)
                    .unwrap_or("candidate"),
                "run_finished" => event
                    .pointer("/data/reason")
                    .and_then(Value::as_str)
                    .unwrap_or("finished"),
                _ => "",
            };
            println!(
                "[gauntlet] {kind}{}",
                if detail.is_empty() {
                    String::new()
                } else {
                    format!(": {detail}")
                }
            );
        }
    }
    Ok(())
}

fn append_provider_usage(artifacts: &Path, provider: &mut CodexExecProvider) -> Result<(), String> {
    if let Some(usage) = provider.take_usage() {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(artifacts.join(USAGE_NAME))
            .map_err(|error| format!("failed opening Gauntlet usage stream: {error}"))?;
        writeln!(
            file,
            "{}",
            serde_json::to_string(&usage).map_err(|error| error.to_string())?
        )
        .map_err(|error| format!("failed writing Gauntlet usage: {error}"))?;
    }
    Ok(())
}

fn append_usage_file(artifacts: &Path, source: &Path) -> Result<(), String> {
    let usage = fs::read_to_string(source)
        .map_err(|error| format!("failed reading builder usage {}: {error}", source.display()))?;
    if usage.is_empty() {
        return Ok(());
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(artifacts.join(USAGE_NAME))
        .map_err(|error| format!("failed opening Gauntlet usage stream: {error}"))?;
    file.write_all(usage.as_bytes())
        .map_err(|error| format!("failed appending builder usage: {error}"))
}

fn generate_report(
    artifacts: &Path,
    state: &GauntletRunStateV1,
    bar: &FrozenBar,
) -> Result<(), String> {
    let bar_json = serde_json::to_string_pretty(bar).map_err(|error| error.to_string())?;
    let events = fs::read_to_string(artifacts.join(EVENTS_NAME)).unwrap_or_default();
    let decisions = fs::read_to_string(artifacts.join(DECISIONS_NAME)).unwrap_or_default();
    let mut captures = String::new();
    if let Ok(entries) = fs::read_dir(artifacts.join("artifacts")) {
        let mut entries = entries.flatten().collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let id = entry.file_name().to_string_lossy().to_string();
            if entry.path().join("frame.png").is_file() {
                captures.push_str(&format!(
                    "<figure><img loading=\"lazy\" src=\"artifacts/{}/frame.png\" alt=\"{}\"><figcaption>{}</figcaption></figure>",
                    escape_html(&id),
                    escape_html(&id),
                    escape_html(&id)
                ));
            }
        }
    }
    let html = format!(
        "<!doctype html><meta charset=\"utf-8\"><title>Stasis Gauntlet {}</title><style>body{{font:16px system-ui;max-width:1100px;margin:40px auto;padding:0 20px;background:#0b1020;color:#e8eefc}}pre{{white-space:pre-wrap;background:#151d33;padding:16px;border-radius:10px}}figure{{display:inline-block;width:46%;vertical-align:top}}img{{max-width:100%;border-radius:10px}}</style><h1>Stasis Gauntlet {}</h1><p>Phase: {:?} &middot; accepted {} &middot; rejected {} &middot; model calls {}</p><h2>Captures</h2>{}<h2>Frozen quality bar</h2><pre>{}</pre><h2>Decision memory</h2><pre>{}</pre><h2>Event stream</h2><pre>{}</pre>",
        escape_html(&state.run_id),
        escape_html(&state.run_id),
        state.phase,
        state.accepted_candidates,
        state.rejected_candidates,
        state.model_calls,
        captures,
        escape_html(&bar_json),
        escape_html(&decisions),
        escape_html(&events)
    );
    fs::write(artifacts.join(REPORT_NAME), html)
        .map_err(|error| format!("failed writing Gauntlet report: {error}"))
}

fn generate_report_shell(artifacts: &Path, state: &GauntletRunStateV1) -> Result<(), String> {
    if artifacts.join(QUALITY_BAR_NAME).is_file() {
        let bar: FrozenBar = read_json(&artifacts.join(QUALITY_BAR_NAME))?;
        generate_report(artifacts, state, &bar)
    } else {
        Ok(())
    }
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn should_stop(artifacts: &Path) -> bool {
    artifacts.join(STOP_NAME).is_file()
}

fn run_artifacts(root: &Path, run_id: &str) -> PathBuf {
    root.join(RUNS_PATH).join(run_id)
}

fn new_run_id(base_commit: &str) -> String {
    format!("{}-{}", unix_ms(), &base_commit[..base_commit.len().min(8)])
}

fn validate_run_id(run_id: &str) -> Result<(), String> {
    if run_id.is_empty()
        || run_id.len() > 80
        || !run_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err("invalid Gauntlet run id".to_string());
    }
    Ok(())
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

fn format_status(state: &GauntletRunStateV1, artifacts: &Path) -> String {
    format!("Gauntlet {}: {:?}\nbranch: {}\nbest: {}\naccepted: {}, rejected: {}, model calls: {}\nworktree: {}\nreport: {}", state.run_id, state.phase, state.branch, state.best_commit.as_deref().unwrap_or("none"), state.accepted_candidates, state.rejected_candidates, state.model_calls, state.project_root, artifacts.join(REPORT_NAME).display())
}

fn scout_schema() -> Value {
    json!({"type":"object","required":["summary","sources"],"properties":{"summary":{"type":"string"},"sources":{"type":"array","maxItems":5,"items":{"type":"object","required":["url","relevance"],"properties":{"url":{"type":"string"},"relevance":{"type":"string"}},"additionalProperties":false}}},"additionalProperties":false})
}

fn bootstrap_schema() -> Value {
    json!({"type":"object","required":["workstreams"],"properties":{"workstreams":{"type":"array","minItems":4,"maxItems":10,"items":{"type":"string"}}},"additionalProperties":false})
}

fn lead_schema() -> Value {
    json!({"type":"object","required":["done","workstream","builder_prompt","rationale","next_step"],"properties":{"done":{"type":"boolean"},"workstream":{"type":"string","maxLength":2000},"builder_prompt":{"type":"string","maxLength":2000},"rationale":{"type":"string","maxLength":2000},"next_step":{"type":"string","maxLength":2000}},"additionalProperties":false})
}

fn critic_schema() -> Value {
    json!({"type":"object","required":["preferred","score_a","score_b","largest_gap","summary"],"properties":{"preferred":{"type":"string","enum":["a","b","neither","equivalent"]},"score_a":{"type":"integer","minimum":0,"maximum":100},"score_b":{"type":"integer","minimum":0,"maximum":100},"largest_gap":{"type":"string"},"summary":{"type":"string"}},"additionalProperties":false})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_ids_and_capture_artifacts_are_bounded() {
        assert!(validate_run_id("1234567890-deadbeef").is_ok());
        assert!(validate_run_id("../escape").is_err());
        let value = "candidate 1/evil".replace(
            |character: char| {
                !character.is_ascii_alphanumeric() && character != '-' && character != '_'
            },
            "_",
        );
        assert_eq!(value, "candidate_1_evil");
    }

    #[test]
    fn schemas_are_closed_and_role_outputs_are_strict() {
        let source = r#"{"preferred":"a","score_a":80,"score_b":60,"largest_gap":"audio","summary":"A is clearer","extra":true}"#;
        assert!(serde_json::from_str::<BlindCritique>(source).is_err());
        assert_eq!(critic_schema()["additionalProperties"], false);
    }

    #[test]
    fn html_report_content_is_escaped() {
        assert_eq!(escape_html("<script>&\""), "&lt;script&gt;&amp;&quot;");
    }

    #[test]
    fn decision_memory_is_bounded_and_restores_the_latest_next_step() {
        let root = std::env::temp_dir().join(format!(
            "stasis_gauntlet_memory_{}_{}",
            std::process::id(),
            unix_ms()
        ));
        fs::create_dir_all(&root).expect("memory root");
        for index in 0..55 {
            append_decision(
                &root,
                "builder",
                "test",
                &format!("decision-{index}"),
                "bounded rationale",
                "bounded evidence",
                &format!("next-{index}"),
            )
            .expect("append decision");
        }
        let snapshot = decision_memory_snapshot(&root).expect("memory snapshot");
        assert!(!snapshot.contains("decision-0\""));
        assert!(snapshot.contains("decision-54"));
        assert!(snapshot.lines().count() <= MAX_MEMORY_RECORDS);
        assert_eq!(latest_decision_next_step(&root).as_deref(), Some("next-54"));
        OpenOptions::new()
            .append(true)
            .open(root.join(DECISIONS_NAME))
            .expect("decision log")
            .write_all(b"{\"torn\":")
            .expect("torn final record");
        assert_eq!(latest_decision_next_step(&root).as_deref(), Some("next-54"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn frozen_references_remain_relative_and_resume_valid() {
        let root = std::env::temp_dir().join(format!(
            "stasis_gauntlet_reference_{}_{}",
            std::process::id(),
            unix_ms()
        ));
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(root.clone());
        fs::create_dir_all(root.join("input")).expect("reference input");
        let bytes = b"bounded-reference-image";
        fs::write(root.join("input/reference.png"), bytes).expect("reference");
        let references = freeze_references(
            &root,
            "123-deadbeef",
            vec![GauntletReference {
                path: "input/reference.png".to_string(),
                sha256: hex_sha256(bytes),
                source_url: None,
            }],
        )
        .expect("freeze reference");
        let config =
            GauntletConfigV1::new(false, references.clone(), 1, 1, GauntletObserver::Jsonl)
                .expect("resume-valid config");
        assert!(config.validate().is_ok());
        assert!(!Path::new(&references[0].path).is_absolute());
        assert_eq!(
            fs::read(root.join(&references[0].path)).expect("frozen reference"),
            bytes
        );
    }

    #[test]
    fn isolated_worktree_can_live_under_ignored_run_artifacts() {
        let root = std::env::temp_dir().join(format!(
            "stasis_gauntlet_worktree_{}_{}",
            std::process::id(),
            unix_ms()
        ));
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(root.clone());
        fs::create_dir_all(&root).expect("root");
        git_ok(&root, &["init", "--quiet"]).expect("git init");
        fs::write(root.join(".gitignore"), "build/\n").expect("ignore");
        fs::write(root.join("stasis.json"), "{}\n").expect("manifest");
        ensure_initial_commit(&root).expect("initial commit");
        let target = root.join("build/gauntlet/worktrees/test-run");
        git_ok(
            &root,
            &[
                "worktree",
                "add",
                "-b",
                "stasis/gauntlet/test-run",
                &target.to_string_lossy(),
                "HEAD",
            ],
        )
        .expect("nested ignored worktree");
        assert!(target.join("stasis.json").is_file());
        ensure_worktree_ignores(&target).expect("artifact ignores");
        assert!(git_stdout(&target, &["status", "--porcelain"])
            .expect("clean worktree")
            .is_empty());
    }
}
