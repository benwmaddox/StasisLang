use crate::{
    ActionId, ActionKind, ActionState, ConnectionState, ProviderState, ScreenshotAnalysisState,
    ScreenshotAttachment, Task, TaskId, TaskLifecycle, TaskSession, TaskSessionError, ThreadEntry,
    UploadState, VisionCapability,
};
use serde_json::Value;
use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Instant;

const SAFE_PROVIDER_ERROR: &str = "AI provider request failed";
const SAFE_SESSION_ERROR: &str = "AI response could not be added to the task";
const MAX_WORKERS: usize = 8;
const MAX_PROGRESS_EVENTS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RequestId(u64);

impl RequestId {
    pub fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderRequest {
    pub request_id: RequestId,
    pub task_id: TaskId,
    /// Task-selected provider at admission; later task switches cannot reroute this request.
    pub selected_provider: Option<crate::task_session::ProviderSelection>,
    pub objective: String,
    pub project_summary: String,
    pub relevant_files: Vec<String>,
    pub relevant_symbols: Vec<String>,
    pub relevant_tests: Vec<String>,
    pub context: Vec<ThreadEntry>,
    pub actions: Vec<ProviderActionContext>,
    /// Immutable screenshot set selected when this provider request was admitted.
    pub screenshots: Vec<ScreenshotAttachment>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ProviderActionContext {
    pub id: ActionId,
    pub state: &'static str,
    pub description: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProviderUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub estimated_cost_micros: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderReply {
    pub text: String,
    pub provider: ProviderState,
    pub usage: ProviderUsage,
    pub proposals: Vec<ProviderActionProposal>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderActionProposal {
    pub id: String,
    pub kind: ActionKind,
    pub description: String,
    pub payload: Value,
    pub repair: bool,
}

impl ProviderReply {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            provider: ProviderState::default(),
            usage: ProviderUsage::default(),
            proposals: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskRequestState {
    Running,
    Completed,
    Failed,
    Canceled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressStage {
    Queued,
    InspectingSymbols,
    ContactingProvider,
    FirstResponse,
    FirstAction,
    PreparingProposal,
    WaitingForApproval,
    ApplyingAtomically,
    Compiling,
    RunningFocusedTests,
    FocusedTestsPassed,
    CommittingBetweenTicks,
    RollingBack,
    CancelRequested,
    Failed,
    Canceled,
    Completed,
    Fallback,
}

impl ProgressStage {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Queued => "Queued",
            Self::InspectingSymbols => "Inspecting symbols",
            Self::ContactingProvider => "Contacting provider",
            Self::FirstResponse => "First response received",
            Self::FirstAction => "First action received",
            Self::PreparingProposal => "Preparing proposal",
            Self::WaitingForApproval => "Waiting for approval",
            Self::ApplyingAtomically => "Applying atomically",
            Self::Compiling => "Compiling",
            Self::RunningFocusedTests => "Running focused tests",
            Self::FocusedTestsPassed => "Focused tests passed",
            Self::CommittingBetweenTicks => "Committing between ticks",
            Self::RollingBack => "Rolling back",
            Self::CancelRequested => "Cancel requested; finishing atomic work",
            Self::Failed => "Failed",
            Self::Canceled => "Canceled",
            Self::Completed => "Completed",
            Self::Fallback => "Using fallback",
        }
    }

    const fn is_terminal(self) -> bool {
        matches!(self, Self::Failed | Self::Canceled | Self::Completed)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgressEvent {
    pub task_id: TaskId,
    pub request_id: RequestId,
    pub sequence: u64,
    pub stage: ProgressStage,
    pub elapsed_ms: u64,
    pub provider_elapsed_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRequestSnapshot {
    pub request_id: RequestId,
    pub task_id: TaskId,
    pub state: TaskRequestState,
    pub elapsed_ms: u64,
    pub usage: ProviderUsage,
    pub retry_count: u32,
    pub error: Option<String>,
    pub progress: Vec<ProgressEvent>,
    pub provider_first_response_ms: Option<u64>,
    pub provider_first_action_ms: Option<u64>,
    pub focused_tests_passed_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskControllerEvent {
    Completed {
        request_id: RequestId,
        task_id: TaskId,
        proposals: Vec<ProviderActionProposal>,
    },
    Failed {
        request_id: RequestId,
        task_id: TaskId,
        message: String,
    },
    Canceled {
        request_id: RequestId,
        task_id: TaskId,
    },
    Stale {
        request_id: RequestId,
        task_id: TaskId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskControllerConfig {
    pub workers: usize,
    pub max_context_entries: usize,
    pub max_context_chars: usize,
}

impl Default for TaskControllerConfig {
    fn default() -> Self {
        Self {
            workers: 2,
            max_context_entries: 32,
            max_context_chars: 32 * 1024,
        }
    }
}

#[derive(Debug)]
pub enum TaskControllerError {
    InvalidConfig,
    TaskBusy(TaskId),
    TaskClosed(TaskId),
    TaskDisconnected(TaskId),
    NoPreviousRequest(TaskId),
    CapacityReached,
    Session(TaskSessionError),
    Unavailable,
}

impl fmt::Display for TaskControllerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig => f.write_str("invalid task controller configuration"),
            Self::TaskBusy(id) => write!(f, "task already has an AI request: {id}"),
            Self::TaskClosed(id) => write!(f, "task is not active: {id}"),
            Self::TaskDisconnected(id) => write!(f, "task provider is disconnected: {id}"),
            Self::NoPreviousRequest(id) => write!(f, "task has no AI request to retry: {id}"),
            Self::CapacityReached => f.write_str("AI provider controller is at capacity"),
            Self::Session(error) => error.fmt(f),
            Self::Unavailable => f.write_str("AI provider controller is unavailable"),
        }
    }
}

impl std::error::Error for TaskControllerError {}

impl From<TaskSessionError> for TaskControllerError {
    fn from(error: TaskSessionError) -> Self {
        Self::Session(error)
    }
}

type ProviderFn = dyn Fn(ProviderRequest, Arc<AtomicBool>, ProgressReporter) -> Result<ProviderReply, String>
    + Send
    + Sync
    + 'static;

struct Job {
    client_id: u64,
    request: ProviderRequest,
    canceled: Arc<AtomicBool>,
    client_alive: Arc<AtomicBool>,
    admitted: Arc<AtomicBool>,
    enqueued_at: Instant,
}

struct Completion {
    client_id: u64,
    request_id: RequestId,
    task_id: TaskId,
    elapsed_ms: u64,
    result: Result<ProviderReply, ()>,
    admitted: Arc<AtomicBool>,
}

#[derive(Clone)]
struct RequestRecord {
    request: ProviderRequest,
    canceled: Arc<AtomicBool>,
    started_at: Instant,
    snapshot: TaskRequestSnapshot,
}

#[derive(Default)]
struct SharedState {
    requests: BTreeMap<(u64, TaskId), RequestRecord>,
    completions: VecDeque<Completion>,
    events: VecDeque<(u64, TaskControllerEvent)>,
    admitted: usize,
}

#[derive(Clone)]
pub struct ProgressReporter {
    client_id: u64,
    task_id: TaskId,
    request_id: RequestId,
    started_at: Instant,
    state: Arc<Mutex<SharedState>>,
    closed: Arc<AtomicBool>,
}

impl fmt::Debug for ProgressReporter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProgressReporter")
            .field("task_id", &self.task_id)
            .field("request_id", &self.request_id)
            .finish_non_exhaustive()
    }
}

impl ProgressReporter {
    pub fn task_id(&self) -> &TaskId {
        &self.task_id
    }

    pub fn request_id(&self) -> RequestId {
        self.request_id
    }

    pub fn report(&self, stage: ProgressStage) -> bool {
        if matches!(
            stage,
            ProgressStage::FirstResponse | ProgressStage::FirstAction
        ) {
            return false;
        }
        self.report_at(stage, None)
    }

    /// Records provider-measured latency without exposing provider payloads or transport details.
    pub fn report_provider(&self, stage: ProgressStage, provider_elapsed_ms: u64) -> bool {
        if !matches!(
            stage,
            ProgressStage::ContactingProvider
                | ProgressStage::FirstResponse
                | ProgressStage::FirstAction
        ) {
            return false;
        }
        self.report_at(stage, Some(provider_elapsed_ms))
    }

    fn report_at(&self, stage: ProgressStage, provider_elapsed_ms: Option<u64>) -> bool {
        if stage.is_terminal() || self.closed.load(Ordering::Acquire) {
            return false;
        }
        let mut state = lock(&self.state);
        let Some(record) = state
            .requests
            .get_mut(&(self.client_id, self.task_id.clone()))
        else {
            return false;
        };
        if record.snapshot.request_id != self.request_id
            || record.snapshot.state != TaskRequestState::Running
            || record.canceled.load(Ordering::Acquire)
        {
            return false;
        }
        let elapsed_ms = elapsed_ms(self.started_at);
        push_progress(&mut record.snapshot, stage, elapsed_ms, provider_elapsed_ms)
    }
}

pub struct TaskController {
    client_id: u64,
    next_client_id: Arc<AtomicU64>,
    next_request_id: Arc<AtomicU64>,
    config: TaskControllerConfig,
    jobs: mpsc::SyncSender<Job>,
    state: Arc<Mutex<SharedState>>,
    alive: Arc<AtomicBool>,
    max_admitted: usize,
}

impl Clone for TaskController {
    fn clone(&self) -> Self {
        Self {
            client_id: self.next_client_id.fetch_add(1, Ordering::Relaxed),
            next_client_id: Arc::clone(&self.next_client_id),
            next_request_id: Arc::clone(&self.next_request_id),
            config: self.config,
            jobs: self.jobs.clone(),
            state: Arc::clone(&self.state),
            alive: Arc::new(AtomicBool::new(true)),
            max_admitted: self.max_admitted,
        }
    }
}

impl Drop for TaskController {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Release);
        let mut state = lock(&self.state);
        for ((client_id, _), record) in &mut state.requests {
            if *client_id == self.client_id && record.snapshot.state == TaskRequestState::Running {
                record.canceled.store(true, Ordering::Release);
                record.snapshot.state = TaskRequestState::Canceled;
            }
        }
        let mut retained = VecDeque::with_capacity(state.completions.len());
        while let Some(completion) = state.completions.pop_front() {
            if completion.client_id == self.client_id {
                release_admission(&mut state.admitted, &completion.admitted);
            } else {
                retained.push_back(completion);
            }
        }
        state.completions = retained;
        state
            .events
            .retain(|(client_id, _)| *client_id != self.client_id);
        state
            .requests
            .retain(|(client_id, _), _| *client_id != self.client_id);
    }
}

impl TaskController {
    /// Retained thread characters and their host budget, not model token-window occupancy.
    pub fn thread_context_usage(&self, task: &Task) -> (usize, usize) {
        let retained = bounded_context(
            &task.thread,
            self.config.max_context_entries,
            self.config.max_context_chars,
        );
        (
            retained
                .iter()
                .map(|entry| entry.text.chars().count())
                .sum(),
            self.config.max_context_chars,
        )
    }

    pub fn new<F>(provider: F) -> Self
    where
        F: Fn(ProviderRequest, Arc<AtomicBool>) -> Result<ProviderReply, String>
            + Send
            + Sync
            + 'static,
    {
        Self::with_config(provider, TaskControllerConfig::default())
            .expect("default task controller configuration is valid")
    }

    pub fn with_config<F>(
        provider: F,
        config: TaskControllerConfig,
    ) -> Result<Self, TaskControllerError>
    where
        F: Fn(ProviderRequest, Arc<AtomicBool>) -> Result<ProviderReply, String>
            + Send
            + Sync
            + 'static,
    {
        Self::with_config_and_progress(
            move |request, canceled, reporter| {
                reporter.report_provider(ProgressStage::ContactingProvider, 0);
                provider(request, canceled)
            },
            config,
        )
    }

    pub fn new_with_progress<F>(provider: F) -> Self
    where
        F: Fn(ProviderRequest, Arc<AtomicBool>, ProgressReporter) -> Result<ProviderReply, String>
            + Send
            + Sync
            + 'static,
    {
        Self::with_config_and_progress(provider, TaskControllerConfig::default())
            .expect("default task controller configuration is valid")
    }

    pub fn with_config_and_progress<F>(
        provider: F,
        config: TaskControllerConfig,
    ) -> Result<Self, TaskControllerError>
    where
        F: Fn(ProviderRequest, Arc<AtomicBool>, ProgressReporter) -> Result<ProviderReply, String>
            + Send
            + Sync
            + 'static,
    {
        if config.workers == 0
            || config.workers > MAX_WORKERS
            || config.max_context_entries == 0
            || config.max_context_chars == 0
        {
            return Err(TaskControllerError::InvalidConfig);
        }
        let provider: Arc<ProviderFn> = Arc::new(provider);
        let state = Arc::new(Mutex::new(SharedState::default()));
        let max_admitted = config.workers.saturating_mul(4);
        let (jobs, receiver) = mpsc::sync_channel::<Job>(max_admitted);
        let receiver = Arc::new(Mutex::new(receiver));
        for _ in 0..config.workers {
            let provider = Arc::clone(&provider);
            let receiver = Arc::clone(&receiver);
            let state = Arc::clone(&state);
            thread::spawn(move || worker_loop(provider, receiver, state));
        }
        Ok(Self {
            client_id: 1,
            next_client_id: Arc::new(AtomicU64::new(2)),
            next_request_id: Arc::new(AtomicU64::new(1)),
            config,
            jobs,
            state,
            alive: Arc::new(AtomicBool::new(true)),
            max_admitted,
        })
    }

    pub fn send_active(&self, session: &TaskSession) -> Result<RequestId, TaskControllerError> {
        let task_id = session
            .active_task_id()
            .cloned()
            .ok_or(TaskSessionError::NoActiveTask)?;
        self.send(session, &task_id)
    }

    pub fn send(
        &self,
        session: &TaskSession,
        task_id: &TaskId,
    ) -> Result<RequestId, TaskControllerError> {
        let task = session.task(task_id)?;
        if task.lifecycle != TaskLifecycle::Active {
            return Err(TaskControllerError::TaskClosed(task_id.clone()));
        }
        if task.connection != ConnectionState::Connected {
            return Err(TaskControllerError::TaskDisconnected(task_id.clone()));
        }
        let key = (self.client_id, task_id.clone());
        let mut state = lock(&self.state);
        if state
            .requests
            .get(&key)
            .is_some_and(|request| request.snapshot.state == TaskRequestState::Running)
        {
            return Err(TaskControllerError::TaskBusy(task_id.clone()));
        }
        if state.admitted >= self.max_admitted {
            return Err(TaskControllerError::CapacityReached);
        }
        let request_id = RequestId(self.next_request_id.fetch_add(1, Ordering::Relaxed));
        let request = ProviderRequest {
            request_id,
            task_id: task_id.clone(),
            selected_provider: task.selected_provider,
            objective: task.objective.clone(),
            project_summary: task.project_summary.clone(),
            relevant_files: task.relevant_files.clone(),
            relevant_symbols: task.relevant_symbols.clone(),
            relevant_tests: task.relevant_tests.clone(),
            context: bounded_context(
                &task.thread,
                self.config.max_context_entries,
                self.config.max_context_chars,
            ),
            actions: task
                .actions
                .values()
                .map(|action| ProviderActionContext {
                    id: action.id.clone(),
                    state: match action.state {
                        ActionState::Proposed => "proposed",
                        ActionState::Accepted => "accepted",
                        ActionState::Applied => "applied",
                        ActionState::Rejected { .. } => "rejected",
                        ActionState::NeedsRepair { .. } => "needs_repair",
                    },
                    description: action
                        .description
                        .chars()
                        .take((self.config.max_context_chars / task.actions.len().max(1)).min(256))
                        .collect(),
                })
                .collect(),
            screenshots: task
                .screenshots
                .values()
                .filter(|screenshot| {
                    screenshot.provenance.task_id == task.id
                        && screenshot.vision == VisionCapability::Available
                })
                .cloned()
                .collect(),
        };
        let canceled = Arc::new(AtomicBool::new(false));
        let admitted = Arc::new(AtomicBool::new(true));
        let started_at = Instant::now();
        let retry_count = state
            .requests
            .get(&key)
            .map_or(0, |record| record.snapshot.retry_count);
        state.requests.insert(
            key,
            RequestRecord {
                request: request.clone(),
                canceled: Arc::clone(&canceled),
                started_at,
                snapshot: initial_snapshot(request_id, task_id.clone(), retry_count),
            },
        );
        state.admitted = state.admitted.saturating_add(1);
        drop(state);
        if self
            .jobs
            .try_send(Job {
                client_id: self.client_id,
                request,
                canceled,
                client_alive: Arc::clone(&self.alive),
                admitted: Arc::clone(&admitted),
                enqueued_at: started_at,
            })
            .is_err()
        {
            let mut state = lock(&self.state);
            release_admission(&mut state.admitted, &admitted);
            if let Some(record) = state.requests.get_mut(&(self.client_id, task_id.clone())) {
                record.snapshot.state = TaskRequestState::Failed;
                record.snapshot.error = Some(SAFE_PROVIDER_ERROR.to_string());
                let elapsed = elapsed_ms(record.started_at);
                push_progress(&mut record.snapshot, ProgressStage::Failed, elapsed, None);
            }
            return Err(TaskControllerError::CapacityReached);
        }
        Ok(request_id)
    }

    pub fn retry(
        &self,
        session: &mut TaskSession,
        task_id: &TaskId,
    ) -> Result<RequestId, TaskControllerError> {
        let mut task = session.task(task_id)?.clone();
        if task.lifecycle != TaskLifecycle::Active {
            return Err(TaskControllerError::TaskClosed(task_id.clone()));
        }
        if task.connection != ConnectionState::Connected {
            task.reconnect()?;
        }
        let request_id = self.resubmit(task_id, false, None)?;
        update_screenshots(
            &mut task,
            &self.requested_screenshots(task_id),
            ScreenshotOutcome::Pending,
        );
        task.metrics.record_retry();
        *session
            .task_mut(task_id)
            .expect("task was present while retry was validated") = task;
        Ok(request_id)
    }

    pub fn cancel(
        &self,
        session: &mut TaskSession,
        task_id: &TaskId,
    ) -> Result<(), TaskControllerError> {
        let mut task = session.task(task_id)?.clone();
        task.cancel()?;
        let mut state = lock(&self.state);
        if let Some(record) = state.requests.get_mut(&(self.client_id, task_id.clone())) {
            record.canceled.store(true, Ordering::Release);
            record.snapshot.state = TaskRequestState::Canceled;
            record.snapshot.elapsed_ms =
                u64::try_from(record.started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
            record.snapshot.error = None;
            let elapsed = record.snapshot.elapsed_ms;
            push_progress(&mut record.snapshot, ProgressStage::Canceled, elapsed, None);
            let request_id = record.snapshot.request_id;
            update_screenshots(
                &mut task,
                &record.request.screenshots,
                ScreenshotOutcome::Canceled,
            );
            state.events.push_back((
                self.client_id,
                TaskControllerEvent::Canceled {
                    request_id,
                    task_id: task_id.clone(),
                },
            ));
        }
        drop(state);
        *session
            .task_mut(task_id)
            .expect("task was present while cancellation was validated") = task;
        Ok(())
    }

    pub fn reconnect(
        &self,
        session: &mut TaskSession,
        task_id: &TaskId,
    ) -> Result<RequestId, TaskControllerError> {
        let mut task = session.task(task_id)?.clone();
        task.reconnect()?;
        let request_id = self.resubmit(task_id, true, task.selected_provider)?;
        update_screenshots(
            &mut task,
            &self.requested_screenshots(task_id),
            ScreenshotOutcome::Pending,
        );
        task.metrics.record_retry();
        *session
            .task_mut(task_id)
            .expect("task was present while reconnect was validated") = task;
        Ok(request_id)
    }

    fn resubmit(
        &self,
        task_id: &TaskId,
        replace_running: bool,
        provider: Option<crate::task_session::ProviderSelection>,
    ) -> Result<RequestId, TaskControllerError> {
        let key = (self.client_id, task_id.clone());
        let mut state = lock(&self.state);
        let prior = state
            .requests
            .get(&key)
            .cloned()
            .ok_or_else(|| TaskControllerError::NoPreviousRequest(task_id.clone()))?;
        if prior.snapshot.state == TaskRequestState::Running && !replace_running {
            return Err(TaskControllerError::TaskBusy(task_id.clone()));
        }
        if state.admitted >= self.max_admitted {
            return Err(TaskControllerError::CapacityReached);
        }
        prior.canceled.store(true, Ordering::Release);
        let request_id = RequestId(self.next_request_id.fetch_add(1, Ordering::Relaxed));
        let mut request = prior.request;
        request.request_id = request_id;
        if let Some(provider) = provider {
            request.selected_provider = Some(provider);
        }
        let canceled = Arc::new(AtomicBool::new(false));
        let admitted = Arc::new(AtomicBool::new(true));
        let started_at = Instant::now();
        let retry_count = prior.snapshot.retry_count.saturating_add(1);
        state.requests.insert(
            key,
            RequestRecord {
                request: request.clone(),
                canceled: Arc::clone(&canceled),
                started_at,
                snapshot: initial_snapshot(request_id, task_id.clone(), retry_count),
            },
        );
        state.admitted = state.admitted.saturating_add(1);
        drop(state);
        if self
            .jobs
            .try_send(Job {
                client_id: self.client_id,
                request,
                canceled,
                client_alive: Arc::clone(&self.alive),
                admitted: Arc::clone(&admitted),
                enqueued_at: started_at,
            })
            .is_err()
        {
            let mut state = lock(&self.state);
            release_admission(&mut state.admitted, &admitted);
            if let Some(record) = state.requests.get_mut(&(self.client_id, task_id.clone())) {
                record.snapshot.state = TaskRequestState::Failed;
                record.snapshot.error = Some(SAFE_PROVIDER_ERROR.to_string());
                let elapsed = elapsed_ms(record.started_at);
                push_progress(&mut record.snapshot, ProgressStage::Failed, elapsed, None);
            }
            return Err(TaskControllerError::CapacityReached);
        }
        Ok(request_id)
    }

    fn requested_screenshots(&self, task_id: &TaskId) -> Vec<ScreenshotAttachment> {
        lock(&self.state)
            .requests
            .get(&(self.client_id, task_id.clone()))
            .map(|record| record.request.screenshots.clone())
            .unwrap_or_default()
    }

    pub fn snapshot(&self, task_id: &TaskId) -> Option<TaskRequestSnapshot> {
        lock(&self.state)
            .requests
            .get(&(self.client_id, task_id.clone()))
            .map(|request| {
                let mut snapshot = request.snapshot.clone();
                if snapshot.state == TaskRequestState::Running {
                    snapshot.elapsed_ms =
                        u64::try_from(request.started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
                }
                snapshot
            })
    }

    pub fn poll(&self, session: &mut TaskSession) -> Vec<TaskControllerEvent> {
        let mut events = Vec::new();
        let mut completions = Vec::new();
        {
            let mut state = lock(&self.state);
            let mut retained_events = VecDeque::with_capacity(state.events.len());
            while let Some((client_id, event)) = state.events.pop_front() {
                if client_id == self.client_id {
                    events.push(event);
                } else {
                    retained_events.push_back((client_id, event));
                }
            }
            state.events = retained_events;
            let mut retained = VecDeque::with_capacity(state.completions.len());
            while let Some(completion) = state.completions.pop_front() {
                if completion.client_id == self.client_id {
                    completions.push(completion);
                } else {
                    retained.push_back(completion);
                }
            }
            state.completions = retained;
        }
        events.extend(
            completions
                .into_iter()
                .map(|completion| self.apply_completion(session, completion)),
        );
        events
    }

    fn apply_completion(
        &self,
        session: &mut TaskSession,
        completion: Completion,
    ) -> TaskControllerEvent {
        let key = (self.client_id, completion.task_id.clone());
        let mut state = lock(&self.state);
        release_admission(&mut state.admitted, &completion.admitted);
        let Some(record) = state.requests.get_mut(&key) else {
            return stale_event(completion);
        };
        if record.snapshot.request_id != completion.request_id
            || record.snapshot.state != TaskRequestState::Running
            || record.canceled.load(Ordering::Acquire)
        {
            return stale_event(completion);
        }
        record.snapshot.elapsed_ms = completion.elapsed_ms;
        match completion.result {
            Ok(reply) => {
                let proposals = reply.proposals.clone();
                let applied = session
                    .task(&completion.task_id)
                    .cloned()
                    .and_then(|mut task| {
                        task.append_result(&reply.text)?;
                        task.set_provider_state(reply.provider.clone())?;
                        task.record_turn(
                            completion.elapsed_ms,
                            reply.usage.input_tokens,
                            reply.usage.output_tokens,
                            reply.usage.estimated_cost_micros,
                        )?;
                        for proposal in &reply.proposals {
                            if proposal.repair {
                                let state = task
                                    .actions
                                    .get(proposal.id.as_str())
                                    .ok_or_else(|| {
                                        TaskSessionError::ActionNotFound(
                                            proposal.id.as_str().into(),
                                        )
                                    })?
                                    .state
                                    .clone();
                                if !matches!(
                                    state,
                                    crate::ActionState::Rejected { .. }
                                        | crate::ActionState::NeedsRepair { .. }
                                ) {
                                    return Err(TaskSessionError::InvalidTransition {
                                        entity: "action",
                                        action: "repair",
                                        state: "accepted work is retained".to_string(),
                                    });
                                }
                                task.repair_action_with_payload(
                                    proposal.id.as_str(),
                                    &proposal.description,
                                    proposal.payload.clone(),
                                )?;
                            } else {
                                task.propose_action_with_payload(
                                    proposal.id.as_str(),
                                    proposal.kind.clone(),
                                    &proposal.description,
                                    proposal.payload.clone(),
                                )?;
                            }
                        }
                        update_screenshots(
                            &mut task,
                            &record.request.screenshots,
                            ScreenshotOutcome::Completed,
                        );
                        Ok(task)
                    });
                let Ok(updated_task) = applied else {
                    if let Ok(task) = session.task_mut(&completion.task_id) {
                        update_screenshots(
                            task,
                            &record.request.screenshots,
                            ScreenshotOutcome::Failed(SAFE_SESSION_ERROR),
                        );
                    }
                    record.snapshot.state = TaskRequestState::Failed;
                    record.snapshot.error = Some(SAFE_SESSION_ERROR.to_string());
                    push_progress(
                        &mut record.snapshot,
                        ProgressStage::Failed,
                        completion.elapsed_ms,
                        None,
                    );
                    return TaskControllerEvent::Failed {
                        request_id: completion.request_id,
                        task_id: completion.task_id,
                        message: SAFE_SESSION_ERROR.to_string(),
                    };
                };
                *session
                    .task_mut(&completion.task_id)
                    .expect("task was present while response was validated") = updated_task;
                if !proposals.is_empty() {
                    reserve_progress_slots(&mut record.snapshot, 2);
                    push_progress(
                        &mut record.snapshot,
                        ProgressStage::WaitingForApproval,
                        completion.elapsed_ms,
                        None,
                    );
                }
                record.snapshot.state = TaskRequestState::Completed;
                record.snapshot.usage = reply.usage;
                push_progress(
                    &mut record.snapshot,
                    ProgressStage::Completed,
                    completion.elapsed_ms,
                    None,
                );
                TaskControllerEvent::Completed {
                    request_id: completion.request_id,
                    task_id: completion.task_id,
                    proposals,
                }
            }
            Err(()) => {
                record.snapshot.state = TaskRequestState::Failed;
                record.snapshot.error = Some(SAFE_PROVIDER_ERROR.to_string());
                push_progress(
                    &mut record.snapshot,
                    ProgressStage::Failed,
                    completion.elapsed_ms,
                    None,
                );
                if let Ok(task) = session.task_mut(&completion.task_id) {
                    update_screenshots(
                        task,
                        &record.request.screenshots,
                        ScreenshotOutcome::Failed(SAFE_PROVIDER_ERROR),
                    );
                    if task.connection == ConnectionState::Connected
                        && task.lifecycle == TaskLifecycle::Active
                    {
                        let _ = task.disconnect();
                    }
                }
                TaskControllerEvent::Failed {
                    request_id: completion.request_id,
                    task_id: completion.task_id,
                    message: SAFE_PROVIDER_ERROR.to_string(),
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
enum ScreenshotOutcome<'a> {
    Pending,
    Completed,
    Failed(&'a str),
    Canceled,
}

fn update_screenshots(
    task: &mut Task,
    requested: &[ScreenshotAttachment],
    outcome: ScreenshotOutcome<'_>,
) {
    task.start_activity_recording();
    for requested_screenshot in requested {
        if requested_screenshot.provenance.task_id != task.id {
            continue;
        }
        let Some(screenshot) = task.screenshots.get_mut(&requested_screenshot.id) else {
            continue;
        };
        if screenshot.provenance.task_id != task.id
            || screenshot.source != requested_screenshot.source
            || screenshot.content_sha256 != requested_screenshot.content_sha256
        {
            continue;
        }
        let previous = (screenshot.upload.clone(), screenshot.analysis.clone());
        match outcome {
            ScreenshotOutcome::Pending => {
                screenshot.upload = UploadState::Pending;
                screenshot.analysis = ScreenshotAnalysisState::Pending;
            }
            ScreenshotOutcome::Completed => {
                screenshot.upload = UploadState::Uploaded;
                screenshot.analysis = ScreenshotAnalysisState::Completed;
            }
            ScreenshotOutcome::Failed(reason) => {
                screenshot.upload = UploadState::Failed {
                    reason: reason.to_string(),
                };
                screenshot.analysis = ScreenshotAnalysisState::Failed {
                    reason: reason.to_string(),
                };
            }
            ScreenshotOutcome::Canceled => {
                screenshot.analysis = ScreenshotAnalysisState::Canceled;
            }
        }
        if previous != (screenshot.upload.clone(), screenshot.analysis.clone()) {
            task.record_screenshot_activity(&requested_screenshot.id);
        }
    }
}

fn worker_loop(
    provider: Arc<ProviderFn>,
    receiver: Arc<Mutex<mpsc::Receiver<Job>>>,
    state: Arc<Mutex<SharedState>>,
) {
    loop {
        let job = {
            let receiver = lock(&receiver);
            receiver.recv()
        };
        let Ok(job) = job else { return };
        if !job.client_alive.load(Ordering::Acquire) {
            let mut state = lock(&state);
            release_admission(&mut state.admitted, &job.admitted);
            continue;
        }
        if job.canceled.load(Ordering::Acquire) {
            let elapsed_ms =
                u64::try_from(job.enqueued_at.elapsed().as_millis()).unwrap_or(u64::MAX);
            lock(&state).completions.push_back(Completion {
                client_id: job.client_id,
                request_id: job.request.request_id,
                task_id: job.request.task_id,
                elapsed_ms,
                result: Err(()),
                admitted: job.admitted,
            });
            continue;
        }
        let reporter = ProgressReporter {
            client_id: job.client_id,
            task_id: job.request.task_id.clone(),
            request_id: job.request.request_id,
            started_at: job.enqueued_at,
            state: Arc::clone(&state),
            closed: Arc::new(AtomicBool::new(false)),
        };
        let result = catch_unwind(AssertUnwindSafe(|| {
            provider(
                job.request.clone(),
                Arc::clone(&job.canceled),
                reporter.clone(),
            )
        }))
        .ok()
        .and_then(Result::ok)
        .ok_or(());
        reporter.closed.store(true, Ordering::Release);
        let elapsed_ms = u64::try_from(job.enqueued_at.elapsed().as_millis()).unwrap_or(u64::MAX);
        let mut state = lock(&state);
        if !job.client_alive.load(Ordering::Acquire) {
            release_admission(&mut state.admitted, &job.admitted);
            continue;
        }
        state.completions.push_back(Completion {
            client_id: job.client_id,
            request_id: job.request.request_id,
            task_id: job.request.task_id,
            elapsed_ms,
            result,
            admitted: job.admitted,
        });
    }
}

fn bounded_context(
    entries: &[ThreadEntry],
    max_entries: usize,
    max_chars: usize,
) -> Vec<ThreadEntry> {
    let mut remaining = max_chars;
    let mut selected = Vec::new();
    for entry in entries.iter().rev().take(max_entries) {
        if remaining == 0 {
            break;
        }
        let text: String = entry
            .text
            .chars()
            .rev()
            .take(remaining)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        remaining = remaining.saturating_sub(text.chars().count());
        selected.push(ThreadEntry {
            sequence: entry.sequence,
            kind: entry.kind,
            text,
        });
    }
    selected.reverse();
    selected
}

fn stale_event(completion: Completion) -> TaskControllerEvent {
    TaskControllerEvent::Stale {
        request_id: completion.request_id,
        task_id: completion.task_id,
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn initial_snapshot(
    request_id: RequestId,
    task_id: TaskId,
    retry_count: u32,
) -> TaskRequestSnapshot {
    let queued = ProgressEvent {
        task_id: task_id.clone(),
        request_id,
        sequence: 0,
        stage: ProgressStage::Queued,
        elapsed_ms: 0,
        provider_elapsed_ms: None,
    };
    TaskRequestSnapshot {
        request_id,
        task_id,
        state: TaskRequestState::Running,
        elapsed_ms: 0,
        usage: ProviderUsage::default(),
        retry_count,
        error: None,
        progress: vec![queued],
        provider_first_response_ms: None,
        provider_first_action_ms: None,
        focused_tests_passed_ms: None,
    }
}

fn push_progress(
    snapshot: &mut TaskRequestSnapshot,
    stage: ProgressStage,
    elapsed_ms: u64,
    provider_elapsed_ms: Option<u64>,
) -> bool {
    if snapshot
        .progress
        .last()
        .is_some_and(|event| event.stage.is_terminal())
    {
        return false;
    }
    let elapsed_ms = snapshot
        .progress
        .last()
        .map_or(elapsed_ms, |event| elapsed_ms.max(event.elapsed_ms));
    if snapshot
        .progress
        .last()
        .is_some_and(|event| event.stage == stage)
    {
        return false;
    }
    match stage {
        ProgressStage::FirstResponse => {
            if snapshot.provider_first_response_ms.is_some() {
                return false;
            }
            snapshot.provider_first_response_ms = provider_elapsed_ms;
        }
        ProgressStage::FirstAction => {
            if snapshot.provider_first_action_ms.is_some() {
                return false;
            }
            snapshot.provider_first_action_ms = provider_elapsed_ms;
        }
        ProgressStage::FocusedTestsPassed => {
            if snapshot.focused_tests_passed_ms.is_none() {
                snapshot.focused_tests_passed_ms = Some(elapsed_ms);
            }
        }
        _ => {}
    }
    let limit = if stage.is_terminal() {
        MAX_PROGRESS_EVENTS
    } else {
        MAX_PROGRESS_EVENTS - 1
    };
    if snapshot.progress.len() >= limit {
        if !stage.is_terminal() {
            return false;
        }
        let Some(index) = snapshot
            .progress
            .iter()
            .enumerate()
            .skip(1)
            .find_map(|(index, event)| (!event.stage.is_terminal()).then_some(index))
        else {
            return false;
        };
        snapshot.progress.remove(index);
    }
    let sequence = snapshot
        .progress
        .last()
        .map_or(0, |event| event.sequence.saturating_add(1));
    snapshot.progress.push(ProgressEvent {
        task_id: snapshot.task_id.clone(),
        request_id: snapshot.request_id,
        sequence,
        stage,
        elapsed_ms,
        provider_elapsed_ms,
    });
    true
}

fn reserve_progress_slots(snapshot: &mut TaskRequestSnapshot, slots: usize) {
    while snapshot.progress.len().saturating_add(slots) > MAX_PROGRESS_EVENTS {
        let Some(index) = snapshot
            .progress
            .iter()
            .enumerate()
            .skip(1)
            .find_map(|(index, event)| (!event.stage.is_terminal()).then_some(index))
        else {
            break;
        };
        snapshot.progress.remove(index);
    }
}

fn elapsed_ms(started_at: Instant) -> u64 {
    u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn release_admission(count: &mut usize, admitted: &AtomicBool) {
    if admitted.swap(false, Ordering::AcqRel) {
        *count = count.saturating_sub(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FallbackState, RoutingState};
    use std::sync::Barrier;
    use std::time::Duration;

    fn session(ids: &[&str]) -> TaskSession {
        let mut session = TaskSession::new();
        for id in ids {
            session
                .new_task(*id, format!("objective {id}"), "project")
                .unwrap();
            session.append_reply(format!("reply {id}")).unwrap();
        }
        session
    }

    #[test]
    fn provider_selection_is_snapshotted_per_request_and_task() {
        let (sent, received) = mpsc::channel();
        let controller = TaskController::new(move |request, _| {
            sent.send((request.task_id, request.selected_provider))
                .unwrap();
            Ok(ProviderReply::new("complete"))
        });
        let mut session = session(&["one", "two"]);
        let first = crate::task_session::ProviderSelection::Codex;
        let second = crate::task_session::ProviderSelection::OpenRouter;
        session
            .task_mut("one")
            .unwrap()
            .select_provider(first)
            .unwrap();
        session
            .task_mut("two")
            .unwrap()
            .select_provider(second)
            .unwrap();
        controller.send(&session, &TaskId::new("one")).unwrap();
        session
            .task_mut("one")
            .unwrap()
            .select_provider(second)
            .unwrap();
        controller.send(&session, &TaskId::new("two")).unwrap();
        let mut snapshots = BTreeMap::new();
        for _ in 0..2 {
            let (task, provider) = received.recv_timeout(Duration::from_secs(2)).unwrap();
            snapshots.insert(task, provider);
        }
        assert_eq!(snapshots[&TaskId::new("one")], Some(first));
        assert_eq!(snapshots[&TaskId::new("two")], Some(second));
    }

    #[test]
    fn reconnect_uses_new_provider_after_failure_without_changing_prior_payload() {
        use crate::task_session::ProviderSelection;
        let (sent, received) = mpsc::channel();
        let controller = TaskController::new(move |request, _| {
            sent.send(request.clone()).unwrap();
            Err("provider unavailable".into())
        });
        let mut session = session(&["one"]);
        let id = TaskId::new("one");
        session
            .task_mut(&id)
            .unwrap()
            .select_provider(ProviderSelection::Codex)
            .unwrap();
        controller.send(&session, &id).unwrap();
        wait_for(&controller, &mut session);
        let prior = received.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(
            session.task(&id).unwrap().connection,
            ConnectionState::Disconnected
        );
        session
            .task_mut(&id)
            .unwrap()
            .select_provider(ProviderSelection::OpenRouter)
            .unwrap();
        controller.reconnect(&mut session, &id).unwrap();
        wait_for(&controller, &mut session);
        let next = received.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(next.selected_provider, Some(ProviderSelection::OpenRouter));
        assert_eq!(prior.selected_provider, Some(ProviderSelection::Codex));
        assert_ne!(next.request_id, prior.request_id);
        assert_eq!(next.context, prior.context);
        assert_eq!(next.task_id, prior.task_id);
    }

    #[test]
    fn thread_meter_matches_retained_unicode_context_budget() {
        let controller = TaskController::with_config(
            |_, _| Ok(ProviderReply::new("complete")),
            TaskControllerConfig {
                workers: 1,
                max_context_entries: 2,
                max_context_chars: 5,
            },
        )
        .unwrap();
        let mut session = session(&["one"]);
        session.append_reply("abc\u{e9}ef").unwrap();
        assert_eq!(
            controller.thread_context_usage(session.active_task().unwrap()),
            (5, 5)
        );
    }

    fn wait_for(
        controller: &TaskController,
        session: &mut TaskSession,
    ) -> Vec<TaskControllerEvent> {
        for _ in 0..100 {
            let events = controller.poll(session);
            if !events.is_empty() {
                return events;
            }
            thread::sleep(Duration::from_millis(2));
        }
        panic!("provider did not complete")
    }

    #[test]
    fn completions_are_isolated_by_task_and_cloned_client() {
        let controller =
            TaskController::new(|request, _| Ok(ProviderReply::new(request.task_id.as_str())));
        let clone = controller.clone();
        let mut owner = session(&["one", "two"]);
        let mut stranger = session(&["other"]);
        controller.send(&owner, &TaskId::new("one")).unwrap();
        assert!(clone.poll(&mut stranger).is_empty());
        let events = wait_for(&controller, &mut owner);
        assert!(
            matches!(events[0], TaskControllerEvent::Completed { ref task_id, .. } if task_id.as_str() == "one")
        );
        assert_eq!(
            owner.task("one").unwrap().thread.last().unwrap().text,
            "one"
        );
        assert_eq!(owner.task("two").unwrap().thread.len(), 1);
    }

    #[test]
    fn cancellation_rejects_late_completion() {
        let started = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let worker_started = Arc::clone(&started);
        let worker_release = Arc::clone(&release);
        let controller = TaskController::new(move |_, _| {
            worker_started.wait();
            worker_release.wait();
            Ok(ProviderReply::new("late"))
        });
        let mut session = session(&["one"]);
        session
            .task_mut("one")
            .unwrap()
            .set_vision_capability(true)
            .unwrap();
        session
            .task_mut("one")
            .unwrap()
            .attach_screenshot("shot", "shot.png")
            .unwrap();
        controller.send(&session, &TaskId::new("one")).unwrap();
        started.wait();
        controller
            .cancel(&mut session, &TaskId::new("one"))
            .unwrap();
        release.wait();
        let mut events = Vec::new();
        for _ in 0..100 {
            events.extend(controller.poll(&mut session));
            if events
                .iter()
                .any(|event| matches!(event, TaskControllerEvent::Stale { .. }))
            {
                break;
            }
            thread::sleep(Duration::from_millis(2));
        }
        assert!(events
            .iter()
            .any(|event| matches!(event, TaskControllerEvent::Canceled { .. })));
        assert!(events
            .iter()
            .any(|event| matches!(event, TaskControllerEvent::Stale { .. })));
        assert_eq!(session.task("one").unwrap().thread.len(), 1);
        assert_eq!(
            session.task("one").unwrap().screenshots[&crate::ScreenshotId::new("shot")].analysis,
            ScreenshotAnalysisState::Canceled
        );
    }

    #[test]
    fn request_snapshots_only_originating_screenshots_and_completion_updates_that_snapshot() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let provider_seen = Arc::clone(&seen);
        let controller = TaskController::new(move |request, _| {
            provider_seen.lock().unwrap().push(request);
            Ok(ProviderReply::new("analyzed"))
        });
        let mut session = session(&["one", "two"]);
        for id in ["one", "two"] {
            let task = session.task_mut(id).unwrap();
            task.set_vision_capability(true).unwrap();
            task.attach_screenshot(format!("{id}-shot"), format!("{id}.png"))
                .unwrap();
        }
        let foreign =
            session.task("two").unwrap().screenshots[&crate::ScreenshotId::new("two-shot")].clone();
        session
            .task_mut("one")
            .unwrap()
            .screenshots
            .insert(crate::ScreenshotId::new("foreign"), foreign);

        controller.send(&session, &TaskId::new("one")).unwrap();
        session
            .task_mut("one")
            .unwrap()
            .attach_screenshot("late", "late.png")
            .unwrap();
        let events = wait_for(&controller, &mut session);
        assert!(matches!(events[0], TaskControllerEvent::Completed { .. }));

        let requests = seen.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].screenshots.len(), 1);
        assert_eq!(requests[0].screenshots[0].id.as_str(), "one-shot");
        assert_eq!(
            requests[0].screenshots[0].provenance.task_id.as_str(),
            "one"
        );
        let task = session.task("one").unwrap();
        assert_eq!(
            task.screenshots[&crate::ScreenshotId::new("one-shot")].upload,
            UploadState::Uploaded
        );
        assert_eq!(
            task.screenshots[&crate::ScreenshotId::new("one-shot")].analysis,
            ScreenshotAnalysisState::Completed
        );
        assert_eq!(
            task.screenshots[&crate::ScreenshotId::new("late")].analysis,
            ScreenshotAnalysisState::Pending
        );
        let completed: Vec<_> = task
            .activity
            .iter()
            .filter_map(|entry| match &entry.kind {
                crate::task_session::ActivityKind::Attachment {
                    screenshot_id,
                    analysis: ScreenshotAnalysisState::Completed,
                    ..
                } => Some(screenshot_id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(completed, ["one-shot"]);
    }

    #[test]
    fn provider_failure_and_retry_keep_screenshot_lifecycle_truthful() {
        let calls = Arc::new(AtomicU64::new(0));
        let provider_calls = Arc::clone(&calls);
        let controller = TaskController::new(move |_, _| {
            if provider_calls.fetch_add(1, Ordering::Relaxed) == 0 {
                Err("private provider error".to_string())
            } else {
                Ok(ProviderReply::new("analyzed"))
            }
        });
        let mut session = session(&["one"]);
        let task = session.task_mut("one").unwrap();
        task.set_vision_capability(true).unwrap();
        task.attach_screenshot("shot", "shot.png").unwrap();

        controller.send(&session, &TaskId::new("one")).unwrap();
        wait_for(&controller, &mut session);
        let shot = &session.task("one").unwrap().screenshots[&crate::ScreenshotId::new("shot")];
        assert!(matches!(shot.upload, UploadState::Failed { .. }));
        assert!(matches!(
            shot.analysis,
            ScreenshotAnalysisState::Failed { .. }
        ));

        controller.retry(&mut session, &TaskId::new("one")).unwrap();
        let shot = &session.task("one").unwrap().screenshots[&crate::ScreenshotId::new("shot")];
        assert_eq!(shot.upload, UploadState::Pending);
        assert_eq!(shot.analysis, ScreenshotAnalysisState::Pending);
        wait_for(&controller, &mut session);
        let shot = &session.task("one").unwrap().screenshots[&crate::ScreenshotId::new("shot")];
        assert_eq!(shot.upload, UploadState::Uploaded);
        assert_eq!(shot.analysis, ScreenshotAnalysisState::Completed);
    }

    #[test]
    fn reconnect_replaces_failed_request_and_activates_fallback() {
        let calls = Arc::new(AtomicU64::new(0));
        let provider_calls = Arc::clone(&calls);
        let controller = TaskController::new(move |_, _| {
            if provider_calls.fetch_add(1, Ordering::Relaxed) == 0 {
                return Err("secret bearer token".to_string());
            }
            let mut reply = ProviderReply::new("fallback reply");
            reply.provider = ProviderState::new(
                Some("backup".to_string()),
                Some("model-b".to_string()),
                RoutingState::Assigned {
                    route: "fallback".to_string(),
                },
                FallbackState::Active {
                    provider: "backup".to_string(),
                    model: Some("model-b".to_string()),
                    route: Some("fallback".to_string()),
                },
            )
            .unwrap();
            reply.usage = ProviderUsage {
                input_tokens: 10,
                output_tokens: 4,
                estimated_cost_micros: 9,
            };
            Ok(reply)
        });
        let mut session = session(&["one"]);
        controller.send(&session, &TaskId::new("one")).unwrap();
        let failure = wait_for(&controller, &mut session);
        assert!(
            matches!(&failure[0], TaskControllerEvent::Failed { message, .. } if message == SAFE_PROVIDER_ERROR)
        );
        assert!(!format!("{:?}", controller.snapshot(&TaskId::new("one"))).contains("secret"));
        controller
            .reconnect(&mut session, &TaskId::new("one"))
            .unwrap();
        let completed = wait_for(&controller, &mut session);
        assert!(matches!(
            completed[0],
            TaskControllerEvent::Completed { .. }
        ));
        let task = session.task("one").unwrap();
        assert!(matches!(
            task.provider.fallback,
            FallbackState::Active { .. }
        ));
        assert_eq!(task.metrics.input_tokens, 10);
        assert_eq!(task.metrics.estimated_cost_micros, 9);
    }

    #[test]
    fn reconnect_fences_a_running_request() {
        let started = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let worker_started = Arc::clone(&started);
        let worker_release = Arc::clone(&release);
        let controller = TaskController::new(move |request, _| {
            if request.request_id.get() == 1 {
                worker_started.wait();
                worker_release.wait();
                Ok(ProviderReply::new("stale"))
            } else {
                Ok(ProviderReply::new("fresh"))
            }
        });
        let mut session = session(&["one"]);
        controller.send(&session, &TaskId::new("one")).unwrap();
        started.wait();
        session.task_mut("one").unwrap().disconnect().unwrap();
        controller
            .reconnect(&mut session, &TaskId::new("one"))
            .unwrap();
        let completed = wait_for(&controller, &mut session);
        assert!(matches!(
            completed[0],
            TaskControllerEvent::Completed { .. }
        ));
        release.wait();
        let stale = wait_for(&controller, &mut session);
        assert!(matches!(stale[0], TaskControllerEvent::Stale { .. }));
        let task = session.task("one").unwrap();
        assert_eq!(task.thread.last().unwrap().text, "fresh");
        assert_eq!(task.metrics.turn_count, 1);
    }

    #[test]
    fn admission_is_bounded_without_blocking_sender() {
        let barrier = Arc::new(Barrier::new(2));
        let worker_barrier = Arc::clone(&barrier);
        let calls = Arc::new(AtomicU64::new(0));
        let provider_calls = Arc::clone(&calls);
        let controller = TaskController::with_config(
            move |_, _| {
                if provider_calls.fetch_add(1, Ordering::Relaxed) == 0 {
                    worker_barrier.wait();
                }
                Ok(ProviderReply::new("done"))
            },
            TaskControllerConfig {
                workers: 1,
                ..TaskControllerConfig::default()
            },
        )
        .unwrap();
        let session = session(&["one", "two", "three", "four", "five"]);
        for id in ["one", "two", "three", "four"] {
            controller.send(&session, &TaskId::new(id)).unwrap();
        }
        assert!(matches!(
            controller.send(&session, &TaskId::new("five")),
            Err(TaskControllerError::CapacityReached)
        ));
        barrier.wait();
    }

    #[test]
    fn repeated_reconnects_cannot_bypass_admission_bound() {
        let barrier = Arc::new(Barrier::new(2));
        let worker_barrier = Arc::clone(&barrier);
        let calls = Arc::new(AtomicU64::new(0));
        let provider_calls = Arc::clone(&calls);
        let controller = TaskController::with_config(
            move |_, _| {
                if provider_calls.fetch_add(1, Ordering::Relaxed) == 0 {
                    worker_barrier.wait();
                }
                Ok(ProviderReply::new("done"))
            },
            TaskControllerConfig {
                workers: 1,
                ..TaskControllerConfig::default()
            },
        )
        .unwrap();
        let mut session = session(&["one"]);
        controller.send(&session, &TaskId::new("one")).unwrap();
        for _ in 0..3 {
            session.task_mut("one").unwrap().disconnect().unwrap();
            controller
                .reconnect(&mut session, &TaskId::new("one"))
                .unwrap();
        }
        session.task_mut("one").unwrap().disconnect().unwrap();
        assert!(matches!(
            controller.reconnect(&mut session, &TaskId::new("one")),
            Err(TaskControllerError::CapacityReached)
        ));
        assert_eq!(
            session.task("one").unwrap().connection,
            ConnectionState::Disconnected
        );
        barrier.wait();
    }

    #[test]
    fn retry_and_bounded_context_use_a_new_request_id() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let provider_seen = Arc::clone(&seen);
        let config = TaskControllerConfig {
            workers: 1,
            max_context_entries: 2,
            max_context_chars: 9,
        };
        let controller = TaskController::with_config(
            move |request, _| {
                provider_seen.lock().unwrap().push(request.clone());
                Err("failure".to_string())
            },
            config,
        )
        .unwrap();
        let mut session = session(&["one"]);
        session.switch_task("one").unwrap();
        session.append_reply("abcdefgh").unwrap();
        session.append_reply("ijklmnop").unwrap();
        let first = controller.send(&session, &TaskId::new("one")).unwrap();
        wait_for(&controller, &mut session);
        let second = controller.retry(&mut session, &TaskId::new("one")).unwrap();
        assert_ne!(first, second);
        assert_eq!(session.task("one").unwrap().metrics.retry_count, 1);
        wait_for(&controller, &mut session);
        let requests = seen.lock().unwrap();
        assert_eq!(requests[0].context.len(), 2);
        assert!(
            requests[0]
                .context
                .iter()
                .map(|entry| entry.text.chars().count())
                .sum::<usize>()
                <= 9
        );
        assert_eq!(
            controller
                .snapshot(&TaskId::new("one"))
                .unwrap()
                .retry_count,
            1
        );
    }

    #[test]
    fn invalid_provider_metadata_does_not_publish_a_partial_reply() {
        let controller = TaskController::new(|_, _| {
            let mut reply = ProviderReply::new("must not be published");
            reply.provider.provider = Some(String::new());
            reply.usage.estimated_cost_micros = 100;
            Ok(reply)
        });
        let mut session = session(&["one"]);
        let before = session.task("one").unwrap().clone();
        controller.send(&session, &TaskId::new("one")).unwrap();
        let events = wait_for(&controller, &mut session);
        assert!(matches!(&events[0], TaskControllerEvent::Failed { .. }));
        assert_eq!(session.task("one").unwrap(), &before);
    }

    #[test]
    fn callback_panic_settles_as_safe_failure() {
        let controller =
            TaskController::new(|_, _| -> Result<ProviderReply, String> { panic!("secret panic") });
        let mut session = session(&["one"]);
        controller.send(&session, &TaskId::new("one")).unwrap();
        let events = wait_for(&controller, &mut session);
        assert!(
            matches!(&events[0], TaskControllerEvent::Failed { message, .. } if message == SAFE_PROVIDER_ERROR)
        );
    }

    #[test]
    fn typed_progress_is_ordered_bounded_and_keeps_provider_timings() {
        let controller = TaskController::new_with_progress(|_, _, reporter| {
            assert!(reporter.report(ProgressStage::InspectingSymbols));
            assert!(!reporter.report(ProgressStage::InspectingSymbols));
            assert!(!reporter.report_provider(ProgressStage::Compiling, 3));
            assert!(reporter.report_provider(ProgressStage::ContactingProvider, 0));
            assert!(reporter.report_provider(ProgressStage::FirstResponse, 7));
            assert!(reporter.report_provider(ProgressStage::FirstAction, 11));
            assert!(reporter.report(ProgressStage::PreparingProposal));
            assert!(!reporter.report_provider(ProgressStage::FirstResponse, 19));
            assert!(!reporter.report_provider(ProgressStage::FirstAction, 23));
            for index in 0..MAX_PROGRESS_EVENTS {
                reporter.report(if index % 2 == 0 {
                    ProgressStage::PreparingProposal
                } else {
                    ProgressStage::Compiling
                });
            }
            let mut reply = ProviderReply::new("done");
            reply.proposals.push(ProviderActionProposal {
                id: "proposal".into(),
                kind: ActionKind::Edit,
                description: "bounded proposal".into(),
                payload: Value::Null,
                repair: false,
            });
            Ok(reply)
        });
        let mut session = session(&["one"]);
        let request_id = controller.send(&session, &TaskId::new("one")).unwrap();
        wait_for(&controller, &mut session);

        let snapshot = controller.snapshot(&TaskId::new("one")).unwrap();
        assert_eq!(snapshot.request_id, request_id);
        assert_eq!(snapshot.provider_first_response_ms, Some(7));
        assert_eq!(snapshot.provider_first_action_ms, Some(11));
        assert_eq!(snapshot.progress.len(), MAX_PROGRESS_EVENTS);
        assert_eq!(snapshot.progress[0].stage, ProgressStage::Queued);
        assert_eq!(
            snapshot.progress[MAX_PROGRESS_EVENTS - 2].stage,
            ProgressStage::WaitingForApproval
        );
        assert_eq!(
            snapshot.progress.last().unwrap().stage,
            ProgressStage::Completed
        );
        assert!(snapshot
            .progress
            .windows(2)
            .all(|events| events[0].sequence < events[1].sequence));
        assert!(snapshot.progress.iter().all(|event| {
            event.task_id == TaskId::new("one") && event.request_id == request_id
        }));
    }

    #[test]
    fn progress_is_isolated_between_clients_with_the_same_task_id() {
        let (sent, received) = mpsc::channel();
        let controller = TaskController::new_with_progress(move |_, _, reporter| {
            let (release, wait) = mpsc::channel();
            sent.send((reporter, release)).unwrap();
            wait.recv_timeout(Duration::from_secs(5)).unwrap();
            Ok(ProviderReply::new("done"))
        });
        let other = controller.clone();
        let first_session = session(&["one"]);
        let second_session = session(&["one"]);
        let first_id = controller.send_active(&first_session).unwrap();
        let second_id = other.send_active(&second_session).unwrap();
        let mut releases = Vec::new();
        for _ in 0..2 {
            let (reporter, release) = received.recv_timeout(Duration::from_secs(5)).unwrap();
            let latency = if reporter.request_id() == first_id {
                11
            } else {
                22
            };
            assert!(reporter.report_provider(ProgressStage::FirstAction, latency));
            releases.push(release);
        }
        let first = controller.snapshot(&TaskId::new("one")).unwrap();
        let second = other.snapshot(&TaskId::new("one")).unwrap();
        assert_eq!(first.provider_first_action_ms, Some(11));
        assert_eq!(second.provider_first_action_ms, Some(22));
        assert!(first
            .progress
            .iter()
            .all(|event| event.request_id == first_id));
        assert!(second
            .progress
            .iter()
            .all(|event| event.request_id == second_id));
        for release in releases {
            release.send(()).unwrap();
        }
    }

    #[test]
    fn canceled_and_stale_reporters_cannot_publish_progress() {
        let (sent, received) = mpsc::channel();
        let release = Arc::new(Barrier::new(2));
        let provider_release = Arc::clone(&release);
        let controller = TaskController::new_with_progress(move |_, _, reporter| {
            sent.send(reporter.clone()).unwrap();
            provider_release.wait();
            Ok(ProviderReply::new("late"))
        });
        let mut session = session(&["one"]);
        controller.send(&session, &TaskId::new("one")).unwrap();
        let reporter = received.recv_timeout(Duration::from_secs(2)).unwrap();
        controller
            .cancel(&mut session, &TaskId::new("one"))
            .unwrap();
        assert!(!reporter.report(ProgressStage::PreparingProposal));
        let snapshot = controller.snapshot(&TaskId::new("one")).unwrap();
        assert_eq!(
            snapshot.progress.last().unwrap().stage,
            ProgressStage::Canceled
        );
        release.wait();
    }

    #[test]
    fn retries_replace_progress_ownership_and_reset_the_timeline() {
        let reporters = Arc::new(Mutex::new(Vec::new()));
        let provider_reporters = Arc::clone(&reporters);
        let calls = Arc::new(AtomicU64::new(0));
        let provider_calls = Arc::clone(&calls);
        let controller = TaskController::new_with_progress(move |_, _, reporter| {
            provider_reporters.lock().unwrap().push(reporter.clone());
            reporter.report(ProgressStage::Fallback);
            if provider_calls.fetch_add(1, Ordering::Relaxed) == 0 {
                Err("retry".into())
            } else {
                Ok(ProviderReply::new("done"))
            }
        });
        let mut session = session(&["one"]);
        let first = controller.send(&session, &TaskId::new("one")).unwrap();
        wait_for(&controller, &mut session);
        let second = controller.retry(&mut session, &TaskId::new("one")).unwrap();
        assert_ne!(first, second);
        wait_for(&controller, &mut session);

        let reporters = reporters.lock().unwrap();
        assert!(!reporters[0].report(ProgressStage::Compiling));
        assert!(!reporters[1].report(ProgressStage::Compiling));
        let snapshot = controller.snapshot(&TaskId::new("one")).unwrap();
        assert_eq!(snapshot.request_id, second);
        assert_eq!(snapshot.progress[0].sequence, 0);
        assert_eq!(snapshot.progress[0].stage, ProgressStage::Queued);
        assert!(snapshot
            .progress
            .iter()
            .all(|event| event.request_id == second));
    }

    #[test]
    fn timing_survives_capacity_and_total_elapsed_never_regresses() {
        let mut snapshot = initial_snapshot(RequestId(1), TaskId::new("one"), 0);
        for index in 1..MAX_PROGRESS_EVENTS - 1 {
            assert!(push_progress(
                &mut snapshot,
                if index % 2 == 0 {
                    ProgressStage::Compiling
                } else {
                    ProgressStage::PreparingProposal
                },
                100 + index as u64,
                None,
            ));
        }
        assert!(!push_progress(
            &mut snapshot,
            ProgressStage::FirstAction,
            1,
            Some(27),
        ));
        assert_eq!(snapshot.provider_first_action_ms, Some(27));
        assert!(!push_progress(
            &mut snapshot,
            ProgressStage::FirstAction,
            132,
            Some(99),
        ));
        assert_eq!(snapshot.provider_first_action_ms, Some(27));
        assert_eq!(snapshot.progress[0].stage, ProgressStage::Queued);
        assert!(push_progress(
            &mut snapshot,
            ProgressStage::Completed,
            1,
            None,
        ));
        assert_eq!(snapshot.progress.last().unwrap().elapsed_ms, 130);
        assert_eq!(snapshot.progress.len(), MAX_PROGRESS_EVENTS);
    }
}
