use super::{
    atomic_write_bytes, hex_sha256, image_extension, import_references, read_bounded_bytes,
    read_bounded_utf8, write_json, GauntletConfigV1, GauntletIsolation, GauntletObserver,
    GauntletReference, GauntletRoleModel, GauntletRunPhase, GauntletRunStateV1,
    GauntletScenarioRequirement, GAUNTLET_SCHEMA_VERSION, MAX_GOAL_BYTES, MAX_REFERENCE_BYTES,
};
use crate::toolchain_cli::live_tui;
use crate::toolchain_cli::{CommandResult, Workspace};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use stasis::{run_live_in_process, LiveRunConfig};
use stasis_ai::{AgentCompactionPolicy, AgentProfile, CodexExecProvider, ModelProvider};
use stasis_runner::live::{
    live_session, LiveCommand, LivePointerInput, LiveRequest, LiveResponse, LiveSessionClient,
};
use std::collections::{BTreeMap, BTreeSet};
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
const HEARTBEAT_NAME: &str = "heartbeat.json";
const QUALITY_BAR_NAME: &str = "quality-bar.json";
const CREATIVE_DIRECTION_NAME: &str = "creative-direction.md";
const PROJECT_CREATIVE_DIRECTION_NAME: &str = "CREATIVE_DIRECTION.md";
const MAX_CAPTURE_WAIT: Duration = Duration::from_secs(10);
const MAX_LIVE_REQUEST_WAIT: Duration = Duration::from_secs(30);
const FINAL_ACCEPTANCES: u32 = 2;
const MAX_MEMORY_RECORDS: usize = 48;
const MAX_MEMORY_CHARS: usize = 32 * 1024;
const MAX_DECISION_FIELD_CHARS: usize = 2_000;
const MAX_FAILURE_TRACE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_FAILURE_ERROR_KINDS: usize = 8;
const MAX_PRIOR_RUNS: usize = 4;
const MAX_PRIOR_RUN_LESSONS: usize = 12;
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(2);
const HEARTBEAT_STALE_AFTER_MS: u64 = 15_000;

struct StopWatcher {
    done: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

struct LiveQuitGuard(LiveSessionClient);

impl Drop for LiveQuitGuard {
    fn drop(&mut self) {
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(5) {
            match self.0.submit(LiveRequest::new(u64::MAX, LiveCommand::Quit)) {
                Ok(()) => break,
                Err(error) if error == "live-session command queue is full" => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
    }
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

struct RunHeartbeat {
    done: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
    path: PathBuf,
}

impl RunHeartbeat {
    fn start(artifacts: &Path) -> Result<Self, String> {
        let path = artifacts.join(HEARTBEAT_NAME);
        if heartbeat_is_fresh(&path) {
            return Err("Gauntlet run already has a live controller heartbeat".to_string());
        }
        if path.exists() {
            fs::remove_file(&path)
                .map_err(|error| format!("failed clearing stale Gauntlet heartbeat: {error}"))?;
        }
        let done = Arc::new(AtomicBool::new(false));
        let worker_done = Arc::clone(&done);
        let worker_path = path.clone();
        let mut owner = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| format!("failed claiming Gauntlet heartbeat: {error}"))?;
        owner
            .write_all(&heartbeat_bytes()?)
            .map_err(|error| format!("failed writing Gauntlet heartbeat: {error}"))?;
        owner
            .sync_all()
            .map_err(|error| format!("failed syncing Gauntlet heartbeat: {error}"))?;
        let worker = thread::spawn(move || {
            while !worker_done.load(Ordering::Acquire) {
                thread::sleep(HEARTBEAT_INTERVAL);
                if !worker_done.load(Ordering::Acquire) {
                    let _ = write_heartbeat(&worker_path);
                }
            }
        });
        Ok(Self {
            done,
            worker: Some(worker),
            path,
        })
    }
}

impl Drop for RunHeartbeat {
    fn drop(&mut self) {
        self.done.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        let _ = fs::remove_file(&self.path);
    }
}

fn write_heartbeat(path: &Path) -> Result<(), String> {
    atomic_write_bytes(path, &heartbeat_bytes()?)
        .map_err(|error| format!("failed writing Gauntlet heartbeat: {error}"))
}

fn heartbeat_bytes() -> Result<Vec<u8>, String> {
    serde_json::to_vec(&json!({
        "schema_version": 1,
        "pid": std::process::id(),
        "unix_ms": unix_ms(),
    }))
    .map_err(|error| error.to_string())
}

fn heartbeat_unix_ms(path: &Path) -> Option<u64> {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| value.get("unix_ms").and_then(Value::as_u64))
}

fn heartbeat_is_fresh(path: &Path) -> bool {
    heartbeat_unix_ms(path)
        .is_some_and(|heartbeat| unix_ms().saturating_sub(heartbeat) <= HEARTBEAT_STALE_AFTER_MS)
}

fn is_terminal_phase(phase: &GauntletRunPhase) -> bool {
    matches!(
        phase,
        GauntletRunPhase::Converged
            | GauntletRunPhase::BudgetExhausted
            | GauntletRunPhase::Stalled
            | GauntletRunPhase::Canceled
            | GauntletRunPhase::Failed
    )
}

fn phase_is_resumable(phase: &GauntletRunPhase) -> bool {
    !matches!(phase, GauntletRunPhase::Converged)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrozenBar {
    schema_version: u32,
    goal_sha256: String,
    goal: String,
    #[serde(default)]
    direction_source_markdown: String,
    #[serde(default)]
    direction_source_sha256: String,
    #[serde(default)]
    creative_direction: CreativeDirection,
    workstreams: Vec<String>,
    hard_gates: Vec<String>,
    required_scenarios: Vec<Value>,
    references: Vec<GauntletReference>,
    web_sources: Vec<ScoutSource>,
    acceptance_score: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreativeDirection {
    title: String,
    narrative_promise: String,
    player_fantasy: String,
    rule_pillars: Vec<String>,
    visual_language: Vec<String>,
    interaction_grammar: Vec<String>,
    progression_and_pacing: Vec<String>,
    non_negotiables: Vec<String>,
}

impl Default for CreativeDirection {
    fn default() -> Self {
        Self {
            title: "Coherent playable game direction".to_string(),
            narrative_promise: "Preserve the project brief's setting and player-facing promise."
                .to_string(),
            player_fantasy: "Make every turn feel intentional, legible, and consequential."
                .to_string(),
            rule_pillars: vec![
                "Rules must be deterministic, teachable, and visible in play.".to_string(),
            ],
            visual_language: vec![
                "Visual hierarchy must communicate gameplay before decoration.".to_string(),
            ],
            interaction_grammar: vec![
                "The screen must show current state, available actions, and action results."
                    .to_string(),
            ],
            progression_and_pacing: vec![
                "Each turn should present a readable decision and visible consequence.".to_string(),
            ],
            non_negotiables: vec![
                "Do not trade playability, deterministic behavior, or coherence for polish."
                    .to_string(),
            ],
        }
    }
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
    creative_direction: CreativeDirection,
    workstreams: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LeadDecision {
    done: bool,
    workstream: String,
    builder_prompt: String,
    playability_guidance: String,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScreenComprehension {
    current_state_clear: bool,
    available_actions_clear: bool,
    board_semantics_clear: bool,
    action_feedback_clear: bool,
    evidence: String,
}

impl ScreenComprehension {
    fn passes(&self) -> bool {
        self.current_state_clear
            && self.available_actions_clear
            && self.board_semantics_clear
            && self.action_feedback_clear
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct VisualCritique {
    preferred: String,
    score_a: u32,
    score_b: u32,
    screen_a: ScreenComprehension,
    screen_b: ScreenComprehension,
    largest_gap: String,
    summary: String,
}

#[derive(Debug, Clone)]
struct ScenarioCapture {
    initial_frame: PathBuf,
    action_frame: PathBuf,
    required_frames: Vec<RequiredScenarioFrame>,
    state: Value,
}

#[derive(Debug, Clone)]
struct RequiredScenarioFrame {
    id: String,
    description: String,
    frame: PathBuf,
}

struct PriorRunLesson {
    unix_ms: u64,
    source_run_id: String,
    source_kind: String,
    summary: String,
    rationale: String,
    evidence: String,
    next_step: String,
}

fn preference_selects_candidate(critique: &BlindCritique, candidate_is_a: bool) -> bool {
    matches!(
        (critique.preferred.as_str(), candidate_is_a),
        ("a", true) | ("b", false)
    )
}

fn preference_supports_candidate(critique: &BlindCritique, candidate_is_a: bool) -> bool {
    preference_selects_candidate(critique, candidate_is_a) || critique.preferred == "equivalent"
}

fn score_for_candidate(critique: &BlindCritique, candidate_is_a: bool) -> u32 {
    if candidate_is_a {
        critique.score_a
    } else {
        critique.score_b
    }
}

fn visual_preference_selects_candidate(critique: &VisualCritique, candidate_is_a: bool) -> bool {
    matches!(
        (critique.preferred.as_str(), candidate_is_a),
        ("a", true) | ("b", false)
    )
}

fn visual_preference_supports_candidate(critique: &VisualCritique, candidate_is_a: bool) -> bool {
    visual_preference_selects_candidate(critique, candidate_is_a)
        || critique.preferred == "equivalent"
}

fn visual_score_for_candidate(critique: &VisualCritique, candidate_is_a: bool) -> u32 {
    if candidate_is_a {
        critique.score_a
    } else {
        critique.score_b
    }
}

fn screen_for_candidate(critique: &VisualCritique, candidate_is_a: bool) -> &ScreenComprehension {
    if candidate_is_a {
        &critique.screen_a
    } else {
        &critique.screen_b
    }
}

fn candidate_meets_quality_bar(
    checkpoint_passes: bool,
    visual_score: u32,
    gameplay_score: u32,
    acceptance_score: u32,
    screen: &ScreenComprehension,
) -> bool {
    checkpoint_passes
        && visual_score >= acceptance_score
        && gameplay_score >= acceptance_score
        && screen.passes()
}

fn critics_allow_checkpoint(
    visual: &VisualCritique,
    gameplay: &BlindCritique,
    candidate_is_a: bool,
) -> bool {
    visual_preference_supports_candidate(visual, candidate_is_a)
        && preference_supports_candidate(gameplay, candidate_is_a)
        && (visual_preference_selects_candidate(visual, candidate_is_a)
            || preference_selects_candidate(gameplay, candidate_is_a))
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
    sync_vendor_checkpoint(workspace)?;
    require_clean_checkout(&workspace.root)?;
    let base_commit = git_stdout(&workspace.root, &["rev-parse", "HEAD"])?;
    let run_id = new_run_id(&base_commit);
    let artifacts = run_artifacts(&workspace.root, &run_id);
    fs::create_dir_all(&artifacts)
        .map_err(|error| format!("failed creating Gauntlet run directory: {error}"))?;
    let mut references = config.quality_bar.references.clone();
    references.extend(import_references(&workspace.root, additional_references)?);
    config.quality_bar.references = freeze_references(&workspace.root, &run_id, references)?;
    config.validate()?;
    write_json(&artifacts.join(EFFECTIVE_CONFIG_NAME), &config)?;
    let (project_root, branch) = match &config.execution.isolation {
        GauntletIsolation::InPlace => {
            let branch = git_stdout(&workspace.root, &["branch", "--show-current"])?;
            if branch.is_empty() {
                return Err("in-place Gauntlet runs require a checked-out branch".to_string());
            }
            (workspace.root.clone(), branch)
        }
        GauntletIsolation::Worktree => {
            let branch = format!("stasis/gauntlet/{run_id}");
            let worktree = workspace
                .root
                .join(RUNS_PATH)
                .join("worktrees")
                .join(&run_id);
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
            (worktree, branch)
        }
    };
    let bootstrap_commit = ensure_worktree_ignores(&project_root)?;
    let now = unix_ms();
    let mut state = GauntletRunStateV1 {
        schema_version: GAUNTLET_SCHEMA_VERSION,
        run_id: run_id.clone(),
        phase: GauntletRunPhase::Created,
        project_root: project_root.to_string_lossy().to_string(),
        original_root: workspace.root.to_string_lossy().to_string(),
        branch,
        base_commit: base_commit.clone(),
        best_commit: Some(bootstrap_commit),
        current_workstream: None,
        model_calls: 0,
        accepted_candidates: 0,
        rejected_candidates: 0,
        consecutive_stalls: 0,
        quality_acceptance_streak: 0,
        started_unix_ms: now,
        session_started_unix_ms: now,
        updated_unix_ms: now,
        terminal_reason: None,
    };
    persist_state(&artifacts, &mut state)?;
    emit_event(
        &artifacts,
        "run_created",
        json!({
            "run_id": run_id,
            "branch": state.branch,
            "project_root": project_root,
            "isolation": config.execution.isolation,
        }),
    )?;
    let imported = import_prior_run_lessons(&workspace.root, &artifacts, &run_id)?;
    if imported > 0 {
        emit_event(
            &artifacts,
            "prior_run_lessons_imported",
            json!({"count": imported}),
        )?;
    }
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
    if !phase_is_resumable(&state.phase) {
        return status(workspace, run_id);
    }
    if heartbeat_is_fresh(&artifacts.join(HEARTBEAT_NAME)) {
        return Err(format!(
            "Gauntlet {run_id} still has a live controller; use status or stop instead of starting a concurrent resume"
        ));
    }
    let mut config: GauntletConfigV1 = read_json(&artifacts.join(EFFECTIVE_CONFIG_NAME))?;
    config.execution.observer = observer;
    config.validate()?;
    write_json(&artifacts.join(EFFECTIVE_CONFIG_NAME), &config)?;
    let project_root = PathBuf::from(&state.project_root);
    if !project_root.join("stasis.json").is_file() {
        return Err(format!(
            "Gauntlet project workspace is unavailable: {}",
            project_root.display()
        ));
    }
    rollback_candidate(
        &project_root,
        state.best_commit.as_deref().unwrap_or(&state.base_commit),
    )?;
    let stop = artifacts.join(STOP_NAME);
    if stop.exists() {
        fs::remove_file(&stop)
            .map_err(|error| format!("failed clearing prior stop request: {error}"))?;
    }
    prepare_state_for_resume(&mut state, unix_ms());
    persist_state(&artifacts, &mut state)?;
    emit_event(&artifacts, "run_resumed", json!({}))?;
    let imported = import_prior_run_lessons(&workspace.root, &artifacts, run_id)?;
    if imported > 0 {
        emit_event(
            &artifacts,
            "prior_run_lessons_imported",
            json!({"count": imported}),
        )?;
    }
    run_persistent(workspace, config, state, artifacts)
}

fn prepare_state_for_resume(state: &mut GauntletRunStateV1, resumed_unix_ms: u64) {
    state.phase = GauntletRunPhase::Building;
    state.terminal_reason = None;
    state.consecutive_stalls = 0;
    state.session_started_unix_ms = resumed_unix_ms;
}

pub(super) fn status(workspace: &Workspace, run_id: &str) -> Result<CommandResult, String> {
    validate_run_id(run_id)?;
    let artifacts = run_artifacts(&workspace.root, run_id);
    let state = load_state(&artifacts)?;
    let heartbeat = heartbeat_unix_ms(&artifacts.join(HEARTBEAT_NAME));
    let active =
        heartbeat.is_some_and(|value| unix_ms().saturating_sub(value) <= HEARTBEAT_STALE_AFTER_MS);
    let recoverable = !active && phase_is_resumable(&state.phase);
    let mut data = serde_json::to_value(&state).map_err(|error| error.to_string())?;
    data["health"] = json!({
        "active": active,
        "recoverable": recoverable,
        "heartbeat_unix_ms": heartbeat,
        "stale_after_ms": HEARTBEAT_STALE_AFTER_MS,
    });
    let health = if active {
        "active"
    } else if recoverable {
        "interrupted; resume is safe"
    } else {
        "terminal"
    };
    Ok(CommandResult::success(
        format!("{}\nhealth: {health}", format_status(&state, &artifacts)),
        data,
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
    let in_place =
        fs::canonicalize(&workspace.root).ok() == fs::canonicalize(&state.project_root).ok();
    if in_place {
        if head != best {
            return Err(format!(
                "in-place promotion expected the accepted checkpoint {best}; current HEAD is {head}"
            ));
        }
        emit_event(
            &artifacts,
            "promoted",
            json!({"commit": best, "already_in_place": true}),
        )?;
        return Ok(CommandResult::success(
            format!("Gauntlet {run_id} checkpoint {best} is already in the main project"),
            json!({"run_id": run_id, "commit": best, "branch": state.branch, "already_in_place": true}),
        ));
    }
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
    let _heartbeat = RunHeartbeat::start(&artifacts)?;
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
        let _quit = LiveQuitGuard(controller_client.clone());
        controller_loop(
            controller_client.clone(),
            config,
            state,
            &controller_artifacts,
        )
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
    if !is_terminal_phase(&state.phase) {
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
    let session_started_unix_ms = if state.session_started_unix_ms == 0 {
        state.started_unix_ms
    } else {
        state.session_started_unix_ms
    };
    let elapsed_before = Duration::from_millis(unix_ms().saturating_sub(session_started_unix_ms));
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
        let bar = match bootstrap_bar(
            &goal,
            &project_root,
            &config,
            &mut state,
            artifacts,
            &canceled,
        ) {
            Ok(bar) => bar,
            Err(error) if canceled.load(Ordering::Acquire) => {
                finish_canceled_or_budget(&mut state, artifacts)?;
                emit_event(
                    artifacts,
                    "role_interrupted",
                    json!({"role": "quality_bar", "error": error}),
                )?;
                generate_report_shell(artifacts, &state)?;
                return Ok(state);
            }
            Err(error) => return Err(error),
        };
        persist_state(artifacts, &mut state)?;
        write_json(&artifacts.join(QUALITY_BAR_NAME), &bar)?;
        emit_event(
            artifacts,
            "quality_bar_frozen",
            serde_json::to_value(&bar).map_err(|e| e.to_string())?,
        )?;
        bar
    };
    write_creative_direction(artifacts, &bar)?;
    let reference_images = resolve_reference_images(
        &bar.references,
        &project_root,
        Path::new(&state.original_root),
    );
    let scenario_pointer = logical_center(&project_root);
    request_live(&client, 1, LiveCommand::Pause)?;
    request_live(&client, 2, LiveCommand::ValidationSnapshot)?;
    let mut next_request = 3_u64;
    let mut baseline = capture_scenario(
        &client,
        &project_root,
        artifacts,
        "baseline",
        scenario_pointer,
        &config.quality_bar.required_scenarios,
        &mut next_request,
    )?;
    let (readiness, baseline_tests) =
        verify_harness_readiness(&project_root, artifacts, &canceled)?;
    baseline.state = gameplay_evidence(baseline.state, baseline_tests);
    emit_event(artifacts, "harness_ready", json!({"evidence": readiness}))?;
    append_decision(
        artifacts,
        "controller",
        "harness_ready",
        "Verified the Stasis test harness before model work",
        "Harness provisioning is controller-owned and must not be delegated to a semantic builder.",
        &readiness,
        "Select the highest-value game implementation gap; the harness is ready and requires no builder investigation.",
    )?;
    let mut largest_gap = match restore_largest_evidenced_gap(artifacts)? {
        Some(gap) => gap,
        None => "The controller verified the Stasis executable and existing deterministic project tests. Select the highest-value game implementation gap; do not assign harness provisioning or CLI discovery to the builder.".to_string(),
    };
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
            &baseline,
            &reference_images,
            &state,
            &mut model_calls,
            artifacts,
            &config.models.lead,
            config.models.controller_escalation.as_ref(),
            config.budget.model_calls,
            &canceled,
        );
        state.model_calls = model_calls;
        persist_state(artifacts, &mut state)?;
        let decision = match decision {
            Ok(decision) => decision,
            Err(error) if canceled.load(Ordering::Acquire) => {
                finish_canceled_or_budget(&mut state, artifacts)?;
                emit_event(
                    artifacts,
                    "role_interrupted",
                    json!({"role": "lead", "error": error}),
                )?;
                break;
            }
            Err(error) => {
                emit_event(
                    artifacts,
                    "lead_fallback",
                    json!({"reason": error, "recovery": "use the frozen workstream and largest gap"}),
                )?;
                fallback_lead_decision(&bar, &largest_gap)
            }
        };
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
                "Accepted {} candidates and rejected {}; largest gap: {largest_gap}; playability guidance: {}",
                state.accepted_candidates,
                state.rejected_candidates,
                decision.playability_guidance
            ),
            &decision.next_step,
        )?;
        if decision.done && state.quality_acceptance_streak >= FINAL_ACCEPTANCES {
            match run_final_gates(&project_root, artifacts, &canceled) {
                Ok(()) => {
                    finish(
                        &mut state,
                        artifacts,
                        GauntletRunPhase::Converged,
                        "independent evaluations and release gates met the frozen bar",
                    )?;
                    break;
                }
                Err(error) if canceled.load(Ordering::Acquire) => {
                    finish_canceled_or_budget(&mut state, artifacts)?;
                    emit_event(
                        artifacts,
                        "final_validation_interrupted",
                        json!({"error": error}),
                    )?;
                    break;
                }
                Err(error) => {
                    largest_gap = format!("Final release validation failed: {error}");
                    state.quality_acceptance_streak = 0;
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
        let decision = if decision.builder_prompt.trim().is_empty()
            || decision.workstream.trim().is_empty()
        {
            emit_event(
                artifacts,
                "lead_fallback",
                json!({"reason": "lead returned an empty work item", "recovery": "use the frozen workstream and largest gap"}),
            )?;
            fallback_lead_decision(&bar, &largest_gap)
        } else {
            decision
        };
        state.phase = GauntletRunPhase::Building;
        state.current_workstream = Some(decision.workstream.clone());
        persist_state(artifacts, &mut state)?;
        let builder_calls = builder_turn_allowance(&config, state.model_calls);
        if builder_calls == 0 {
            finish(
                &mut state,
                artifacts,
                GauntletRunPhase::BudgetExhausted,
                "model-call budget cannot fund a builder plus both independent critics",
            )?;
            break;
        }
        let candidate_id = format!(
            "candidate-{:04}",
            state.accepted_candidates + state.rejected_candidates + 1
        );
        let memory = decision_memory_snapshot(artifacts)?;
        let require_imagegen = requires_authored_imagegen(&decision.workstream);
        let asset_guidance = if require_imagegen {
            "This is an authored visual-art workstream. You must request, fulfill, transactionally import, load, and visibly draw at least one ImageGen PNG before completion; importing an unused file does not satisfy the gate. Request one isolated foreground subject on a flat removable background by default. Render contract v2 draws line primitives before sprites, so an opaque full-board sprite will cover line-rendered terrain, units, and overlays; do not request or import one unless the project already has a verified sprite-first background path. Primitive vectors may supplement the bitmap only for basic UI, selection/range overlays, or a deterministic fallback after an actual ImageGen blocker."
        } else {
            "ImageGen is optional for this logic or basic-interface workstream. Use primitive vectors for basic UI, simple icons, selection/range overlays, and deterministic fallbacks."
        };
        let creative_direction = creative_direction_context(&bar)?;
        let prompt = format!(
            "Frozen game brief:\n{goal}\n\nAuthoritative creative direction (do not silently revise):\n{creative_direction}\n\nWorkstream: {}\nTask: {}\nLargest evidenced gap: {largest_gap}\n\nPlayability and visual-coherence direction for the accepted frame:\n{}\n\nDurable decision memory (explicit conclusions only):\n{memory}\n\nAsset guidance: {asset_guidance}\n\nMake one coherent, end-to-end improvement. Preserve deterministic tick semantics. Add or update durable Stasis tests in the same atomic write when behavior changes. Use record_decision for consequential choices and finish after the tested write succeeds.",
            decision.workstream, decision.builder_prompt, decision.playability_guidance,
        );
        let run_builder = |model: &GauntletRoleModel,
                           role: &str,
                           instruction: &str,
                           agent_prompt: &str,
                           available_calls: u32| {
            let profile = builder_agent_profile(&config, model, role, instruction, available_calls);
            live_tui::run_scripted_ai_profile(
                &client,
                &project_root,
                agent_prompt,
                profile,
                vec![
                    baseline.initial_frame.clone(),
                    baseline.action_frame.clone(),
                ],
                false,
                true,
                true,
                Some(&artifacts.join(DECISIONS_NAME)),
                require_imagegen,
                &canceled,
            )
        };
        let primary_instruction = "Use only the supplied Stasis live-workspace tools. Inspect relevant symbols and references, then make one contiguous atomic semantic edit batch. For readable UI text, use the existing project-local assets/gauntlet-ui.ttf font; system and absolute font paths are invalid and rejected. Authored visual-art workstreams require request_imagegen_asset plus a transactionally imported PNG that project Stasis source actually loads and emits through a sprite draw path; importing an unused file does not satisfy the completion gate. Reserve primitive shapes primarily for basic UI, simple icons, selection/range overlays, and deterministic fallbacks. ImageGen remains optional for pure logic or basic interface geometry. Request one isolated foreground subject per PNG on a flat removable background rather than an atlas. Render contract v2 draws line primitives before sprites; never place an opaque full-board sprite over a line-rendered battlefield unless the project already has a verified sprite-first background path. Use the 1024x1024 master default; request up to 2048x2048 only when extra detail or crop latitude is needed. The tool persists the request and waits for the host PNG, then returns the source_path for import_png_asset crop/background-removal. Use delete_asset in the same rollback-safe asset/source batch when a replacement makes an older generated asset obsolete; place deletion before a replacement that reuses the same id. You may also create JSON/CSV or procedural WAV assets. Put one contiguous asset-tool group immediately before the related source writes in the same response. Use record_decision during exploration and after consequential tested choices to preserve concise conclusions, tradeoffs, evidence, and next steps for future agents; never record hidden chain-of-thought. The write must compile and run tests. Do not grade your own visual quality. Return done immediately after a successful tested write and decision record. If a non-recoverable environment, harness, permission, or missing-capability condition makes completion impossible with the supplied tools, call report_blocked once; it terminates this attempt immediately. Never retry the same terminal failure.";
        emit_builder_attempt_started(
            artifacts,
            &candidate_id,
            "primary",
            &config.models.builder,
            builder_calls,
        )?;
        let mut completed_attempt = "primary";
        let mut outcome = run_builder(
            &config.models.builder,
            "Fresh Stasis Gauntlet builder",
            primary_instruction,
            &prompt,
            builder_calls,
        );
        let mut latest_failure_evidence = None;
        if let Err(failure) = &outcome {
            let primary_failure_evidence = record_builder_attempt_failure(
                &mut state,
                artifacts,
                &candidate_id,
                "primary",
                &config.models.builder,
                failure,
            )?;
            latest_failure_evidence = Some(primary_failure_evidence.clone());
            let rescue_calls = builder_turn_allowance(&config, state.model_calls);
            let escalation = config
                .models
                .builder_escalation
                .as_ref()
                .filter(|_| should_escalate_builder(failure, &canceled));
            if let Some(escalation) = escalation.filter(|_| rescue_calls > 0) {
                rollback_candidate(
                    &project_root,
                    state.best_commit.as_deref().unwrap_or(&state.base_commit),
                )?;
                emit_event(
                    artifacts,
                    "builder_escalated",
                    json!({
                        "candidate": candidate_id,
                        "from_model": config.models.builder.model,
                        "to_model": escalation.model,
                        "reason": failure.message,
                        "evidence": primary_failure_evidence,
                        "fresh_turn_allowance": rescue_calls,
                    }),
                )?;
                append_decision(
                    artifacts,
                    "controller",
                    "builder_escalated",
                    &format!("Escalated {candidate_id} to the rescue builder"),
                    "The primary builder could not finish, so the configured one-shot escalation policy applies.",
                    &primary_failure_evidence,
                    "Make one bounded rescue attempt, then either complete or preserve the terminal blocker.",
                )?;
                let escalation_instruction = "You are the one-shot rescue builder after the primary builder failed. Use the durable decision memory and current evidence to avoid repeating failed exploration. Make one bounded atomic correction that completes the assigned work. For readable UI text, use the existing project-local assets/gauntlet-ui.ttf font; system and absolute font paths are invalid and rejected. Authored visual-art workstreams require request_imagegen_asset plus a transactionally imported PNG that project Stasis source actually loads and emits through a sprite draw path; importing an unused file does not satisfy the completion gate. Request an isolated foreground subject with a removable background by default: render contract v2 draws lines before sprites, so an opaque full-board sprite obscures a line-rendered battlefield. Reserve primitive shapes primarily for basic UI and overlays. If generated bitmap art is the missing input, request it once, import the returned source_path transactionally, and visibly draw it. If the failure is environmental or otherwise non-recoverable from the supplied tools, call report_blocked once to terminate immediately. Never loop on the same failed operation.";
                let rescue_memory = decision_memory_snapshot(artifacts)?;
                let rescue_prompt = format!(
                    "{prompt}\n\nPrimary builder failure evidence (do not repeat this failure):\n{primary_failure_evidence}\n\nUpdated durable decision memory:\n{rescue_memory}"
                );
                emit_builder_attempt_started(
                    artifacts,
                    &candidate_id,
                    "escalation",
                    escalation,
                    rescue_calls,
                )?;
                completed_attempt = "escalation";
                outcome = run_builder(
                    escalation,
                    "Escalated Stasis Gauntlet builder",
                    escalation_instruction,
                    &rescue_prompt,
                    rescue_calls,
                );
                if let Err(failure) = &outcome {
                    latest_failure_evidence = Some(record_builder_attempt_failure(
                        &mut state,
                        artifacts,
                        &candidate_id,
                        "escalation",
                        escalation,
                        failure,
                    )?);
                }
            } else if escalation.is_some() && rescue_calls == 0 {
                emit_event(
                    artifacts,
                    "builder_escalation_skipped",
                    json!({
                        "candidate": candidate_id,
                        "reason": "model-call budget cannot fund a rescue builder plus both independent critics",
                        "remaining_calls": config.budget.model_calls.saturating_sub(state.model_calls),
                    }),
                )?;
            }
        }
        match outcome {
            Ok(outcome) => {
                state.model_calls = state.model_calls.saturating_add(outcome.model_calls);
                persist_state(artifacts, &mut state)?;
                append_usage_file(artifacts, &outcome.usage_trace)?;
                emit_event(
                    artifacts,
                    "role_attempt_completed",
                    json!({
                        "role": "builder",
                        "attempt": completed_attempt,
                        "candidate": candidate_id,
                        "model_calls": outcome.model_calls,
                    }),
                )?;
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
            Err(failure) => {
                rollback_candidate(
                    &project_root,
                    state.best_commit.as_deref().unwrap_or(&state.base_commit),
                )?;
                reject(
                    &mut state,
                    artifacts,
                    &candidate_id,
                    &format!("builder failed: {}", failure.message),
                )?;
                let evidence = latest_failure_evidence.unwrap_or_else(|| {
                    builder_failure_evidence(&failure.message, failure.trace.as_deref())
                });
                largest_gap = format!(
                    "The previous builder failed before producing a valid candidate: {evidence}"
                );
                continue;
            }
        }
        state.phase = GauntletRunPhase::Evaluating;
        persist_state(artifacts, &mut state)?;
        let candidate_capture = capture_scenario(
            &client,
            &project_root,
            artifacts,
            &candidate_id,
            scenario_pointer,
            &config.quality_bar.required_scenarios,
            &mut next_request,
        );
        let mut candidate = match candidate_capture {
            Ok(evidence) => evidence,
            Err(error) => {
                rollback_candidate(
                    &project_root,
                    state.best_commit.as_deref().unwrap_or(&state.base_commit),
                )?;
                reject(
                    &mut state,
                    artifacts,
                    &candidate_id,
                    &format!("candidate evidence capture failed: {error}"),
                )?;
                largest_gap = format!(
                    "The candidate could not be evaluated because deterministic evidence capture failed: {error}"
                );
                continue;
            }
        };
        let candidate_tests = match run_project_test_evidence(
            &project_root,
            &artifacts.join(format!("{candidate_id}-tests.log")),
            &canceled,
        ) {
            Ok(evidence) => evidence,
            Err(error) if canceled.load(Ordering::Acquire) => {
                rollback_candidate(
                    &project_root,
                    state.best_commit.as_deref().unwrap_or(&state.base_commit),
                )?;
                finish_canceled_or_budget(&mut state, artifacts)?;
                emit_event(
                    artifacts,
                    "role_interrupted",
                    json!({"role": "candidate_tests", "error": error}),
                )?;
                break;
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
                    &format!("candidate deterministic tests failed: {error}"),
                )?;
                largest_gap = format!(
                    "The candidate could not be evaluated because its deterministic tests failed: {error}"
                );
                continue;
            }
        };
        candidate.state = gameplay_evidence(candidate.state, candidate_tests);
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
        let candidate_is_a =
            hex_sha256(format!("{}:{candidate_id}", state.run_id).as_bytes()).as_bytes()[0] % 2
                == 0;
        let (a, b) = if candidate_is_a {
            (&candidate, &baseline)
        } else {
            (&baseline, &candidate)
        };
        let visual_call_limit = state.model_calls.saturating_add(2);
        let critique = blind_critic(
            &goal,
            &bar,
            a,
            b,
            &reference_images,
            &mut state.model_calls,
            artifacts,
            &config.models.visual_critic,
            config.models.controller_escalation.as_ref(),
            visual_call_limit,
            &canceled,
        );
        persist_state(artifacts, &mut state)?;
        let critique = match critique {
            Ok(critique) => critique,
            Err(error) if canceled.load(Ordering::Acquire) => {
                rollback_candidate(
                    &project_root,
                    state.best_commit.as_deref().unwrap_or(&state.base_commit),
                )?;
                finish_canceled_or_budget(&mut state, artifacts)?;
                emit_event(
                    artifacts,
                    "role_interrupted",
                    json!({"role": "visual_critic", "error": error}),
                )?;
                break;
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
                    &format!("visual critic exhausted recovery attempts: {error}"),
                )?;
                largest_gap =
                    format!("The candidate needs fresh visual evaluation evidence: {error}");
                continue;
            }
        };
        let preferred_candidate = visual_preference_selects_candidate(&critique, candidate_is_a);
        let candidate_score = visual_score_for_candidate(&critique, candidate_is_a);
        let (state_a, state_b) = if candidate_is_a {
            (&candidate.state, &baseline.state)
        } else {
            (&baseline.state, &candidate.state)
        };
        let gameplay_call_limit = state.model_calls.saturating_add(2);
        let gameplay = gameplay_critic(
            &goal,
            &bar,
            state_a,
            state_b,
            &mut state.model_calls,
            artifacts,
            &config.models.gameplay_critic,
            config.models.controller_escalation.as_ref(),
            gameplay_call_limit,
            &canceled,
        );
        persist_state(artifacts, &mut state)?;
        let gameplay = match gameplay {
            Ok(gameplay) => gameplay,
            Err(error) if canceled.load(Ordering::Acquire) => {
                rollback_candidate(
                    &project_root,
                    state.best_commit.as_deref().unwrap_or(&state.base_commit),
                )?;
                finish_canceled_or_budget(&mut state, artifacts)?;
                emit_event(
                    artifacts,
                    "role_interrupted",
                    json!({"role": "gameplay_critic", "error": error}),
                )?;
                break;
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
                    &format!("gameplay critic exhausted recovery attempts: {error}"),
                )?;
                largest_gap =
                    format!("The candidate needs fresh gameplay evaluation evidence: {error}");
                continue;
            }
        };
        let visual_passes = visual_preference_supports_candidate(&critique, candidate_is_a);
        let gameplay_passes = preference_supports_candidate(&gameplay, candidate_is_a);
        let checkpoint_passes = critics_allow_checkpoint(&critique, &gameplay, candidate_is_a);
        let gameplay_score = score_for_candidate(&gameplay, candidate_is_a);
        let screen_comprehension = screen_for_candidate(&critique, candidate_is_a);
        let screen_comprehension_passes = screen_comprehension.passes();
        let quality_bar_passes = candidate_meets_quality_bar(
            checkpoint_passes,
            candidate_score,
            gameplay_score,
            bar.acceptance_score,
            screen_comprehension,
        );
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
                "gameplay_score": gameplay_score,
                "screen_comprehension": screen_comprehension,
                "screen_comprehension_passes": screen_comprehension_passes,
                "checkpoint_passes": checkpoint_passes,
                "quality_bar_passes": quality_bar_passes,
            }),
        )?;
        if checkpoint_passes {
            state.phase = GauntletRunPhase::Checkpointing;
            persist_state(artifacts, &mut state)?;
            let commit = checkpoint(&project_root, &candidate_id, &decision.workstream)?;
            state.best_commit = Some(commit.clone());
            state.accepted_candidates = state.accepted_candidates.saturating_add(1);
            state.consecutive_stalls = 0;
            if quality_bar_passes {
                state.quality_acceptance_streak = state.quality_acceptance_streak.saturating_add(1);
            } else {
                state.quality_acceptance_streak = 0;
            }
            baseline = candidate;
            largest_gap = if gameplay_score < candidate_score {
                gameplay.largest_gap.clone()
            } else {
                critique.largest_gap.clone()
            };
            persist_state(artifacts, &mut state)?;
            emit_event(
                artifacts,
                "candidate_accepted",
                json!({
                    "candidate": candidate_id,
                    "commit": commit,
                    "quality_bar_passes": quality_bar_passes,
                    "visual_score": candidate_score,
                    "gameplay_score": gameplay_score,
                }),
            )?;
            append_decision(
                artifacts,
                "controller",
                "candidate_accepted",
                &format!("Accepted {candidate_id} for {}", decision.workstream),
                "At least one independent critic preferred the candidate and neither critic found a regression.",
                &format!(
                    "commit={commit}; visual_score={candidate_score}; gameplay_score={gameplay_score}; quality_bar_passes={quality_bar_passes}"
                ),
                &largest_gap,
            )?;
            generate_report_shell(artifacts, &state)?;
        } else {
            rollback_candidate(
                &project_root,
                state.best_commit.as_deref().unwrap_or(&state.base_commit),
            )?;
            largest_gap = if !gameplay_passes {
                gameplay.largest_gap.clone()
            } else {
                critique.largest_gap.clone()
            };
            state.quality_acceptance_streak = 0;
            let rejection_reason = if !visual_passes {
                "visual critic preferred the accepted checkpoint or found neither candidate sufficient"
            } else if !gameplay_passes {
                "gameplay critic preferred the accepted checkpoint or found neither candidate sufficient"
            } else {
                "critics found no evidenced improvement over the accepted checkpoint"
            };
            reject(&mut state, artifacts, &candidate_id, rejection_reason)?;
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

fn fallback_lead_decision(bar: &FrozenBar, largest_gap: &str) -> LeadDecision {
    let workstream = bar
        .workstreams
        .first()
        .cloned()
        .unwrap_or_else(|| "Integration and release quality".to_string());
    LeadDecision {
        done: false,
        workstream,
        builder_prompt: format!(
            "Make one bounded, tested improvement that directly addresses this evidenced gap: {largest_gap}"
        ),
        playability_guidance: "Make the board teach its rules visually: establish unmistakable cell boundaries and terrain categories, show the selected unit and its legal destinations, distinguish attack targets from movement, keep turn/objective information visible, and ensure every action has an obvious next tap and cancel path without obscuring the grid.".to_string(),
        rationale: "The configured lead exhausted its bounded attempts, so the controller selected a deterministic recovery task from the frozen bar.".to_string(),
        next_step: "Produce one compiler-tested candidate, then return it to independent evaluation.".to_string(),
    }
}

fn finish_canceled_or_budget(
    state: &mut GauntletRunStateV1,
    artifacts: &Path,
) -> Result<(), String> {
    if should_stop(artifacts) {
        finish(
            state,
            artifacts,
            GauntletRunPhase::Canceled,
            "stop requested during an active operation",
        )
    } else {
        finish(
            state,
            artifacts,
            GauntletRunPhase::BudgetExhausted,
            "wall-time budget exhausted during an active operation",
        )
    }
}

fn bootstrap_bar(
    goal: &str,
    project_root: &Path,
    config: &GauntletConfigV1,
    state: &mut GauntletRunStateV1,
    artifacts: &Path,
    canceled: &AtomicBool,
) -> Result<FrozenBar, String> {
    let direction_path = project_root.join(PROJECT_CREATIVE_DIRECTION_NAME);
    let direction_source_markdown = if direction_path.is_file() {
        read_bounded_utf8(
            &direction_path,
            MAX_GOAL_BYTES,
            "project creative direction",
        )?
    } else {
        String::new()
    };
    let direction_source_sha256 = if direction_source_markdown.is_empty() {
        String::new()
    } else {
        hex_sha256(direction_source_markdown.as_bytes())
    };
    let mut web_sources = Vec::new();
    if config.quality_bar.allow_web_discovery
        && config.budget.model_calls.saturating_sub(state.model_calls) >= 2
    {
        let prompt = format!(
            "Act as a read-only visual reference scout for this 2D game brief. Find up to five reputable HTTPS pages whose visual or interaction patterns can form a concrete quality bar. Do not suggest copying protected assets. Return only the requested JSON.\n\n{goal}"
        );
        let scout = call_structured_role::<ScoutResult>(
            "reference_scout",
            &prompt,
            &scout_schema(),
            &config.models.scout,
            config.models.controller_escalation.as_ref(),
            &[],
            true,
            &mut state.model_calls,
            config.budget.model_calls.saturating_sub(1),
            artifacts,
            canceled,
        );
        persist_state(artifacts, state)?;
        match scout {
            Ok(scout) => {
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
            Err(error) if canceled.load(Ordering::Acquire) => return Err(error),
            Err(error) => emit_event(
                artifacts,
                "reference_scout_skipped",
                json!({"reason": error, "recovery": "continue with local brief and references"}),
            )?,
        }
    }
    let prior_direction =
        restore_prior_creative_direction(artifacts, goal, &direction_source_sha256)?;
    let (creative_direction, workstreams) = if let Some((source_run, direction, workstreams)) =
        prior_direction
    {
        emit_event(
            artifacts,
            "creative_direction_reused",
            json!({"source_run_id": source_run, "goal_sha256": hex_sha256(goal.as_bytes())}),
        )?;
        (direction, workstreams)
    } else {
        let source_instruction = if direction_source_markdown.is_empty() {
            "No project-authored CREATIVE_DIRECTION.md was supplied; derive the direction faithfully from the immutable brief.".to_string()
        } else {
            format!("The project-authored CREATIVE_DIRECTION.md below is authoritative user direction. Produce a faithful structured operational digest; resolve omissions from the immutable brief but never contradict or weaken the source document.\n\n{direction_source_markdown}")
        };
        let prompt = format!(
            "Act as the creative director for an autonomous Stasis 2D game build. Turn the immutable brief and any project-authored direction into a durable direction bible that fresh agents can follow without drifting. Define the narrative promise and player fantasy; 3-8 concrete rule pillars; 3-8 visual-language rules covering hierarchy, faction/role/terrain recognition, authored imagery, motion, and mobile scale; 3-8 interaction-grammar rules covering visible current state, available actions, selection, legal movement/attack, feedback, end turn, and cancel/reselect; 2-6 progression/pacing rules; and 3-8 non-negotiables. Make these project-specific and operational rather than aspirational. Then decompose the direction into 4-10 independently improvable workstreams using short noun phrases. This direction is authoritative for the run and may not be silently rewritten by later builders. Return only the requested JSON.\n\nImmutable game brief:\n{goal}\n\n{source_instruction}"
        );
        let bootstrap = call_structured_role::<LeadBootstrap>(
            "quality_bar_lead",
            &prompt,
            &bootstrap_schema(),
            &config.models.lead,
            config.models.controller_escalation.as_ref(),
            &[],
            false,
            &mut state.model_calls,
            config.budget.model_calls,
            artifacts,
            canceled,
        );
        persist_state(artifacts, state)?;
        match bootstrap {
            Ok(bootstrap) => (
                bootstrap.creative_direction,
                bootstrap
                    .workstreams
                    .into_iter()
                    .filter(|value| !value.trim().is_empty())
                    .take(10)
                    .collect::<Vec<_>>(),
            ),
            Err(error) if canceled.load(Ordering::Acquire) => return Err(error),
            Err(error) => {
                emit_event(
                    artifacts,
                    "quality_bar_lead_fallback",
                    json!({"reason": error, "recovery": "use deterministic workstreams"}),
                )?;
                (CreativeDirection::default(), default_workstreams())
            }
        }
    };
    let workstreams = if workstreams.is_empty() {
        emit_event(
            artifacts,
            "quality_bar_lead_fallback",
            json!({"reason": "lead returned no usable workstreams", "recovery": "use deterministic workstreams"}),
        )?;
        default_workstreams()
    } else {
        workstreams
    };
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
        schema_version: 3,
        goal_sha256: hex_sha256(goal.as_bytes()),
        goal: goal.to_string(),
        direction_source_markdown,
        direction_source_sha256,
        creative_direction,
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

fn default_workstreams() -> Vec<String> {
    [
        "Core gameplay and deterministic rules",
        "Controls and mobile interaction",
        "Visual identity and animation",
        "HUD and player feedback",
        "Enemy behavior and balance",
        "Audio and presentation",
        "Integration and release quality",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

fn restore_prior_creative_direction(
    artifacts: &Path,
    goal: &str,
    direction_source_sha256: &str,
) -> Result<Option<(String, CreativeDirection, Vec<String>)>, String> {
    let Some(runs) = artifacts.parent() else {
        return Ok(None);
    };
    if !runs.is_dir() {
        return Ok(None);
    }
    let goal_sha256 = hex_sha256(goal.as_bytes());
    let mut run_dirs = fs::read_dir(runs)
        .map_err(|error| format!("failed reading prior Gauntlet directions: {error}"))?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter(|entry| entry.path() != artifacts && entry.file_name() != "worktrees")
        .collect::<Vec<_>>();
    run_dirs.sort_by_key(|entry| std::cmp::Reverse(run_sort_key(entry)));
    for entry in run_dirs.into_iter().take(MAX_PRIOR_RUNS) {
        let bar_path = entry.path().join(QUALITY_BAR_NAME);
        if !bar_path.is_file() {
            continue;
        }
        let Ok(bar) = read_json::<FrozenBar>(&bar_path) else {
            continue;
        };
        if bar.schema_version < 2
            || bar.goal_sha256 != goal_sha256
            || bar.direction_source_sha256 != direction_source_sha256
            || bar.workstreams.is_empty()
        {
            continue;
        }
        let source_run = entry.file_name().to_string_lossy().to_string();
        return Ok(Some((source_run, bar.creative_direction, bar.workstreams)));
    }
    Ok(None)
}

fn write_creative_direction(artifacts: &Path, bar: &FrozenBar) -> Result<(), String> {
    fn section(markdown: &mut String, heading: &str, values: &[String]) {
        markdown.push_str(&format!("## {heading}\n\n"));
        for value in values {
            markdown.push_str(&format!("- {}\n", value.trim()));
        }
        markdown.push('\n');
    }

    let direction = &bar.creative_direction;
    let mut markdown = "# Gauntlet Creative Direction Record\n\nThis controller-owned record is authoritative for this Gauntlet run. Builders may refine implementation, but must not silently change these commitments.\n\n".to_string();
    if !bar.direction_source_markdown.is_empty() {
        markdown.push_str("## Project-authored authority (verbatim)\n\n");
        markdown.push_str(bar.direction_source_markdown.trim());
        markdown.push_str("\n\n---\n\n");
    }
    markdown.push_str(&format!(
        "## Structured director digest: {}\n\n### Narrative promise\n\n{}\n\n### Player fantasy\n\n{}\n\n",
        direction.title.trim(),
        direction.narrative_promise.trim(),
        direction.player_fantasy.trim(),
    ));
    section(
        &mut markdown,
        "Digest: rule pillars",
        &direction.rule_pillars,
    );
    section(
        &mut markdown,
        "Digest: visual language",
        &direction.visual_language,
    );
    section(
        &mut markdown,
        "Digest: interaction grammar",
        &direction.interaction_grammar,
    );
    section(
        &mut markdown,
        "Digest: progression and pacing",
        &direction.progression_and_pacing,
    );
    section(
        &mut markdown,
        "Digest: non-negotiables",
        &direction.non_negotiables,
    );
    fs::write(artifacts.join(CREATIVE_DIRECTION_NAME), markdown)
        .map_err(|error| format!("failed writing Gauntlet creative direction: {error}"))
}

fn creative_direction_context(bar: &FrozenBar) -> Result<String, String> {
    let digest = serde_json::to_string(&bar.creative_direction)
        .map_err(|error| format!("failed encoding creative direction: {error}"))?;
    if bar.direction_source_markdown.is_empty() {
        Ok(format!("Structured director digest:\n{digest}"))
    } else {
        Ok(format!(
            "Project-authored authority (verbatim; sha256={}):\n{}\n\nStructured director digest:\n{digest}",
            bar.direction_source_sha256,
            bar.direction_source_markdown.trim(),
        ))
    }
}

fn requires_authored_imagegen(workstream: &str) -> bool {
    let workstream = workstream.to_ascii_lowercase();
    [
        "art",
        "visual",
        "graphics",
        "sprite",
        "illustration",
        "animation",
    ]
    .iter()
    .any(|keyword| workstream.contains(keyword))
}

fn provider_for_role(role: &GauntletRoleModel) -> CodexExecProvider {
    let mut provider = CodexExecProvider::default()
        .with_timeout(Duration::from_secs(u64::from(role.timeout_minutes) * 60));
    if let Some(model) = role.model.as_deref() {
        provider = provider.with_model(model);
    }
    if let Some(reasoning_effort) = role.reasoning_effort.as_deref() {
        provider = provider.with_reasoning_effort(reasoning_effort);
    }
    provider
}

#[allow(clippy::too_many_arguments)]
fn call_structured_role<T: DeserializeOwned>(
    role_name: &str,
    prompt: &str,
    schema: &Value,
    primary: &GauntletRoleModel,
    escalation: Option<&GauntletRoleModel>,
    images: &[PathBuf],
    web_search: bool,
    model_calls: &mut u32,
    model_call_limit: u32,
    artifacts: &Path,
    canceled: &AtomicBool,
) -> Result<T, String> {
    let attempts = [Some(primary), escalation];
    let mut failures = Vec::new();
    for (index, model) in attempts.into_iter().flatten().enumerate() {
        if *model_calls >= model_call_limit {
            break;
        }
        let attempt = if index == 0 { "primary" } else { "escalation" };
        emit_event(
            artifacts,
            "role_attempt_started",
            json!({
                "role": role_name,
                "attempt": attempt,
                "model": model.model,
                "reasoning_effort": model.reasoning_effort,
                "timeout_minutes": model.timeout_minutes,
            }),
        )?;
        let mut provider = provider_for_role(model)
            .with_images(images.to_vec())
            .with_web_search(web_search);
        let result = provider.respond_structured(prompt, schema, canceled);
        *model_calls = model_calls.saturating_add(provider.call_count());
        append_provider_usage(artifacts, &mut provider)?;
        match result {
            Ok(value) => {
                emit_event(
                    artifacts,
                    "role_attempt_completed",
                    json!({"role": role_name, "attempt": attempt}),
                )?;
                return Ok(value);
            }
            Err(error) => {
                emit_event(
                    artifacts,
                    "role_attempt_failed",
                    json!({"role": role_name, "attempt": attempt, "error": error}),
                )?;
                failures.push(format!("{attempt}: {error}"));
                if canceled.load(Ordering::Acquire) {
                    break;
                }
            }
        }
    }
    Err(format!(
        "{role_name} exhausted its bounded attempts{}",
        if failures.is_empty() {
            String::new()
        } else {
            format!(": {}", failures.join("; "))
        }
    ))
}

fn lead_decision(
    goal: &str,
    bar: &FrozenBar,
    largest_gap: &str,
    accepted: &ScenarioCapture,
    references: &[PathBuf],
    state: &GauntletRunStateV1,
    model_calls: &mut u32,
    artifacts: &Path,
    model: &GauntletRoleModel,
    escalation: Option<&GauntletRoleModel>,
    model_call_limit: u32,
    canceled: &AtomicBool,
) -> Result<LeadDecision, String> {
    let memory = decision_memory_snapshot(artifacts)?;
    let runtime_evidence = serde_json::to_string(&accepted.state)
        .map_err(|error| format!("failed encoding accepted runtime evidence: {error}"))?;
    let creative_direction = creative_direction_context(bar)?;
    let accepted_image_order = capture_image_order("accepted", accepted);
    let prompt = format!(
        "Act as the fresh playability and visual-coherence director for a Stasis Gauntlet. The attached images begin with the latest accepted initial frame, its frame after the controller's fixed interaction probe, and any configured deterministic scenario frames; later images are optional quality references. Accepted image order: {accepted_image_order}. The controller-owned creative direction below is authoritative: enforce and interpret it, but do not silently rewrite it. Use every supplied scenario frame together with runtime and passing-test evidence. First produce playability_guidance that teaches the next builder exactly how a new player should parse the grid and complete one turn: board and cell boundaries, meaningful terrain, faction and unit-role recognition, selection, legal movement, attack/counterattack preview, objective and economy, turn ownership, end turn, and cancel/reselect. Identify which relationships are unclear from evidence and whether each exercised interaction's result is visibly understandable; do not invent mechanics. Then choose exactly one highest-value next work item from the frozen workstreams whose builder prompt improves that comprehension and preserves already-readable relationships. Visual polish is valuable only when it strengthens this hierarchy. The live workspace contains only the latest accepted checkpoint: rejected candidate edits were rolled back, so use their evidence as lessons but never assume their implementation exists. Set done=true only if the largest gap says the bar is fully met; otherwise done=false. Preserve a concise rationale and next step for future fresh agents; do not provide hidden chain-of-thought. Return only JSON.\n\nBrief:\n{goal}\n\nAuthoritative creative direction:\n{creative_direction}\n\nWorkstreams: {}\nAccepted: {} Rejected: {}\nLargest gap: {largest_gap}\n\nAccepted runtime and deterministic-test evidence:\n{runtime_evidence}\n\nDurable decision memory (explicit conclusions only):\n{memory}",
        bar.workstreams.join(", "), state.accepted_candidates, state.rejected_candidates
    );
    let mut images = capture_images(accepted);
    images.extend(references.iter().take(4).cloned());
    let decision: LeadDecision = call_structured_role(
        "lead",
        &prompt,
        &lead_schema(),
        model,
        escalation,
        &images,
        false,
        model_calls,
        model_call_limit,
        artifacts,
        canceled,
    )?;
    if decision.playability_guidance.trim().is_empty()
        || decision.rationale.trim().is_empty()
        || decision.next_step.trim().is_empty()
    {
        return Err("Gauntlet lead returned empty decision memory fields".to_string());
    }
    Ok(decision)
}

fn blind_critic(
    goal: &str,
    bar: &FrozenBar,
    scenario_a: &ScenarioCapture,
    scenario_b: &ScenarioCapture,
    references: &[PathBuf],
    model_calls: &mut u32,
    artifacts: &Path,
    model: &GauntletRoleModel,
    escalation: Option<&GauntletRoleModel>,
    model_call_limit: u32,
    canceled: &AtomicBool,
) -> Result<VisualCritique, String> {
    let mut images = capture_images(scenario_a);
    images.extend(capture_images(scenario_b));
    images.extend(references.iter().take(5).cloned());
    let a_image_order = capture_image_order("A", scenario_a);
    let b_image_order = capture_image_order("B", scenario_b);
    let prompt = format!(
        "You are a fresh read-only visual and screen-comprehension critic. The attached images contain anonymous, identically exercised scenario sets. Image order for A: {a_image_order}. Image order for B: {b_image_order}. Any later images are hashed quality references. You do not know which candidate is newer. Compare every corresponding A/B frame relative to each other and against the frozen brief, authoritative creative direction, and references. Prefer a or b when one is visually better without an evidenced visual regression. Return equivalent when the scenario sets are materially indistinguishable; incompleteness against the full brief is not a reason to return neither. Return neither only when both have different material regressions or the evidence is invalid. Scores are integers 0-100 and measure absolute quality against the frozen bar.\n\nFor each scenario set, independently answer four release-gate questions. current_state_clear means a new player can identify turn/faction, selection, relevant resources/objective, and important ownership or board state. available_actions_clear means the screen itself distinguishes selectable things and the next legal actions, including movement, attack, end turn, and cancel/reselect when relevant. board_semantics_clear means cells, traversable terrain, obstacles, factions, unit roles, structures, and tactical overlays have an understandable hierarchy. action_feedback_clear means the exercised frames visibly communicate the result of each configured interaction; if an interaction caused no meaningful state change, the no-op or unchanged state must still be understandable rather than silently ambiguous. Set a boolean true only when the attached pixels provide affirmative evidence, not merely because runtime behavior may exist. Put concise observed evidence in each assessment. Identify one largest remaining gap that prioritizes failed comprehension questions over decorative polish. Do not discuss source code and return only JSON.\n\nBrief:\n{goal}\n\nAuthoritative creative direction:\n{}\n\nWorkstreams: {}\nHard gates already passed: {}\n\nA runtime evidence after probe:\n{}\n\nB runtime evidence after probe:\n{}",
        creative_direction_context(bar)?,
        bar.workstreams.join(", "),
        bar.hard_gates.join(", "),
        serde_json::to_string(&scenario_a.state).map_err(|error| error.to_string())?,
        serde_json::to_string(&scenario_b.state).map_err(|error| error.to_string())?,
    );
    let critique: VisualCritique = call_structured_role(
        "visual_critic",
        &prompt,
        &visual_critic_schema(),
        model,
        escalation,
        &images,
        false,
        model_calls,
        model_call_limit,
        artifacts,
        canceled,
    )?;
    if !matches!(
        critique.preferred.as_str(),
        "a" | "b" | "neither" | "equivalent"
    ) || critique.score_a > 100
        || critique.score_b > 100
        || critique.screen_a.evidence.trim().is_empty()
        || critique.screen_b.evidence.trim().is_empty()
        || critique.largest_gap.trim().is_empty()
    {
        return Err("critic returned an invalid preference, score, or gap".to_string());
    }
    Ok(critique)
}

fn capture_images(capture: &ScenarioCapture) -> Vec<PathBuf> {
    let mut images = vec![capture.initial_frame.clone(), capture.action_frame.clone()];
    images.extend(
        capture
            .required_frames
            .iter()
            .map(|scenario| scenario.frame.clone()),
    );
    images
}

fn capture_image_order(label: &str, capture: &ScenarioCapture) -> String {
    let mut order = vec![
        format!("{label} initial state"),
        format!("{label} after fixed interaction probe"),
    ];
    order.extend(capture.required_frames.iter().map(|scenario| {
        format!(
            "{label} scenario {} ({})",
            scenario.id, scenario.description
        )
    }));
    order.join("; ")
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
    escalation: Option<&GauntletRoleModel>,
    model_call_limit: u32,
    canceled: &AtomicBool,
) -> Result<BlindCritique, String> {
    let prompt = format!(
        "You are a fresh read-only gameplay critic. Two anonymous candidates A and B were run from the same runtime snapshot for the same deterministic ticks, and the controller independently executed each candidate's durable Stasis tests. Judge behavioral improvement and regression relative to each other and the authoritative creative direction; the absolute scores still measure progress against the complete brief. Passing test names are behavioral evidence, not proof of the whole brief: weigh them with the runtime state, and do not infer behavior that neither source demonstrates. Prefer a or b when one has better behavioral evidence without an evidenced regression. Return equivalent whenever gameplay is materially unchanged, including when a visual-only improvement leaves gameplay intact or both remain equally incomplete. Never return neither merely because both fail the full brief. Return neither only when both have different material regressions or the evidence is invalid. Return only JSON.\n\nBrief:\n{goal}\n\nAuthoritative creative direction:\n{}\n\nRequired scenarios: {}\n\nA evidence:\n{}\n\nB evidence:\n{}",
        creative_direction_context(bar)?,
        serde_json::to_string(&bar.required_scenarios).map_err(|error| error.to_string())?,
        serde_json::to_string(state_a).map_err(|error| error.to_string())?,
        serde_json::to_string(state_b).map_err(|error| error.to_string())?,
    );
    let critique: BlindCritique = call_structured_role(
        "gameplay_critic",
        &prompt,
        &critic_schema(),
        model,
        escalation,
        &[],
        false,
        model_calls,
        model_call_limit,
        artifacts,
        canceled,
    )?;
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
    required_scenarios: &[GauntletScenarioRequirement],
    request_id: &mut u64,
) -> Result<ScenarioCapture, String> {
    request_live(client, *request_id, LiveCommand::ValidationRestore)?;
    *request_id = request_id.saturating_add(1);
    let initial_frame = capture_frame(
        client,
        project_root,
        artifacts,
        &format!("{id}-initial"),
        request_id,
    )?;
    if let Some((x, y)) = pointer {
        apply_pointer_tap(client, request_id, x, y, 29)?;
    } else {
        request_live(client, *request_id, LiveCommand::Step { ticks: 30 })?;
        *request_id = request_id.saturating_add(1);
        wait_for_steps(client, request_id)?;
    }
    let action_frame = capture_frame(client, project_root, artifacts, id, request_id)?;
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
    let mut required_frames = Vec::new();
    for scenario in required_scenarios
        .iter()
        .filter(|scenario| !scenario.taps.is_empty())
    {
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
        for tap in &scenario.taps {
            apply_pointer_tap(client, request_id, tap.x, tap.y, tap.ticks_after)?;
        }
        let frame = capture_frame(
            client,
            project_root,
            artifacts,
            &format!("{id}-scenario-{}", scenario.id),
            request_id,
        )?;
        required_frames.push(RequiredScenarioFrame {
            id: scenario.id.clone(),
            description: scenario.description.clone(),
            frame,
        });
    }
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
    Ok(ScenarioCapture {
        initial_frame,
        action_frame,
        required_frames,
        state: inspection.data.unwrap_or(Value::Null),
    })
}

fn apply_pointer_tap(
    client: &LiveSessionClient,
    request_id: &mut u64,
    x: i32,
    y: i32,
    ticks_after: u32,
) -> Result<(), String> {
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
    request_live(
        client,
        *request_id,
        LiveCommand::Step { ticks: ticks_after },
    )?;
    *request_id = request_id.saturating_add(1);
    wait_for_steps(client, request_id)
}

fn gameplay_evidence(runtime: Value, deterministic_tests: Value) -> Value {
    json!({
        "runtime": runtime,
        "deterministic_tests": deterministic_tests,
    })
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
            let parent = destination
                .parent()
                .ok_or_else(|| "Gauntlet capture destination has no parent".to_string())?;
            fs::create_dir_all(parent).map_err(|error| {
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
    let deadline = Instant::now() + MAX_LIVE_REQUEST_WAIT;
    while Instant::now() < deadline {
        if let Some(response) = client.try_receive()? {
            if response.request_id == request_id {
                if response.ok {
                    return Ok(response);
                }
                return Err(response
                    .error
                    .unwrap_or_else(|| "live request failed".to_string()));
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
    Err(format!(
        "live request {request_id} did not finish within {} seconds",
        MAX_LIVE_REQUEST_WAIT.as_secs()
    ))
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

fn verify_harness_readiness(
    project_root: &Path,
    artifacts: &Path,
    canceled: &AtomicBool,
) -> Result<(String, Value), String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("failed locating Stasis for harness readiness: {error}"))?;
    let log_path = artifacts.join("harness-readiness.log");
    let evidence = run_project_test_evidence(project_root, &log_path, canceled)?;
    Ok((
        format!(
            "{} ran the existing deterministic project tests successfully; log={}",
            executable.display(),
            log_path.display()
        ),
        evidence,
    ))
}

fn run_project_test_evidence(
    project_root: &Path,
    log_path: &Path,
    canceled: &AtomicBool,
) -> Result<Value, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("failed locating Stasis for deterministic tests: {error}"))?;
    let stdout = fs::File::create(&log_path)
        .map_err(|error| format!("failed creating {}: {error}", log_path.display()))?;
    let stderr = stdout
        .try_clone()
        .map_err(|error| format!("failed cloning harness readiness log: {error}"))?;
    let mut command = Command::new(&executable);
    command
        .arg("--workspace")
        .arg(project_root)
        .args(["--json", "test"])
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
        .map_err(|error| format!("failed starting harness readiness test: {error}"))?;
    let started = Instant::now();
    let status = loop {
        if canceled.load(Ordering::Acquire) || started.elapsed() >= Duration::from_secs(300) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "deterministic tests were canceled or exceeded 300 seconds; see {}",
                log_path.display()
            ));
        }
        match child
            .try_wait()
            .map_err(|error| format!("failed waiting for harness readiness test: {error}"))?
        {
            Some(status) => break status,
            None => thread::sleep(Duration::from_millis(100)),
        }
    };
    if !status.success() {
        return Err(format!(
            "deterministic tests exited with {status}; see {}",
            log_path.display()
        ));
    }
    let source = fs::read_to_string(log_path)
        .map_err(|error| format!("failed reading {}: {error}", log_path.display()))?;
    parse_test_evidence_log(&source).ok_or_else(|| {
        format!(
            "deterministic tests produced no valid JSON evidence; see {}",
            log_path.display()
        )
    })
}

fn parse_test_evidence_log(source: &str) -> Option<Value> {
    source.lines().rev().find_map(|line| {
        let envelope = serde_json::from_str::<Value>(line).ok()?;
        (envelope.get("ok").and_then(Value::as_bool) == Some(true))
            .then(|| envelope.get("result").cloned())
            .flatten()
    })
}

fn run_final_gates(
    project_root: &Path,
    artifacts: &Path,
    canceled: &AtomicBool,
) -> Result<(), String> {
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
        if should_stop(artifacts) || canceled.load(Ordering::Acquire) {
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
            if should_stop(artifacts)
                || canceled.load(Ordering::Acquire)
                || started.elapsed() >= Duration::from_secs(900)
            {
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

fn builder_agent_profile(
    config: &GauntletConfigV1,
    model: &GauntletRoleModel,
    role: &str,
    instruction: &str,
    available_calls: u32,
) -> AgentProfile {
    let mut profile = AgentProfile::default();
    profile.role = role.to_string();
    profile.instruction.push(' ');
    profile.instruction.push_str(instruction);
    profile.max_turns = usize::try_from(available_calls.min(config.execution.builder_max_turns))
        .unwrap_or(stasis_ai::DEFAULT_AGENT_TURNS);
    profile.model = model.model.clone();
    profile.reasoning_effort = model.reasoning_effort.clone();
    profile.request_timeout = Some(Duration::from_secs(u64::from(model.timeout_minutes) * 60));
    profile.compaction = config
        .execution
        .compaction
        .enabled
        .then(|| AgentCompactionPolicy {
            max_request_bytes: config.execution.compaction.max_request_bytes,
            retain_recent_turns: config.execution.compaction.retain_recent_turns,
        });
    profile
}

fn should_escalate_builder(failure: &live_tui::ScriptedAiFailure, canceled: &AtomicBool) -> bool {
    !canceled.load(Ordering::Acquire) && failure.message != "AI request canceled"
}

fn builder_turn_allowance(config: &GauntletConfigV1, used_calls: u32) -> u32 {
    config
        .budget
        .model_calls
        .saturating_sub(used_calls)
        .saturating_sub(2)
        .min(config.execution.builder_max_turns)
}

fn emit_builder_attempt_started(
    artifacts: &Path,
    candidate: &str,
    attempt: &str,
    model: &GauntletRoleModel,
    max_turns: u32,
) -> Result<(), String> {
    emit_event(
        artifacts,
        "role_attempt_started",
        json!({
            "role": "builder",
            "attempt": attempt,
            "candidate": candidate,
            "model": model.model,
            "reasoning_effort": model.reasoning_effort,
            "timeout_minutes": model.timeout_minutes,
            "max_turns": max_turns,
        }),
    )
}

fn record_builder_attempt_failure(
    state: &mut GauntletRunStateV1,
    artifacts: &Path,
    candidate: &str,
    attempt: &str,
    model: &GauntletRoleModel,
    failure: &live_tui::ScriptedAiFailure,
) -> Result<String, String> {
    let evidence = builder_failure_evidence(&failure.message, failure.trace.as_deref());
    state.model_calls = state.model_calls.saturating_add(failure.model_calls);
    persist_state(artifacts, state)?;
    if let Some(usage_trace) = failure.usage_trace.as_deref().filter(|path| path.is_file()) {
        append_usage_file(artifacts, usage_trace)?;
    }
    emit_event(
        artifacts,
        "builder_attempt_failed",
        json!({
            "candidate": candidate,
            "attempt": attempt,
            "model": model.model,
            "reasoning_effort": model.reasoning_effort,
            "reason": failure.message,
            "evidence": evidence,
            "trace": failure.trace,
            "usage": failure.usage_trace,
            "model_calls": failure.model_calls,
        }),
    )?;
    append_decision(
        artifacts,
        "controller",
        "builder_attempt_failed",
        &format!("Builder {attempt} attempt failed for {candidate}"),
        "Later attempts need the actual repeated tool or completion failure, not only the terminal turn-limit message.",
        &evidence,
        "Avoid repeating the evidenced failure; use a different bounded correction or report the blocker.",
    )?;
    Ok(evidence)
}

fn builder_failure_evidence(message: &str, trace: Option<&Path>) -> String {
    let mut errors = BTreeMap::<String, (u32, usize)>::new();
    let mut ordinal = 0_usize;
    if let Some(trace) = trace {
        if let Ok(source) = read_bounded_utf8(trace, MAX_FAILURE_TRACE_BYTES, "AI trace") {
            for line in source.lines() {
                let Ok(record) = serde_json::from_str::<Value>(line) else {
                    continue;
                };
                if record.get("event").and_then(Value::as_str) != Some("tool_observations") {
                    continue;
                }
                let Some(observations) = record.get("observations").and_then(Value::as_array)
                else {
                    continue;
                };
                for observation in observations {
                    let Some(error) = observation.get("error").and_then(Value::as_str) else {
                        continue;
                    };
                    let tool = observation
                        .get("tool")
                        .and_then(Value::as_str)
                        .unwrap_or("tool");
                    ordinal = ordinal.saturating_add(1);
                    let detail = format!("{tool}: {error}");
                    let entry = errors.entry(detail).or_insert((0, ordinal));
                    entry.0 = entry.0.saturating_add(1);
                    entry.1 = ordinal;
                }
            }
        }
    }
    let mut errors = errors.into_iter().collect::<Vec<_>>();
    errors.sort_by_key(|(_, (_, last_seen))| *last_seen);
    let start = errors.len().saturating_sub(MAX_FAILURE_ERROR_KINDS);
    let details = errors[start..]
        .iter()
        .map(|(detail, (count, _))| {
            if *count > 1 {
                format!("{detail} (repeated {count} times)")
            } else {
                detail.clone()
            }
        })
        .collect::<Vec<_>>()
        .join("; ");
    let evidence = if details.is_empty() {
        message.to_string()
    } else {
        format!("{message}; observed errors: {details}")
    };
    bounded_decision_text(&evidence)
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
        json!({
            "candidate": candidate,
            "reason": reason,
            "consecutive_stalls": state.consecutive_stalls,
        }),
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

pub(super) fn sync_vendor_checkpoint(workspace: &Workspace) -> Result<(), String> {
    let mut manifest = workspace.manifest.clone();
    if !super::super::update_vendor_snapshot(&workspace.root, &mut manifest)? {
        return Ok(());
    }
    git_ok(&workspace.root, &["add", "stasis.json", "vendor/stasis"])?;
    git_ok(
        &workspace.root,
        &[
            "-c",
            "user.name=Stasis Gauntlet",
            "-c",
            "user.email=gauntlet@stasis.local",
            "commit",
            "--no-verify",
            "-m",
            "chore: sync Stasis vendor",
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

fn import_prior_run_lessons(
    original_root: &Path,
    artifacts: &Path,
    current_run_id: &str,
) -> Result<usize, String> {
    let runs = original_root.join(RUNS_PATH);
    if !runs.is_dir() {
        return Ok(0);
    }
    let existing = read_decision_records(artifacts)?;
    let mut imported = existing
        .iter()
        .filter(|record| {
            matches!(
                record.get("kind").and_then(Value::as_str),
                Some("prior_run_lesson" | "prior_run_context")
            )
        })
        .filter_map(|record| {
            Some((
                record.get("source_run_id")?.as_str()?.to_string(),
                record.get("source_kind")?.as_str()?.to_string(),
                record.get("source_unix_ms")?.as_u64()?,
            ))
        })
        .collect::<BTreeSet<_>>();
    let mut run_dirs = fs::read_dir(&runs)
        .map_err(|error| format!("failed reading prior Gauntlet runs: {error}"))?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name != current_run_id && name != "worktrees"
        })
        .collect::<Vec<_>>();
    run_dirs.sort_by_key(|entry| std::cmp::Reverse(run_sort_key(entry)));

    let mut lessons = Vec::new();
    for entry in run_dirs.into_iter().take(MAX_PRIOR_RUNS) {
        collect_prior_run_lessons(original_root, &entry.path(), &mut lessons)?;
    }
    lessons.sort_by_key(|lesson| std::cmp::Reverse(lesson.unix_ms));
    lessons.truncate(MAX_PRIOR_RUN_LESSONS);
    lessons.reverse();

    let mut appended = 0_usize;
    for lesson in lessons {
        let identity = (
            lesson.source_run_id.clone(),
            lesson.source_kind.clone(),
            lesson.unix_ms,
        );
        if !imported.insert(identity) {
            continue;
        }
        let imported_kind = if matches!(
            lesson.source_kind.as_str(),
            "candidate_accepted" | "pending_work_item"
        ) {
            "prior_run_context"
        } else {
            "prior_run_lesson"
        };
        let record = json!({
            "schema_version": 1,
            "unix_ms": unix_ms(),
            "source_unix_ms": lesson.unix_ms,
            "source_run_id": bounded_decision_text(&lesson.source_run_id),
            "source_kind": bounded_decision_text(&lesson.source_kind),
            "audience": "lead_builder",
            "role": "controller",
            "kind": imported_kind,
            "summary": bounded_decision_text(&lesson.summary),
            "rationale": bounded_decision_text(&lesson.rationale),
            "evidence": bounded_decision_text(&lesson.evidence),
            "next_step": bounded_decision_text(&lesson.next_step),
        });
        append_decision_record(artifacts, &record)?;
        appended = appended.saturating_add(1);
    }
    Ok(appended)
}

fn run_sort_key(entry: &fs::DirEntry) -> u64 {
    entry
        .file_name()
        .to_string_lossy()
        .split('-')
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

fn collect_prior_run_lessons(
    original_root: &Path,
    run_dir: &Path,
    lessons: &mut Vec<PriorRunLesson>,
) -> Result<(), String> {
    let Some(source_run_id) = run_dir.file_name().and_then(|name| name.to_str()) else {
        return Ok(());
    };
    let records = read_jsonl_records(&run_dir.join(DECISIONS_NAME), "prior decision memory")?;
    let mut has_builder_failure = false;
    let mut latest_work_item = None;
    let mut latest_work_outcome_unix_ms = 0_u64;
    for record in &records {
        let Some(kind) = record.get("kind").and_then(Value::as_str) else {
            continue;
        };
        let record_unix_ms = record.get("unix_ms").and_then(Value::as_u64).unwrap_or(0);
        if kind == "work_item_selected" {
            latest_work_item = Some((record_unix_ms, record));
        }
        if matches!(
            kind,
            "builder_completed"
                | "builder_attempt_failed"
                | "candidate_accepted"
                | "candidate_rejected"
        ) {
            latest_work_outcome_unix_ms = latest_work_outcome_unix_ms.max(record_unix_ms);
        }
        if kind == "candidate_accepted" {
            lessons.push(PriorRunLesson {
                unix_ms: record_unix_ms,
                source_run_id: source_run_id.to_string(),
                source_kind: kind.to_string(),
                summary: json_string(record, "summary", "A prior candidate was accepted"),
                rationale: json_string(
                    record,
                    "rationale",
                    "Accepted checkpoint behavior exists in the current workspace.",
                ),
                evidence: json_string(record, "evidence", "No acceptance evidence was recorded."),
                next_step: json_string(
                    record,
                    "next_step",
                    "Preserve the accepted checkpoint while selecting the next gap.",
                ),
            });
        }
        if kind == "builder_attempt_failed" {
            has_builder_failure = true;
        }
        if !matches!(
            kind,
            "builder_attempt_failed"
                | "atomic_write_failed"
                | "completion_gate_failed"
                | "candidate_rejected"
                | "largest_gap"
                | "final_validation_failed"
        ) {
            continue;
        }
        lessons.push(PriorRunLesson {
            unix_ms: record.get("unix_ms").and_then(Value::as_u64).unwrap_or(0),
            source_run_id: source_run_id.to_string(),
            source_kind: kind.to_string(),
            summary: json_string(&record, "summary", kind),
            rationale: json_string(
                &record,
                "rationale",
                "Prior run evidence remains relevant until corrected.",
            ),
            evidence: json_string(&record, "evidence", "No detailed evidence was recorded."),
            next_step: json_string(
                &record,
                "next_step",
                "Address this prior failure before repeating the same approach.",
            ),
        });
    }
    if let Some((selected_unix_ms, record)) = latest_work_item {
        if selected_unix_ms > latest_work_outcome_unix_ms {
            lessons.push(PriorRunLesson {
                unix_ms: selected_unix_ms,
                source_run_id: source_run_id.to_string(),
                source_kind: "pending_work_item".to_string(),
                summary: json_string(record, "summary", "A prior run selected pending work"),
                rationale: json_string(
                    record,
                    "rationale",
                    "The prior run selected this work but ended before a builder outcome.",
                ),
                evidence: json_string(
                    record,
                    "evidence",
                    "No later builder outcome was recorded for this selection.",
                ),
                next_step: json_string(
                    record,
                    "next_step",
                    "Re-evaluate this pending work against the current workspace.",
                ),
            });
        }
    }
    if has_builder_failure {
        return Ok(());
    }

    for event in read_jsonl_records(&run_dir.join(EVENTS_NAME), "prior event stream")? {
        if event.get("kind").and_then(Value::as_str) != Some("builder_attempt_failed") {
            continue;
        }
        let data = event.get("data").unwrap_or(&Value::Null);
        let reason = json_string(data, "reason", "Builder attempt failed");
        let trace = data
            .get("trace")
            .and_then(Value::as_str)
            .and_then(|path| safe_prior_trace(original_root, Path::new(path)));
        lessons.push(PriorRunLesson {
            unix_ms: event.get("unix_ms").and_then(Value::as_u64).unwrap_or(0),
            source_run_id: source_run_id.to_string(),
            source_kind: "builder_attempt_failed".to_string(),
            summary: format!(
                "Builder {} attempt failed in prior run {source_run_id}",
                json_string(data, "attempt", "unknown")
            ),
            rationale: "The next builder must receive the actual tool failure so it can change course instead of exhausting another allowance.".to_string(),
            evidence: builder_failure_evidence(&reason, trace.as_deref()),
            next_step: "Avoid repeating the evidenced failure; use a different bounded correction or report the blocker.".to_string(),
        });
    }
    Ok(())
}

fn json_string(record: &Value, field: &str, fallback: &str) -> String {
    record
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or(fallback)
        .to_string()
}

fn safe_prior_trace(original_root: &Path, trace: &Path) -> Option<PathBuf> {
    let trace = if trace.is_absolute() {
        trace.to_path_buf()
    } else {
        original_root.join(trace)
    };
    let allowed = original_root.join("build/ai-traces").canonicalize().ok()?;
    let trace = trace.canonicalize().ok()?;
    trace.starts_with(allowed).then_some(trace)
}

fn read_jsonl_records(path: &Path, label: &str) -> Result<Vec<Value>, String> {
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let source = read_bounded_utf8(path, 8 * 1024 * 1024, label)?;
    let lines = source
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    let mut records = Vec::with_capacity(lines.len());
    for (index, line) in lines.iter().enumerate() {
        match serde_json::from_str::<Value>(line) {
            Ok(record) => records.push(record),
            Err(_) if index + 1 == lines.len() => break,
            Err(error) => return Err(format!("invalid {label} record: {error}")),
        }
    }
    Ok(records)
}

fn append_decision_record(artifacts: &Path, record: &Value) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(artifacts.join(DECISIONS_NAME))
        .map_err(|error| format!("failed opening Gauntlet decision memory: {error}"))?;
    writeln!(
        file,
        "{}",
        serde_json::to_string(record).map_err(|error| error.to_string())?
    )
    .map_err(|error| format!("failed writing Gauntlet decision memory: {error}"))?;
    file.flush()
        .map_err(|error| format!("failed flushing Gauntlet decision memory: {error}"))?;
    file.sync_data()
        .map_err(|error| format!("failed syncing Gauntlet decision memory: {error}"))
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

fn restore_largest_evidenced_gap(artifacts: &Path) -> Result<Option<String>, String> {
    for record in read_decision_records(artifacts)?.into_iter().rev() {
        let kind = record.get("kind").and_then(Value::as_str);
        if !matches!(kind, Some("candidate_accepted" | "largest_gap")) {
            continue;
        }
        let Some(gap) = record.get("next_step").and_then(Value::as_str) else {
            continue;
        };
        if !gap.trim().is_empty() {
            return Ok(Some(gap.to_string()));
        }
    }
    Ok(None)
}

fn read_decision_records(artifacts: &Path) -> Result<Vec<Value>, String> {
    read_jsonl_records(&artifacts.join(DECISIONS_NAME), "Gauntlet decision memory")
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
    let creative_direction = fs::read_to_string(artifacts.join(CREATIVE_DIRECTION_NAME))
        .unwrap_or_else(|_| "Creative direction is unavailable.".to_string());
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
        "<!doctype html><meta charset=\"utf-8\"><title>Stasis Gauntlet {}</title><style>body{{font:16px system-ui;max-width:1100px;margin:40px auto;padding:0 20px;background:#0b1020;color:#e8eefc}}pre{{white-space:pre-wrap;background:#151d33;padding:16px;border-radius:10px}}figure{{display:inline-block;width:46%;vertical-align:top}}img{{max-width:100%;border-radius:10px}}</style><h1>Stasis Gauntlet {}</h1><p>Phase: {:?} &middot; accepted {} &middot; rejected {} &middot; model calls {}</p><h2>Captures</h2>{}<h2>Authoritative creative direction</h2><pre>{}</pre><h2>Frozen quality bar</h2><pre>{}</pre><h2>Decision memory</h2><pre>{}</pre><h2>Event stream</h2><pre>{}</pre>",
        escape_html(&state.run_id),
        escape_html(&state.run_id),
        state.phase,
        state.accepted_candidates,
        state.rejected_candidates,
        state.model_calls,
        captures,
        escape_html(&creative_direction),
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
    format!("Gauntlet {}: {:?}\nbranch: {}\nbest: {}\naccepted: {}, rejected: {}, model calls: {}\nworkspace: {}\nreport: {}", state.run_id, state.phase, state.branch, state.best_commit.as_deref().unwrap_or("none"), state.accepted_candidates, state.rejected_candidates, state.model_calls, state.project_root, artifacts.join(REPORT_NAME).display())
}

fn scout_schema() -> Value {
    json!({"type":"object","required":["summary","sources"],"properties":{"summary":{"type":"string"},"sources":{"type":"array","maxItems":5,"items":{"type":"object","required":["url","relevance"],"properties":{"url":{"type":"string"},"relevance":{"type":"string"}},"additionalProperties":false}}},"additionalProperties":false})
}

fn bootstrap_schema() -> Value {
    let bounded_rules = json!({
        "type": "array",
        "minItems": 2,
        "maxItems": 8,
        "items": {"type": "string", "minLength": 1, "maxLength": 1000}
    });
    json!({
        "type": "object",
        "required": ["creative_direction", "workstreams"],
        "properties": {
            "creative_direction": {
                "type": "object",
                "required": ["title", "narrative_promise", "player_fantasy", "rule_pillars", "visual_language", "interaction_grammar", "progression_and_pacing", "non_negotiables"],
                "properties": {
                    "title": {"type": "string", "minLength": 1, "maxLength": 200},
                    "narrative_promise": {"type": "string", "minLength": 1, "maxLength": 2000},
                    "player_fantasy": {"type": "string", "minLength": 1, "maxLength": 2000},
                    "rule_pillars": bounded_rules.clone(),
                    "visual_language": bounded_rules.clone(),
                    "interaction_grammar": bounded_rules.clone(),
                    "progression_and_pacing": bounded_rules.clone(),
                    "non_negotiables": bounded_rules
                },
                "additionalProperties": false
            },
            "workstreams": {"type":"array","minItems":4,"maxItems":10,"items":{"type":"string", "minLength": 1, "maxLength": 200}}
        },
        "additionalProperties": false
    })
}

fn lead_schema() -> Value {
    json!({"type":"object","required":["done","workstream","builder_prompt","playability_guidance","rationale","next_step"],"properties":{"done":{"type":"boolean"},"workstream":{"type":"string","maxLength":2000},"builder_prompt":{"type":"string","maxLength":2000},"playability_guidance":{"type":"string","maxLength":2000},"rationale":{"type":"string","maxLength":2000},"next_step":{"type":"string","maxLength":2000}},"additionalProperties":false})
}

fn critic_schema() -> Value {
    json!({"type":"object","required":["preferred","score_a","score_b","largest_gap","summary"],"properties":{"preferred":{"type":"string","enum":["a","b","neither","equivalent"]},"score_a":{"type":"integer","minimum":0,"maximum":100},"score_b":{"type":"integer","minimum":0,"maximum":100},"largest_gap":{"type":"string"},"summary":{"type":"string"}},"additionalProperties":false})
}

fn visual_critic_schema() -> Value {
    let screen = json!({
        "type": "object",
        "required": [
            "current_state_clear",
            "available_actions_clear",
            "board_semantics_clear",
            "action_feedback_clear",
            "evidence"
        ],
        "properties": {
            "current_state_clear": {"type": "boolean"},
            "available_actions_clear": {"type": "boolean"},
            "board_semantics_clear": {"type": "boolean"},
            "action_feedback_clear": {"type": "boolean"},
            "evidence": {"type": "string", "maxLength": 4000}
        },
        "additionalProperties": false
    });
    json!({
        "type": "object",
        "required": ["preferred", "score_a", "score_b", "screen_a", "screen_b", "largest_gap", "summary"],
        "properties": {
            "preferred": {"type": "string", "enum": ["a", "b", "neither", "equivalent"]},
            "score_a": {"type": "integer", "minimum": 0, "maximum": 100},
            "score_b": {"type": "integer", "minimum": 0, "maximum": 100},
            "screen_a": screen.clone(),
            "screen_b": screen,
            "largest_gap": {"type": "string", "maxLength": 4000},
            "summary": {"type": "string", "maxLength": 4000}
        },
        "additionalProperties": false
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authored_visual_workstreams_require_imagegen() {
        assert!(requires_authored_imagegen(
            "Medieval world art and animation"
        ));
        assert!(requires_authored_imagegen("Rendered-frame visual polish"));
        assert!(!requires_authored_imagegen("Grid movement and terrain"));
        assert!(!requires_authored_imagegen("Mobile controls and HUD"));
    }

    #[test]
    fn scenario_images_and_labels_keep_the_same_deterministic_order() {
        let capture = ScenarioCapture {
            initial_frame: PathBuf::from("initial.png"),
            action_frame: PathBuf::from("fixed.png"),
            required_frames: vec![
                RequiredScenarioFrame {
                    id: "select".to_string(),
                    description: "Select a ready unit".to_string(),
                    frame: PathBuf::from("select.png"),
                },
                RequiredScenarioFrame {
                    id: "move".to_string(),
                    description: "Move to a legal cell".to_string(),
                    frame: PathBuf::from("move.png"),
                },
            ],
            state: Value::Null,
        };

        assert_eq!(
            capture_images(&capture),
            vec![
                PathBuf::from("initial.png"),
                PathBuf::from("fixed.png"),
                PathBuf::from("select.png"),
                PathBuf::from("move.png"),
            ]
        );
        assert_eq!(
            capture_image_order("A", &capture),
            "A initial state; A after fixed interaction probe; A scenario select (Select a ready unit); A scenario move (Move to a legal cell)"
        );
    }

    #[test]
    fn heartbeat_distinguishes_live_and_interrupted_runs() {
        let root = std::env::temp_dir().join(format!(
            "stasis-gauntlet-heartbeat-{}-{}",
            std::process::id(),
            unix_ms()
        ));
        fs::create_dir_all(&root).expect("heartbeat temp directory");
        let path = root.join(HEARTBEAT_NAME);
        write_heartbeat(&path).expect("fresh heartbeat");
        assert!(heartbeat_is_fresh(&path));
        fs::write(&path, [0_u8; 55]).expect("torn heartbeat");
        assert_eq!(heartbeat_unix_ms(&path), None);
        assert!(!heartbeat_is_fresh(&path));
        fs::write(&path, br#"{"schema_version":1,"pid":1,"unix_ms":0}"#).expect("stale heartbeat");
        assert!(!heartbeat_is_fresh(&path));
        fs::remove_file(&path).expect("remove heartbeat");
        fs::remove_dir(&root).expect("remove heartbeat directory");
    }

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
        assert!(bootstrap_schema()["required"]
            .as_array()
            .expect("bootstrap required fields")
            .contains(&json!("creative_direction")));
        assert!(visual_critic_schema()["required"]
            .as_array()
            .expect("visual critic required fields")
            .contains(&json!("screen_a")));
        assert!(lead_schema()["required"]
            .as_array()
            .expect("lead required fields")
            .contains(&json!("playability_guidance")));
        let fallback = fallback_lead_decision(
            &FrozenBar {
                schema_version: 1,
                goal_sha256: "0".repeat(64),
                goal: "game".to_string(),
                direction_source_markdown: String::new(),
                direction_source_sha256: String::new(),
                creative_direction: CreativeDirection::default(),
                workstreams: vec!["HUD".to_string()],
                hard_gates: Vec::new(),
                required_scenarios: Vec::new(),
                references: Vec::new(),
                web_sources: Vec::new(),
                acceptance_score: 65,
            },
            "The grid is difficult to parse.",
        );
        assert!(fallback.playability_guidance.contains("cell boundaries"));
        assert!(fallback.playability_guidance.contains("cancel"));
    }

    #[test]
    fn screen_comprehension_is_an_absolute_quality_gate() {
        let mut screen = ScreenComprehension {
            current_state_clear: true,
            available_actions_clear: true,
            board_semantics_clear: true,
            action_feedback_clear: false,
            evidence: "The input result is not visible.".to_string(),
        };
        assert!(!candidate_meets_quality_bar(true, 90, 90, 65, &screen));
        screen.action_feedback_clear = true;
        assert!(candidate_meets_quality_bar(true, 90, 90, 65, &screen));
        assert!(!candidate_meets_quality_bar(false, 90, 90, 65, &screen));
    }

    #[test]
    fn creative_direction_is_durable_and_old_bars_get_a_safe_default() {
        let root = std::env::temp_dir().join(format!(
            "stasis_gauntlet_direction_{}_{}",
            std::process::id(),
            unix_ms()
        ));
        fs::create_dir_all(&root).expect("direction root");
        let bar = FrozenBar {
            schema_version: 3,
            goal_sha256: "goal".to_string(),
            goal: "game".to_string(),
            direction_source_markdown: "# Authored direction\n\nKeep the board readable."
                .to_string(),
            direction_source_sha256: "source".to_string(),
            creative_direction: CreativeDirection::default(),
            workstreams: Vec::new(),
            hard_gates: Vec::new(),
            required_scenarios: Vec::new(),
            references: Vec::new(),
            web_sources: Vec::new(),
            acceptance_score: 65,
        };
        write_creative_direction(&root, &bar).expect("write direction");
        let markdown =
            fs::read_to_string(root.join(CREATIVE_DIRECTION_NAME)).expect("read direction");
        assert!(markdown.contains("## Project-authored authority (verbatim)"));
        assert!(markdown.contains("Keep the board readable."));
        assert!(markdown.contains("### Narrative promise"));
        assert!(markdown.contains("## Digest: interaction grammar"));

        let old_bar = r#"{"schema_version":1,"goal_sha256":"old","goal":"game","workstreams":[],"hard_gates":[],"required_scenarios":[],"references":[],"web_sources":[],"acceptance_score":65}"#;
        let restored: FrozenBar = serde_json::from_str(old_bar).expect("old bar restores");
        assert!(!restored.creative_direction.interaction_grammar.is_empty());
        fs::remove_file(root.join(CREATIVE_DIRECTION_NAME)).expect("remove direction");
        fs::remove_dir(root).expect("remove direction root");
    }

    #[test]
    fn identical_goal_reuses_the_latest_versioned_creative_direction() {
        let root = std::env::temp_dir().join(format!(
            "stasis_gauntlet_prior_direction_{}_{}",
            std::process::id(),
            unix_ms()
        ));
        let prior = root.join("100-prior");
        let current = root.join("200-current");
        fs::create_dir_all(&prior).expect("prior run");
        fs::create_dir_all(&current).expect("current run");
        let goal = "same immutable goal";
        let bar = FrozenBar {
            schema_version: 3,
            goal_sha256: hex_sha256(goal.as_bytes()),
            goal: goal.to_string(),
            direction_source_markdown: "# Stable source".to_string(),
            direction_source_sha256: "same-source".to_string(),
            creative_direction: CreativeDirection {
                title: "Remembered direction".to_string(),
                ..CreativeDirection::default()
            },
            workstreams: vec!["Readable turns".to_string()],
            hard_gates: Vec::new(),
            required_scenarios: Vec::new(),
            references: Vec::new(),
            web_sources: Vec::new(),
            acceptance_score: 65,
        };
        write_json(&prior.join(QUALITY_BAR_NAME), &bar).expect("prior bar");
        let (source, direction, workstreams) =
            restore_prior_creative_direction(&current, goal, "same-source")
                .expect("restore direction")
                .expect("matching direction");
        assert_eq!(source, "100-prior");
        assert_eq!(direction.title, "Remembered direction");
        assert_eq!(workstreams, vec!["Readable turns"]);
        assert!(
            restore_prior_creative_direction(&current, "different goal", "same-source")
                .expect("different goal lookup")
                .is_none()
        );
        assert!(
            restore_prior_creative_direction(&current, goal, "changed-source")
                .expect("changed source lookup")
                .is_none()
        );
        fs::remove_dir_all(root).expect("remove direction runs");
    }

    #[test]
    fn visual_first_and_gameplay_first_candidates_can_checkpoint() {
        let gameplay = |preferred: &str, score_a: u32, score_b: u32| BlindCritique {
            preferred: preferred.to_string(),
            score_a,
            score_b,
            largest_gap: "next gap".to_string(),
            summary: "bounded comparison".to_string(),
        };
        let visual = |preferred: &str, score_a: u32, score_b: u32| VisualCritique {
            preferred: preferred.to_string(),
            score_a,
            score_b,
            screen_a: ScreenComprehension {
                current_state_clear: false,
                available_actions_clear: false,
                board_semantics_clear: false,
                action_feedback_clear: false,
                evidence: "screen A evidence".to_string(),
            },
            screen_b: ScreenComprehension {
                current_state_clear: false,
                available_actions_clear: false,
                board_semantics_clear: false,
                action_feedback_clear: false,
                evidence: "screen B evidence".to_string(),
            },
            largest_gap: "next gap".to_string(),
            summary: "bounded comparison".to_string(),
        };

        let visual_improvement = visual("a", 20, 5);
        let unchanged_gameplay = gameplay("equivalent", 1, 1);
        assert!(critics_allow_checkpoint(
            &visual_improvement,
            &unchanged_gameplay,
            true
        ));

        let unchanged_visuals = visual("equivalent", 5, 5);
        let gameplay_improvement = gameplay("b", 2, 25);
        assert!(critics_allow_checkpoint(
            &unchanged_visuals,
            &gameplay_improvement,
            false
        ));

        let insufficient_gameplay_evidence = gameplay("neither", 1, 1);
        assert!(!critics_allow_checkpoint(
            &visual_improvement,
            &insufficient_gameplay_evidence,
            true
        ));

        let gameplay_regression = gameplay("b", 1, 30);
        assert!(!critics_allow_checkpoint(
            &visual_improvement,
            &gameplay_regression,
            true
        ));
    }

    #[test]
    fn html_report_content_is_escaped() {
        assert_eq!(escape_html("<script>&\""), "&lt;script&gt;&amp;&quot;");
    }

    #[test]
    fn gameplay_evidence_includes_controller_verified_test_names() {
        let source = concat!(
            "diagnostic before JSON\n",
            "{\"command\":\"test\",\"ok\":true,\"result\":{\"tests_passed\":2,",
            "\"passed_tests\":[\"tests/main.test.stasis :: enemy turn completes\"]}}\n",
        );
        let tests = parse_test_evidence_log(source).expect("test evidence");
        let evidence = gameplay_evidence(json!({"game": {"round": 2}}), tests);

        assert_eq!(evidence["runtime"]["game"]["round"], 2);
        assert_eq!(evidence["deterministic_tests"]["tests_passed"], 2);
        assert_eq!(
            evidence["deterministic_tests"]["passed_tests"][0],
            "tests/main.test.stasis :: enemy turn completes"
        );
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
        append_decision(
            &root,
            "controller",
            "candidate_accepted",
            "accepted candidate",
            "critic evidence persists",
            "scores",
            "No behavioral gameplay evidence is exposed.",
        )
        .expect("accepted gap");
        append_decision(
            &root,
            "controller",
            "harness_ready",
            "harness ready",
            "readiness is not a product gap",
            "tests passed",
            "Select the next work item.",
        )
        .expect("readiness decision");
        assert_eq!(
            restore_largest_evidenced_gap(&root).expect("restored gap"),
            Some("No behavioral gameplay evidence is exposed.".to_string())
        );
        OpenOptions::new()
            .append(true)
            .open(root.join(DECISIONS_NAME))
            .expect("decision log")
            .write_all(b"{\"torn\":")
            .expect("torn final record");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn builder_failure_evidence_summarizes_repeated_tool_errors() {
        let root = std::env::temp_dir().join(format!(
            "stasis_gauntlet_failure_evidence_{}_{}",
            std::process::id(),
            unix_ms()
        ));
        fs::create_dir_all(&root).expect("trace root");
        let trace = root.join("trace.jsonl");
        let mut source = String::new();
        for _ in 0..15 {
            source.push_str(
                &serde_json::to_string(&json!({
                    "event": "tool_observations",
                    "observations": [{
                        "tool": "completion_gate",
                        "error": "live response transaction data was absent"
                    }]
                }))
                .expect("trace record"),
            );
            source.push('\n');
        }
        fs::write(&trace, source).expect("trace");

        let evidence = builder_failure_evidence("AI agent reached the turn limit", Some(&trace));

        assert!(evidence.contains(
            "completion_gate: live response transaction data was absent (repeated 15 times)"
        ));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn prior_run_lessons_import_legacy_failure_detail_once() {
        let root = std::env::temp_dir().join(format!(
            "stasis_gauntlet_prior_lessons_{}_{}",
            std::process::id(),
            unix_ms()
        ));
        let old_run = root.join(RUNS_PATH).join("100-old");
        let current_run = root.join(RUNS_PATH).join("200-current");
        let traces = root.join("build/ai-traces");
        fs::create_dir_all(&old_run).expect("old run");
        fs::create_dir_all(&current_run).expect("current run");
        fs::create_dir_all(&traces).expect("trace root");
        let trace = traces.join("builder.jsonl");
        let trace_record = serde_json::to_string(&json!({
            "event": "tool_observations",
            "observations": [{
                "tool": "completion_gate",
                "error": "live response transaction data was absent"
            }]
        }))
        .expect("trace record");
        fs::write(&trace, format!("{trace_record}\n{trace_record}\n")).expect("failure trace");
        let event = json!({
            "schema_version": 1,
            "unix_ms": 101,
            "kind": "builder_attempt_failed",
            "data": {
                "attempt": "primary",
                "reason": "AI agent reached the turn limit",
                "trace": trace
            }
        });
        fs::write(
            old_run.join(EVENTS_NAME),
            format!("{}\n", serde_json::to_string(&event).expect("event")),
        )
        .expect("events");

        assert_eq!(
            import_prior_run_lessons(&root, &current_run, "200-current").expect("first import"),
            1
        );
        assert_eq!(
            import_prior_run_lessons(&root, &current_run, "200-current")
                .expect("idempotent import"),
            0
        );
        let memory = decision_memory_snapshot(&current_run).expect("memory");
        assert!(memory.contains("100-old"));
        assert!(memory.contains(
            "completion_gate: live response transaction data was absent (repeated 2 times)"
        ));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn prior_run_lessons_import_accepted_checkpoint_and_only_pending_work() {
        let root = std::env::temp_dir().join(format!(
            "stasis_gauntlet_prior_progress_{}_{}",
            std::process::id(),
            unix_ms()
        ));
        let completed_run = root.join(RUNS_PATH).join("100-completed");
        let pending_run = root.join(RUNS_PATH).join("200-pending");
        let current_run = root.join(RUNS_PATH).join("300-current");
        fs::create_dir_all(&completed_run).expect("completed run");
        fs::create_dir_all(&pending_run).expect("pending run");
        fs::create_dir_all(&current_run).expect("current run");

        let completed_records = [
            json!({
                "schema_version": 1,
                "unix_ms": 101,
                "audience": "lead_builder",
                "role": "lead",
                "kind": "work_item_selected",
                "summary": "Selected movement",
                "rationale": "Movement was the largest gap.",
                "evidence": "No accepted movement checkpoint.",
                "next_step": "Implement movement."
            }),
            json!({
                "schema_version": 1,
                "unix_ms": 102,
                "audience": "lead_builder",
                "role": "controller",
                "kind": "candidate_accepted",
                "summary": "Accepted movement checkpoint",
                "rationale": "Both critics allowed the checkpoint.",
                "evidence": "commit=abc; gameplay_score=70",
                "next_step": "Improve terrain art."
            }),
        ];
        fs::write(
            completed_run.join(DECISIONS_NAME),
            completed_records
                .iter()
                .map(|record| serde_json::to_string(record).expect("record"))
                .collect::<Vec<_>>()
                .join("\n")
                + "\n",
        )
        .expect("completed decisions");

        let pending_records = [
            json!({
                "schema_version": 1,
                "unix_ms": 201,
                "audience": "lead_builder",
                "role": "controller",
                "kind": "candidate_accepted",
                "summary": "Accepted state loop checkpoint",
                "rationale": "The state loop passed independent evaluation.",
                "evidence": "commit=def; gameplay_score=72",
                "next_step": "Improve the sparse battlefield."
            }),
            json!({
                "schema_version": 1,
                "unix_ms": 202,
                "audience": "lead_builder",
                "role": "lead",
                "kind": "work_item_selected",
                "summary": "Selected medieval art",
                "rationale": "Visual quality is now the largest gap.",
                "evidence": "The run had no remaining builder budget.",
                "next_step": "Build the medieval diorama."
            }),
        ];
        fs::write(
            pending_run.join(DECISIONS_NAME),
            pending_records
                .iter()
                .map(|record| serde_json::to_string(record).expect("record"))
                .collect::<Vec<_>>()
                .join("\n")
                + "\n",
        )
        .expect("pending decisions");

        assert_eq!(
            import_prior_run_lessons(&root, &current_run, "300-current").expect("progress import"),
            3
        );
        let memory = decision_memory_snapshot(&current_run).expect("memory");
        assert!(memory.contains("Accepted movement checkpoint"));
        assert!(memory.contains("Accepted state loop checkpoint"));
        assert!(memory.contains("Selected medieval art"));
        assert!(memory.contains("pending_work_item"));
        assert!(memory.contains("prior_run_context"));
        assert!(!memory.contains("Selected movement"));

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
    fn resume_reopens_the_stall_budget() {
        let mut state = GauntletRunStateV1 {
            schema_version: 1,
            run_id: "123-deadbeef".to_string(),
            phase: GauntletRunPhase::Stalled,
            project_root: "worktree".to_string(),
            original_root: "project".to_string(),
            branch: "stasis/gauntlet/123-deadbeef".to_string(),
            base_commit: "deadbeef".to_string(),
            best_commit: Some("cafebabe".to_string()),
            current_workstream: Some("battle rules".to_string()),
            model_calls: 7,
            accepted_candidates: 0,
            rejected_candidates: 5,
            consecutive_stalls: 5,
            quality_acceptance_streak: 1,
            started_unix_ms: 1,
            session_started_unix_ms: 1,
            updated_unix_ms: 2,
            terminal_reason: Some("consecutive candidate limit reached".to_string()),
        };

        prepare_state_for_resume(&mut state, 99);

        assert_eq!(state.phase, GauntletRunPhase::Building);
        assert_eq!(state.consecutive_stalls, 0);
        assert_eq!(state.terminal_reason, None);
        assert_eq!(state.rejected_candidates, 5);
        assert_eq!(state.quality_acceptance_streak, 1);
        assert_eq!(state.best_commit.as_deref(), Some("cafebabe"));
        assert_eq!(state.started_unix_ms, 1);
        assert_eq!(state.session_started_unix_ms, 99);
        assert!(phase_is_resumable(&GauntletRunPhase::Failed));
        assert!(phase_is_resumable(&GauntletRunPhase::BudgetExhausted));
        assert!(!phase_is_resumable(&GauntletRunPhase::Converged));

        let mut legacy = serde_json::to_value(&state).expect("state JSON");
        legacy
            .as_object_mut()
            .expect("state object")
            .remove("session_started_unix_ms");
        let restored: GauntletRunStateV1 =
            serde_json::from_value(legacy).expect("legacy state remains readable");
        assert_eq!(restored.session_started_unix_ms, 0);
    }

    #[test]
    fn builder_escalation_skips_canceled_attempts() {
        let failure = live_tui::ScriptedAiFailure {
            message: "AI agent reached the 30-turn limit".to_string(),
            trace: None,
            usage_trace: None,
            model_calls: 30,
        };
        let canceled = AtomicBool::new(false);
        assert!(should_escalate_builder(&failure, &canceled));
        canceled.store(true, Ordering::Release);
        assert!(!should_escalate_builder(&failure, &canceled));

        let canceled_failure = live_tui::ScriptedAiFailure {
            message: "AI request canceled".to_string(),
            trace: None,
            usage_trace: None,
            model_calls: 1,
        };
        canceled.store(false, Ordering::Release);
        assert!(!should_escalate_builder(&canceled_failure, &canceled));
    }

    #[test]
    fn builder_escalation_gets_a_fresh_turn_allowance() {
        let mut config = GauntletConfigV1::new(false, Vec::new(), 8, 100, GauntletObserver::Jsonl)
            .expect("config");
        config.execution.builder_max_turns = 30;

        assert_eq!(builder_turn_allowance(&config, 40), 30);
    }

    #[test]
    fn builder_allowance_reserves_both_critic_calls() {
        let mut config = GauntletConfigV1::new(false, Vec::new(), 8, 12, GauntletObserver::Jsonl)
            .expect("config");
        config.execution.builder_max_turns = 30;

        assert_eq!(builder_turn_allowance(&config, 7), 3);
        assert_eq!(builder_turn_allowance(&config, 10), 0);
    }

    #[test]
    fn builder_attempt_start_is_visible_in_the_controller_event_stream() {
        let root = std::env::temp_dir().join(format!(
            "stasis-gauntlet-builder-event-{}-{}",
            std::process::id(),
            unix_ms()
        ));
        fs::create_dir_all(&root).expect("builder event temp directory");
        let model = GauntletRoleModel {
            model: Some("gpt-5.6-luna".to_string()),
            reasoning_effort: Some("max".to_string()),
            timeout_minutes: 30,
        };

        emit_builder_attempt_started(&root, "candidate-0001", "primary", &model, 17)
            .expect("builder start event");

        let events = read_jsonl_records(&root.join(EVENTS_NAME), "events").expect("event records");
        let event = events.last().expect("builder event");
        assert_eq!(event["kind"], "role_attempt_started");
        assert_eq!(event["data"]["role"], "builder");
        assert_eq!(event["data"]["candidate"], "candidate-0001");
        assert_eq!(event["data"]["model"], "gpt-5.6-luna");
        assert_eq!(event["data"]["reasoning_effort"], "max");
        assert_eq!(event["data"]["max_turns"], 17);
        fs::remove_dir_all(&root).expect("remove builder event temp directory");
    }

    #[test]
    fn gauntlet_builder_keeps_the_virtual_stasis_tool_protocol() {
        let config = GauntletConfigV1::new(false, Vec::new(), 8, 100, GauntletObserver::Jsonl)
            .expect("config");
        let profile = builder_agent_profile(
            &config,
            &config.models.builder,
            "Fresh builder",
            "Gauntlet-specific instruction.",
            100,
        );

        assert!(profile
            .instruction
            .contains("first JSONL record is the immutable request header"));
        assert!(profile
            .instruction
            .contains("host-mediated virtual tools described by tool_specs"));
        assert!(profile.instruction.contains(
            "never search for them in or reject them because of the native callable-tool registry"
        ));
        assert!(profile
            .instruction
            .contains("Return exactly one JSON object matching the response contract"));
        assert!(profile
            .instruction
            .contains("Gauntlet-specific instruction."));
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
