mod semantic_diff;

use eframe::egui::{self, Color32, RichText};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use stasis_ai::task_session::{
    ActionState, FallbackState, Key, KeyChord, Modifiers, ProviderState, RoutingState,
    ScreenshotAnalysisState, ShortcutMapper, TaskId, TaskSession, TaskSessionCommand,
    ThreadEntryKind, UploadState,
};
use stasis_ai::{
    action_id_for_tool, run_agent_with_profile, AgentEvent, AgentProfile, ProviderActionProposal,
    ProviderConfig, ProviderReply, ProviderRequest, ProviderUsage, TaskController,
    TaskControllerEvent, ToolCall, ToolExecutor, ToolObservation, ToolSpec,
};
use stasis_runner::live::{LiveCommand, LiveRequest, LiveRuntimeIdentity, LiveSessionClient};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
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

fn run_reply_provider(
    request: ProviderRequest,
    canceled: Arc<AtomicBool>,
    project_root: PathBuf,
) -> Result<ProviderReply, String> {
    let config = ProviderConfig::from_env()?;
    let image_paths = verified_provider_screenshot_paths(&config, &request)?;
    let mut provider = config
        .clone()
        .build()?
        .with_timeout(Duration::from_secs(120))
        .with_images(image_paths)?;
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
    if !config.supports_image_input() {
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
    task.append_result(format!(
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
    after_entries: usize,
    description: String,
    result: Option<Result<super::DesktopSemanticPreview, String>>,
    stale: bool,
}

struct SemanticPreviewJob {
    key: SemanticPreviewKey,
    result: Receiver<Result<super::DesktopSemanticPreview, String>>,
    worker: thread::JoinHandle<()>,
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
                for (revision, previous) in action.revisions.iter().enumerate() {
                    let Some(payload) = &previous.payload else {
                        continue;
                    };
                    let key = SemanticPreviewKey::new(
                        task.id.as_str(),
                        action.id.as_str(),
                        revision,
                        payload,
                    );
                    self.state.semantic_previews.entry(key).or_insert_with(|| SemanticPreviewRecord {
                        after_entries: previous.thread_position,
                        description: previous.description.clone(),
                        result: Some(Err("Historical revision has no retained preview; it will not be regenerated".into())),
                        stale: false,
                    });
                }
                let Some(payload) = &action.payload else {
                    continue;
                };
                let key = SemanticPreviewKey::new(
                    task.id.as_str(),
                    action.id.as_str(),
                    action.revisions.len(),
                    payload,
                );
                self.state
                    .semantic_previews
                    .entry(key.clone())
                    .or_insert_with(|| SemanticPreviewRecord {
                        after_entries: action.thread_position,
                        description: action.description.clone(),
                        result: if matches!(action.state, ActionState::Proposed) {
                            None
                        } else {
                            Some(Err("No retained preview for this revision".into()))
                        },
                        stale: false,
                    });
                if self.state.semantic_previews[&key].result.is_none() {
                    queued.push((key, payload.clone()));
                }
            }
        }
        if Instant::now() >= self.next_semantic_check {
            self.next_semantic_check = Instant::now() + Duration::from_millis(500);
            let current = super::desktop_source_fingerprint(&self.project_root, &[]);
            for record in self.state.semantic_previews.values_mut() {
                if let Some(Ok(preview)) = &record.result {
                    record.stale |= current
                        .as_ref()
                        .map_or(true, |hash| hash != &preview.source_fingerprint);
                }
            }
        }
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
    path: PathBuf,
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
    evidence: CaptureEvidence,
) -> Result<ScreenshotPreview, String> {
    let decoded = image::load_from_memory_with_format(&evidence.bytes, image::ImageFormat::Png)
        .map_err(|error| format!("captured frame is not a valid PNG: {error}"))?
        .to_rgba8();
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
        path: evidence.path,
        rgba: decoded.into_raw(),
        width: width as usize,
        height: height as usize,
        scheduled_tick: evidence.scheduled_tick,
        captured_tick: evidence.captured_tick,
        sha256: evidence.sha256,
        runtime_identity: evidence.runtime_identity,
    })
}

impl DesktopEditor {
    fn new(client: LiveSessionClient, project_root: PathBuf, shutdown: Arc<AtomicBool>) -> Self {
        let host = HostExecutor::new(project_root.clone());
        let provider_root = project_root.clone();
        let (capture_result_tx, capture_results) = mpsc::channel();
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
        let mut pending = Vec::with_capacity(intents.len());
        for intent in intents {
            match intent {
                EditorIntent::SendReply(task, text) => {
                    let task = TaskId::new(task);
                    let mut candidate = self.state.session.clone();
                    let accepted = candidate
                        .task_mut(&task)
                        .and_then(|task| task.append_reply(&text))
                        .map_err(|error| error.to_string())
                        .and_then(|()| {
                            if let Ok(config) = ProviderConfig::from_env() {
                                candidate
                                    .task_mut(&task)
                                    .and_then(|task| {
                                        task.set_provider_state(configured_provider_state(&config))
                                    })
                                    .map_err(|error| error.to_string())?;
                            }
                            let task = candidate
                                .task_mut(&task)
                                .map_err(|error| error.to_string())?;
                            for screenshot in task.screenshots.values_mut() {
                                screenshot.upload = UploadState::Pending;
                                screenshot.analysis = ScreenshotAnalysisState::Pending;
                            }
                            Ok(())
                        })
                        .and_then(|()| {
                            self.controller
                                .send(&candidate, &task)
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
                            let _ = task_state.append_result(format!(
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
                EditorIntent::Screenshot(task) => self.start_capture(TaskId::new(task)),
                intent => pending.push(intent),
            }
        }
        self.state.intents = pending;
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
        let config = match ProviderConfig::from_env() {
            Ok(config) => config,
            Err(error) => {
                self.state.notice = Some(format!("Cannot attach screenshot: {error}"));
                return;
            }
        };
        if !config.supports_image_input() {
            self.state.notice = Some(format!(
                "Cannot attach screenshot: selected {} model {} does not support image input.",
                config.provider_name(),
                config.model()
            ));
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
                    match screenshot_preview(&completed.task_id, &completed.screenshot_id, evidence)
                    {
                        Ok(preview) => {
                            let source = preview.path.to_string_lossy().into_owned();
                            match self
                                .state
                                .session
                                .task_mut(&completed.task_id)
                                .and_then(|task| {
                                    task.attach_screenshot_with_sha256(
                                        completed.screenshot_id.as_str(),
                                        source,
                                        preview.sha256.clone(),
                                    )
                                }) {
                                Ok(()) => {
                                    self.state.notice = Some(format!(
                                        "Captured {}x{} game frame for {}.",
                                        preview.width, preview.height, completed.task_id
                                    ));
                                    self.state.preview = Some(preview);
                                    self.preview_texture = None;
                                }
                                Err(error) => self.state.notice = Some(error.to_string()),
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
                            task.append_result(format!(
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
                        let _ = task.append_result(format!(
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
                            task.append_result(format!(
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
        ui.heading("AI tasks");
        ui.label(
            RichText::new(self.project_root.display().to_string())
                .small()
                .weak(),
        );
        ui.horizontal(|ui| {
            let objective = ui.text_edit_singleline(&mut self.state.objective);
            if self.state.focus == FocusArea::Tasks {
                objective.request_focus();
            }
            let submitted =
                objective.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
            if ui.button("Create").clicked() || submitted {
                self.state.notice = self.state.create_task().err();
            }
        });
        ui.separator();
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
        for (id, objective, lifecycle, connection, elapsed, cost, retries) in cards {
            let request = self.controller.snapshot(&TaskId::new(&id));
            let request_status = request
                .as_ref()
                .map(|snapshot| format!(" | {:?}", snapshot.state))
                .unwrap_or_default();
            let retries = request
                .as_ref()
                .map_or(retries, |snapshot| snapshot.retry_count);
            let label = format!(
                "{objective}\n{lifecycle:?} | {connection:?}{request_status} | {elapsed}ms | ${:.4} | retry {retries}",
                cost as f64 / 1_000_000.0
            );
            if ui
                .selectable_label(active.as_deref() == Some(&id), label)
                .clicked()
            {
                self.state.notice = self.state.switch_task(&id).err().map(|e| e.to_string());
            }
        }
    }
}

#[derive(Debug)]
struct EditorState {
    session: TaskSession,
    shortcuts: ShortcutMapper,
    focus: FocusArea,
    task_fraction: f32,
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
    project_root: Option<PathBuf>,
    semantic_previews: BTreeMap<SemanticPreviewKey, SemanticPreviewRecord>,
}

impl Default for EditorState {
    fn default() -> Self {
        Self {
            session: TaskSession::new(),
            shortcuts: ShortcutMapper::new(),
            focus: FocusArea::Tasks,
            task_fraction: 0.42,
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
        let key =
            SemanticPreviewKey::new(task, action.id.as_str(), action.revisions.len(), payload);
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

    fn pane_widths(&self, available: f32) -> (f32, f32) {
        let task = if available <= 680.0 {
            (available * self.task_fraction).clamp(0.0, available)
        } else {
            (available * self.task_fraction).clamp(320.0, available - 360.0)
        };
        (task, available - task)
    }

    fn set_task_width(&mut self, width: f32, available: f32) {
        if available > 0.0 {
            self.task_fraction = width / available;
            self.task_fraction = self.pane_widths(available).0 / available;
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
                self.intents.push(EditorIntent::Cancel(task));
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
    fn detail(&mut self, ui: &mut egui::Ui) {
        let Ok(task) = self.state.session.active_task() else {
            ui.centered_and_justified(|ui| {
                ui.label("Create a focused task to start a fresh AI thread.");
            });
            return;
        };
        let task = task.clone();
        ui.horizontal_wrapped(|ui| {
            ui.heading(&task.objective);
            ui.label(format!(
                "{:?} | {:?} | {:?}",
                task.validation, task.lifecycle, task.connection
            ));
        });
        ui.label(format!(
            "Provider: {} | Model: {} | Route: {:?} | Fallback: {:?}",
            task.provider.provider.as_deref().unwrap_or("unresolved"),
            task.provider.model.as_deref().unwrap_or("unresolved"),
            task.provider.routing,
            task.provider.fallback
        ));
        ui.label(format!(
            "{}ms | {} input / {} output tokens | ${:.4} | {} retries",
            task.metrics.elapsed_ms,
            task.metrics.input_tokens,
            task.metrics.output_tokens,
            task.metrics.estimated_cost_micros as f64 / 1_000_000.0,
            task.metrics.retry_count,
        ));
        if let Some(request) = self.controller.snapshot(&task.id) {
            ui.label(format!(
                "Request {} | {:?} | {}ms | retry {}{}",
                request.request_id.get(),
                request.state,
                request.elapsed_ms,
                request.retry_count,
                request
                    .error
                    .as_deref()
                    .map(|error| format!(" | {error}"))
                    .unwrap_or_default(),
            ));
        }
        ui.separator();
        egui::ScrollArea::vertical()
            .id_source("task-thread")
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for position in 0..=task.thread.len() {
                    for action in task.actions.values().filter(|action| action.payload.is_none()
                        && action.thread_position.min(task.thread.len()) == position) {
                        ui.group(|ui| {
                            ui.label(RichText::new(format!("Action | {:?}", action.state)).strong());
                            ui.label(&action.description);
                        });
                    }
                    for (key, record) in self.state.semantic_previews.iter().filter(|(key, record)| {
                        key.task == task.id.as_str() && record.after_entries.min(task.thread.len()) == position
                    }) {
                        let action = task.actions.get(key.action.as_str());
                        let current = action.is_some_and(|action| action.revisions.len() == key.revision
                            && action.payload.as_ref().is_some_and(|payload| SemanticPreviewKey::new(task.id.as_str(), &key.action, key.revision, payload) == *key));
                        ui.push_id((&key.task, &key.action, key.revision, &key.payload_hash), |ui| {
                            ui.group(|ui| {
                                ui.label(RichText::new(format!("{} | Revision {}{}", key.action, key.revision + 1,
                                    if current { "" } else { " | Previous revision" })).strong());
                                if current {
                                    if let Some(action) = action { ui.label(format!("{:?}", action.state)); }
                                }
                                ui.label(&record.description);
                                if record.stale {
                                    ui.colored_label(Color32::from_rgb(245, 180, 80), "Stale: project sources changed. Acceptance and Apply disabled.");
                                }
                                match &record.result {
                                    None => { ui.spinner(); ui.label("Planning semantic changes..."); }
                                    Some(Err(error)) => { ui.colored_label(Color32::from_rgb(240, 120, 120), format!("Preview unavailable: {error}")); }
                                    Some(Ok(preview)) => {
                                        semantic_diff::render(ui, &preview.plan, "semantic-files");
                                        ui.collapsing("Preview identity", |ui| {
                                            ui.label(format!("Task {} | Action {} | Revision {}", key.task, key.action, key.revision + 1));
                                            ui.add(egui::Label::new(format!("Sources: {}\nPayload: {}", preview.source_fingerprint, key.payload_hash)).wrap(true).selectable(true));
                                        });
                                    }
                                }
                            });
                        });
                    }
                    if let Some(entry) = task.thread.get(position) {
                        let speaker = if matches!(entry.kind, ThreadEntryKind::Reply) { "You" } else { "AI" };
                        ui.group(|ui| {
                            ui.label(RichText::new(speaker).strong());
                            ui.label(&entry.text);
                        });
                    }
                }
                for screenshot in task.screenshots.values() {
                    ui.group(|ui| {
                        ui.label(
                            RichText::new(format!(
                                "Screenshot {} | upload {:?} | analysis {:?}",
                                screenshot.id, screenshot.upload, screenshot.analysis
                            ))
                            .strong(),
                        );
                        ui.label(format!(
                            "task {} | sha256 {} | {}",
                            screenshot.provenance.task_id,
                            screenshot.content_sha256.as_deref().unwrap_or("unverified"),
                            screenshot.source
                        ));
                    });
                }
            });
        ui.separator();
        let reply = ui.add_sized(
            [ui.available_width(), 72.0],
            egui::TextEdit::multiline(&mut self.state.reply).hint_text("Reply to this task..."),
        );
        if self.state.focus == FocusArea::Reply {
            reply.request_focus();
        }
        ui.horizontal_wrapped(|ui| {
            for (label, command) in [
                ("Send  Ctrl+Enter", TaskSessionCommand::SendReply),
                ("Accept  Ctrl+Y", TaskSessionCommand::AcceptAction),
                ("Reject", TaskSessionCommand::RejectAction),
                ("Apply  Ctrl+Alt+Enter", TaskSessionCommand::ApplyAction),
                ("Test  Ctrl+T", TaskSessionCommand::RunFocusedTests),
                ("Retry  Ctrl+R", TaskSessionCommand::Retry),
                ("Cancel  Ctrl+Esc", TaskSessionCommand::Cancel),
                ("Reconnect  Ctrl+Shift+R", TaskSessionCommand::Reconnect),
                ("Done  Ctrl+Shift+D", TaskSessionCommand::MarkDone),
            ] {
                if ui
                    .add_enabled(
                        self.state.review_command_enabled(&command),
                        egui::Button::new(label),
                    )
                    .clicked()
                {
                    self.state.dispatch(command);
                }
            }
        });
    }

    fn game(&mut self, ui: &mut egui::Ui) {
        let response = ui.allocate_response(ui.available_size(), egui::Sense::click());
        if response.clicked() {
            self.state.focus = FocusArea::Game;
        }
        let painter = ui.painter_at(response.rect);
        painter.rect_filled(response.rect, 6.0, Color32::from_rgb(15, 18, 24));
        let color = if self.state.focus == FocusArea::Game {
            Color32::from_rgb(115, 220, 190)
        } else {
            Color32::LIGHT_GRAY
        };
        let preview = self
            .state
            .preview
            .as_ref()
            .filter(|preview| self.state.session.active_task_id() == Some(&preview.task_id));
        if let Some(preview) = preview {
            let needs_texture = self
                .preview_texture
                .as_ref()
                .map_or(true, |(id, _)| id != &preview.screenshot_id);
            if needs_texture {
                let image = egui::ColorImage::from_rgba_unmultiplied(
                    [preview.width, preview.height],
                    &preview.rgba,
                );
                let texture = ui.ctx().load_texture(
                    format!("screenshot-preview-{}", preview.screenshot_id),
                    image,
                    egui::TextureOptions::LINEAR,
                );
                self.preview_texture = Some((preview.screenshot_id.clone(), texture));
            }
            let texture = &self.preview_texture.as_ref().expect("preview texture").1;
            let available = response.rect.shrink2(egui::vec2(12.0, 42.0));
            let scale = (available.width() / preview.width as f32)
                .min(available.height() / preview.height as f32);
            let size = egui::vec2(preview.width as f32, preview.height as f32) * scale;
            let image_rect = egui::Rect::from_center_size(available.center(), size);
            painter.image(
                texture.id(),
                image_rect,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
            painter.text(
                response.rect.left_bottom() + egui::vec2(12.0, -10.0),
                egui::Align2::LEFT_BOTTOM,
                format!(
                    "{} | {}x{} | tick {} -> {} | runtime {}:{} | sha256 {}",
                    preview.screenshot_id,
                    preview.width,
                    preview.height,
                    preview.scheduled_tick,
                    preview.captured_tick,
                    preview.runtime_identity.session_id,
                    preview.runtime_identity.generation,
                    &preview.sha256[..12]
                ),
                egui::FontId::proportional(13.0),
                color,
            );
        } else {
            painter.text(response.rect.center(), egui::Align2::CENTER_CENTER,
                "LIVE GAME\n\nThe interactive game runs in its native window\nand keeps independent keyboard and mouse focus.\n\nCtrl+Alt+G focuses this surface.",
                egui::FontId::proportional(18.0), color);
        }
        if self.state.focus == FocusArea::Game {
            painter.rect_stroke(response.rect, 6.0, egui::Stroke::new(2.0_f32, color));
        }
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
    fn ui(&mut self, context: &egui::Context) {
        if self.shutdown.load(Ordering::Acquire) {
            context.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        context.request_repaint_after(Duration::from_millis(100));
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
        egui::TopBottomPanel::top("top-bar").show(context, |ui| {
            ui.horizontal(|ui| {
                ui.strong("STASIS EDITOR");
                ui.separator();
                ui.label("Ctrl+K commands | Ctrl+N new task | Ctrl+Alt+G game");
                if let Some(notice) = &self.state.notice {
                    ui.colored_label(Color32::from_rgb(245, 180, 80), notice);
                }
            });
        });
        egui::CentralPanel::default().show(context, |ui| {
            let available = ui.available_width();
            let task_width = self.state.pane_widths(available).0;
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(task_width, ui.available_height()),
                    egui::Layout::left_to_right(egui::Align::Min),
                    |ui| {
                        egui::SidePanel::left("tasks")
                            .resizable(true)
                            .default_width(210.0)
                            .show_inside(ui, |ui| self.sidebar(ui));
                        egui::CentralPanel::default().show_inside(ui, |ui| self.detail(ui));
                    },
                );
                let splitter = ui
                    .allocate_response(egui::vec2(8.0, ui.available_height()), egui::Sense::drag());
                if splitter.dragged() {
                    self.state
                        .set_task_width(task_width + splitter.drag_delta().x, available);
                    context.request_repaint();
                }
                ui.allocate_ui(ui.available_size(), |ui| self.game(ui));
            });
        });
        self.flush_intents();
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
            .with_min_inner_size([900.0, 600.0]),
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

    fn review_fixture(name: &str) -> (DesktopEditor, PathBuf, Value) {
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

    fn finish_preview(editor: &mut DesktopEditor) {
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
            editor.state.semantic_previews[&original_key].after_entries,
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
    fn split_preserves_both_panes() {
        let mut state = EditorState::default();
        state.set_task_width(900.0, 1200.0);
        assert_eq!(state.pane_widths(1200.0), (840.0, 360.0));
        let panes = state.pane_widths(600.0);
        assert_eq!(panes.0 + panes.1, 600.0);
    }

    #[test]
    fn new_task_shortcut_enters_objective_focus() {
        let mut state = task_state();
        state.handle(TaskSessionCommand::NewTask).unwrap();
        assert_eq!(state.focus, FocusArea::Tasks);
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
        assert!(matches!(
            state.intents.last(),
            Some(EditorIntent::Cancel(task)) if task == "task-2"
        ));
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
}
