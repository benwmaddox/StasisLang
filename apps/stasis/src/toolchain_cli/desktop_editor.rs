mod image_attachments;
#[cfg(test)]
mod request_image_tests;
mod semantic_diff;
mod semantic_revisions;

use image_attachments::{AttachmentOrigin, SessionAttachmentStore};
use semantic_revisions::proposal_revisions;

use eframe::egui::{self, Color32, RichText};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use stasis_ai::task_session::{
    ActionState, ActivityKind, ConnectionState, FallbackState, ImageHandoffState, ImageReviewState,
    Key, KeyChord, Modifiers, ProviderSelection, ProviderState, RoutingState,
    ScreenshotAnalysisState, ShortcutMapper, TaskId, TaskLifecycle, TaskSession,
    TaskSessionCommand, UploadState, ValidationStatus,
};
use stasis_ai::{
    action_id_for_tool, run_agent_with_profile, AgentEvent, AgentProfile, ProviderActionProposal,
    ProviderConfig, ProviderReply, ProviderRequest, ProviderUsage, TaskController,
    TaskControllerEvent, ToolCall, ToolExecutor, ToolObservation, ToolSpec,
};
use stasis_runner::live::{LiveCommand, LiveRequest, LiveRuntimeIdentity, LiveSessionClient};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const CAPTURE_DEADLINE: Duration = Duration::from_secs(15);
const CAPTURE_POLL_INTERVAL: Duration = Duration::from_millis(100);
const MAX_CAPTURE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusArea {
    Tasks,
    Reply,
    Game,
    Palette,
}
const EMPTY_TASK: &str = "Create a task";
const COMPACT_WIDTH: f32 = 760.0;
const RAIL_WIDTH: f32 = 224.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditorLayout {
    Compact,
    Wide,
}

impl EditorLayout {
    fn for_width(width: f32) -> Self {
        if width < COMPACT_WIDTH {
            Self::Compact
        } else {
            Self::Wide
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PrimaryAction {
    label: &'static str,
    command: TaskSessionCommand,
    enabled: bool,
    disabled_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TimelineAction {
    Accept(String, String),
    Reject(String, String),
    Apply(String, String),
    ApproveImage(String, String),
    RejectImage(String, String),
    Import(String, String),
    SelectAttachment(String, String),
    UnselectAttachment(String, String),
    RemoveAttachment(String, String),
    PreviewAttachment(String, String),
}

#[derive(Default)]
struct ProposalTools {
    proposals: Vec<ProviderActionProposal>,
}

impl ToolExecutor for ProposalTools {
    fn execute(&mut self, calls: &[ToolCall], canceled: &AtomicBool) -> Vec<ToolObservation> {
        calls
            .iter()
            .map(|call| {
                if canceled.load(Ordering::Acquire) {
                    return ToolObservation::error(&call.tool, "AI request canceled");
                }
                let repair = call.tool == "repair_semantic_edit";
                let result = (|| {
                    let id = call
                        .args
                        .get("proposal_id")
                        .and_then(Value::as_str)
                        .ok_or_else(|| "proposal_id must be a string".to_string())?;
                    let description = call
                        .args
                        .get("description")
                        .and_then(Value::as_str)
                        .ok_or_else(|| "description must be a string".to_string())?;
                    let payload = call
                        .args
                        .get("batch")
                        .cloned()
                        .ok_or_else(|| "batch is required".to_string())?;
                    let batch = serde_json::from_value::<
                        stasis_compiler::frontend::workshop::WorkshopSemanticEditBatch,
                    >(payload.clone())
                    .map_err(|error| format!("invalid semantic edit batch: {error}"))?;
                    if batch.edits.iter().any(|edit| {
                        edit.operation
                            != stasis_compiler::frontend::workshop::WorkshopSemanticEditOperation::Add
                            && edit.expected_source_hash.is_none()
                    }) {
                        return Err(
                            "desktop update/delete proposals require expected_source_hash"
                                .to_string(),
                        );
                    }
                    self.proposals.push(ProviderActionProposal {
                        id: id.to_string(),
                        kind: stasis_ai::ActionKind::Edit,
                        description: description.to_string(),
                        payload,
                        repair,
                    });
                    Ok(json!({"status": "proposed", "proposal_id": id}))
                })();
                match result {
                    Ok(value) => ToolObservation::result(&call.tool, value),
                    Err(error) => ToolObservation::error(&call.tool, error),
                }
            })
            .collect()
    }
}

fn proposal_tool_specs() -> Vec<ToolSpec> {
    [
        (
            "propose_semantic_edit",
            "Propose an atomic semantic edit for explicit user acceptance.",
        ),
        (
            "repair_semantic_edit",
            "Replace only a rejected or needs-repair proposal; accepted work is retained.",
        ),
    ]
    .into_iter()
    .map(|(tool, purpose)| ToolSpec {
        tool: tool.to_string(),
        action_id: action_id_for_tool(tool),
        purpose: purpose.to_string(),
        required_args: vec![
            "proposal_id".to_string(),
            "description".to_string(),
            "batch".to_string(),
        ],
        optional_args: Vec::new(),
    })
    .collect()
}

fn bounded_provider_label(value: Option<&str>, fallback: &str) -> String {
    value
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(fallback)
        .chars()
        .take(96)
        .collect()
}

fn usage_u64(usage: &Value, pointers: &[&str]) -> u64 {
    pointers
        .iter()
        .find_map(|pointer| usage.pointer(pointer).and_then(Value::as_u64))
        .unwrap_or(0)
}

fn configured_provider_state(config: &ProviderConfig) -> ProviderState {
    let (route, fallback) = match config {
        ProviderConfig::Codex => ("direct".to_string(), FallbackState::Unconfigured),
        ProviderConfig::OpenRouter(openrouter) => {
            let route = if !openrouter.routing.only.is_empty() {
                format!("only:{}", openrouter.routing.only.join(","))
            } else if !openrouter.routing.order.is_empty() {
                format!("order:{}", openrouter.routing.order.join(","))
            } else {
                format!("openrouter:{:?}", openrouter.routing.sort).to_ascii_lowercase()
            };
            let fallback = if openrouter.routing.allow_fallbacks {
                FallbackState::Ready {
                    provider: "openrouter".to_string(),
                    model: Some(bounded_provider_label(
                        Some(&openrouter.model),
                        "configured",
                    )),
                    route: Some(bounded_provider_label(Some(&route), "openrouter")),
                }
            } else {
                FallbackState::Unconfigured
            };
            (route, fallback)
        }
    };
    ProviderState {
        provider: Some(config.provider_name().to_string()),
        model: Some(bounded_provider_label(Some(&config.model()), "configured")),
        routing: RoutingState::Assigned {
            route: bounded_provider_label(Some(&route), "direct"),
        },
        fallback,
    }
}

fn provider_reply_state(config: &ProviderConfig, usage: Option<&Value>) -> ProviderState {
    let provider = bounded_provider_label(
        usage
            .and_then(|value| value.get("resolved_provider"))
            .and_then(Value::as_str),
        config.provider_name(),
    );
    let model = bounded_provider_label(
        usage
            .and_then(|value| value.get("resolved_model"))
            .and_then(Value::as_str),
        &config.model(),
    );
    let route = match usage.and_then(|value| value.get("route")) {
        Some(Value::String(route)) => bounded_provider_label(Some(route), "direct"),
        Some(Value::Object(_)) => bounded_provider_label(
            Some(&format!("{}:{provider}", config.provider_name())),
            "direct",
        ),
        _ => {
            let RoutingState::Assigned { route } = configured_provider_state(config).routing else {
                unreachable!("configured provider route is always assigned")
            };
            route
        }
    };
    let fallback = if usage
        .and_then(|value| value.get("fallback"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        FallbackState::Active {
            provider: provider.clone(),
            model: Some(model.clone()),
            route: Some(route.clone()),
        }
    } else {
        configured_provider_state(config).fallback
    };
    ProviderState {
        provider: Some(provider),
        model: Some(model),
        routing: RoutingState::Assigned { route },
        fallback,
    }
}

fn provider_reply_usage(usage: Option<&Value>) -> ProviderUsage {
    let input_tokens = usage
        .map(|value| {
            usage_u64(
                value,
                &["/tokens/input_tokens", "/tokens/prompt", "/input_tokens"],
            )
        })
        .unwrap_or(0);
    let output_tokens = usage
        .map(|value| {
            usage_u64(
                value,
                &[
                    "/tokens/output_tokens",
                    "/tokens/completion",
                    "/output_tokens",
                ],
            )
        })
        .unwrap_or(0);
    let estimated_cost_micros = usage
        .and_then(|value| value.get("cost"))
        .and_then(Value::as_f64)
        .filter(|cost| cost.is_finite() && *cost > 0.0)
        .map(|cost| (cost * 1_000_000.0).round() as u64)
        .unwrap_or(0);
    ProviderUsage {
        input_tokens,
        output_tokens,
        estimated_cost_micros,
    }
}

fn selected_provider_config(
    selection: Option<ProviderSelection>,
) -> Result<ProviderConfig, String> {
    match selection {
        Some(ProviderSelection::Codex) => Ok(ProviderConfig::Codex),
        Some(ProviderSelection::OpenRouter) => Ok(ProviderConfig::OpenRouter(
            stasis_ai::OpenRouterConfig::from_env()?,
        )),
        None => ProviderConfig::from_env(),
    }
}

fn run_reply_provider(
    request: ProviderRequest,
    canceled: Arc<AtomicBool>,
    project_root: PathBuf,
) -> Result<ProviderReply, String> {
    let config = selected_provider_config(request.selected_provider)?;
    let image_paths = verified_provider_screenshot_paths(&config, &request)?;
    if canceled.load(Ordering::Acquire) {
        return Err("AI request canceled".into());
    }
    let provider = config
        .clone()
        .build()?
        .with_timeout(Duration::from_secs(120));
    let mut provider = if matches!(config, ProviderConfig::OpenRouter(_)) {
        let images = image_paths
            .iter()
            .zip(&request.screenshots)
            .map(|(path, screenshot)| {
                let bytes = image_attachments::read_bounded(path)?;
                let mime = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
                    "image/png"
                } else {
                    "image/jpeg"
                };
                stasis_ai::OpenRouterImageInput::new(
                    mime,
                    bytes,
                    screenshot.content_sha256.as_deref().unwrap_or_default(),
                )
            })
            .collect::<Result<Vec<_>, String>>()?;
        provider.with_openrouter_image_inputs(images)?
    } else {
        provider.with_images(image_paths)?
    };
    let prompt = request
        .context
        .last()
        .map(|entry| entry.text.trim())
        .filter(|text| !text.is_empty())
        .unwrap_or(request.objective.as_str())
        .to_string();
    let source_context = super::desktop_source_context(&project_root)?;
    let initial_context = json!({
        "task_id": request.task_id,
        "objective": request.objective,
        "project_summary": request.project_summary,
        "relevant_files": request.relevant_files,
        "relevant_symbols": request.relevant_symbols,
        "relevant_tests": request.relevant_tests,
        "screenshots": request.screenshots,
        "thread": request.context,
        "actions": request.actions,
        "editable_sources": source_context,
    });
    let profile = AgentProfile {
        role: "Stasis desktop task assistant".to_string(),
        instruction: "Answer the user's task-scoped message. Use propose_semantic_edit for new source changes. Use repair_semantic_edit only for an action the task context shows as rejected or needing repair. Never regenerate or replace accepted work. Proposals require explicit user acceptance and are not executed during this request. Keep the response concise and self-contained.".to_string(),
        max_turns: 3,
        ..AgentProfile::default()
    };
    let mut usage = None;
    let mut tools = ProposalTools::default();
    let text = run_agent_with_profile(
        &mut provider,
        &mut tools,
        &profile,
        &prompt,
        initial_context,
        proposal_tool_specs(),
        &canceled,
        |event| {
            if let AgentEvent::ProviderUsage(value) = event {
                usage = Some(value);
            }
        },
    )?;
    let mut reply = ProviderReply::new(text);
    reply.proposals = tools.proposals;
    reply.provider = provider_reply_state(&config, usage.as_ref());
    reply.usage = provider_reply_usage(usage.as_ref());
    Ok(reply)
}

fn verified_provider_screenshot_paths(
    config: &ProviderConfig,
    request: &ProviderRequest,
) -> Result<Vec<PathBuf>, String> {
    if request.screenshots.is_empty() {
        return Ok(Vec::new());
    }
    if matches!(config, ProviderConfig::Codex) && !config.supports_image_input() {
        return Err(format!(
            "selected {} model {} does not support image input",
            config.provider_name(),
            config.model()
        ));
    }
    request
        .screenshots
        .iter()
        .map(|screenshot| {
            if screenshot.provenance.task_id != request.task_id {
                return Err("screenshot provenance does not match provider task".to_string());
            }
            if screenshot.request_id != Some(request.request_id.get()) {
                return Err("image provenance does not match provider request".to_string());
            }
            if !screenshot.selected_for_request || !screenshot.consent_to_send {
                return Err("image pixels require explicit request consent".to_string());
            }
            let expected = screenshot
                .content_sha256
                .as_deref()
                .ok_or_else(|| "editor screenshot has no verified content SHA-256".to_string())?;
            let path = PathBuf::from(&screenshot.source);
            verify_screenshot_file(&path, expected)
                .map_err(|error| format!("screenshot {} {error}", screenshot.id))?;
            Ok(path)
        })
        .collect()
}

fn verify_screenshot_file(path: &std::path::Path, expected_sha256: &str) -> Result<(), String> {
    let file = std::fs::File::open(path)
        .map_err(|error| format!("could not be opened at {}: {error}", path.display()))?;
    let mut bytes = Vec::new();
    file.take((MAX_CAPTURE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("could not be read at {}: {error}", path.display()))?;
    if bytes.len() > MAX_CAPTURE_BYTES {
        return Err(format!("exceeds {MAX_CAPTURE_BYTES} bytes"));
    }
    let actual = format!("{:x}", Sha256::digest(&bytes));
    if actual != expected_sha256 {
        return Err("changed after it was previewed".into());
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum EditorIntent {
    SendReply(String, String),
    Apply(String, String),
    Test(String, u64),
    Screenshot(String),
    GenerateImage(String),
    ImportImage(String, String),
    Cancel(String),
    Retry(String),
    Reconnect(String),
    MarkDone(String),
}

#[derive(Debug)]
enum HostOperation {
    Apply {
        action_id: String,
        preview: super::DesktopSemanticPreview,
    },
    Test {
        paths: Vec<String>,
        run_id: u64,
    },
}

#[derive(Debug)]
struct HostRequest {
    task_id: String,
    operation: HostOperation,
    source_before: Option<String>,
}

#[derive(Debug)]
struct HostResult {
    task_id: String,
    operation: HostOperation,
    result: Result<(String, Value, String), String>,
}

fn receipt_has_test_evidence(receipt: &Value) -> bool {
    [
        "/validation/test_result/tests_run",
        "/validation/test_result/scenario_cases_run",
    ]
    .into_iter()
    .filter_map(|pointer| receipt.pointer(pointer).and_then(Value::as_u64))
    .sum::<u64>()
        > 0
}

fn bounded_failure(error: &str, max: usize) -> String {
    let bounded = error.chars().take(max).collect::<String>();
    if error.chars().count() > max {
        format!("{bounded}...")
    } else {
        bounded
    }
}

fn human_bytes(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

fn record_focused_test_failure(
    task: &mut stasis_ai::Task,
    run_id: u64,
    error: &str,
) -> Result<(), stasis_ai::TaskSessionError> {
    let summary = bounded_failure(error, 3500);
    task.finish_focused_test_run(
        run_id,
        stasis_ai::FocusedTestResult::failed(summary.clone()),
    )?;
    task.append_host_result(format!(
        "Focused tests failed: {summary}. Fix the reported failure and rerun."
    ))
}

struct HostExecutor {
    requests: Option<mpsc::Sender<HostRequest>>,
    results: mpsc::Receiver<HostResult>,
    canceled: Arc<Mutex<BTreeSet<String>>>,
    shutdown: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl HostExecutor {
    fn new(project_root: PathBuf) -> Self {
        let (request_tx, request_rx) = mpsc::channel::<HostRequest>();
        let (result_tx, result_rx) = mpsc::channel::<HostResult>();
        let canceled = Arc::new(Mutex::new(BTreeSet::<String>::new()));
        let worker_canceled = Arc::clone(&canceled);
        let shutdown = Arc::new(AtomicBool::new(false));
        let worker_shutdown = Arc::clone(&shutdown);
        let worker = thread::spawn(move || {
            while let Ok(request) = request_rx.recv() {
                let skip = worker_shutdown.load(Ordering::Acquire)
                    || worker_canceled
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .contains(&request.task_id);
                let result = if skip {
                    Err("operation canceled before execution".to_string())
                } else {
                    let operation_result = match &request.operation {
                        HostOperation::Apply { preview, .. } => {
                            super::desktop_apply_semantic_preview(&project_root, preview)
                        }
                        HostOperation::Test { paths, .. } => {
                            super::desktop_run_focused_tests(&project_root, paths)
                        }
                    };
                    operation_result.and_then(|(summary, receipt)| match &request.operation {
                        HostOperation::Apply { .. } => receipt
                            .get("source_fingerprint")
                            .and_then(Value::as_str)
                            .map(|fingerprint| (summary, receipt.clone(), fingerprint.to_string()))
                            .ok_or_else(|| {
                                "semantic edit receipt omitted its source fingerprint".to_string()
                            }),
                        HostOperation::Test { paths, .. } => {
                            super::desktop_source_fingerprint(&project_root, paths).and_then(
                                |fingerprint| {
                                    if request.source_before.as_ref() != Some(&fingerprint) {
                                        Err("project sources changed while focused tests were running; validation was discarded".to_string())
                                    } else {
                                        Ok((summary, receipt, fingerprint))
                                    }
                                },
                            )
                        }
                    })
                };
                if result_tx
                    .send(HostResult {
                        task_id: request.task_id,
                        operation: request.operation,
                        result,
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            requests: Some(request_tx),
            results: result_rx,
            canceled,
            shutdown,
            worker: Some(worker),
        }
    }

    fn submit(&self, request: HostRequest) -> Result<(), String> {
        self.requests
            .as_ref()
            .ok_or_else(|| "desktop host executor is shut down".to_string())?
            .send(request)
            .map_err(|_| "desktop host executor is unavailable".to_string())
    }

    fn cancel(&self, task_id: &str) {
        self.canceled
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(task_id.to_string());
    }

    fn poll(&self) -> Vec<HostResult> {
        self.results.try_iter().collect()
    }

    fn shutdown_and_join(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        // Disconnect before joining so an idle worker can leave recv().
        self.requests.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for HostExecutor {
    fn drop(&mut self) {
        self.shutdown_and_join();
    }
}

struct DesktopEditor {
    state: EditorState,
    controller: TaskController,
    client: LiveSessionClient,
    project_root: PathBuf,
    shutdown: Arc<AtomicBool>,
    host: HostExecutor,
    validation_fingerprints: BTreeMap<String, (String, Vec<String>)>,
    busy_tasks: BTreeSet<String>,
    execution_receipts: BTreeMap<(String, String), Value>,
    validation_receipts: BTreeMap<String, Value>,
    capture: Option<PendingCapture>,
    capture_results: Receiver<CaptureResult>,
    capture_result_tx: mpsc::Sender<CaptureResult>,
    next_capture: u64,
    preview_texture: Option<(String, egui::TextureHandle)>,
    asset_textures: BTreeMap<(String, String), Option<egui::TextureHandle>>,
    attachment_store: SessionAttachmentStore,
    attachment_textures: BTreeMap<(String, String), egui::TextureHandle>,
    next_attachment: u64,
    attachment_preview: Option<(String, String)>,
    capability_cache: BTreeMap<(String, String, String), Result<(), String>>,
    capability_pending: BTreeSet<(String, String, String)>,
    capability_results: Receiver<CapabilityResult>,
    capability_result_tx: mpsc::Sender<CapabilityResult>,
    semantic_job: Option<SemanticPreviewJob>,
    next_semantic_check: Instant,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct SemanticPreviewKey {
    task: String,
    action: String,
    revision: usize,
    payload_hash: String,
}

impl SemanticPreviewKey {
    fn new(task: &str, action: &str, revision: usize, payload: &Value) -> Self {
        Self {
            task: task.into(),
            action: action.into(),
            revision,
            payload_hash: format!("{:x}", Sha256::digest(payload.to_string().as_bytes())),
        }
    }
}

#[derive(Debug)]
struct SemanticPreviewRecord {
    result: Option<Result<super::DesktopSemanticPreview, String>>,
    stale: bool,
}

struct SemanticPreviewJob {
    key: SemanticPreviewKey,
    result: Receiver<Result<super::DesktopSemanticPreview, String>>,
    worker: thread::JoinHandle<()>,
}

fn refresh_semantic_preview_staleness(
    records: &mut BTreeMap<SemanticPreviewKey, SemanticPreviewRecord>,
    next_check: &mut Instant,
    now: Instant,
    fingerprint: impl FnOnce() -> Result<String, String>,
) {
    if now < *next_check
        || !records
            .values()
            .any(|record| !record.stale && matches!(&record.result, Some(Ok(_))))
    {
        return;
    }
    *next_check = now + Duration::from_millis(500);
    let current = fingerprint();
    for record in records.values_mut() {
        if let Some(Ok(preview)) = &record.result {
            record.stale |= current
                .as_ref()
                .map_or(true, |hash| hash != &preview.source_fingerprint);
        }
    }
}

impl DesktopEditor {
    fn poll_semantic_previews(&mut self) {
        if let Some(job) = &self.semantic_job {
            let result = match job.result.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Disconnected) => Some(Err("Preview worker stopped".into())),
                Err(TryRecvError::Empty) => None,
            };
            if let Some(result) = result {
                let job = self.semantic_job.take().expect("pending preview");
                let _ = job.worker.join();
                if let Some(record) = self.state.semantic_previews.get_mut(&job.key) {
                    record.result = Some(result);
                }
            }
        }
        // Retain each observed revision at its original position in the thread.
        let mut queued = Vec::new();
        for task in self.state.session.tasks() {
            for action in task.actions.values() {
                for proposal in proposal_revisions(action) {
                    let Some(payload) = proposal.payload else {
                        continue;
                    };
                    let key = SemanticPreviewKey::new(
                        task.id.as_str(),
                        action.id.as_str(),
                        proposal.revision,
                        payload,
                    );
                    let can_plan =
                        proposal.current && matches!(proposal.state, ActionState::Proposed);
                    self.state.semantic_previews.entry(key.clone()).or_insert_with(|| SemanticPreviewRecord {
                        result: if can_plan { None } else {
                            Some(Err("No retained preview for this proposal; it will not be regenerated".into()))
                        },
                        stale: false,
                    });
                    if can_plan && self.state.semantic_previews[&key].result.is_none() {
                        queued.push((key, payload.clone()));
                    }
                }
            }
        }
        let project_root = &self.project_root;
        refresh_semantic_preview_staleness(
            &mut self.state.semantic_previews,
            &mut self.next_semantic_check,
            Instant::now(),
            move || super::desktop_source_fingerprint(project_root, &[]),
        );
        if self.semantic_job.is_none() {
            if let Some((key, payload)) = queued.into_iter().next() {
                let root = self.project_root.clone();
                let (tx, result) = mpsc::channel();
                let worker = thread::spawn(move || {
                    let _ = tx.send(super::desktop_preview_semantic_batch(&root, payload));
                });
                self.semantic_job = Some(SemanticPreviewJob {
                    key,
                    result,
                    worker,
                });
            }
        }
    }
}

#[derive(Debug)]
struct PendingCapture {
    task_id: TaskId,
    screenshot_id: String,
    request_id: u64,
    canceled: Arc<AtomicBool>,
}

#[derive(Debug)]
struct CaptureResult {
    task_id: TaskId,
    screenshot_id: String,
    request_id: u64,
    result: Result<CaptureEvidence, String>,
}

#[derive(Debug)]
struct CapabilityResult {
    key: (String, String, String),
    result: Result<(), String>,
}

#[derive(Debug)]
struct CaptureEvidence {
    path: PathBuf,
    bytes: Vec<u8>,
    width: usize,
    height: usize,
    scheduled_tick: u64,
    captured_tick: u64,
    sha256: String,
    runtime_identity: LiveRuntimeIdentity,
}

#[derive(Debug, Clone)]
struct ScreenshotPreview {
    task_id: TaskId,
    screenshot_id: String,
    rgba: Vec<u8>,
    width: usize,
    height: usize,
    scheduled_tick: u64,
    captured_tick: u64,
    sha256: String,
    runtime_identity: LiveRuntimeIdentity,
}

fn cancel_live_capture(client: &LiveSessionClient, request_id: u64) {
    let _ = client.submit(LiveRequest::new(
        request_id.saturating_add(1),
        LiveCommand::Cancel { request_id },
    ));
}

fn capture_frame(
    client: &LiveSessionClient,
    request_id: u64,
    artifact: String,
    canceled: &AtomicBool,
    deadline: Duration,
) -> Result<CaptureEvidence, String> {
    client.submit(LiveRequest::new(
        request_id,
        LiveCommand::CaptureFrame { artifact },
    ))?;
    let started = Instant::now();
    loop {
        if canceled.load(Ordering::Acquire) {
            cancel_live_capture(client, request_id);
            return Err("capture canceled".into());
        }
        let Some(remaining) = deadline.checked_sub(started.elapsed()) else {
            cancel_live_capture(client, request_id);
            return Err(format!(
                "capture did not complete within {} seconds",
                deadline.as_secs()
            ));
        };
        let response = match client.receive_timeout(remaining.min(CAPTURE_POLL_INTERVAL)) {
            Ok(response) => response,
            Err(error) if error.contains("timed out") => continue,
            Err(error) => return Err(error),
        };
        if response.request_id != request_id {
            continue;
        }
        if !response.ok {
            return Err(response
                .error
                .unwrap_or_else(|| "runtime rejected screenshot capture".into()));
        }
        if response.kind != "capture_completed" {
            return Err(format!(
                "runtime returned {} before screenshot completion",
                response.kind
            ));
        }
        let data = response
            .data
            .ok_or_else(|| "completed screenshot response has no evidence".to_string())?;
        let path = data
            .get("path")
            .and_then(Value::as_str)
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .ok_or_else(|| "completed screenshot response has no path".to_string())?;
        let width = data
            .get("width")
            .and_then(Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
            .filter(|value| *value > 0)
            .ok_or_else(|| "completed screenshot response has invalid width".to_string())?;
        let height = data
            .get("height")
            .and_then(Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
            .filter(|value| *value > 0)
            .ok_or_else(|| "completed screenshot response has invalid height".to_string())?;
        let scheduled_tick = data
            .get("scheduled_tick")
            .and_then(Value::as_u64)
            .ok_or_else(|| "completed screenshot response has no scheduled tick".to_string())?;
        let captured_tick = data
            .get("captured_tick")
            .and_then(Value::as_u64)
            .ok_or_else(|| "completed screenshot response has no captured tick".to_string())?;
        let expected_bytes = data
            .get("byte_length")
            .and_then(Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
            .filter(|value| *value > 0)
            .ok_or_else(|| "completed screenshot response has invalid byte length".to_string())?;
        if expected_bytes > MAX_CAPTURE_BYTES {
            return Err(format!(
                "captured frame is {expected_bytes} bytes; limit is {MAX_CAPTURE_BYTES} bytes"
            ));
        }
        let expected_sha256 = data
            .get("sha256")
            .and_then(Value::as_str)
            .filter(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .ok_or_else(|| "completed screenshot response has invalid SHA-256".to_string())?
            .to_ascii_lowercase();
        let runtime_identity = response
            .runtime_identity
            .ok_or_else(|| "completed screenshot response has no runtime provenance".to_string())?;
        let file = std::fs::File::open(&path).map_err(|error| {
            format!("failed opening captured frame {}: {error}", path.display())
        })?;
        let mut bytes = Vec::with_capacity(expected_bytes);
        file.take((MAX_CAPTURE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|error| {
                format!("failed reading captured frame {}: {error}", path.display())
            })?;
        if bytes.len() > MAX_CAPTURE_BYTES {
            return Err(format!("captured frame exceeds {MAX_CAPTURE_BYTES} bytes"));
        }
        if bytes.len() != expected_bytes {
            return Err(format!(
                "captured frame byte length changed: expected {expected_bytes}, found {}",
                bytes.len()
            ));
        }
        let sha256 = format!("{:x}", Sha256::digest(&bytes));
        if sha256 != expected_sha256 {
            return Err("captured frame SHA-256 does not match runtime evidence".into());
        }
        return Ok(CaptureEvidence {
            path,
            bytes,
            width,
            height,
            scheduled_tick,
            captured_tick,
            sha256,
            runtime_identity,
        });
    }
}

fn screenshot_preview(
    task_id: &TaskId,
    screenshot_id: &str,
    evidence: &CaptureEvidence,
) -> Result<ScreenshotPreview, String> {
    let decoded = image_attachments::decode_png_rgba_limited(&evidence.bytes)
        .map_err(|error| format!("captured frame is not a valid bounded PNG: {error}"))?;
    let (width, height) = decoded.dimensions();
    if width as usize != evidence.width || height as usize != evidence.height {
        return Err(format!(
            "captured frame dimensions changed: runtime reported {}x{}, PNG is {width}x{height}",
            evidence.width, evidence.height
        ));
    }
    Ok(ScreenshotPreview {
        task_id: task_id.clone(),
        screenshot_id: screenshot_id.to_string(),
        rgba: decoded.into_raw(),
        width: width as usize,
        height: height as usize,
        scheduled_tick: evidence.scheduled_tick,
        captured_tick: evidence.captured_tick,
        sha256: evidence.sha256.clone(),
        runtime_identity: evidence.runtime_identity.clone(),
    })
}

impl DesktopEditor {
    fn new(client: LiveSessionClient, project_root: PathBuf, shutdown: Arc<AtomicBool>) -> Self {
        let host = HostExecutor::new(project_root.clone());
        let provider_root = project_root.clone();
        let (capture_result_tx, capture_results) = mpsc::channel();
        let (capability_result_tx, capability_results) = mpsc::channel();
        Self {
            state: EditorState {
                project_root: Some(project_root.clone()),
                ..EditorState::default()
            },
            controller: TaskController::new(move |request, canceled| {
                run_reply_provider(request, canceled, provider_root.clone())
            }),
            client,
            project_root,
            shutdown,
            host,
            validation_fingerprints: BTreeMap::new(),
            busy_tasks: BTreeSet::new(),
            execution_receipts: BTreeMap::new(),
            validation_receipts: BTreeMap::new(),
            capture: None,
            capture_results,
            capture_result_tx,
            next_capture: 1,
            preview_texture: None,
            asset_textures: BTreeMap::new(),
            attachment_store: SessionAttachmentStore::new(),
            attachment_textures: BTreeMap::new(),
            next_attachment: 1,
            attachment_preview: None,
            capability_cache: BTreeMap::new(),
            capability_pending: BTreeSet::new(),
            capability_results,
            capability_result_tx,
            semantic_job: None,
            next_semantic_check: Instant::now(),
        }
    }

    fn process_shortcuts(&mut self, context: &egui::Context) {
        if self.state.palette_open {
            return;
        }
        let events = context.input(|input| input.events.clone());
        for event in events {
            if let egui::Event::Key {
                key: egui::Key::V,
                pressed: true,
                modifiers,
                ..
            } = event
            {
                if modifiers.ctrl && modifiers.shift {
                    if let Some(task_id) = self.state.session.active_task_id().cloned() {
                        context.input_mut(|input| input.consume_key(modifiers, egui::Key::V));
                        self.paste_clipboard_image(&task_id);
                    }
                    continue;
                }
            }
            if let Some(command) =
                EditorState::chord(&event).and_then(|chord| self.state.shortcuts.command_for(chord))
            {
                if let egui::Event::Key { key, modifiers, .. } = event {
                    context.input_mut(|input| {
                        input.consume_key(modifiers, key);
                    });
                }
                self.state.dispatch(command);
                if self.state.palette_open {
                    break;
                }
            }
        }
    }

    fn flush_intents(&mut self) {
        let intents = std::mem::take(&mut self.state.intents);
        for intent in intents {
            match intent {
                EditorIntent::SendReply(task, text) => {
                    let task = TaskId::new(task);
                    let mut candidate = self.state.session.clone();
                    let accepted = candidate
                        .task_mut(&task)
                        .and_then(|task| task.append_user_message(&text))
                        .map_err(|error| error.to_string())
                        .and_then(|()| {
                            let needs_provider = candidate
                                .task(&task)
                                .map(|task| task.provider.provider.is_none())
                                .unwrap_or(false);
                            if needs_provider {
                                if let Ok(config) = ProviderConfig::from_env() {
                                    candidate
                                        .task_mut(&task)
                                        .and_then(|task| {
                                            task.set_provider_state(configured_provider_state(
                                                &config,
                                            ))
                                        })
                                        .map_err(|error| error.to_string())?;
                                }
                            }
                            Ok(())
                        })
                        .and_then(|()| {
                            self.controller
                                .send(&mut candidate, &task)
                                .map_err(|error| error.to_string())
                        });
                    match accepted {
                        Ok(_) => self.state.session = candidate,
                        Err(error) => {
                            if self.state.session.active_task_id() == Some(&task)
                                && self.state.reply.is_empty()
                            {
                                self.state.reply = text;
                            }
                            self.state.notice = Some(error);
                        }
                    }
                }
                EditorIntent::Retry(task) => {
                    let task = TaskId::new(task);
                    if let Err(error) = self.controller.retry(&mut self.state.session, &task) {
                        self.state.notice = Some(error.to_string());
                    }
                }
                EditorIntent::Cancel(task) => {
                    let task = TaskId::new(task);
                    self.host.cancel(task.as_str());
                    let canceled_capture = self.cancel_capture_for(&task);
                    if let Err(error) = self.controller.cancel(&mut self.state.session, &task) {
                        if !canceled_capture {
                            self.state.notice = Some(error.to_string());
                        }
                    }
                }
                EditorIntent::Reconnect(task) => {
                    let task = TaskId::new(task);
                    self.cancel_capture_for(&task);
                    if let Err(error) = self.controller.reconnect(&mut self.state.session, &task) {
                        self.state.notice = Some(error.to_string());
                    }
                }
                EditorIntent::Apply(task, action) => {
                    let accepted = self
                        .state
                        .session
                        .task(task.as_str())
                        .ok()
                        .and_then(|task| task.actions.get(action.as_str()))
                        .is_some_and(|action| matches!(action.state, ActionState::Accepted));
                    if !accepted {
                        self.state.notice =
                            Some("Only the accepted action revision can be applied".into());
                        continue;
                    }
                    if !self.busy_tasks.insert(task.clone()) {
                        self.state.notice = Some(format!(
                            "An apply or focused-test operation is already running for {task}."
                        ));
                        continue;
                    }
                    let submitted = self
                        .state
                        .reviewed_preview(&task, &action)
                        .cloned()
                        .and_then(|preview| {
                            self.host.submit(HostRequest {
                                task_id: task.clone(),
                                operation: HostOperation::Apply {
                                    action_id: action.clone(),
                                    preview,
                                },
                                source_before: None,
                            })
                        });
                    if let Err(error) = submitted {
                        self.busy_tasks.remove(&task);
                        let reason = bounded_failure(&error, 900);
                        if let Ok(task_state) = self.state.session.task_mut(task.as_str()) {
                            let _ =
                                task_state.mark_action_for_repair(action.as_str(), reason.clone());
                            let _ = task_state.append_host_result(format!(
                                "Could not apply {action}: {reason}. Repair this proposal and retry."
                            ));
                        }
                        self.state.notice = Some(error);
                    }
                }
                EditorIntent::Test(task, run_id) => {
                    if !self.busy_tasks.insert(task.clone()) {
                        let message = format!(
                            "An apply or focused-test operation is already running for {task}."
                        );
                        if let Ok(task_state) = self.state.session.task_mut(task.as_str()) {
                            let _ = record_focused_test_failure(task_state, run_id, &message);
                        }
                        self.state.notice = Some(message);
                        continue;
                    }
                    let paths = self
                        .state
                        .session
                        .task(task.as_str())
                        .map(|task| task.relevant_tests.clone())
                        .unwrap_or_default();
                    let submitted = super::desktop_source_fingerprint(&self.project_root, &paths)
                        .and_then(|source_before| {
                            self.host.submit(HostRequest {
                                task_id: task.clone(),
                                operation: HostOperation::Test { paths, run_id },
                                source_before: Some(source_before),
                            })
                        });
                    if let Err(error) = submitted {
                        self.busy_tasks.remove(&task);
                        if let Ok(task_state) = self.state.session.task_mut(task.as_str()) {
                            let _ = record_focused_test_failure(task_state, run_id, &error);
                        }
                        self.state.notice = Some(error);
                    }
                }
                EditorIntent::MarkDone(task) => {
                    let paths = self
                        .validation_fingerprints
                        .get(&task)
                        .map(|(_, paths)| paths.clone())
                        .unwrap_or_default();
                    let current = super::desktop_source_fingerprint(&self.project_root, &paths);
                    let expected = self
                        .validation_fingerprints
                        .get(&task)
                        .map(|(fingerprint, _)| fingerprint);
                    match current {
                        Ok(current) if expected == Some(&current) => {
                            if let Err(error) = self
                                .state
                                .session
                                .task_mut(task.as_str())
                                .and_then(|task| task.mark_done())
                            {
                                self.state.notice = Some(error.to_string());
                            }
                        }
                        Ok(_) => {
                            self.state.notice = Some(
                                "Project sources changed after validation; run focused tests again."
                                    .to_string(),
                            );
                        }
                        Err(error) => self.state.notice = Some(error),
                    }
                }
                EditorIntent::GenerateImage(task) | EditorIntent::ImportImage(task, _) => {
                    let message = "Image generation and asset import are unavailable in the desktop editor. No asset was generated or imported.";
                    if let Ok(task) = self.state.session.task_mut(task.as_str()) {
                        let _ = task.append_host_result(message);
                    }
                    self.state.notice = Some(message.into());
                }
                EditorIntent::Screenshot(task) => self.start_capture(TaskId::new(task)),
            }
        }
    }

    fn image_attachment_capability(&self, task_id: &TaskId) -> Result<(), String> {
        let task = self
            .state
            .session
            .task(task_id)
            .map_err(|error| error.to_string())?;
        let config = selected_provider_config(task.selected_provider)?;
        match &config {
            ProviderConfig::Codex if config.supports_image_input() => Ok(()),
            ProviderConfig::Codex => Err(format!(
                "{} model {} does not support image input",
                config.provider_name(),
                config.model()
            )),
            ProviderConfig::OpenRouter(openrouter) => {
                let key = (
                    config.provider_name().to_string(),
                    openrouter.base_url.clone(),
                    config.model(),
                );
                self.capability_cache.get(&key).cloned().unwrap_or_else(|| {
                    Err(format!(
                        "image support has not been verified for {} model {}",
                        key.0, key.2
                    ))
                })
            }
        }
    }

    fn poll_image_capabilities(&mut self) {
        for completed in self.capability_results.try_iter() {
            self.capability_pending.remove(&completed.key);
            self.capability_cache
                .insert(completed.key, completed.result);
        }
    }

    fn ensure_active_image_capability(&mut self) {
        let Some(task_id) = self.state.session.active_task_id().cloned() else {
            return;
        };
        let Ok(task) = self.state.session.task(&task_id) else {
            return;
        };
        let Ok(ProviderConfig::OpenRouter(mut config)) =
            selected_provider_config(task.selected_provider)
        else {
            return;
        };
        let key = (
            "openrouter".to_string(),
            config.base_url.clone(),
            config.model.clone(),
        );
        if self.capability_cache.contains_key(&key) || !self.capability_pending.insert(key.clone())
        {
            return;
        }
        let tx = self.capability_result_tx.clone();
        let canceled = Arc::clone(&self.shutdown);
        config.timeout = config.timeout.min(Duration::from_secs(15));
        thread::spawn(move || {
            let result = stasis_ai::OpenRouterProvider::new(config)
                .and_then(|mut provider| provider.refresh_image_input_capability(&canceled))
                .and_then(|capability| {
                    if capability.supported {
                        Ok(())
                    } else {
                        Err(capability.reason)
                    }
                });
            let _ = tx.send(CapabilityResult { key, result });
        });
    }

    fn refresh_active_image_capability(&mut self) {
        let Some(task_id) = self.state.session.active_task_id().cloned() else {
            return;
        };
        let Ok(task) = self.state.session.task(&task_id) else {
            return;
        };
        let Ok(ProviderConfig::OpenRouter(config)) =
            selected_provider_config(task.selected_provider)
        else {
            return;
        };
        let key = ("openrouter".to_string(), config.base_url, config.model);
        self.capability_cache.remove(&key);
        self.capability_pending.remove(&key);
        self.ensure_active_image_capability();
    }

    fn next_attachment_id(&mut self, prefix: &str) -> String {
        let sequence = self.next_attachment;
        self.next_attachment = self.next_attachment.saturating_add(1);
        format!("{prefix}-{sequence}")
    }

    fn attach_encoded_image(
        &mut self,
        task_id: &TaskId,
        id: String,
        name: String,
        origin: AttachmentOrigin,
        bytes: &[u8],
    ) -> Result<(), String> {
        if self.state.session.active_task_id() != Some(task_id) {
            return Err("image attachment belongs to an inactive task".into());
        }
        self.image_attachment_capability(task_id)?;
        let attachment =
            self.attachment_store
                .insert_encoded(task_id, id.clone(), name, origin, bytes)?;
        let result = self
            .state
            .session
            .task_mut(task_id)
            .and_then(|task| task.set_vision_capability(true))
            .and_then(|()| {
                self.state.session.task_mut(task_id).and_then(|task| {
                    task.attach_screenshot_with_sha256(
                        id.as_str(),
                        attachment.path.to_string_lossy().into_owned(),
                        attachment.sha256.clone(),
                    )
                })
            })
            .map_err(|error| error.to_string());
        if result.is_err() {
            self.attachment_store.remove(task_id, &id);
        }
        result
    }

    fn attach_file_path(
        &mut self,
        task_id: &TaskId,
        path: &std::path::Path,
        origin: AttachmentOrigin,
    ) {
        let result = image_attachments::read_bounded(path).and_then(|bytes| {
            let id = self.next_attachment_id("image");
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("image")
                .to_string();
            self.attach_encoded_image(task_id, id, name, origin, &bytes)
        });
        self.state.notice = Some(match result {
            Ok(()) => format!(
                "Attached image from {}. Select Include once before sending pixels.",
                origin.label()
            ),
            Err(error) => format!("Could not attach image: {error}"),
        });
    }

    fn select_image_files(&mut self, task_id: &TaskId) {
        if let Err(reason) = self.image_attachment_capability(task_id) {
            self.state.notice = Some(format!("Cannot attach image: {reason}."));
            return;
        }
        let files = rfd::FileDialog::new()
            .add_filter("PNG or JPEG image", &["png", "jpg", "jpeg"])
            .pick_files()
            .unwrap_or_default();
        for path in files.into_iter().take(8) {
            self.attach_file_path(task_id, &path, AttachmentOrigin::FilePicker);
        }
    }

    fn paste_clipboard_image(&mut self, task_id: &TaskId) {
        let result = (|| {
            self.image_attachment_capability(task_id)?;
            let mut clipboard = arboard::Clipboard::new()
                .map_err(|error| format!("clipboard is unavailable: {error}"))?;
            let image = clipboard
                .get_image()
                .map_err(|error| format!("clipboard has no readable image: {error}"))?;
            let id = self.next_attachment_id("paste");
            let attachment = self.attachment_store.insert_rgba(
                task_id,
                id.clone(),
                "clipboard.png".into(),
                AttachmentOrigin::Clipboard,
                image.width,
                image.height,
                image.bytes.as_ref(),
            )?;
            let attach = self
                .state
                .session
                .task_mut(task_id)
                .and_then(|task| task.set_vision_capability(true))
                .and_then(|()| {
                    self.state.session.task_mut(task_id).and_then(|task| {
                        task.attach_screenshot_with_sha256(
                            id.as_str(),
                            attachment.path.to_string_lossy().into_owned(),
                            attachment.sha256.clone(),
                        )
                    })
                })
                .map_err(|error| error.to_string());
            if attach.is_err() {
                self.attachment_store.remove(task_id, &id);
            }
            attach
        })();
        self.state.notice = Some(match result {
            Ok(()) => "Pasted clipboard image. Select Include once before sending pixels.".into(),
            Err(error) => format!("Could not paste image: {error}"),
        });
    }

    fn process_dropped_images(&mut self, context: &egui::Context) {
        let dropped = context.input(|input| input.raw.dropped_files.clone());
        if dropped.is_empty() {
            return;
        }
        let Some(task_id) = self.state.session.active_task_id().cloned() else {
            self.state.notice = Some("Create a task before dropping an image.".into());
            return;
        };
        for file in dropped.into_iter().take(8) {
            if let Some(path) = file.path {
                self.attach_file_path(&task_id, &path, AttachmentOrigin::FileDrop);
            } else if let Some(bytes) = file.bytes {
                let id = self.next_attachment_id("drop");
                let name = if file.name.is_empty() {
                    "dropped-image".into()
                } else {
                    file.name
                };
                let result = self.attach_encoded_image(
                    &task_id,
                    id,
                    name,
                    AttachmentOrigin::FileDrop,
                    bytes.as_ref(),
                );
                self.state.notice = Some(match result {
                    Ok(()) => {
                        "Attached dropped image. Select Include once before sending pixels.".into()
                    }
                    Err(error) => format!("Could not attach dropped image: {error}"),
                });
            }
        }
    }

    fn start_capture(&mut self, task_id: TaskId) {
        if self.capture.is_some() {
            self.state.notice = Some("A game screenshot capture is already in progress.".into());
            return;
        }
        if self.state.session.active_task_id() != Some(&task_id) {
            self.state.notice = Some("Ignored screenshot request from an inactive task.".into());
            return;
        }
        if let Err(error) = self.image_attachment_capability(&task_id) {
            self.state.notice = Some(format!("Cannot attach screenshot: {error}."));
            return;
        }
        if let Err(error) = self
            .state
            .session
            .task_mut(&task_id)
            .and_then(|task| task.set_vision_capability(true))
        {
            self.state.notice = Some(error.to_string());
            return;
        }

        let sequence = self.next_capture;
        self.next_capture = self.next_capture.saturating_add(1);
        let screenshot_id = format!("game-{sequence}");
        let artifact = format!("editor-{}-{sequence}", task_id.as_str());
        let request_id = 10_000_u64.saturating_add(sequence);
        let canceled = Arc::new(AtomicBool::new(false));
        let worker_canceled = Arc::clone(&canceled);
        let worker_task = task_id.clone();
        let worker_screenshot = screenshot_id.clone();
        let worker_client = self.client.clone();
        let result_tx = self.capture_result_tx.clone();
        std::thread::spawn(move || {
            let result = capture_frame(
                &worker_client,
                request_id,
                artifact,
                &worker_canceled,
                CAPTURE_DEADLINE,
            );
            let _ = result_tx.send(CaptureResult {
                task_id: worker_task,
                screenshot_id: worker_screenshot,
                request_id,
                result,
            });
        });
        self.capture = Some(PendingCapture {
            task_id,
            screenshot_id,
            request_id,
            canceled,
        });
        self.state.notice = Some("Capturing the next presented game frame...".into());
    }

    fn cancel_capture_for(&mut self, task_id: &TaskId) -> bool {
        if self
            .capture
            .as_ref()
            .is_some_and(|capture| &capture.task_id == task_id)
        {
            if let Some(capture) = self.capture.take() {
                capture.canceled.store(true, Ordering::Release);
                self.state.notice = Some(format!("Screenshot capture canceled for {task_id}."));
                return true;
            }
        }
        false
    }

    fn poll_capture(&mut self) {
        loop {
            let completed = match self.capture_results.try_recv() {
                Ok(completed) => completed,
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            };
            let is_current = self.capture.as_ref().is_some_and(|pending| {
                pending.task_id == completed.task_id
                    && pending.screenshot_id == completed.screenshot_id
                    && pending.request_id == completed.request_id
            });
            if !is_current {
                continue;
            }
            self.capture = None;
            if self.state.session.active_task_id() != Some(&completed.task_id) {
                self.state.notice = Some(format!(
                    "Ignored obsolete screenshot for inactive task {}.",
                    completed.task_id
                ));
                continue;
            }
            match completed.result {
                Ok(evidence) => {
                    match screenshot_preview(
                        &completed.task_id,
                        &completed.screenshot_id,
                        &evidence,
                    ) {
                        Ok(preview) => {
                            let original = self.attachment_store.insert_encoded(
                                &completed.task_id,
                                completed.screenshot_id.clone(),
                                "game-frame.png".into(),
                                AttachmentOrigin::GameCapture,
                                &evidence.bytes,
                            );
                            let _ = std::fs::remove_file(&evidence.path);
                            let result = original.and_then(|owned| {
                                if owned.sha256 != preview.sha256 {
                                    self.attachment_store
                                        .remove(&completed.task_id, &completed.screenshot_id);
                                    return Err(
                                        "captured frame changed before it could be copied".into()
                                    );
                                }
                                self.state
                                    .session
                                    .task_mut(&completed.task_id)
                                    .and_then(|task| {
                                        task.attach_screenshot_with_sha256(
                                            completed.screenshot_id.as_str(),
                                            owned.path.to_string_lossy().into_owned(),
                                            owned.sha256,
                                        )
                                    })
                                    .map_err(|error| error.to_string())
                            });
                            match result {
                                Ok(()) => {
                                    self.state.notice = Some(format!(
                                        "Captured {}x{} game frame for {}. Select Include once before sending pixels.",
                                        preview.width, preview.height, completed.task_id
                                    ));
                                    self.state.preview = Some(preview);
                                    self.preview_texture = None;
                                }
                                Err(error) => {
                                    self.attachment_store
                                        .remove(&completed.task_id, &completed.screenshot_id);
                                    self.state.notice = Some(error);
                                }
                            }
                        }
                        Err(error) => self.state.notice = Some(error),
                    }
                }
                Err(error) => self.state.notice = Some(format!("Screenshot failed: {error}")),
            }
        }
    }

    fn poll_controller(&mut self) {
        for event in self.controller.poll(&mut self.state.session) {
            self.state.notice = match event {
                TaskControllerEvent::Completed {
                    task_id, proposals, ..
                } => Some(format!(
                    "AI reply completed for {task_id}; {} action(s) proposed",
                    proposals.len()
                )),
                TaskControllerEvent::Failed {
                    task_id, message, ..
                } => Some(format!("AI reply failed for {task_id}: {message}")),
                TaskControllerEvent::Canceled { task_id, .. } => {
                    Some(format!("AI reply canceled for {task_id}"))
                }
                TaskControllerEvent::Stale { task_id, .. } => {
                    Some(format!("Ignored an obsolete AI reply for {task_id}"))
                }
            };
        }
    }

    fn poll_host(&mut self) {
        for completed in self.host.poll() {
            let task_id = completed.task_id;
            self.busy_tasks.remove(&task_id);
            let task = self.state.session.task_mut(task_id.as_str());
            match (completed.operation, completed.result, task) {
                (
                    HostOperation::Apply { action_id, .. },
                    Ok((summary, receipt, fingerprint)),
                    Ok(task),
                ) => {
                    let validated = receipt_has_test_evidence(&receipt);
                    let receipt_summary = receipt
                        .get("receipt")
                        .and_then(Value::as_str)
                        .unwrap_or("recorded validation receipt")
                        .to_string();
                    self.execution_receipts
                        .insert((task_id.clone(), action_id.clone()), receipt);
                    if task.lifecycle == stasis_ai::TaskLifecycle::Active {
                        let result = task.apply_action(action_id.as_str()).and_then(|()| {
                            if validated {
                                task.begin_focused_tests()?;
                                task.finish_focused_tests(stasis_ai::FocusedTestResult::passed(
                                    summary.clone(),
                                ))?;
                            }
                            task.append_host_result(format!(
                                "Applied {action_id}: {summary}\nValidation receipt: {receipt_summary}"
                            ))?;
                            Ok(())
                        });
                        if let Err(error) = result {
                            self.state.notice = Some(error.to_string());
                            continue;
                        }
                        if validated {
                            self.validation_fingerprints
                                .insert(task_id.clone(), (fingerprint, Vec::new()));
                        }
                        self.state.notice = Some(format!("Applied {action_id} for {task_id}"));
                    } else if let Some(action) = task.actions.get_mut(action_id.as_str()) {
                        action.state = ActionState::Applied;
                        self.state.notice = Some(format!(
                            "{action_id} committed before {task_id} cancellation completed"
                        ));
                    }
                }
                (HostOperation::Apply { action_id, .. }, Err(error), Ok(task)) => {
                    let repair_reason = bounded_failure(&error, 900);
                    if task.lifecycle == stasis_ai::TaskLifecycle::Active {
                        let _ =
                            task.mark_action_for_repair(action_id.as_str(), repair_reason.clone());
                        let _ = task.append_host_result(format!(
                            "Could not apply {action_id}: {repair_reason}. Repair this proposal and retry."
                        ));
                    }
                    self.state.notice = Some(format!("Apply failed for {task_id}: {error}"));
                }
                (
                    HostOperation::Test { paths, run_id },
                    Ok((summary, receipt, fingerprint)),
                    Ok(task),
                ) => {
                    let result = task
                        .finish_focused_test_run(
                            run_id,
                            stasis_ai::FocusedTestResult::passed(summary.clone()),
                        )
                        .and_then(|()| {
                            task.append_host_result(format!(
                                "Focused tests passed: {summary}. Validation receipt recorded."
                            ))
                        });
                    if let Err(error) = result {
                        self.state.notice = Some(error.to_string());
                        continue;
                    }
                    self.validation_receipts.insert(task_id.clone(), receipt);
                    self.validation_fingerprints
                        .insert(task_id.clone(), (fingerprint, paths));
                    self.state.notice = Some(format!("Focused tests passed for {task_id}"));
                }
                (HostOperation::Test { run_id, .. }, Err(error), Ok(task)) => {
                    if task.lifecycle == stasis_ai::TaskLifecycle::Active
                        && record_focused_test_failure(task, run_id, &error).is_err()
                    {
                        self.state.notice = Some(format!(
                            "Ignored obsolete focused-test result for {task_id}"
                        ));
                        continue;
                    }
                    self.state.notice =
                        Some(format!("Focused tests failed for {task_id}: {error}"));
                }
                (_, _, Err(error)) => self.state.notice = Some(error.to_string()),
            }
        }
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.label(
            RichText::new("STASIS")
                .size(11.0)
                .strong()
                .color(muted_text()),
        );
        ui.label(
            RichText::new(project_name(&self.project_root))
                .size(17.0)
                .strong(),
        );
        ui.label(
            RichText::new("Stasis project")
                .size(12.0)
                .color(muted_text()),
        );
        ui.add_space(16.0);
        ui.label(
            RichText::new("NEW TASK")
                .size(10.0)
                .strong()
                .color(muted_text()),
        );
        let objective = ui.add_sized(
            [ui.available_width(), 34.0],
            egui::TextEdit::singleline(&mut self.state.objective).hint_text("What should change?"),
        );
        if self.state.focus == FocusArea::Tasks && self.state.focus_pending {
            objective.request_focus();
            self.state.focus_pending = false;
        }
        let submitted =
            objective.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        let create = ui.add_sized(
            [ui.available_width(), 34.0],
            egui::Button::new(RichText::new("+  Create task").strong()),
        );
        if create.clicked() || submitted {
            self.state.notice = self.state.create_task().err();
        }
        ui.add_space(18.0);
        let active = self.state.session.active_task_id().map(|id| id.to_string());
        let cards = self
            .state
            .session
            .tasks()
            .map(|task| {
                (
                    task.id.to_string(),
                    task.objective.clone(),
                    task.lifecycle,
                    task.connection,
                    task.metrics.elapsed_ms,
                    task.metrics.estimated_cost_micros,
                    task.metrics.retry_count,
                )
            })
            .collect::<Vec<_>>();
        let queued = cards.len().saturating_sub(usize::from(active.is_some()));
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("TASKS")
                    .size(10.0)
                    .strong()
                    .color(muted_text()),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(queued.to_string())
                        .size(11.0)
                        .color(muted_text()),
                );
            });
        });
        ui.add_space(4.0);
        for (id, objective, lifecycle, connection, _elapsed, _cost, _retries) in cards {
            let request = self.controller.snapshot(&TaskId::new(&id));
            let request_status = request
                .as_ref()
                .map(|snapshot| format!("{:?}", snapshot.state).to_ascii_lowercase());
            let selected = active.as_deref() == Some(&id);
            let state = request_status.unwrap_or_else(|| {
                if !selected && lifecycle == TaskLifecycle::Active {
                    "queued".into()
                } else {
                    task_state_label(lifecycle, connection)
                }
            });
            let fill = if selected {
                selected_fill()
            } else {
                panel_fill()
            };
            let response = egui::Frame::none()
                .fill(fill)
                .stroke(egui::Stroke::new(
                    1.0_f32,
                    if selected { accent() } else { border() },
                ))
                .rounding(7.0)
                .inner_margin(egui::Margin::symmetric(10.0, 9.0))
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.label(RichText::new(&objective).size(13.0).strong());
                    ui.label(RichText::new(state).size(11.0).color(if selected {
                        accent()
                    } else {
                        muted_text()
                    }));
                })
                .response
                .interact(egui::Sense::click());
            if response.clicked() {
                self.state.notice = self.state.switch_task(&id).err().map(|e| e.to_string());
            }
            response.on_hover_text(format!("Switch to {objective}"));
            ui.add_space(6.0);
        }
        ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
            ui.label(
                RichText::new("Ctrl+K  Command palette")
                    .size(11.0)
                    .color(muted_text()),
            );
        });
    }
}

fn project_name(root: &std::path::Path) -> String {
    root.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("Stasis project")
        .to_string()
}

fn task_state_label(lifecycle: TaskLifecycle, connection: ConnectionState) -> String {
    if connection == ConnectionState::Disconnected {
        return "disconnected".into();
    }
    match lifecycle {
        TaskLifecycle::Active => "current".into(),
        TaskLifecycle::Canceled => "canceled".into(),
        TaskLifecycle::Completed => "done".into(),
    }
}

fn canvas_fill() -> Color32 {
    Color32::from_rgb(11, 16, 23)
}
fn rail_fill() -> Color32 {
    Color32::from_rgb(9, 14, 20)
}
fn panel_fill() -> Color32 {
    Color32::from_rgb(17, 24, 34)
}
fn raised_fill() -> Color32 {
    Color32::from_rgb(21, 30, 42)
}
fn selected_fill() -> Color32 {
    Color32::from_rgb(21, 38, 55)
}
fn border() -> Color32 {
    Color32::from_rgb(38, 49, 63)
}
fn muted_text() -> Color32 {
    Color32::from_rgb(146, 158, 174)
}
fn accent() -> Color32 {
    Color32::from_rgb(77, 226, 164)
}
fn warning() -> Color32 {
    Color32::from_rgb(245, 183, 78)
}
fn failure() -> Color32 {
    Color32::from_rgb(242, 112, 118)
}

fn configure_visuals(context: &egui::Context) {
    let mut style = (*context.style()).clone();
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(12.0, 7.0);
    style.spacing.interact_size.y = 30.0;
    style.visuals = egui::Visuals::dark();
    style.visuals.panel_fill = canvas_fill();
    style.visuals.window_fill = panel_fill();
    style.visuals.extreme_bg_color = Color32::from_rgb(7, 11, 17);
    style.visuals.faint_bg_color = raised_fill();
    style.visuals.widgets.inactive.bg_fill = raised_fill();
    style.visuals.widgets.inactive.weak_bg_fill = raised_fill();
    style.visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0_f32, border());
    style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(29, 42, 56);
    style.visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(29, 42, 56);
    style.visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0_f32, accent());
    style.visuals.widgets.active.bg_fill = selected_fill();
    style.visuals.widgets.active.weak_bg_fill = selected_fill();
    style.visuals.selection.bg_fill = Color32::from_rgb(31, 100, 82);
    context.set_style(style);
}

fn status_chip(ui: &mut egui::Ui, label: &str, color: Color32) {
    egui::Frame::none()
        .fill(color.gamma_multiply(0.16))
        .stroke(egui::Stroke::new(1.0_f32, color.gamma_multiply(0.6)))
        .rounding(12.0)
        .inner_margin(egui::Margin::symmetric(8.0, 3.0))
        .show(ui, |ui| {
            ui.label(RichText::new(label).size(10.0).strong().color(color));
        });
}

fn validation_label_ui(status: &ValidationStatus) -> &'static str {
    match status {
        ValidationStatus::NotRun => "not tested",
        ValidationStatus::Running => "testing",
        ValidationStatus::Passed { .. } => "passed",
        ValidationStatus::Failed { .. } => "failed",
    }
}

fn validation_color(status: &ValidationStatus) -> Color32 {
    match status {
        ValidationStatus::Passed { .. } => accent(),
        ValidationStatus::Failed { .. } => failure(),
        ValidationStatus::Running => warning(),
        ValidationStatus::NotRun => muted_text(),
    }
}

fn task_header_status(task: &stasis_ai::Task) -> (&'static str, Color32) {
    match task.lifecycle {
        TaskLifecycle::Canceled => return ("canceled", failure()),
        TaskLifecycle::Completed => return ("done", accent()),
        TaskLifecycle::Active => {}
    }
    if task.connection == ConnectionState::Disconnected {
        return ("disconnected", failure());
    }
    if task.validation.is_running() {
        return ("testing", warning());
    }
    if task
        .actions
        .values()
        .any(|action| matches!(action.state, ActionState::NeedsRepair { .. }))
    {
        return ("needs repair", failure());
    }
    (
        validation_label_ui(&task.validation),
        validation_color(&task.validation),
    )
}

fn status_color(task: &stasis_ai::Task) -> Color32 {
    task_header_status(task).1
}

fn action_state_label(state: &ActionState) -> &'static str {
    match state {
        ActionState::Proposed => "proposed",
        ActionState::Accepted => "accepted",
        ActionState::Applied => "applied",
        ActionState::Rejected { .. } => "rejected",
        ActionState::NeedsRepair { .. } => "needs repair",
    }
}

fn action_state_color(state: &ActionState) -> Color32 {
    match state {
        ActionState::Applied => accent(),
        ActionState::Rejected { .. } | ActionState::NeedsRepair { .. } => failure(),
        ActionState::Accepted => Color32::from_rgb(104, 185, 255),
        ActionState::Proposed => warning(),
    }
}

fn status_for_upload(status: &UploadState) -> Color32 {
    match status {
        UploadState::Uploaded => accent(),
        UploadState::Failed { .. } => failure(),
        UploadState::Pending => warning(),
    }
}

fn status_for_analysis(status: &ScreenshotAnalysisState) -> Color32 {
    match status {
        ScreenshotAnalysisState::Completed => accent(),
        ScreenshotAnalysisState::Failed { .. } => failure(),
        ScreenshotAnalysisState::Pending => warning(),
        ScreenshotAnalysisState::Canceled => muted_text(),
    }
}

fn image_review_color(status: &ImageReviewState) -> Color32 {
    match status {
        ImageReviewState::Approved => accent(),
        ImageReviewState::Rejected { .. } => failure(),
        ImageReviewState::Pending => warning(),
    }
}

fn image_handoff_color(status: &ImageHandoffState) -> Color32 {
    match status {
        ImageHandoffState::Imported => accent(),
        ImageHandoffState::Rejected { .. } => failure(),
        ImageHandoffState::Pending => warning(),
    }
}

fn render_compiler_changes(ui: &mut egui::Ui, receipt: &Value) {
    let Some(changes) = receipt
        .pointer("/plan/changed_files")
        .and_then(Value::as_array)
    else {
        return;
    };
    ui.add_space(7.0);
    ui.label(
        RichText::new(format!(
            "Compiler plan / {} changed file{}",
            changes.len(),
            if changes.len() == 1 { "" } else { "s" }
        ))
        .size(12.0)
        .strong()
        .color(accent()),
    );
    for change in changes {
        let file = change
            .get("file")
            .and_then(Value::as_str)
            .unwrap_or("source");
        let before = change.get("before_source").and_then(Value::as_str);
        let after = change.get("after_source").and_then(Value::as_str);
        ui.collapsing(file, |ui| {
            if let (Some(before), Some(after)) = (before, after) {
                egui::ScrollArea::horizontal()
                    .id_source(("compiler-diff", file))
                    .show(ui, |ui| {
                        ui.label(RichText::new("Before").size(10.0).strong().color(failure()));
                        render_numbered_source(ui, before, failure());
                        ui.label(RichText::new("After").size(10.0).strong().color(accent()));
                        render_numbered_source(ui, after, accent());
                    });
            } else {
                ui.label(
                    RichText::new("Exact source is unavailable in this receipt.")
                        .size(11.0)
                        .color(muted_text()),
                );
            }
        });
    }
}

fn render_numbered_source(ui: &mut egui::Ui, source: &str, color: Color32) {
    let text = source
        .lines()
        .enumerate()
        .map(|(line, text)| format!("{:>4}  {text}", line + 1))
        .collect::<Vec<_>>()
        .join("\n");
    egui::Frame::none()
        .fill(color.gamma_multiply(0.08))
        .inner_margin(egui::Margin::same(7.0))
        .show(ui, |ui| {
            ui.add(egui::Label::new(RichText::new(text).monospace().size(11.0)).selectable(true));
        });
}

#[derive(Debug)]
struct EditorState {
    session: TaskSession,
    shortcuts: ShortcutMapper,
    focus: FocusArea,
    focus_pending: bool,
    objective: String,
    reply: String,
    palette_query: String,
    palette_open: bool,
    palette_selected: usize,
    palette_return_focus: FocusArea,
    drafts: BTreeMap<String, (String, String)>,
    next_task: u64,
    intents: Vec<EditorIntent>,
    notice: Option<String>,
    preview: Option<ScreenshotPreview>,
    cancel_confirmation: Option<String>,
    project_root: Option<PathBuf>,
    semantic_previews: BTreeMap<SemanticPreviewKey, SemanticPreviewRecord>,
}

impl Default for EditorState {
    fn default() -> Self {
        Self {
            session: TaskSession::new(),
            shortcuts: ShortcutMapper::new(),
            focus: FocusArea::Tasks,
            focus_pending: true,
            objective: String::new(),
            reply: String::new(),
            palette_query: String::new(),
            palette_open: false,
            palette_selected: 0,
            palette_return_focus: FocusArea::Tasks,
            drafts: BTreeMap::new(),
            next_task: 1,
            intents: Vec::new(),
            notice: None,
            preview: None,
            cancel_confirmation: None,
            project_root: None,
            semantic_previews: BTreeMap::new(),
        }
    }
}

impl EditorState {
    fn reviewed_preview(
        &self,
        task: &str,
        action: &str,
    ) -> Result<&super::DesktopSemanticPreview, String> {
        self.check_preview(task, action, true)
    }

    fn check_preview(
        &self,
        task: &str,
        action: &str,
        read_sources: bool,
    ) -> Result<&super::DesktopSemanticPreview, String> {
        let action = self
            .session
            .task(task)
            .map_err(|e| e.to_string())?
            .actions
            .get(action)
            .ok_or_else(|| "Action no longer exists".to_string())?;
        let payload = action
            .payload
            .as_ref()
            .ok_or_else(|| "Action has no semantic payload".to_string())?;
        let key = SemanticPreviewKey::new(
            task,
            action.id.as_str(),
            proposal_revisions(action).len() - 1,
            payload,
        );
        let record = self
            .semantic_previews
            .get(&key)
            .ok_or_else(|| "Review the semantic preview first".to_string())?;
        let preview = record
            .result
            .as_ref()
            .ok_or_else(|| "Semantic preview is still loading".to_string())?
            .as_ref()
            .map_err(Clone::clone)?;
        let root = self
            .project_root
            .as_ref()
            .ok_or_else(|| "Project is unavailable".to_string())?;
        if record.stale
            || preview.payload != *payload
            || (read_sources
                && super::desktop_source_fingerprint(root, &[])? != preview.source_fingerprint)
        {
            return Err(
                "Stale preview: sources changed. Request a new proposal for the current sources."
                    .into(),
            );
        }
        if preview.plan.changed_files.is_empty() {
            return Err("This revision has no source changes".into());
        }
        Ok(preview)
    }

    fn review_command_enabled(&self, command: &TaskSessionCommand) -> bool {
        let accepted = match command {
            TaskSessionCommand::AcceptAction => false,
            TaskSessionCommand::ApplyAction => true,
            _ => return true,
        };
        let Ok(task) = self.session.active_task() else {
            return false;
        };
        task.actions
            .values()
            .find(|action| {
                if accepted {
                    matches!(action.state, ActionState::Accepted)
                } else {
                    matches!(action.state, ActionState::Proposed)
                }
            })
            .is_some_and(|action| {
                self.check_preview(task.id.as_str(), action.id.as_str(), false)
                    .is_ok()
            })
    }

    fn primary_action(&self, busy: bool) -> PrimaryAction {
        let Ok(task) = self.session.active_task() else {
            return PrimaryAction {
                label: "Create task",
                command: TaskSessionCommand::NewTask,
                enabled: !self.objective.trim().is_empty(),
                disabled_reason: Some("Enter a task objective in the project rail.".into()),
            };
        };
        if task.connection == ConnectionState::Disconnected {
            return PrimaryAction {
                label: "Reconnect",
                command: TaskSessionCommand::Reconnect,
                enabled: task.lifecycle == TaskLifecycle::Active,
                disabled_reason: Some("This task is closed.".into()),
            };
        }
        if task.lifecycle != TaskLifecycle::Active {
            return PrimaryAction {
                label: "Task closed",
                command: TaskSessionCommand::MarkDone,
                enabled: false,
                disabled_reason: Some(
                    match task.lifecycle {
                        TaskLifecycle::Canceled => "Canceled tasks are read-only.",
                        TaskLifecycle::Completed => "This task is complete.",
                        TaskLifecycle::Active => unreachable!(),
                    }
                    .into(),
                ),
            };
        }
        if busy {
            return PrimaryAction {
                label: "Cancel task",
                command: TaskSessionCommand::Cancel,
                enabled: true,
                disabled_reason: None,
            };
        }
        if task
            .actions
            .values()
            .any(|action| matches!(action.state, ActionState::Proposed))
        {
            return PrimaryAction {
                label: "Accept proposal",
                command: TaskSessionCommand::AcceptAction,
                enabled: true,
                disabled_reason: None,
            };
        }
        if task
            .actions
            .values()
            .any(|action| matches!(action.state, ActionState::Accepted))
        {
            return PrimaryAction {
                label: "Apply change",
                command: TaskSessionCommand::ApplyAction,
                enabled: true,
                disabled_reason: None,
            };
        }
        if task
            .actions
            .values()
            .any(|action| matches!(action.state, ActionState::NeedsRepair { .. }))
        {
            let enabled = !self.reply.trim().is_empty();
            return PrimaryAction {
                label: "Request repair",
                command: TaskSessionCommand::SendReply,
                enabled,
                disabled_reason: (!enabled).then(|| "Describe the repair before sending.".into()),
            };
        }
        if task.validation.is_running() {
            return PrimaryAction {
                label: "Testing...",
                command: TaskSessionCommand::RunFocusedTests,
                enabled: false,
                disabled_reason: Some("Focused tests are already running.".into()),
            };
        }
        let has_applied_change = task
            .actions
            .values()
            .any(|action| matches!(action.state, ActionState::Applied));
        if has_applied_change && !task.validation.is_passing() {
            return PrimaryAction {
                label: "Run focused tests",
                command: TaskSessionCommand::RunFocusedTests,
                enabled: true,
                disabled_reason: None,
            };
        }
        if task.validation.is_passing()
            && task.actions.values().all(|action| !action.is_pending())
            && task.pending_generated_images().next().is_none()
            && !task.screenshots.values().any(|screenshot| {
                matches!(
                    screenshot.upload,
                    UploadState::Pending | UploadState::Failed { .. }
                )
            })
        {
            return PrimaryAction {
                label: "Mark done",
                command: TaskSessionCommand::MarkDone,
                enabled: true,
                disabled_reason: None,
            };
        }
        let enabled = !self.reply.trim().is_empty();
        PrimaryAction {
            label: "Send to AI",
            command: TaskSessionCommand::SendReply,
            enabled,
            disabled_reason: (!enabled).then(|| "Write a task-scoped message first.".into()),
        }
    }

    fn active_id(&self) -> Result<String, String> {
        self.session
            .active_task_id()
            .map(ToString::to_string)
            .ok_or_else(|| format!("{EMPTY_TASK} first (Ctrl+N)."))
    }

    fn create_task(&mut self) -> Result<(), String> {
        let objective = self.objective.trim().to_string();
        if objective.is_empty() {
            self.focus = FocusArea::Tasks;
            self.focus_pending = true;
            return Err("Enter a task objective first.".into());
        }
        let id = format!("task-{}", self.next_task);
        let previous = self.session.active_task_id().map(ToString::to_string);
        self.session
            .new_task(
                id.as_str(),
                &objective,
                "Stasis project; fresh task-scoped context",
            )
            .map_err(|e| e.to_string())?;
        self.next_task = self.next_task.saturating_add(1);
        self.objective.clear();
        if let Some(previous) = previous {
            self.drafts
                .insert(previous, (String::new(), std::mem::take(&mut self.reply)));
        } else {
            self.reply.clear();
        }
        self.focus = FocusArea::Reply;
        self.focus_pending = true;
        Ok(())
    }

    fn switch_relative(&mut self, offset: isize) -> Result<(), String> {
        let ids = self
            .session
            .tasks()
            .map(|t| t.id.to_string())
            .collect::<Vec<_>>();
        if ids.is_empty() {
            return Err("No tasks to switch.".into());
        }
        let current = self
            .session
            .active_task_id()
            .and_then(|id| ids.iter().position(|value| value == id.as_str()))
            .unwrap_or(0);
        let next = (current as isize + offset).rem_euclid(ids.len() as isize) as usize;
        self.switch_task(&ids[next])
    }

    fn switch_task(&mut self, id: &str) -> Result<(), String> {
        let previous = self.active_id()?;
        self.session.switch_task(id).map_err(|e| e.to_string())?;
        self.drafts.insert(
            previous,
            (
                std::mem::take(&mut self.objective),
                std::mem::take(&mut self.reply),
            ),
        );
        (self.objective, self.reply) = self.drafts.remove(id).unwrap_or_default();
        Ok(())
    }

    fn close_palette(&mut self) {
        self.palette_open = false;
        self.palette_query.clear();
        self.palette_selected = 0;
        self.focus = self.palette_return_focus;
    }

    fn first_action(&self, predicate: impl Fn(&ActionState) -> bool) -> Option<String> {
        self.session
            .active_task()
            .ok()?
            .actions
            .values()
            .find(|action| predicate(&action.state))
            .map(|action| action.id.to_string())
    }

    fn dispatch(&mut self, command: TaskSessionCommand) {
        self.notice = self.handle(command).err();
    }

    fn handle(&mut self, command: TaskSessionCommand) -> Result<(), String> {
        match command {
            TaskSessionCommand::OpenCommandPalette | TaskSessionCommand::Search => {
                if !self.palette_open {
                    self.palette_return_focus = self.focus;
                    self.palette_selected = 0;
                }
                self.palette_open = true;
                self.focus = FocusArea::Palette;
                Ok(())
            }
            TaskSessionCommand::NewTask => {
                self.focus = FocusArea::Tasks;
                self.focus_pending = true;
                if self.objective.trim().is_empty() {
                    Ok(())
                } else {
                    self.create_task()
                }
            }
            TaskSessionCommand::SwitchNextTask => self.switch_relative(1),
            TaskSessionCommand::SwitchPreviousTask => self.switch_relative(-1),
            TaskSessionCommand::SwitchTask(slot) => {
                let id = self
                    .session
                    .tasks()
                    .nth(usize::from(slot.saturating_sub(1)))
                    .map(|task| task.id.to_string())
                    .ok_or_else(|| format!("Task slot {slot} is empty."))?;
                self.switch_task(&id)
            }
            TaskSessionCommand::FocusReply => {
                self.active_id()?;
                self.focus = FocusArea::Reply;
                self.focus_pending = true;
                Ok(())
            }
            TaskSessionCommand::FocusGame => {
                self.focus = FocusArea::Game;
                Ok(())
            }
            TaskSessionCommand::SendReply => {
                let task = self.active_id()?;
                let text = self.reply.trim().to_string();
                if text.is_empty() {
                    return Err("Reply is empty.".into());
                }
                self.intents.push(EditorIntent::SendReply(task, text));
                self.reply.clear();
                Ok(())
            }
            TaskSessionCommand::AcceptAction => {
                let id = self
                    .first_action(|s| matches!(s, ActionState::Proposed))
                    .ok_or_else(|| "No proposed action to accept.".to_string())?;
                if self
                    .session
                    .active_task()
                    .map_err(|e| e.to_string())?
                    .actions[id.as_str()]
                .payload
                .is_some()
                {
                    self.reviewed_preview(&self.active_id()?, &id)?;
                }
                self.session.accept_action(id).map_err(|e| e.to_string())
            }
            TaskSessionCommand::RejectAction => {
                let id = self
                    .first_action(|s| {
                        matches!(s, ActionState::Proposed | ActionState::NeedsRepair { .. })
                    })
                    .ok_or_else(|| "No action to reject.".to_string())?;
                self.session
                    .reject_action(id, "Rejected in desktop editor")
                    .map_err(|e| e.to_string())
            }
            TaskSessionCommand::ApplyAction => {
                let task = self.active_id()?;
                let action = self
                    .first_action(|s| matches!(s, ActionState::Accepted))
                    .ok_or_else(|| "Accept an action before applying it.".to_string())?;
                if self
                    .session
                    .active_task()
                    .map_err(|e| e.to_string())?
                    .actions[action.as_str()]
                .payload
                .is_some()
                {
                    self.reviewed_preview(&task, &action)?;
                }
                self.intents.push(EditorIntent::Apply(task, action));
                Ok(())
            }
            TaskSessionCommand::RunFocusedTests => {
                let task = self.active_id()?;
                self.session
                    .begin_focused_tests()
                    .map_err(|e| e.to_string())?;
                let run_id = self
                    .session
                    .active_task()
                    .map_err(|error| error.to_string())?
                    .validation_run_id;
                self.intents.push(EditorIntent::Test(task, run_id));
                Ok(())
            }
            TaskSessionCommand::Retry => {
                let task = self.active_id()?;
                self.intents.push(EditorIntent::Retry(task));
                Ok(())
            }
            TaskSessionCommand::AttachScreenshot => {
                let task = self.active_id()?;
                self.intents.push(EditorIntent::Screenshot(task));
                Ok(())
            }
            TaskSessionCommand::GenerateImage => {
                let task = self.active_id()?;
                self.intents.push(EditorIntent::GenerateImage(task));
                Ok(())
            }
            TaskSessionCommand::ImportGeneratedImage => {
                let task = self.active_id()?;
                let image = self
                    .session
                    .active_task()
                    .ok()
                    .and_then(|value| value.pending_generated_images().next())
                    .map(|value| value.id.to_string())
                    .ok_or_else(|| "No generated image is ready to import.".to_string())?;
                self.intents.push(EditorIntent::ImportImage(task, image));
                Ok(())
            }
            TaskSessionCommand::MarkDone => {
                let task = self.active_id()?;
                self.intents.push(EditorIntent::MarkDone(task));
                Ok(())
            }
            TaskSessionCommand::Cancel => {
                let task = self.active_id()?;
                self.cancel_confirmation = Some(task);
                Ok(())
            }
            TaskSessionCommand::Reconnect => {
                let task = self.active_id()?;
                self.intents.push(EditorIntent::Reconnect(task));
                Ok(())
            }
        }
    }

    fn chord(event: &egui::Event) -> Option<KeyChord> {
        let egui::Event::Key {
            key,
            pressed: true,
            modifiers,
            repeat: false,
            ..
        } = event
        else {
            return None;
        };
        let key = match key {
            egui::Key::Enter => Key::Enter,
            egui::Key::Escape => Key::Escape,
            egui::Key::Tab => Key::Tab,
            egui::Key::Backspace => Key::Backspace,
            egui::Key::ArrowUp => Key::Up,
            egui::Key::ArrowDown => Key::Down,
            egui::Key::ArrowLeft => Key::Left,
            egui::Key::ArrowRight => Key::Right,
            egui::Key::Num1 => Key::Digit(1),
            egui::Key::Num2 => Key::Digit(2),
            egui::Key::Num3 => Key::Digit(3),
            egui::Key::Num4 => Key::Digit(4),
            egui::Key::Num5 => Key::Digit(5),
            egui::Key::Num6 => Key::Digit(6),
            egui::Key::Num7 => Key::Digit(7),
            egui::Key::Num8 => Key::Digit(8),
            egui::Key::Num9 => Key::Digit(9),
            other => {
                let mut name = other.name().chars();
                let character = name.next()?;
                if name.next().is_some() {
                    return None;
                }
                Key::Char(character.to_ascii_lowercase())
            }
        };
        Some(KeyChord::new(
            Modifiers {
                ctrl: modifiers.ctrl,
                alt: modifiers.alt,
                shift: modifiers.shift,
                meta: modifiers.mac_cmd,
            },
            key,
        ))
    }
}

impl DesktopEditor {
    fn ui_busy(&self, task: &stasis_ai::Task) -> bool {
        self.busy_tasks.contains(task.id.as_str())
            || self
                .controller
                .snapshot(&task.id)
                .is_some_and(|request| request.state == stasis_ai::TaskRequestState::Running)
            || self
                .capture
                .as_ref()
                .is_some_and(|capture| capture.task_id == task.id)
    }

    fn detail(&mut self, ui: &mut egui::Ui) {
        let Ok(task) = self.state.session.active_task() else {
            egui::Frame::none().inner_margin(32.0).show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(80.0);
                    ui.label(RichText::new("Start a focused change").size(24.0).strong());
                    ui.label(RichText::new("Create a task in the project rail. Each task keeps its own requests, evidence, edits, and validation.").size(14.0).color(muted_text()));
                });
            });
            return;
        };
        let task = task.clone();
        egui::TopBottomPanel::bottom("task-composer")
            .resizable(false)
            .frame(
                egui::Frame::none()
                    .fill(canvas_fill())
                    .inner_margin(egui::Margin::symmetric(18.0, 12.0)),
            )
            .show_inside(ui, |ui| self.composer(ui, &task));
        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(canvas_fill())
                    .inner_margin(egui::Margin::symmetric(18.0, 12.0)),
            )
            .show_inside(ui, |ui| {
                self.task_header(ui, &task);
                ui.add_space(12.0);
                self.timeline(ui, &task);
            });
    }

    fn task_header(&mut self, ui: &mut egui::Ui, task: &stasis_ai::Task) {
        let mut provider_choice = None;
        let openrouter = stasis_ai::OpenRouterConfig::from_env().ok();
        egui::Frame::none()
            .fill(panel_fill())
            .stroke(egui::Stroke::new(1.0_f32, border()))
            .rounding(8.0)
            .inner_margin(egui::Margin::same(14.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let (dot, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                    ui.painter().circle_filled(dot.center(), 3.0, status_color(task));
                    ui.label(RichText::new(&task.objective).size(20.0).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        status_chip(
                            ui,
                            task_header_status(task).0,
                            status_color(task),
                        );
                    });
                });
                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    let provider = task
                        .provider
                        .provider
                        .as_deref()
                        .unwrap_or("Provider pending");
                    let provider_label = if provider == "installed_codex_subscription" {
                        "Codex"
                    } else {
                        provider
                    };
                    let model = task.provider.model.as_deref().unwrap_or("model pending");
                    let provider_mutable = task.lifecycle == TaskLifecycle::Active
                        && !self.ui_busy(task);
                    ui.add_enabled_ui(provider_mutable, |ui| {
                        ui.menu_button(format!("Provider: {provider_label} / {model}  v"), |ui| {
                        ui.label(RichText::new("Provider").strong());
                        if ui
                            .selectable_label(
                                provider == "installed_codex_subscription",
                                "Codex subscription",
                            )
                            .clicked()
                        {
                            provider_choice =
                                Some((ProviderSelection::Codex, ProviderConfig::Codex));
                            ui.close_menu();
                        }
                        let available = openrouter.is_some();
                        let response = ui.add_enabled(
                            available,
                            egui::SelectableLabel::new(
                                provider != "installed_codex_subscription",
                                "OpenRouter",
                            ),
                        );
                        let clicked = response.clicked();
                        if !available {
                            response
                                .on_hover_text("OpenRouter is not configured for this process.");
                        }
                        if clicked {
                            provider_choice = openrouter.clone().map(|config| {
                                (
                                    ProviderSelection::OpenRouter,
                                    ProviderConfig::OpenRouter(config),
                                )
                            });
                            ui.close_menu();
                        }
                        ui.separator();
                        ui.label(
                            RichText::new(format!("Route: {:?}", task.provider.routing))
                                .small()
                                .color(muted_text()),
                        );
                        ui.label(
                            RichText::new(format!("Fallback: {:?}", task.provider.fallback))
                                .small()
                                .color(muted_text()),
                        );
                    });
                    });
                    ui.separator();
                    let (retained, budget) = self.controller.thread_context_usage(task);
                    let fraction = if budget == 0 {
                        0.0
                    } else {
                        retained as f32 / budget as f32
                    };
                    ui.label(RichText::new("Thread context").size(11.0).color(muted_text()));
                    ui.add(
                        egui::ProgressBar::new(fraction.clamp(0.0, 1.0))
                            .desired_width(112.0)
                            .text(format!("{retained} / {budget} chars")),
                    )
                    .on_hover_text("Retained conversation characters; excludes source, tools, images, and the model token window.");
                });
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    let total = task
                        .metrics
                        .input_tokens
                        .saturating_add(task.metrics.output_tokens);
                    ui.label(RichText::new(format!("Usage  {total} tokens")).size(12.0));
                    ui.label(
                        RichText::new(format!(
                            "{} in / {} out",
                            task.metrics.input_tokens, task.metrics.output_tokens
                        ))
                        .size(11.0)
                        .color(muted_text()),
                    );
                    if task.metrics.estimated_cost_micros > 0 {
                        ui.label(
                            RichText::new(format!(
                                "${:.4}",
                                task.metrics.estimated_cost_micros as f64 / 1_000_000.0
                            ))
                            .size(11.0)
                            .color(muted_text()),
                        );
                    }
                    if task.metrics.elapsed_ms > 0 {
                        ui.label(
                            RichText::new(format!("{} ms", task.metrics.elapsed_ms))
                                .size(11.0)
                                .color(muted_text()),
                        );
                    }
                });
                if let Some(request) = self.controller.snapshot(&task.id) {
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(format!(
                            "Request {} / {:?} / {} ms / retry {}",
                            request.request_id.get(),
                            request.state,
                            request.elapsed_ms,
                            request.retry_count
                        ))
                        .size(11.0)
                        .color(muted_text()),
                    );
                }
            });
        if let Some((selection, config)) = provider_choice {
            self.state.notice = self
                .state
                .session
                .task_mut(&task.id)
                .and_then(|task| {
                    task.select_provider(selection)?;
                    task.set_provider_state(configured_provider_state(&config))
                })
                .err()
                .map(|error| error.to_string());
        }
    }

    fn timeline(&mut self, ui: &mut egui::Ui, task: &stasis_ai::Task) {
        let activity = task.activity_timeline();
        let mut latest_actions = BTreeMap::new();
        let mut latest_images = BTreeMap::new();
        for item in &activity {
            match &item.kind {
                ActivityKind::SemanticAction { action_id, .. } => {
                    latest_actions.insert(action_id.to_string(), item.sequence);
                }
                ActivityKind::GeneratedAsset { image_id, .. } => {
                    latest_images.insert(image_id.to_string(), item.sequence);
                }
                _ => {}
            }
        }
        let mut command = None;
        egui::ScrollArea::vertical()
            .id_source(("task-timeline", task.id.as_str()))
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                if activity.is_empty() {
                    ui.add_space(30.0);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            RichText::new("Ready for the first message")
                                .size(15.0)
                                .strong(),
                        );
                        ui.label(
                            RichText::new(
                                "Describe the change or attach a verified game frame below.",
                            )
                            .size(12.0)
                            .color(muted_text()),
                        );
                    });
                }
                for entry in activity {
                    let latest_entity_snapshot = match &entry.kind {
                        ActivityKind::SemanticAction { action_id, .. } => latest_actions
                            .get(action_id.as_str())
                            .is_some_and(|sequence| *sequence == entry.sequence),
                        ActivityKind::GeneratedAsset { image_id, .. } => latest_images
                            .get(image_id.as_str())
                            .is_some_and(|sequence| *sequence == entry.sequence),
                        _ => true,
                    };
                    if command.is_none() {
                        command = self.activity_card(ui, task, entry, latest_entity_snapshot);
                    } else {
                        self.activity_card(ui, task, entry, latest_entity_snapshot);
                    }
                    ui.add_space(9.0);
                }
            });
        if let Some(command) = command {
            let result = match command {
                TimelineAction::Accept(task, action) => self
                    .state
                    .reviewed_preview(&task, &action)
                    .map(|_| ())
                    .and_then(|()| {
                        self.state
                            .session
                            .task_mut(task)
                            .and_then(|task| task.accept_action(action))
                            .map_err(|error| error.to_string())
                    }),
                TimelineAction::Reject(task, action) => self
                    .state
                    .session
                    .task_mut(task)
                    .and_then(|task| task.reject_action(action, "Rejected in desktop editor"))
                    .map_err(|error| error.to_string()),
                TimelineAction::Apply(task, action) => {
                    self.state.intents.push(EditorIntent::Apply(task, action));
                    Ok(())
                }
                TimelineAction::ApproveImage(task, image) => self
                    .state
                    .session
                    .task_mut(task)
                    .and_then(|task| task.approve_generated_image(image))
                    .map_err(|error| error.to_string()),
                TimelineAction::RejectImage(task, image) => self
                    .state
                    .session
                    .task_mut(task)
                    .and_then(|task| {
                        task.reject_generated_image(image, "Rejected in desktop editor")
                    })
                    .map_err(|error| error.to_string()),
                TimelineAction::Import(task, image) => {
                    self.state
                        .intents
                        .push(EditorIntent::ImportImage(task, image));
                    Ok(())
                }
                TimelineAction::SelectAttachment(task, screenshot) => self
                    .state
                    .session
                    .task_mut(task)
                    .and_then(|task| task.select_screenshot_for_request(screenshot))
                    .map_err(|error| error.to_string()),
                TimelineAction::UnselectAttachment(task, screenshot) => self
                    .state
                    .session
                    .task_mut(task)
                    .and_then(|task| task.unselect_screenshot_for_request(screenshot))
                    .map_err(|error| error.to_string()),
                TimelineAction::RemoveAttachment(task, screenshot) => {
                    let task_id = TaskId::new(task);
                    self.attachment_textures
                        .remove(&(task_id.to_string(), screenshot.clone()));
                    self.attachment_store.remove(&task_id, &screenshot);
                    self.state
                        .session
                        .task_mut(&task_id)
                        .and_then(|task| task.remove_screenshot(screenshot))
                        .map(|_| ())
                        .map_err(|error| error.to_string())
                }
                TimelineAction::PreviewAttachment(task, screenshot) => {
                    self.attachment_preview = Some((task, screenshot));
                    Ok(())
                }
            };
            self.state.notice = result.err();
        }
    }

    fn activity_card(
        &mut self,
        ui: &mut egui::Ui,
        task: &stasis_ai::Task,
        entry: stasis_ai::task_session::ActivityEntry,
        latest_entity_snapshot: bool,
    ) -> Option<TimelineAction> {
        let can_interact = task.lifecycle == TaskLifecycle::Active
            && task.connection == ConnectionState::Connected
            && !self.ui_busy(task);
        let (title, tint) = match &entry.kind {
            ActivityKind::UserMessage { .. } => ("You", Color32::from_rgb(190, 168, 255)),
            ActivityKind::AiReply { .. } => ("Stasis AI", Color32::from_rgb(228, 233, 240)),
            ActivityKind::Attachment { .. } => ("Attached image", Color32::from_rgb(101, 181, 246)),
            ActivityKind::SemanticAction { .. } => {
                ("Semantic change", Color32::from_rgb(118, 158, 246))
            }
            ActivityKind::GeneratedAsset { .. } => {
                ("Generated asset", Color32::from_rgb(192, 132, 252))
            }
            ActivityKind::HostResult { .. } => ("Host result", Color32::from_rgb(113, 196, 205)),
            ActivityKind::FocusedTest { .. } => ("Focused tests", accent()),
        };
        let mut command = None;
        egui::Frame::none()
            .fill(panel_fill())
            .stroke(egui::Stroke::new(1.0_f32, border()))
            .rounding(8.0)
            .inner_margin(egui::Margin::same(13.0))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(title).size(12.0).strong().color(tint));
                    ui.label(
                        RichText::new(format!("#{:02}", entry.sequence))
                            .size(10.0)
                            .color(muted_text()),
                    );
                    if !entry.recorded {
                        ui.label(RichText::new("restored").size(10.0).color(muted_text()));
                    }
                });
                match entry.kind {
                    ActivityKind::UserMessage { thread_sequence }
                    | ActivityKind::AiReply { thread_sequence }
                    | ActivityKind::HostResult { thread_sequence } => {
                        if let Some(message) = task
                            .thread
                            .iter()
                            .find(|item| item.sequence == thread_sequence)
                        {
                            ui.label(RichText::new(&message.text).size(14.0));
                        }
                    }
                    ActivityKind::Attachment {
                        screenshot_id,
                        upload,
                        analysis,
                    } => {
                        if let Some(screenshot) = task.screenshots.get(&screenshot_id) {
                            self.inline_screenshot(ui, screenshot.id.as_str());
                            ui.horizontal_wrapped(|ui| {
                                status_chip(
                                    ui,
                                    &format!("upload: {:?}", upload).to_ascii_lowercase(),
                                    status_for_upload(&upload),
                                );
                                status_chip(
                                    ui,
                                    &format!("analysis: {:?}", analysis).to_ascii_lowercase(),
                                    status_for_analysis(&analysis),
                                );
                                ui.label(
                                    RichText::new(format!(
                                        "task {} / request {} / {}",
                                        screenshot.provenance.task_id,
                                        screenshot.request_id.map(|id| id.to_string()).unwrap_or_else(|| "not sent".into()),
                                        screenshot
                                            .content_sha256
                                            .as_deref()
                                            .map(|hash| &hash[..12.min(hash.len())])
                                            .unwrap_or("verification pending")
                                    ))
                                    .size(11.0)
                                    .color(muted_text()),
                                );
                            });
                            let name = self
                                .attachment_store
                                .get(&task.id, screenshot.id.as_str())
                                .map(|owned| owned.name.as_str())
                                .unwrap_or_else(|| {
                                    std::path::Path::new(&screenshot.source)
                                        .file_name()
                                        .and_then(|name| name.to_str())
                                        .unwrap_or(screenshot.id.as_str())
                                });
                            ui.label(RichText::new(name).size(12.0).strong())
                                .on_hover_text(&screenshot.source);
                            if let Some(owned) = self.attachment_store.get(&task.id, screenshot.id.as_str()) {
                                ui.label(
                                    RichText::new(format!(
                                        "{} x {} / {} / {} / {} / verified {}",
                                        owned.width,
                                        owned.height,
                                        human_bytes(owned.byte_len),
                                        owned.mime_type,
                                        owned.origin.label(),
                                        &owned.sha256[..12]
                                    ))
                                    .size(11.0)
                                    .color(muted_text()),
                                );
                            }
                            ui.horizontal_wrapped(|ui| {
                                let destination = selected_provider_config(task.selected_provider)
                                    .map(|config| (config.provider_name().to_string(), config.model()))
                                    .unwrap_or_else(|_| ("unavailable provider".into(), "unconfigured model".into()));
                                let (provider, model) = destination;
                                if screenshot.consent_to_send && screenshot.selected_for_request {
                                    status_chip(ui, "included once", accent());
                                    ui.label(RichText::new(format!("will send once to {provider} / {model}")).size(11.0).color(muted_text()));
                                    if can_interact && ui.button("Undo inclusion").clicked() {
                                        command = Some(TimelineAction::UnselectAttachment(task.id.to_string(), screenshot.id.to_string()));
                                    }
                                } else {
                                    let retry = matches!(upload, UploadState::Failed { .. })
                                        || matches!(analysis, ScreenshotAnalysisState::Failed { .. } | ScreenshotAnalysisState::Canceled);
                                    let capability = self.image_attachment_capability(&task.id)
                                        .and_then(|()| task.validate_screenshot_selection(screenshot.id.as_str()).map_err(|error| error.to_string()));
                                    let can_include = can_interact && capability.is_ok();
                                    let include = ui.add_enabled(can_include, egui::Button::new(if retry { "Retry once" } else { "Include once" }));
                                    let clicked = include.clicked();
                                    if let Err(reason) = capability { include.on_disabled_hover_text(reason); }
                                    if clicked {
                                        command = Some(TimelineAction::SelectAttachment(task.id.to_string(), screenshot.id.to_string()));
                                    }
                                    ui.label(RichText::new(format!("pixels stay local; Include once sends them to {provider} / {model}")).size(11.0).color(muted_text()));
                                }
                                if ui.button("Preview").clicked() {
                                    command = Some(TimelineAction::PreviewAttachment(task.id.to_string(), screenshot.id.to_string()));
                                }
                                if can_interact && ui.button("Remove").clicked() {
                                    command = Some(TimelineAction::RemoveAttachment(task.id.to_string(), screenshot.id.to_string()));
                                }
                            });
                        }
                    }
                    ActivityKind::SemanticAction {
                        action_id,
                        description,
                        state,
                        ..
                    } => {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new(description).size(14.0).strong());
                            status_chip(ui, action_state_label(&state), action_state_color(&state));
                        });
                        if latest_entity_snapshot {
                            let action = task.actions.get(&action_id);
                            if let Some(action) = action {
                                for proposal in proposal_revisions(action) {
                                    let Some(payload) = proposal.payload else { continue; };
                                    let key = SemanticPreviewKey::new(task.id.as_str(), action_id.as_str(), proposal.revision, payload);
                                    ui.push_id((&key.task, &key.action, key.revision, &key.payload_hash), |ui| {
                                        semantic_revisions::render_heading(ui, action_id.as_str(), &proposal);
                                        if let Some(record) = self.state.semantic_previews.get(&key) {
                                            if record.stale {
                                                ui.colored_label(warning(), "Stale: project sources changed. Acceptance and Apply disabled.");
                                            }
                                            match &record.result {
                                                None => { ui.spinner(); ui.label("Planning semantic changes..."); }
                                                Some(Err(error)) => { ui.colored_label(failure(), format!("Preview unavailable: {error}")); }
                                                Some(Ok(preview)) => { semantic_diff::render(ui, &preview.plan, "semantic-files"); }
                                            }
                                        } else {
                                            ui.label("Planning semantic changes...");
                                        }
                                    });
                                }
                                if let Some(receipt) = self
                                    .execution_receipts
                                    .get(&(task.id.to_string(), action_id.to_string()))
                                {
                                    render_compiler_changes(ui, receipt);
                                }
                            }
                        }
                        let current_state =
                            task.actions.get(&action_id).map(|action| &action.state);
                        ui.horizontal(|ui| match (state, current_state, latest_entity_snapshot) {
                            (ActionState::Proposed, Some(ActionState::Proposed), true)
                                if can_interact =>
                            {
                                if ui.add_enabled(self.state.check_preview(task.id.as_str(), action_id.as_str(), false).is_ok(), egui::Button::new("Accept")).clicked() {
                                    command = Some(TimelineAction::Accept(
                                        task.id.to_string(),
                                        action_id.to_string(),
                                    ));
                                }
                                if ui.button("Reject").clicked() {
                                    command = Some(TimelineAction::Reject(
                                        task.id.to_string(),
                                        action_id.to_string(),
                                    ));
                                }
                            }
                            (ActionState::Accepted, Some(ActionState::Accepted), true)
                                if can_interact =>
                            {
                                if ui.add_enabled(self.state.check_preview(task.id.as_str(), action_id.as_str(), false).is_ok(), egui::Button::new("Apply change")).clicked() {
                                    command = Some(TimelineAction::Apply(
                                        task.id.to_string(),
                                        action_id.to_string(),
                                    ));
                                }
                            }
                            _ => {}
                        });
                    }
                    ActivityKind::GeneratedAsset {
                        image_id,
                        review,
                        handoff,
                    } => {
                        if let Some(image) = task.generated_images.get(&image_id) {
                            self.inline_generated_asset(
                                ui,
                                task.id.as_str(),
                                &image.id.to_string(),
                                &image.source,
                            );
                            let name = std::path::Path::new(&image.source)
                                .file_name()
                                .and_then(|name| name.to_str())
                                .unwrap_or(image.id.as_str());
                            ui.label(RichText::new(name).size(13.0).strong())
                                .on_hover_text(&image.source);
                            ui.label(
                                RichText::new(format!(
                                    "{} / {}",
                                    image.attribution.provider,
                                    image
                                        .attribution
                                        .model
                                        .as_deref()
                                        .unwrap_or("model unavailable")
                                ))
                                .size(11.0)
                                .color(muted_text()),
                            );
                        }
                        ui.horizontal(|ui| {
                            status_chip(
                                ui,
                                &format!("{:?}", review).to_ascii_lowercase(),
                                image_review_color(&review),
                            );
                            status_chip(
                                ui,
                                &format!("{:?}", handoff).to_ascii_lowercase(),
                                image_handoff_color(&handoff),
                            );
                            let current = task.generated_images.get(&image_id);
                            if can_interact
                                && latest_entity_snapshot
                                && matches!(review, ImageReviewState::Pending)
                                && current.is_some_and(|image| {
                                    matches!(image.review, ImageReviewState::Pending)
                                })
                            {
                                if ui.button("Approve").clicked() {
                                    command = Some(TimelineAction::ApproveImage(
                                        task.id.to_string(),
                                        image_id.to_string(),
                                    ));
                                }
                                if ui.button("Reject").clicked() {
                                    command = Some(TimelineAction::RejectImage(
                                        task.id.to_string(),
                                        image_id.to_string(),
                                    ));
                                }
                            }
                            if can_interact
                                && latest_entity_snapshot
                                && matches!(review, ImageReviewState::Approved)
                                && matches!(handoff, ImageHandoffState::Pending)
                                && current.is_some_and(|image| {
                                    matches!(image.review, ImageReviewState::Approved)
                                        && matches!(image.handoff, ImageHandoffState::Pending)
                                })
                                && ui
                                    .add_enabled(false, egui::Button::new("Import asset"))
                                    .on_disabled_hover_text(
                                        "Asset import is unavailable in the desktop editor.",
                                    )
                                    .clicked()
                            {
                                command = Some(TimelineAction::Import(
                                    task.id.to_string(),
                                    image_id.to_string(),
                                ));
                            }
                        });
                    }
                    ActivityKind::FocusedTest { run_id, status } => {
                        ui.horizontal_wrapped(|ui| {
                            status_chip(
                                ui,
                                validation_label_ui(&status),
                                validation_color(&status),
                            );
                            ui.label(
                                RichText::new(format!("Run {run_id}"))
                                    .size(11.0)
                                    .color(muted_text()),
                            );
                        });
                        match status {
                            ValidationStatus::Passed { summary }
                            | ValidationStatus::Failed { summary } => {
                                ui.label(summary);
                            }
                            ValidationStatus::Running
                                if run_id == task.validation_run_id
                                    && task.validation.is_running() =>
                            {
                                ui.spinner();
                            }
                            ValidationStatus::Running => {
                                ui.label(
                                    RichText::new("phase completed")
                                        .size(11.0)
                                        .color(muted_text()),
                                );
                            }
                            ValidationStatus::NotRun => {}
                        }
                    }
                }
            });
        command
    }

    fn inline_screenshot(&mut self, ui: &mut egui::Ui, screenshot_id: &str) {
        let active = self.state.session.active_task_id().cloned();
        if let Some(task_id) = active.as_ref() {
            let key = (task_id.to_string(), screenshot_id.to_string());
            if let Some(owned) = self.attachment_store.get(task_id, screenshot_id) {
                if !self.attachment_textures.contains_key(&key) {
                    let image = egui::ColorImage::from_rgba_unmultiplied(
                        [
                            owned.thumbnail_width as usize,
                            owned.thumbnail_height as usize,
                        ],
                        &owned.thumbnail_rgba,
                    );
                    let texture = ui.ctx().load_texture(
                        format!("attachment-{}-{screenshot_id}", task_id.as_str()),
                        image,
                        egui::TextureOptions::LINEAR,
                    );
                    self.attachment_textures.insert(key.clone(), texture);
                }
                let texture = &self.attachment_textures[&key];
                let original = texture.size_vec2();
                let scale = (ui.available_width().min(560.0) / original.x)
                    .min(160.0 / original.y)
                    .min(1.0);
                ui.image((texture.id(), original * scale));
                return;
            }
        }
        let Some(preview) = self
            .state
            .preview
            .as_ref()
            .filter(|preview| {
                preview.screenshot_id == screenshot_id && Some(&preview.task_id) == active.as_ref()
            })
            .cloned()
        else {
            return;
        };
        let needs_texture = self
            .preview_texture
            .as_ref()
            .map_or(true, |(id, _)| id != screenshot_id);
        if needs_texture {
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [preview.width, preview.height],
                &preview.rgba,
            );
            let texture = ui.ctx().load_texture(
                format!("timeline-preview-{screenshot_id}"),
                image,
                egui::TextureOptions::LINEAR,
            );
            self.preview_texture = Some((screenshot_id.to_string(), texture));
        }
        let texture = &self.preview_texture.as_ref().expect("preview texture").1;
        let max_width = ui.available_width().min(560.0);
        let scale = (max_width / preview.width as f32)
            .min(120.0 / preview.height as f32)
            .min(1.0);
        let size = egui::vec2(preview.width as f32, preview.height as f32) * scale;
        ui.image((texture.id(), size));
        ui.label(
            RichText::new(format!(
                "{} x {} / tick {} -> {} / runtime {}:{} / verified {}",
                preview.width,
                preview.height,
                preview.scheduled_tick,
                preview.captured_tick,
                preview.runtime_identity.session_id,
                preview.runtime_identity.generation,
                &preview.sha256[..12]
            ))
            .size(11.0)
            .color(muted_text()),
        );
    }

    fn inline_generated_asset(
        &mut self,
        ui: &mut egui::Ui,
        task_id: &str,
        image_id: &str,
        source: &str,
    ) {
        let key = (task_id.to_string(), image_id.to_string());
        if !self.asset_textures.contains_key(&key) {
            let source_path = PathBuf::from(source);
            let path = if source_path.is_absolute() {
                source_path
            } else {
                self.project_root.join(source_path)
            };
            let bytes = std::fs::symlink_metadata(&path)
                .ok()
                .filter(|metadata| {
                    metadata.is_file()
                        && !metadata.file_type().is_symlink()
                        && metadata.len() <= 16 * 1024 * 1024
                })
                .and_then(|_| std::fs::read(path).ok());
            let image = bytes.and_then(|bytes| {
                let mut limits = image::Limits::default();
                limits.max_image_width = Some(4096);
                limits.max_image_height = Some(4096);
                limits.max_alloc = Some(64 * 1024 * 1024);
                let mut reader = image::ImageReader::new(Cursor::new(bytes));
                reader.limits(limits);
                reader.with_guessed_format().ok()?.decode().ok()
            });
            let texture = image.map(|image| {
                let rgba = image.to_rgba8();
                let size = [rgba.width() as usize, rgba.height() as usize];
                ui.ctx().load_texture(
                    format!("generated-{task_id}-{image_id}"),
                    egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()),
                    egui::TextureOptions::LINEAR,
                )
            });
            self.asset_textures.insert(key.clone(), texture);
        }
        if let Some(Some(texture)) = self.asset_textures.get(&key) {
            let original = texture.size_vec2();
            let scale = (ui.available_width().min(460.0) / original.x)
                .min(96.0 / original.y)
                .min(1.0);
            ui.image((texture.id(), original * scale));
        } else {
            ui.label(
                RichText::new("Preview unavailable")
                    .size(11.0)
                    .color(muted_text()),
            );
        }
    }

    fn composer(&mut self, ui: &mut egui::Ui, task: &stasis_ai::Task) {
        egui::Frame::none().fill(panel_fill()).stroke(egui::Stroke::new(1.0_f32, border())).rounding(9.0).inner_margin(egui::Margin::same(10.0)).show(ui, |ui| {
            let reply = ui.add_sized([ui.available_width(), 58.0], egui::TextEdit::multiline(&mut self.state.reply).hint_text("Reply to Stasis AI..."));
            if self.state.focus == FocusArea::Reply && self.state.focus_pending {
                reply.request_focus();
                self.state.focus_pending = false;
            }
            if reply.has_focus() {
                self.state.focus = FocusArea::Reply;
            }
            ui.horizontal_wrapped(|ui| {
                let busy = self.ui_busy(task);
                let interactive = task.lifecycle == TaskLifecycle::Active
                    && task.connection == ConnectionState::Connected
                    && !busy;
                let image_capability = self.image_attachment_capability(&task.id);
                let can_attach = interactive && image_capability.is_ok();
                let disabled_reason = image_capability.as_ref().err().map(String::as_str).unwrap_or("Attachments are unavailable while this task is closed, disconnected, or busy.");
                if ui.add_enabled(can_attach, egui::Button::new("Attach frame")).on_hover_text("Capture a verified frame from the running native game; pixels remain local until Include once").on_disabled_hover_text(disabled_reason).clicked() {
                    self.state.dispatch(TaskSessionCommand::AttachScreenshot);
                }
                if ui.add_enabled(can_attach, egui::Button::new("Attach image")).on_hover_text("Select up to eight bounded PNG or JPEG files").on_disabled_hover_text(disabled_reason).clicked() {
                    self.select_image_files(&task.id);
                }
                if ui.add_enabled(can_attach, egui::Button::new("Paste image")).on_hover_text("Copy clipboard image pixels into this task's session-only attachment storage").on_disabled_hover_text(disabled_reason).clicked() {
                    self.paste_clipboard_image(&task.id);
                }
                if matches!(selected_provider_config(task.selected_provider), Ok(ProviderConfig::OpenRouter(_)))
                    && ui.small_button("Refresh image support").clicked()
                {
                    self.refresh_active_image_capability();
                }
                if ui.add_enabled(false, egui::Button::new("Generate image")).on_disabled_hover_text("Image generation is unavailable in the desktop editor.").clicked() {
                    self.state.dispatch(TaskSessionCommand::GenerateImage);
                }
                ui.label(RichText::new("Ctrl+Enter sends | Ctrl+Shift+V pastes image").size(10.0).color(muted_text()));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let mut primary = self.state.primary_action(busy);
                    if !self.state.review_command_enabled(&primary.command) {
                        primary.enabled = false;
                        primary.disabled_reason = Some("Review a current compiler-owned preview before accepting or applying.".into());
                    }
                    if primary.command == TaskSessionCommand::MarkDone && !self.validation_fingerprints.contains_key(task.id.as_str()) {
                        primary.enabled = false;
                        primary.disabled_reason = Some("Run focused tests against the current sources before marking done.".into());
                    }
                    let response = ui.add_enabled(primary.enabled, egui::Button::new(RichText::new(primary.label).strong().color(if primary.enabled { Color32::BLACK } else { muted_text() })).fill(if primary.enabled { accent() } else { raised_fill() }));
                    let clicked = response.clicked();
                    if let Some(reason) = primary.disabled_reason { response.on_hover_text(reason); }
                    let is_test = primary.command == TaskSessionCommand::RunFocusedTests;
                    if clicked { self.state.dispatch(primary.command); }
                    if task.lifecycle == TaskLifecycle::Active && task.validation.is_passing() && !is_test {
                        if ui.add_enabled(interactive, egui::Button::new("Run focused tests")).on_hover_text(if interactive { "Validate the current project sources" } else { "Tests are unavailable while disconnected or busy." }).clicked() { self.state.dispatch(TaskSessionCommand::RunFocusedTests); }
                    }
                });
            });
        });
    }

    fn palette(&mut self, context: &egui::Context) {
        if !self.state.palette_open {
            return;
        }
        let commands = [
            ("New task", TaskSessionCommand::NewTask),
            ("Next task", TaskSessionCommand::SwitchNextTask),
            ("Previous task", TaskSessionCommand::SwitchPreviousTask),
            ("Send reply", TaskSessionCommand::SendReply),
            ("Reject action", TaskSessionCommand::RejectAction),
            ("Retry", TaskSessionCommand::Retry),
            (
                "Import generated image",
                TaskSessionCommand::ImportGeneratedImage,
            ),
            ("Focus reply", TaskSessionCommand::FocusReply),
            ("Accept action", TaskSessionCommand::AcceptAction),
            ("Apply action", TaskSessionCommand::ApplyAction),
            ("Run focused tests", TaskSessionCommand::RunFocusedTests),
            (
                "Attach game screenshot",
                TaskSessionCommand::AttachScreenshot,
            ),
            ("Generate image", TaskSessionCommand::GenerateImage),
            ("Reconnect", TaskSessionCommand::Reconnect),
            ("Cancel task", TaskSessionCommand::Cancel),
            ("Mark done", TaskSessionCommand::MarkDone),
            ("Focus game", TaskSessionCommand::FocusGame),
        ];
        let mut commands = commands
            .into_iter()
            .map(|(label, command)| (label.to_string(), command))
            .collect::<Vec<_>>();
        commands.extend(
            self.state
                .session
                .tasks()
                .take(255)
                .enumerate()
                .map(|(index, task)| {
                    (
                        format!("Switch to task {}: {}", index + 1, task.objective),
                        TaskSessionCommand::SwitchTask((index + 1) as u8),
                    )
                }),
        );
        let mut keys = Vec::new();
        context.input_mut(|input| {
            input.events.retain(|event| {
                if let egui::Event::Key {
                    key, pressed: true, ..
                } = event
                {
                    if matches!(
                        key,
                        egui::Key::ArrowUp
                            | egui::Key::ArrowDown
                            | egui::Key::Enter
                            | egui::Key::Escape
                    ) {
                        keys.push(*key);
                        return false;
                    }
                }
                true
            });
        });
        let mut chosen = None;
        egui::Window::new("Command palette  Ctrl+K")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_TOP, [0.0, 72.0])
            .show(context, |ui| {
                let query_id = ui.make_persistent_id("palette-query");
                ui.memory_mut(|memory| memory.request_focus(query_id));
                let response =
                    ui.add(egui::TextEdit::singleline(&mut self.state.palette_query).id(query_id));
                if response.changed() {
                    self.state.palette_selected = 0;
                }
                let query = self.state.palette_query.to_ascii_lowercase();
                let filtered = commands
                    .iter()
                    .filter(|(label, _)| label.to_ascii_lowercase().contains(&query))
                    .collect::<Vec<_>>();
                self.state.palette_selected = self
                    .state
                    .palette_selected
                    .min(filtered.len().saturating_sub(1));
                for key in &keys {
                    match key {
                        egui::Key::Escape => {
                            self.state.close_palette();
                            break;
                        }
                        egui::Key::ArrowUp => {
                            self.state.palette_selected =
                                self.state.palette_selected.saturating_sub(1)
                        }
                        egui::Key::ArrowDown => {
                            self.state.palette_selected = (self.state.palette_selected + 1)
                                .min(filtered.len().saturating_sub(1))
                        }
                        egui::Key::Enter => {
                            chosen = filtered
                                .get(self.state.palette_selected)
                                .map(|(_, command)| command.clone());
                            break;
                        }
                        _ => {}
                    }
                }
                egui::ScrollArea::vertical()
                    .max_height(360.0)
                    .show(ui, |ui| {
                        for (index, (label, command)) in filtered.iter().enumerate() {
                            let response = ui.selectable_label(
                                index == self.state.palette_selected,
                                label.as_str(),
                            );
                            if index == self.state.palette_selected && !keys.is_empty() {
                                response.scroll_to_me(None);
                            }
                            if response.clicked() {
                                chosen = Some(command.clone());
                            }
                        }
                        if filtered.is_empty() {
                            ui.label("No matching commands");
                        }
                    });
            });
        if let Some(command) = chosen {
            self.state.close_palette();
            self.state.dispatch(command);
        }
    }
}

impl DesktopEditor {
    fn compact_rail(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(project_name(&self.project_root)).strong());
            let input = ui.add_sized(
                [ui.available_width().min(250.0), 30.0],
                egui::TextEdit::singleline(&mut self.state.objective)
                    .hint_text("New task objective"),
            );
            let submitted =
                input.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
            if ui.button("+ Task").clicked() || submitted {
                self.state.notice = self.state.create_task().err();
            }
        });
        let active = self.state.session.active_task_id().map(ToString::to_string);
        let cards = self
            .state
            .session
            .tasks()
            .map(|task| (task.id.to_string(), task.objective.clone()))
            .collect::<Vec<_>>();
        egui::ScrollArea::horizontal()
            .id_source("compact-task-rail")
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (id, objective) in cards {
                        if ui
                            .selectable_label(active.as_deref() == Some(&id), objective)
                            .clicked()
                        {
                            self.state.notice = self.state.switch_task(&id).err();
                        }
                    }
                });
            });
    }

    fn ui(&mut self, context: &egui::Context) {
        if self.shutdown.load(Ordering::Acquire) {
            context.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        configure_visuals(context);
        context.request_repaint_after(Duration::from_millis(100));
        self.poll_image_capabilities();
        self.ensure_active_image_capability();
        self.process_dropped_images(context);
        self.poll_controller();
        self.poll_host();
        self.poll_capture();
        self.poll_semantic_previews();
        self.process_shortcuts(context);
        let palette_frame = self.state.palette_open;
        self.palette(context);
        if palette_frame {
            context.input_mut(|input| input.events.clear());
        }
        egui::TopBottomPanel::top("top-bar")
            .frame(
                egui::Frame::none()
                    .fill(rail_fill())
                    .inner_margin(egui::Margin::symmetric(12.0, 7.0)),
            )
            .show(context, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Stasis AI Editor").size(13.0).strong());
                    ui.label(
                        RichText::new("Ctrl+K commands  |  Ctrl+N new task")
                            .size(11.0)
                            .color(muted_text()),
                    );
                    if let Some(notice) = &self.state.notice {
                        ui.colored_label(warning(), notice);
                    }
                });
            });
        let width = context.screen_rect().width();
        match EditorLayout::for_width(width) {
            EditorLayout::Compact => {
                egui::TopBottomPanel::top("compact-rail")
                    .frame(
                        egui::Frame::none()
                            .fill(rail_fill())
                            .inner_margin(egui::Margin::symmetric(10.0, 7.0)),
                    )
                    .show(context, |ui| self.compact_rail(ui));
                egui::CentralPanel::default()
                    .frame(egui::Frame::none().fill(canvas_fill()))
                    .show(context, |ui| self.detail(ui));
            }
            EditorLayout::Wide => {
                egui::SidePanel::left("project-task-rail")
                    .resizable(false)
                    .exact_width(RAIL_WIDTH)
                    .frame(
                        egui::Frame::none()
                            .fill(rail_fill())
                            .inner_margin(egui::Margin::symmetric(12.0, 8.0)),
                    )
                    .show(context, |ui| self.sidebar(ui));
                egui::CentralPanel::default()
                    .frame(egui::Frame::none().fill(canvas_fill()))
                    .show(context, |ui| {
                        let content_width = ui.available_width().min(980.0);
                        ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                            ui.allocate_ui_with_layout(
                                egui::vec2(content_width, ui.available_height()),
                                egui::Layout::top_down(egui::Align::Min),
                                |ui| self.detail(ui),
                            );
                        });
                    });
            }
        }
        self.cancel_confirmation(context);
        self.attachment_preview_window(context);
        self.flush_intents();
    }

    fn attachment_preview_window(&mut self, context: &egui::Context) {
        let Some((task, screenshot)) = self.attachment_preview.clone() else {
            return;
        };
        let task_id = TaskId::new(task.clone());
        let mut open = true;
        egui::Window::new("Image attachment preview")
            .open(&mut open)
            .resizable(true)
            .default_size([620.0, 520.0])
            .show(context, |ui| {
                if let Some(owned) = self.attachment_store.get(&task_id, &screenshot) {
                    let key = (task.clone(), screenshot.clone());
                    if let Some(texture) = self.attachment_textures.get(&key) {
                        let original = texture.size_vec2();
                        let scale = (ui.available_width() / original.x)
                            .min((ui.available_height() - 50.0).max(1.0) / original.y)
                            .min(1.0);
                        ui.image((texture.id(), original * scale));
                    }
                    ui.label(format!(
                        "{} | {} x {} | {} | {}",
                        owned.name,
                        owned.width,
                        owned.height,
                        human_bytes(owned.byte_len),
                        owned.mime_type
                    ));
                } else {
                    ui.label("This attachment is no longer available in this editor session.");
                }
            });
        if !open {
            self.attachment_preview = None;
        }
    }

    fn cancel_confirmation(&mut self, context: &egui::Context) {
        let Some(task_id) = self.state.cancel_confirmation.clone() else {
            return;
        };
        egui::Window::new("Cancel task?")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(context, |ui| {
                let objective = self.state.session.task(task_id.as_str())
                    .map(|task| task.objective.as_str()).unwrap_or(&task_id);
                ui.label(format!("Cancel {objective}?"));
                ui.label("This stops its work and permanently closes the task. You cannot continue it afterward.");
                ui.horizontal(|ui| {
                    if ui.button("Keep task open").clicked() {
                        self.state.cancel_confirmation = None;
                    }
                    if ui.button("Permanently cancel task").clicked() {
                        self.state.intents.push(EditorIntent::Cancel(task_id.clone()));
                        self.state.cancel_confirmation = None;
                    }
                });
            });
    }
}

impl eframe::App for DesktopEditor {
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.ui(context);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.host.shutdown_and_join();
        if let Some(job) = self.semantic_job.take() {
            let _ = job.worker.join();
        }
        if let Some(capture) = self.capture.take() {
            capture.canceled.store(true, Ordering::Release);
        }
        let _ = self
            .client
            .submit(LiveRequest::new(u64::MAX, LiveCommand::Quit));
    }
}

pub(super) fn run(
    client: LiveSessionClient,
    project_root: PathBuf,
    shutdown: Arc<AtomicBool>,
) -> Result<(), String> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Stasis Editor")
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([520.0, 600.0]),
        ..Default::default()
    };
    let quit_client = client.clone();
    let result = eframe::run_native(
        "Stasis Editor",
        options,
        Box::new(move |_context| Box::new(DesktopEditor::new(client, project_root, shutdown))),
    );
    let _ = quit_client.submit(LiveRequest::new(u64::MAX, LiveCommand::Quit));
    result.map_err(|error| format!("desktop editor failed: {error}"))
}

#[cfg(test)]
mod interaction_tests;

#[cfg(test)]
mod native_evidence;

#[cfg(test)]
mod tests {
    use super::*;
    use stasis_ai::task_session::{
        ConnectionState, FocusedTestResult, TaskLifecycle, ValidationStatus,
    };
    use stasis_runner::live::{live_session, LiveResponse};
    use std::sync::Barrier;

    fn task_state() -> EditorState {
        let mut state = EditorState::default();
        state.objective = "Change player speed".into();
        state.create_task().unwrap();
        state
    }

    pub(super) fn review_fixture(name: &str) -> (DesktopEditor, PathBuf, Value) {
        let root = super::super::tests::desktop_editor_fixture(name);
        let item = super::super::desktop_source_context(&root)
            .unwrap()
            .into_iter()
            .find(|item| item["target"]["name"] == "value")
            .unwrap();
        let payload = json!({"schema_version": 1, "edits": [{
            "operation": "update", "target": item["target"],
            "expected_source_hash": item["expected_source_hash"],
            "new_source": "function value(): i32 { return 2; }"
        }]});
        let (client, _server) = live_session(1);
        let mut editor = DesktopEditor::new(client, root.clone(), Arc::new(AtomicBool::new(false)));
        editor.state.objective = "Review value".into();
        editor.state.create_task().unwrap();
        editor
            .state
            .session
            .active_task_mut()
            .unwrap()
            .propose_action_with_payload(
                "value",
                stasis_ai::ActionKind::Edit,
                "Update value",
                payload.clone(),
            )
            .unwrap();
        (editor, root, payload)
    }

    pub(super) fn finish_preview(editor: &mut DesktopEditor) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            editor.poll_semantic_previews();
            if editor.semantic_job.is_none() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "preview worker exceeded deadline"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn semantic_preview_gates_keyboard_acceptance_and_apply_on_source_changes() {
        let (mut editor, root, _) = review_fixture("preview_editor_stale");
        assert!(editor
            .state
            .handle(TaskSessionCommand::AcceptAction)
            .is_err());
        finish_preview(&mut editor);
        assert!(editor
            .state
            .review_command_enabled(&TaskSessionCommand::AcceptAction));
        editor
            .state
            .handle(TaskSessionCommand::AcceptAction)
            .unwrap();
        let entry = root.join("src/main.stasis");
        let old = std::fs::read_to_string(&entry).unwrap();
        std::fs::write(&entry, format!("{old}\n// external edit\n")).unwrap();
        assert!(editor
            .state
            .handle(TaskSessionCommand::ApplyAction)
            .unwrap_err()
            .contains("Stale"));
        editor.next_semantic_check = Instant::now();
        editor.poll_semantic_previews();
        assert!(!editor
            .state
            .review_command_enabled(&TaskSessionCommand::ApplyAction));
        std::fs::write(&entry, old).unwrap();
        assert!(
            editor.state.reviewed_preview("task-1", "value").is_err(),
            "observed staleness must be sticky"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn semantic_preview_never_regenerates_accepted_or_applied_work() {
        let (mut editor, root, _) = review_fixture("preview_editor_immutable");
        finish_preview(&mut editor);
        let key = editor
            .state
            .semantic_previews
            .keys()
            .next()
            .unwrap()
            .clone();
        let plan = editor
            .state
            .reviewed_preview("task-1", "value")
            .unwrap()
            .plan
            .clone();
        editor
            .state
            .handle(TaskSessionCommand::AcceptAction)
            .unwrap();
        editor
            .state
            .session
            .active_task_mut()
            .unwrap()
            .apply_action("value")
            .unwrap();
        editor.poll_semantic_previews();
        assert!(editor.semantic_job.is_none());
        assert_eq!(editor.state.semantic_previews.len(), 1);
        assert_eq!(
            editor.state.semantic_previews[&key]
                .result
                .as_ref()
                .unwrap()
                .as_ref()
                .unwrap()
                .plan,
            plan
        );
        // Restored accepted work with no retained preview must not be re-planned.
        editor.state.semantic_previews.clear();
        editor.poll_semantic_previews();
        assert!(editor.semantic_job.is_none());
        assert!(editor.state.semantic_previews[&key]
            .result
            .as_ref()
            .unwrap()
            .is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    fn revision_evidence_frame(label: &str, editor: &DesktopEditor) -> Value {
        let plans = editor
            .state
            .semantic_previews
            .iter()
            .filter_map(|(key, record)| {
                record.result.as_ref()?.as_ref().ok().map(|preview| {
                    json!({
                        "revision": key.revision, "plan": preview.plan,
                    })
                })
            })
            .collect::<Vec<_>>();
        json!({ "label": label, "task": editor.state.session.active_task().unwrap(), "plans": plans })
    }

    fn finish_apply(editor: &mut DesktopEditor) {
        editor
            .state
            .handle(TaskSessionCommand::ApplyAction)
            .unwrap();
        editor.flush_intents();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !editor.busy_tasks.is_empty() {
            editor.poll_host();
            assert!(Instant::now() < deadline, "host apply exceeded deadline");
            thread::sleep(Duration::from_millis(10));
        }
        editor.poll_semantic_previews();
    }

    #[test]
    fn failed_apply_and_repair_keep_one_card_per_proposal() {
        let (mut editor, root, mut payload) = review_fixture("preview_failed_apply_repair");
        payload["edits"][0]["new_source"] = json!("function value(): i32 { return -1; }");
        editor
            .state
            .session
            .active_task_mut()
            .unwrap()
            .actions
            .get_mut("value")
            .unwrap()
            .payload = Some(payload.clone());
        finish_preview(&mut editor);
        let original_key = editor
            .state
            .semantic_previews
            .keys()
            .next()
            .unwrap()
            .clone();
        let original_plan = editor
            .state
            .reviewed_preview("task-1", "value")
            .unwrap()
            .plan
            .clone();
        let before = std::fs::read_to_string(root.join("src/main.stasis")).unwrap();
        editor
            .state
            .handle(TaskSessionCommand::AcceptAction)
            .unwrap();
        let mut frames = vec![revision_evidence_frame(
            "1. Original proposal accepted",
            &editor,
        )];
        finish_apply(&mut editor);
        assert!(matches!(
            editor.state.session.active_task().unwrap().actions["value"].state,
            ActionState::NeedsRepair { .. }
        ));
        assert_eq!(
            std::fs::read_to_string(root.join("src/main.stasis")).unwrap(),
            before,
            "failed Apply must roll back"
        );
        assert_eq!(editor.state.semantic_previews.len(), 1);
        assert!(editor.semantic_job.is_none());
        frames.push(revision_evidence_frame(
            "2. Apply failed its test; original sources restored",
            &editor,
        ));

        payload["edits"][0]["new_source"] = json!("function value(): i32 { return 3; }");
        editor
            .state
            .session
            .active_task_mut()
            .unwrap()
            .repair_action_with_payload(
                "value",
                "Repair value so the positive test passes",
                payload.clone(),
            )
            .unwrap();
        assert!(
            editor.state.reviewed_preview("task-1", "value").is_err(),
            "new proposal needs its own preview"
        );
        finish_preview(&mut editor);
        let action = &editor.state.session.active_task().unwrap().actions["value"];
        assert_eq!(
            action.revisions.len(),
            2,
            "keep both state snapshots in the audit history"
        );
        let proposals = proposal_revisions(action);
        assert_eq!(proposals.len(), 2);
        assert_eq!(
            proposals[1].revision, 1,
            "first repaired proposal displays Revision 2"
        );
        assert!(proposals[1].current);
        assert_eq!(editor.state.semantic_previews.len(), 2);
        assert!(editor
            .state
            .semantic_previews
            .values()
            .all(|record| matches!(record.result, Some(Ok(_)))));
        assert_eq!(
            editor.state.semantic_previews[&original_key]
                .result
                .as_ref()
                .unwrap()
                .as_ref()
                .unwrap()
                .plan,
            original_plan
        );
        assert_eq!(
            editor
                .state
                .reviewed_preview("task-1", "value")
                .unwrap()
                .payload,
            payload
        );
        frames.push(revision_evidence_frame(
            "3. Repaired proposal is Revision 2; original preview retained",
            &editor,
        ));
        editor
            .state
            .handle(TaskSessionCommand::AcceptAction)
            .unwrap();
        finish_apply(&mut editor);
        assert!(matches!(
            editor.state.session.active_task().unwrap().actions["value"].state,
            ActionState::Applied
        ));
        assert_eq!(editor.state.semantic_previews.len(), 2);
        frames.push(revision_evidence_frame(
            "4. Revision 2 applied successfully",
            &editor,
        ));

        // Native evidence renders these real host results with the production grouping and cards.
        let evidence =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/task-520-review");
        std::fs::create_dir_all(&evidence).unwrap();
        std::fs::write(
            evidence.join("failed-apply-repair.json"),
            serde_json::to_vec_pretty(&frames).unwrap(),
        )
        .unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn semantic_preview_retains_revisions_and_binds_task_and_exact_payload() {
        let (mut editor, root, payload) = review_fixture("preview_editor_revisions");
        finish_preview(&mut editor);
        let original_key = editor
            .state
            .semantic_previews
            .keys()
            .next()
            .unwrap()
            .clone();
        assert!(editor.state.reviewed_preview("task-1", "value").is_ok());
        editor
            .state
            .session
            .active_task_mut()
            .unwrap()
            .append_reply("Please revise it")
            .unwrap();
        editor
            .state
            .handle(TaskSessionCommand::RejectAction)
            .unwrap();
        let mut repaired = payload.clone();
        repaired["edits"][0]["new_source"] = json!("function value(): i32 { return 3; }");
        editor
            .state
            .session
            .active_task_mut()
            .unwrap()
            .repair_action_with_payload("value", "Repaired value", repaired.clone())
            .unwrap();
        assert!(editor.state.reviewed_preview("task-1", "value").is_err());
        finish_preview(&mut editor);
        assert_eq!(editor.state.semantic_previews.len(), 2);
        assert_eq!(
            editor.state.session.active_task().unwrap().actions["value"].revisions[0]
                .thread_position,
            0
        );
        let latest = editor.state.reviewed_preview("task-1", "value").unwrap();
        assert_eq!(latest.payload, repaired);
        assert_ne!(
            latest.plan,
            editor.state.semantic_previews[&original_key]
                .result
                .as_ref()
                .unwrap()
                .as_ref()
                .unwrap()
                .plan
        );
        editor
            .state
            .session
            .active_task_mut()
            .unwrap()
            .actions
            .get_mut("value")
            .unwrap()
            .payload = Some(payload);
        assert!(editor.state.reviewed_preview("task-1", "value").is_err());
        editor.state.objective = "Other task".into();
        editor.state.create_task().unwrap();
        assert!(editor.state.reviewed_preview("task-2", "value").is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    fn test_png() -> Vec<u8> {
        use image::ImageEncoder;
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(
                &[255, 0, 0, 255, 0, 255, 0, 255],
                2,
                1,
                image::ExtendedColorType::Rgba8,
            )
            .unwrap();
        bytes
    }

    fn test_runtime_identity() -> LiveRuntimeIdentity {
        LiveRuntimeIdentity {
            session_id: "runtime-test".into(),
            generation: 3,
            source_hashes: std::collections::BTreeMap::new(),
            indexed_collections: Vec::new(),
            complete: true,
        }
    }

    fn temp_png_path(label: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "stasis-editor-{label}-{}-{nonce}.png",
            std::process::id()
        ))
    }

    #[test]
    fn drafts_follow_all_task_switches_and_creation() {
        let mut state = task_state();
        state.reply = "unsent first".into();
        state.objective = "second".into();
        state.create_task().unwrap();
        assert!(state.reply.is_empty());
        state.reply = "unsent second".into();
        state.objective = "future second objective".into();
        state
            .handle(TaskSessionCommand::SwitchPreviousTask)
            .unwrap();
        assert_eq!(state.reply, "unsent first");
        assert!(state.objective.is_empty());
        state.objective = "future first objective".into();
        state.switch_task("task-1").unwrap();
        assert_eq!(state.objective, "future first objective");
        assert_eq!(state.reply, "unsent first");
        state.handle(TaskSessionCommand::SwitchTask(2)).unwrap();
        assert_eq!(state.reply, "unsent second");
        assert_eq!(state.objective, "future second objective");
        state.switch_task("task-1").unwrap();
        assert_eq!(state.objective, "future first objective");
        assert!(state.switch_task("missing").is_err());
        assert_eq!(state.reply, "unsent first");
        state.handle(TaskSessionCommand::SendReply).unwrap();
        state.switch_relative(1).unwrap();
        state.switch_relative(-1).unwrap();
        assert!(state.reply.is_empty());
    }

    fn palette_frame(
        editor: &mut DesktopEditor,
        context: &egui::Context,
        events: Vec<egui::Event>,
    ) {
        let _ = context.run(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |context| {
                editor.ui(context);
                assert!(!context.input(|input| input.key_pressed(egui::Key::Enter)
                    || input.key_pressed(egui::Key::Escape)));
            },
        );
    }

    #[test]
    fn palette_reopening_captures_text_before_underlying_fields() {
        let (client, _server) = live_session(4);
        let mut editor =
            DesktopEditor::new(client, PathBuf::from("."), Arc::new(AtomicBool::new(false)));
        editor.state = task_state();
        editor.state.focus = FocusArea::Tasks;
        editor.state.objective = "unsent objective".into();
        let context = egui::Context::default();
        editor
            .state
            .handle(TaskSessionCommand::OpenCommandPalette)
            .unwrap();
        palette_frame(&mut editor, &context, vec![]);
        editor.state.close_palette();
        palette_frame(&mut editor, &context, vec![]);
        palette_frame(
            &mut editor,
            &context,
            vec![
                key_event(egui::Key::K, egui::Modifiers::CTRL),
                egui::Event::Text("focus game".into()),
            ],
        );
        assert_eq!(editor.state.palette_query, "focus game");
        assert_eq!(editor.state.objective, "unsent objective");
        palette_frame(
            &mut editor,
            &context,
            vec![key_event(egui::Key::Enter, egui::Modifiers::NONE)],
        );
        assert_eq!(editor.state.focus, FocusArea::Game);
        assert_eq!(editor.state.session.task_count(), 1);
        assert_eq!(editor.state.objective, "unsent objective");
    }

    #[test]
    fn palette_shortcut_blocks_other_task_shortcuts() {
        let (client, _server) = live_session(4);
        let mut editor =
            DesktopEditor::new(client, PathBuf::from("."), Arc::new(AtomicBool::new(false)));
        editor.state = task_state();
        editor.state.reply = "unsent".into();
        editor.state.objective = "another task".into();
        let context = egui::Context::default();
        palette_frame(
            &mut editor,
            &context,
            vec![
                key_event(egui::Key::K, egui::Modifiers::CTRL),
                key_event(egui::Key::N, egui::Modifiers::CTRL),
            ],
        );
        assert!(editor.state.palette_open);
        palette_frame(
            &mut editor,
            &context,
            vec![key_event(egui::Key::N, egui::Modifiers::CTRL)],
        );
        assert_eq!(editor.state.session.task_count(), 1);
        assert_eq!(editor.state.objective, "another task");
        assert_eq!(editor.state.reply, "unsent");
    }

    #[test]
    fn palette_filters_navigates_invokes_and_consumes_keys() {
        let (client, _server) = live_session(4);
        let mut editor =
            DesktopEditor::new(client, PathBuf::from("."), Arc::new(AtomicBool::new(false)));
        editor.state = task_state();
        editor.state.reply = "do not send".into();
        editor
            .state
            .handle(TaskSessionCommand::OpenCommandPalette)
            .unwrap();
        let context = egui::Context::default();
        palette_frame(&mut editor, &context, vec![]);
        palette_frame(
            &mut editor,
            &context,
            vec![egui::Event::Text("focus".into())],
        );
        assert_eq!(editor.state.palette_query, "focus");
        palette_frame(
            &mut editor,
            &context,
            vec![key_event(egui::Key::ArrowDown, egui::Modifiers::NONE)],
        );
        assert_eq!(editor.state.palette_selected, 1);
        palette_frame(
            &mut editor,
            &context,
            vec![key_event(egui::Key::ArrowUp, egui::Modifiers::NONE)],
        );
        assert_eq!(editor.state.palette_selected, 0);
        editor.state.palette_query = "focus game".into();
        palette_frame(
            &mut editor,
            &context,
            vec![key_event(egui::Key::Enter, egui::Modifiers::CTRL)],
        );
        assert!(!editor.state.palette_open);
        assert_eq!(editor.state.focus, FocusArea::Game);
        assert_eq!(editor.state.reply, "do not send");
        assert!(editor.state.intents.is_empty());
        editor
            .state
            .handle(TaskSessionCommand::OpenCommandPalette)
            .unwrap();
        editor.state.palette_query = "no such command".into();
        editor.state.palette_selected = 999;
        palette_frame(
            &mut editor,
            &context,
            vec![
                key_event(egui::Key::ArrowDown, egui::Modifiers::NONE),
                key_event(egui::Key::Enter, egui::Modifiers::NONE),
            ],
        );
        assert_eq!(editor.state.palette_selected, 0);
        assert!(editor.state.palette_open);
        palette_frame(
            &mut editor,
            &context,
            vec![key_event(egui::Key::Escape, egui::Modifiers::NONE)],
        );
        assert!(!editor.state.palette_open);
        assert_eq!(editor.state.focus, FocusArea::Game);
        assert_eq!(
            editor.state.session.active_task().unwrap().lifecycle,
            TaskLifecycle::Active
        );
    }

    #[test]
    fn palette_exposes_reject_retry_import_and_task_switching() {
        let (client, _server) = live_session(4);
        let mut editor =
            DesktopEditor::new(client, PathBuf::from("."), Arc::new(AtomicBool::new(false)));
        editor.state = task_state();
        let context = egui::Context::default();
        for query in ["reject action", "retry", "import generated image"] {
            editor
                .state
                .handle(TaskSessionCommand::OpenCommandPalette)
                .unwrap();
            editor.state.palette_query = query.into();
            palette_frame(
                &mut editor,
                &context,
                vec![key_event(egui::Key::Enter, egui::Modifiers::NONE)],
            );
            assert!(!editor.state.palette_open, "{query} must be invocable");
        }
        editor.state.objective = "second".into();
        editor.state.create_task().unwrap();
        for (query, expected) in [
            ("previous task", "task-1"),
            ("next task", "task-2"),
            ("switch to task 1:", "task-1"),
        ] {
            editor
                .state
                .handle(TaskSessionCommand::OpenCommandPalette)
                .unwrap();
            editor.state.palette_query = query.into();
            editor.state.palette_selected = 999;
            palette_frame(
                &mut editor,
                &context,
                vec![key_event(egui::Key::Enter, egui::Modifiers::NONE)],
            );
            assert_eq!(editor.state.active_id().unwrap(), expected);
            assert!(!editor.state.palette_open);
        }
    }

    #[test]
    fn responsive_layout_keeps_narrow_editor_on_one_task_canvas() {
        assert_eq!(EditorLayout::for_width(480.0), EditorLayout::Compact);
        assert_eq!(EditorLayout::for_width(759.0), EditorLayout::Compact);
        assert_eq!(EditorLayout::for_width(760.0), EditorLayout::Wide);
        assert_eq!(EditorLayout::for_width(1920.0), EditorLayout::Wide);
    }

    #[test]
    fn primary_action_tracks_the_next_valid_task_transition() {
        let mut state = EditorState::default();
        let empty = state.primary_action(false);
        assert_eq!(empty.command, TaskSessionCommand::NewTask);
        assert!(!empty.enabled);

        state.objective = "Change movement".into();
        state.create_task().unwrap();
        let message = state.primary_action(false);
        assert_eq!(message.command, TaskSessionCommand::SendReply);
        assert!(!message.enabled);
        state.reply = "Make the player faster".into();
        assert!(state.primary_action(false).enabled);

        state
            .session
            .propose_action("edit-speed", "Update player speed")
            .unwrap();
        assert_eq!(
            state.primary_action(false).command,
            TaskSessionCommand::AcceptAction
        );
        state.session.accept_action("edit-speed").unwrap();
        assert_eq!(
            state.primary_action(false).command,
            TaskSessionCommand::ApplyAction
        );
        assert_eq!(
            state.primary_action(true).command,
            TaskSessionCommand::Cancel
        );
    }

    #[test]
    fn explicit_focus_commands_request_focus_once() {
        let mut state = task_state();
        state.focus_pending = false;
        state.handle(TaskSessionCommand::FocusReply).unwrap();
        assert_eq!(state.focus, FocusArea::Reply);
        assert!(state.focus_pending);
        state.focus_pending = false;
        state.handle(TaskSessionCommand::NewTask).unwrap();
        assert_eq!(state.focus, FocusArea::Tasks);
        assert!(state.focus_pending);
    }

    #[test]
    fn new_task_shortcut_enters_objective_focus() {
        let mut state = task_state();
        state.handle(TaskSessionCommand::NewTask).unwrap();
        assert_eq!(state.focus, FocusArea::Tasks);
        assert!(state.focus_pending);
        assert_eq!(state.session.task_count(), 1);
        state.objective = "Independent objective".into();
        state.handle(TaskSessionCommand::NewTask).unwrap();
        assert_eq!(state.session.task_count(), 2);
        assert!(state.session.active_task().unwrap().thread.is_empty());
    }

    #[test]
    fn independent_tasks_keep_queued_replies_scoped() {
        let mut state = task_state();
        state.reply = "First reply".into();
        state.handle(TaskSessionCommand::SendReply).unwrap();
        state.objective = "Change enemy art".into();
        state.create_task().unwrap();
        state.reply = "Second reply".into();
        state.handle(TaskSessionCommand::SendReply).unwrap();
        assert!(matches!(
            state.intents.as_slice(),
            [EditorIntent::SendReply(first, first_text), EditorIntent::SendReply(second, second_text)]
                if first == "task-1" && first_text == "First reply"
                    && second == "task-2" && second_text == "Second reply"
        ));
    }

    #[test]
    fn apply_and_test_remain_host_approved_intents() {
        let mut state = task_state();
        state
            .session
            .propose_action("edit-1", "Edit speed")
            .unwrap();
        state.handle(TaskSessionCommand::AcceptAction).unwrap();
        state.handle(TaskSessionCommand::ApplyAction).unwrap();
        assert!(matches!(
            state
                .session
                .active_task()
                .unwrap()
                .actions
                .values()
                .next()
                .unwrap()
                .state,
            ActionState::Accepted
        ));
        assert!(matches!(
            state.intents.last(),
            Some(EditorIntent::Apply(_, _))
        ));
        state.handle(TaskSessionCommand::RunFocusedTests).unwrap();
        assert!(matches!(
            state.session.active_task().unwrap().validation,
            ValidationStatus::Running
        ));
    }

    #[test]
    fn game_focus_does_not_mutate_task_context() {
        let mut state = task_state();
        let before = state.session.clone();
        state.handle(TaskSessionCommand::FocusGame).unwrap();
        assert_eq!(state.focus, FocusArea::Game);
        assert_eq!(state.session, before);
    }

    #[test]
    fn reconnect_and_cancel_commands_target_the_active_task() {
        let mut state = task_state();
        state.session.disconnect().unwrap();
        state.handle(TaskSessionCommand::Reconnect).unwrap();
        assert!(matches!(
            state.intents.last(),
            Some(EditorIntent::Reconnect(task)) if task == "task-1"
        ));
        state.session.begin_focused_tests().unwrap();
        state
            .session
            .finish_focused_tests(FocusedTestResult::passed("ok"))
            .unwrap();
        state.handle(TaskSessionCommand::MarkDone).unwrap();
        assert!(matches!(
            state.intents.last(),
            Some(EditorIntent::MarkDone(task)) if task == "task-1"
        ));
        state.objective = "Cancelable task".into();
        state.create_task().unwrap();
        state.handle(TaskSessionCommand::Cancel).unwrap();
        assert_eq!(state.cancel_confirmation.as_deref(), Some("task-2"));
        assert_eq!(
            state.session.active_task().unwrap().lifecycle,
            TaskLifecycle::Active
        );
    }

    #[test]
    fn failed_test_submission_does_not_leave_validation_running() {
        let (client, _server) = live_session(4);
        let mut editor =
            DesktopEditor::new(client, PathBuf::from("."), Arc::new(AtomicBool::new(false)));
        editor.state.objective = "Run focused tests".into();
        editor.state.create_task().unwrap();
        editor
            .state
            .handle(TaskSessionCommand::RunFocusedTests)
            .unwrap();
        editor.state.objective = "Unrelated task".into();
        editor.state.create_task().unwrap();

        editor.flush_intents();

        assert!(editor.state.intents.is_empty());
        assert!(!editor.busy_tasks.contains("task-1"));
        assert!(matches!(
            editor.state.session.task("task-1").unwrap().validation,
            ValidationStatus::Failed { .. }
        ));
        let first = editor.state.session.task("task-1").unwrap();
        assert!(first
            .thread
            .last()
            .unwrap()
            .text
            .contains("Focused tests failed:"));
        assert!(first.thread.last().unwrap().text.contains("rerun"));
        assert!(editor
            .state
            .session
            .active_task()
            .unwrap()
            .thread
            .is_empty());

        let (request_tx, request_rx) = mpsc::channel();
        editor.controller = TaskController::new(move |request, _| {
            request_tx.send(request).unwrap();
            Ok(ProviderReply::new("Repair the test configuration."))
        });
        editor.state.session.switch_task("task-1").unwrap();
        editor.state.reply = "Fix the focused test failure".into();
        editor.state.handle(TaskSessionCommand::SendReply).unwrap();
        editor.flush_intents();
        let request = request_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(request.task_id.as_str(), "task-1");
        assert!(request
            .context
            .iter()
            .any(|entry| entry.text.contains("Focused tests failed:")));
    }

    #[test]
    fn capture_waits_for_completed_png_evidence() {
        let (client, server) = live_session(4);
        let bytes = test_png();
        let path = temp_png_path("capture");
        std::fs::write(&path, &bytes).unwrap();
        let sha256 = format!("{:x}", Sha256::digest(&bytes));
        let canceled = AtomicBool::new(false);
        let worker = std::thread::spawn(move || {
            capture_frame(
                &client,
                41,
                "editor-task-1-1".into(),
                &canceled,
                Duration::from_secs(2),
            )
        });
        let started = Instant::now();
        let request = loop {
            if let Some(request) = server.drain(1).pop() {
                break request;
            }
            assert!(started.elapsed() < Duration::from_secs(1));
            std::thread::yield_now();
        };
        assert!(matches!(request.command, LiveCommand::CaptureFrame { .. }));
        server
            .respond(
                LiveResponse::success(
                    request.request_id,
                    7,
                    "capture_completed",
                    json!({
                        "artifact": "editor-task-1-1",
                        "path": path,
                        "scheduled_tick": 6,
                        "captured_tick": 7,
                        "byte_length": bytes.len(),
                        "width": 2,
                        "height": 1,
                        "sha256": sha256
                    }),
                )
                .with_runtime_identity(test_runtime_identity()),
            )
            .unwrap();
        let evidence = worker.join().unwrap().unwrap();
        assert_eq!((evidence.width, evidence.height), (2, 1));
        assert_eq!((evidence.scheduled_tick, evidence.captured_tick), (6, 7));
        assert_eq!(evidence.bytes, bytes);
        let _ = std::fs::remove_file(evidence.path);
    }

    #[test]
    fn provider_rejects_a_screenshot_changed_after_preview() {
        let path = temp_png_path("changed");
        let previewed = test_png();
        let expected = format!("{:x}", Sha256::digest(&previewed));
        std::fs::write(&path, &previewed).unwrap();
        verify_screenshot_file(&path, &expected).unwrap();
        std::fs::write(&path, b"replacement").unwrap();

        let error = verify_screenshot_file(&path, &expected).unwrap_err();

        assert!(error.contains("changed after it was previewed"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn completed_capture_attaches_only_to_still_active_task() {
        let (client, _server) = live_session(4);
        let mut editor =
            DesktopEditor::new(client, PathBuf::from("."), Arc::new(AtomicBool::new(false)));
        editor.state.objective = "First task".into();
        editor.state.create_task().unwrap();
        editor
            .state
            .session
            .task_mut("task-1")
            .unwrap()
            .set_vision_capability(true)
            .unwrap();
        let canceled = Arc::new(AtomicBool::new(false));
        editor.capture = Some(PendingCapture {
            task_id: TaskId::new("task-1"),
            screenshot_id: "game-1".into(),
            request_id: 10_001,
            canceled,
        });
        editor.state.objective = "Second task".into();
        editor.state.create_task().unwrap();
        editor
            .capture_result_tx
            .send(CaptureResult {
                task_id: TaskId::new("task-1"),
                screenshot_id: "game-1".into(),
                request_id: 10_001,
                result: Ok(CaptureEvidence {
                    path: PathBuf::from("stale.png"),
                    bytes: test_png(),
                    width: 2,
                    height: 1,
                    scheduled_tick: 1,
                    captured_tick: 2,
                    sha256: format!("{:x}", Sha256::digest(test_png())),
                    runtime_identity: test_runtime_identity(),
                }),
            })
            .unwrap();

        editor.poll_capture();

        assert!(editor
            .state
            .session
            .task("task-1")
            .unwrap()
            .screenshots
            .is_empty());
        assert!(editor.state.preview.is_none());
        assert!(editor.state.notice.as_deref().unwrap().contains("obsolete"));
    }

    #[test]
    fn completed_capture_retains_provenance_hash_and_preview() {
        let (client, _server) = live_session(4);
        let mut editor =
            DesktopEditor::new(client, PathBuf::from("."), Arc::new(AtomicBool::new(false)));
        editor.state.objective = "Inspect game".into();
        editor.state.create_task().unwrap();
        editor
            .state
            .session
            .task_mut("task-1")
            .unwrap()
            .set_vision_capability(true)
            .unwrap();
        let bytes = test_png();
        let sha256 = format!("{:x}", Sha256::digest(&bytes));
        editor.capture = Some(PendingCapture {
            task_id: TaskId::new("task-1"),
            screenshot_id: "game-1".into(),
            request_id: 10_001,
            canceled: Arc::new(AtomicBool::new(false)),
        });
        editor
            .capture_result_tx
            .send(CaptureResult {
                task_id: TaskId::new("task-1"),
                screenshot_id: "game-1".into(),
                request_id: 10_001,
                result: Ok(CaptureEvidence {
                    path: PathBuf::from("captured.png"),
                    bytes,
                    width: 2,
                    height: 1,
                    scheduled_tick: 8,
                    captured_tick: 9,
                    sha256: sha256.clone(),
                    runtime_identity: test_runtime_identity(),
                }),
            })
            .unwrap();

        editor.poll_capture();

        let screenshot = editor
            .state
            .session
            .task("task-1")
            .unwrap()
            .screenshots
            .values()
            .next()
            .unwrap();
        assert_eq!(screenshot.provenance.task_id, TaskId::new("task-1"));
        assert_eq!(screenshot.content_sha256.as_deref(), Some(sha256.as_str()));
        assert!(matches!(screenshot.upload, UploadState::Pending));
        assert!(matches!(
            screenshot.analysis,
            ScreenshotAnalysisState::Pending
        ));
        let preview = editor.state.preview.as_ref().unwrap();
        assert_eq!((preview.width, preview.height), (2, 1));
        assert_eq!(preview.runtime_identity.session_id, "runtime-test");
    }

    #[test]
    fn cancel_suppresses_a_late_capture_result() {
        let (client, _server) = live_session(4);
        let mut editor =
            DesktopEditor::new(client, PathBuf::from("."), Arc::new(AtomicBool::new(false)));
        editor.state.objective = "Cancelable capture".into();
        editor.state.create_task().unwrap();
        let canceled = Arc::new(AtomicBool::new(false));
        editor.capture = Some(PendingCapture {
            task_id: TaskId::new("task-1"),
            screenshot_id: "game-1".into(),
            request_id: 10_001,
            canceled: Arc::clone(&canceled),
        });

        assert!(editor.cancel_capture_for(&TaskId::new("task-1")));
        assert!(canceled.load(Ordering::Acquire));
        editor
            .capture_result_tx
            .send(CaptureResult {
                task_id: TaskId::new("task-1"),
                screenshot_id: "game-1".into(),
                request_id: 10_001,
                result: Err("capture canceled".into()),
            })
            .unwrap();
        editor.poll_capture();

        assert!(editor
            .state
            .session
            .active_task()
            .unwrap()
            .screenshots
            .is_empty());
        assert!(editor.state.notice.as_deref().unwrap().contains("canceled"));
    }

    #[test]
    fn reconnect_without_a_request_preserves_disconnected_state() {
        let (client, server) = live_session(4);
        let mut editor =
            DesktopEditor::new(client, PathBuf::from("."), Arc::new(AtomicBool::new(false)));
        editor.state.objective = "Reconnect task".into();
        editor.state.create_task().unwrap();
        editor.state.session.disconnect().unwrap();
        editor.state.handle(TaskSessionCommand::Reconnect).unwrap();

        editor.flush_intents();

        assert!(editor.state.intents.is_empty());
        assert!(matches!(
            editor.state.session.active_task().unwrap().connection,
            ConnectionState::Disconnected
        ));
        assert!(editor
            .state
            .notice
            .as_deref()
            .unwrap()
            .contains("no AI request"));
        assert!(server.drain(4).is_empty());
    }

    #[test]
    fn provider_usage_reads_both_supported_transport_shapes() {
        let openrouter = serde_json::json!({
            "tokens": {"prompt": 12, "completion": 7},
            "cost": 0.00125
        });
        assert_eq!(
            provider_reply_usage(Some(&openrouter)),
            ProviderUsage {
                input_tokens: 12,
                output_tokens: 7,
                estimated_cost_micros: 1_250,
            }
        );
        let codex = serde_json::json!({
            "tokens": {"input_tokens": 20, "output_tokens": 5}
        });
        assert_eq!(provider_reply_usage(Some(&codex)).input_tokens, 20);
        assert_eq!(provider_reply_usage(Some(&codex)).output_tokens, 5);
        let codex_state = provider_reply_state(
            &ProviderConfig::Codex,
            Some(&serde_json::json!({
                "resolved_provider": "installed_codex_subscription",
                "resolved_model": "test-model",
                "route": "direct"
            })),
        );
        assert!(matches!(
            codex_state.routing,
            RoutingState::Assigned { route } if route == "direct"
        ));
    }

    #[test]
    fn openrouter_response_displays_the_resolved_route_without_raw_route_json() {
        let config = ProviderConfig::OpenRouter(stasis_ai::OpenRouterConfig {
            api_key: "test-only".into(),
            base_url: "https://example.invalid".into(),
            model: "example/model".into(),
            routing: stasis_ai::RoutingConfig::default(),
            timeout: Duration::from_secs(1),
        });
        let usage = serde_json::json!({
            "resolved_provider": "cerebras",
            "resolved_model": "example/model",
            "route": {"sort": "throughput", "allow_fallbacks": true},
            "fallback": true
        });

        let state = provider_reply_state(&config, Some(&usage));

        assert_eq!(state.provider.as_deref(), Some("cerebras"));
        assert!(matches!(
            state.routing,
            RoutingState::Assigned { route } if route == "openrouter:cerebras"
        ));
        assert!(matches!(state.fallback, FallbackState::Active { .. }));
    }

    #[test]
    fn flush_does_not_send_reconnect_to_the_live_game_queue() {
        let (client, server) = live_session(1);
        let mut editor =
            DesktopEditor::new(client, PathBuf::from("."), Arc::new(AtomicBool::new(false)));
        editor.state.objective = "Reconnect task".into();
        editor.state.create_task().unwrap();
        editor.state.session.disconnect().unwrap();
        editor.state.handle(TaskSessionCommand::Reconnect).unwrap();

        editor.flush_intents();

        assert!(editor.state.intents.is_empty());
        assert!(server.drain(1).is_empty());
    }

    #[test]
    fn busy_task_restores_the_unsent_reply_draft() {
        let (client, _server) = live_session(1);
        let barrier = Arc::new(Barrier::new(2));
        let worker_barrier = Arc::clone(&barrier);
        let mut editor =
            DesktopEditor::new(client, PathBuf::from("."), Arc::new(AtomicBool::new(false)));
        editor.controller = TaskController::new(move |_, _| {
            worker_barrier.wait();
            Ok(ProviderReply::new("finished"))
        });
        editor.state.objective = "Busy task".into();
        editor.state.create_task().unwrap();
        editor.state.reply = "first".into();
        editor.state.handle(TaskSessionCommand::SendReply).unwrap();
        editor.flush_intents();
        editor.state.reply = "keep this draft".into();
        editor.state.handle(TaskSessionCommand::SendReply).unwrap();

        editor.flush_intents();

        assert_eq!(editor.state.reply, "keep this draft");
        assert_eq!(editor.state.session.active_task().unwrap().thread.len(), 1);
        assert!(editor
            .state
            .notice
            .as_deref()
            .is_some_and(|notice| notice.contains("already has an AI request")));
        barrier.wait();
    }

    fn key_event(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    #[test]
    fn function_and_navigation_keys_do_not_alias_character_shortcuts() {
        let state = EditorState::default();
        let ctrl_shift = egui::Modifiers {
            ctrl: true,
            shift: true,
            ..egui::Modifiers::default()
        };

        assert_eq!(
            EditorState::chord(&key_event(egui::Key::F1, egui::Modifiers::CTRL))
                .and_then(|chord| state.shortcuts.command_for(chord)),
            None
        );
        assert_eq!(
            EditorState::chord(&key_event(egui::Key::Delete, ctrl_shift))
                .and_then(|chord| state.shortcuts.command_for(chord)),
            None
        );
        assert_eq!(
            EditorState::chord(&key_event(egui::Key::F, egui::Modifiers::CTRL))
                .and_then(|chord| state.shortcuts.command_for(chord)),
            Some(TaskSessionCommand::Search)
        );
    }

    #[test]
    fn proposal_tools_require_hashes_for_existing_symbols() {
        let mut tools = ProposalTools::default();
        let observations = tools.execute(
            &[ToolCall {
                tool: "propose_semantic_edit".to_string(),
                args: json!({
                    "proposal_id": "edit-speed",
                    "description": "Update speed",
                    "batch": {
                        "schema_version": 1,
                        "edits": [{
                            "operation": "update",
                            "target": {"name": "tick", "file": "src/main.stasis"},
                            "new_source": "function tick(): i32 { return 1; }"
                        }]
                    }
                }),
            }],
            &AtomicBool::new(false),
        );

        assert!(tools.proposals.is_empty());
        assert!(observations[0]
            .error
            .as_deref()
            .is_some_and(|error| error.contains("require expected_source_hash")));
    }

    #[test]
    fn repaired_proposal_is_structured_and_keeps_its_action_id() {
        let mut tools = ProposalTools::default();
        let observations = tools.execute(
            &[ToolCall {
                tool: "repair_semantic_edit".to_string(),
                args: json!({
                    "proposal_id": "edit-speed",
                    "description": "Repair speed update",
                    "batch": {
                        "schema_version": 1,
                        "edits": [{
                            "operation": "update",
                            "target": {"name": "tick", "file": "src/main.stasis"},
                            "new_source": "function tick(): i32 { return 1; }",
                            "expected_source_hash": "0123456789abcdef"
                        }]
                    }
                }),
            }],
            &AtomicBool::new(false),
        );

        assert!(observations[0].error.is_none());
        assert_eq!(tools.proposals.len(), 1);
        assert_eq!(tools.proposals[0].id, "edit-speed");
        assert!(tools.proposals[0].repair);
    }

    #[test]
    fn apply_receipt_requires_executed_test_evidence_to_unlock_done() {
        assert!(!receipt_has_test_evidence(&json!({
            "validation": {"test_result": {"tests_run": 0, "scenario_cases_run": 0}}
        })));
        assert!(receipt_has_test_evidence(&json!({
            "validation": {"test_result": {"tests_run": 1, "scenario_cases_run": 0}}
        })));
    }

    #[test]
    fn focused_test_failure_drains_only_to_originating_task_after_switch() {
        let (client, _server) = live_session(1);
        let mut editor =
            DesktopEditor::new(client, PathBuf::from("."), Arc::new(AtomicBool::new(false)));
        editor.state.objective = "First task".into();
        editor.state.create_task().unwrap();
        editor.state.session.begin_focused_tests().unwrap();
        let first_run_id = editor
            .state
            .session
            .active_task()
            .unwrap()
            .validation_run_id;
        editor.state.objective = "Second task".into();
        editor.state.create_task().unwrap();

        let (request_tx, _request_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        editor.host = HostExecutor {
            requests: Some(request_tx),
            results: result_rx,
            canceled: Arc::new(Mutex::new(BTreeSet::new())),
            shutdown: Arc::new(AtomicBool::new(false)),
            worker: None,
        };
        editor.busy_tasks.insert("task-1".to_string());
        result_tx
            .send(HostResult {
                task_id: "task-1".to_string(),
                operation: HostOperation::Test {
                    paths: Vec::new(),
                    run_id: first_run_id,
                },
                result: Err("test player_speed failed at tests/player.test.stasis".to_string()),
            })
            .unwrap();

        editor.poll_host();

        let first = editor.state.session.task("task-1").unwrap();
        assert!(matches!(first.validation, ValidationStatus::Failed { .. }));
        assert!(first.thread.last().unwrap().text.contains("player_speed"));
        let second = editor.state.session.task("task-2").unwrap();
        assert!(second.thread.is_empty());
        assert_eq!(
            editor.state.session.active_task_id().unwrap().as_str(),
            "task-2"
        );
    }

    #[test]
    fn host_shutdown_waits_for_in_flight_work() {
        let (request_tx, _request_rx) = mpsc::channel();
        let (_result_tx, result_rx) = mpsc::channel();
        let (started_tx, started_rx) = mpsc::channel();
        let (shutdown_tx, shutdown_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (dropped_tx, dropped_rx) = mpsc::channel();
        let shutdown = Arc::new(AtomicBool::new(false));
        let worker_shutdown = Arc::clone(&shutdown);
        let finished = Arc::new(AtomicBool::new(false));
        let worker_finished = Arc::clone(&finished);
        let worker = thread::spawn(move || {
            started_tx.send(()).unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while !worker_shutdown.load(Ordering::Acquire) {
                assert!(std::time::Instant::now() < deadline);
                thread::yield_now();
            }
            shutdown_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            // Represents completion of receipt publication or rollback.
            worker_finished.store(true, Ordering::Release);
        });
        let host = HostExecutor {
            requests: Some(request_tx),
            results: result_rx,
            canceled: Arc::new(Mutex::new(BTreeSet::new())),
            shutdown,
            worker: Some(worker),
        };
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let dropper = thread::spawn(move || {
            drop(host);
            dropped_tx.send(finished.load(Ordering::Acquire)).unwrap();
        });
        shutdown_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(
            dropped_rx.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        release_tx.send(()).unwrap();
        assert!(dropped_rx.recv_timeout(Duration::from_secs(5)).unwrap());
        dropper.join().unwrap();
    }

    #[test]
    fn host_shutdown_disconnects_an_idle_worker() {
        let host = HostExecutor::new(PathBuf::from("missing-workspace"));
        let (done_tx, done_rx) = mpsc::channel();
        let dropper = thread::spawn(move || {
            drop(host);
            done_tx.send(()).unwrap();
        });
        done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        dropper.join().unwrap();
    }

    #[test]
    fn canceled_task_is_rejected_before_queued_host_execution() {
        let host = HostExecutor::new(PathBuf::from("missing-workspace"));
        host.cancel("task-canceled");
        host.submit(HostRequest {
            task_id: "task-canceled".to_string(),
            operation: HostOperation::Test {
                paths: Vec::new(),
                run_id: 1,
            },
            source_before: Some("unused".to_string()),
        })
        .unwrap();

        let completed = host
            .results
            .recv_timeout(Duration::from_secs(1))
            .expect("canceled request returns without touching the workspace");
        assert_eq!(completed.task_id, "task-canceled");
        assert!(completed
            .result
            .expect_err("canceled request cannot execute")
            .contains("canceled before execution"));
    }

    #[test]
    fn obsolete_and_canceled_test_failures_do_not_change_task_context() {
        for canceled in [false, true] {
            let (client, _server) = live_session(1);
            let mut editor =
                DesktopEditor::new(client, PathBuf::from("."), Arc::new(AtomicBool::new(false)));
            editor.state.objective = "Validate task".into();
            editor.state.create_task().unwrap();
            let task = editor.state.session.active_task_mut().unwrap();
            task.begin_focused_tests().unwrap();
            let old_run = task.validation_run_id;
            if canceled {
                task.cancel().unwrap();
            } else {
                task.finish_focused_tests(FocusedTestResult::failed("previous failure"))
                    .unwrap();
                task.begin_focused_tests().unwrap();
            }
            let before = task.clone();
            let (request_tx, _request_rx) = mpsc::channel();
            let (result_tx, result_rx) = mpsc::channel();
            editor.host = HostExecutor {
                requests: Some(request_tx),
                results: result_rx,
                canceled: Arc::new(Mutex::new(BTreeSet::new())),
                shutdown: Arc::new(AtomicBool::new(false)),
                worker: None,
            };
            result_tx
                .send(HostResult {
                    task_id: "task-1".into(),
                    operation: HostOperation::Test {
                        paths: Vec::new(),
                        run_id: old_run,
                    },
                    result: Err("late failure".into()),
                })
                .unwrap();

            editor.poll_host();

            assert_eq!(editor.state.session.active_task().unwrap(), &before);
            assert!(editor.validation_receipts.is_empty());
            assert!(editor.validation_fingerprints.is_empty());
        }
    }

    #[test]
    fn accepted_action_executes_and_completes_its_originating_task() {
        let root = super::super::tests::desktop_editor_fixture("editor_host_execution");
        let item = super::super::desktop_source_context(&root)
            .unwrap()
            .into_iter()
            .find(|item| item["target"]["name"] == "value")
            .unwrap();
        let batch = json!({"schema_version": 1, "edits": [{
            "operation": "update",
            "target": item["target"],
            "expected_source_hash": item["expected_source_hash"],
            "new_source": "function value(): i32 { return 2; }"
        }]});
        let (client, _server) = live_session(1);
        let mut editor = DesktopEditor::new(client, root.clone(), Arc::new(AtomicBool::new(false)));
        editor.state.objective = "Change value".into();
        editor.state.create_task().unwrap();
        editor
            .state
            .session
            .active_task_mut()
            .unwrap()
            .propose_action_with_payload(
                "edit-value",
                stasis_ai::ActionKind::Edit,
                "Change value to two",
                batch,
            )
            .unwrap();
        let preview_deadline = Instant::now() + Duration::from_secs(10);
        while editor
            .state
            .reviewed_preview("task-1", "edit-value")
            .is_err()
            && Instant::now() < preview_deadline
        {
            editor.poll_semantic_previews();
            thread::sleep(Duration::from_millis(10));
        }
        editor
            .state
            .handle(TaskSessionCommand::AcceptAction)
            .unwrap();
        editor
            .state
            .handle(TaskSessionCommand::ApplyAction)
            .unwrap();
        editor.flush_intents();
        editor.state.objective = "Unrelated task".into();
        editor.state.create_task().unwrap();

        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while editor.busy_tasks.contains("task-1") && std::time::Instant::now() < deadline {
            editor.poll_host();
            thread::yield_now();
        }

        let first = editor.state.session.task("task-1").unwrap();
        assert!(matches!(
            first.actions["edit-value"].state,
            ActionState::Applied
        ));
        assert!(matches!(first.validation, ValidationStatus::Passed { .. }));
        assert!(editor
            .execution_receipts
            .contains_key(&("task-1".to_string(), "edit-value".to_string())));
        assert_eq!(
            editor.state.session.active_task_id().unwrap().as_str(),
            "task-2"
        );

        editor.state.session.switch_task("task-1").unwrap();
        let source_path = root.join("src/main.stasis");
        let mut source = std::fs::read_to_string(&source_path).unwrap();
        source.push_str("\n// external edit after validation\n");
        std::fs::write(&source_path, source).unwrap();
        editor.state.handle(TaskSessionCommand::MarkDone).unwrap();
        editor.flush_intents();
        assert_eq!(
            editor.state.session.task("task-1").unwrap().lifecycle,
            stasis_ai::TaskLifecycle::Active
        );
        assert!(editor
            .state
            .notice
            .as_ref()
            .unwrap()
            .contains("sources changed"));
        editor
            .state
            .handle(TaskSessionCommand::RunFocusedTests)
            .unwrap();
        editor.flush_intents();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while editor.busy_tasks.contains("task-1") && std::time::Instant::now() < deadline {
            editor.poll_host();
            thread::yield_now();
        }
        assert!(editor.validation_receipts.contains_key("task-1"));
        editor.state.handle(TaskSessionCommand::MarkDone).unwrap();
        editor.flush_intents();
        assert!(matches!(
            editor.state.session.task("task-1").unwrap().lifecycle,
            stasis_ai::TaskLifecycle::Completed
        ));
        super::super::tests::remove_temp(&root);
    }

    #[test]
    fn semantic_preview_fingerprint_refresh_requires_a_usable_preview() {
        let (mut editor, root, _) = review_fixture("preview_editor_fingerprint_gate");
        let mut next_check = Instant::now();
        let mut reads = 0;
        let due = next_check + Duration::from_secs(1);

        refresh_semantic_preview_staleness(
            &mut editor.state.semantic_previews,
            &mut next_check,
            due,
            || {
                reads += 1;
                Ok("unused".into())
            },
        );
        assert_eq!(reads, 0, "empty preview cache must not scan the workspace");

        editor.next_semantic_check = Instant::now() + Duration::from_secs(60);
        editor.poll_semantic_previews();
        let mut next_check = Instant::now();
        let due = next_check + Duration::from_secs(1);
        refresh_semantic_preview_staleness(
            &mut editor.state.semantic_previews,
            &mut next_check,
            due,
            || {
                reads += 1;
                Ok("unused".into())
            },
        );
        assert_eq!(reads, 0, "a pending preview must not scan the workspace");

        editor.next_semantic_check = Instant::now() + Duration::from_secs(60);
        finish_preview(&mut editor);
        let expected = editor
            .state
            .semantic_previews
            .values()
            .find_map(|record| match &record.result {
                Some(Ok(preview)) => Some(preview.source_fingerprint.clone()),
                _ => None,
            })
            .expect("fixture should produce a usable preview");

        let mut next_check = Instant::now();
        let due = next_check + Duration::from_secs(1);
        refresh_semantic_preview_staleness(
            &mut editor.state.semantic_previews,
            &mut next_check,
            due,
            || {
                reads += 1;
                Ok(expected.clone())
            },
        );
        assert_eq!(reads, 1, "a usable preview should trigger one scan");

        for record in editor.state.semantic_previews.values_mut() {
            record.stale = true;
        }
        let mut next_check = Instant::now();
        let due = next_check + Duration::from_secs(1);
        refresh_semantic_preview_staleness(
            &mut editor.state.semantic_previews,
            &mut next_check,
            due,
            || {
                reads += 1;
                Ok(expected.clone())
            },
        );
        assert_eq!(reads, 1, "a stale preview must not scan again");

        for record in editor.state.semantic_previews.values_mut() {
            record.stale = false;
            record.result = Some(Err("preview failed".into()));
        }
        let mut next_check = Instant::now();
        let due = next_check + Duration::from_secs(1);
        refresh_semantic_preview_staleness(
            &mut editor.state.semantic_previews,
            &mut next_check,
            due,
            || {
                reads += 1;
                Ok(expected.clone())
            },
        );
        assert_eq!(reads, 1, "a failed preview must not scan the workspace");

        super::super::tests::remove_temp(&root);
    }
}
