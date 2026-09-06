use serde::{Deserialize, Serialize};
use std::borrow::Borrow;
use std::collections::BTreeMap;
use std::fmt;

pub const MAX_TASKS: usize = 32;
pub const MAX_ID_CHARS: usize = 96;
pub const MAX_OBJECTIVE_CHARS: usize = 512;
pub const MAX_PROJECT_SUMMARY_CHARS: usize = 1_024;
pub const MAX_REFERENCE_CHARS: usize = 256;
pub const MAX_RELEVANT_FILES: usize = 32;
pub const MAX_RELEVANT_SYMBOLS: usize = 64;
pub const MAX_RELEVANT_TESTS: usize = 32;
pub const MAX_THREAD_ENTRIES: usize = 128;
pub const MAX_THREAD_TEXT_CHARS: usize = 4_096;
pub const MAX_ACTIONS: usize = 64;
pub const MAX_ACTION_TEXT_CHARS: usize = 1_024;
pub const MAX_ACTION_REVISIONS: usize = 16;
pub const MAX_ACTION_PAYLOAD_BYTES: usize = 256 * 1024;
pub const MAX_SCREENSHOTS: usize = 16;
pub const MAX_IMAGES: usize = 16;
pub const MAX_ARTIFACT_SOURCE_CHARS: usize = 512;
pub const MAX_ATTRIBUTION_CHARS: usize = 256;

macro_rules! id_type {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }
            pub fn try_new(value: impl Into<String>) -> Result<Self, TaskSessionError> {
                Ok(Self(validate_text(
                    stringify!($name),
                    &value.into(),
                    MAX_ID_CHARS,
                )?))
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
            pub fn into_inner(self) -> String {
                self.0
            }
        }
        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self::new(value)
            }
        }
        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self::new(value)
            }
        }
        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }
        impl Borrow<str> for $name {
            fn borrow(&self) -> &str {
                self.as_str()
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

id_type!(TaskId);
id_type!(ActionId);
id_type!(ScreenshotId);
id_type!(GeneratedImageId);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskSessionError {
    EmptyField {
        field: &'static str,
    },
    FieldTooLong {
        field: &'static str,
        max: usize,
        actual: usize,
    },
    TaskLimitReached,
    DuplicateTaskId(TaskId),
    DuplicateActionId(ActionId),
    DuplicateScreenshotId(ScreenshotId),
    DuplicateImageId(GeneratedImageId),
    NoActiveTask,
    TaskNotFound(TaskId),
    ActionNotFound(ActionId),
    ScreenshotNotFound(ScreenshotId),
    ImageNotFound(GeneratedImageId),
    InvalidTransition {
        entity: &'static str,
        action: &'static str,
        state: String,
    },
    VisionUnavailable,
    InvalidScreenshotSha256,
}

impl fmt::Display for TaskSessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyField { field } => write!(f, "{field} must not be empty"),
            Self::FieldTooLong { field, max, actual } => {
                write!(f, "{field} is {actual} characters; maximum is {max}")
            }
            Self::TaskLimitReached => write!(f, "task limit of {MAX_TASKS} reached"),
            Self::DuplicateTaskId(id) => write!(f, "task ID already exists: {id}"),
            Self::DuplicateActionId(id) => write!(f, "action ID already exists: {id}"),
            Self::DuplicateScreenshotId(id) => write!(f, "screenshot ID already exists: {id}"),
            Self::DuplicateImageId(id) => write!(f, "generated image ID already exists: {id}"),
            Self::NoActiveTask => write!(f, "no active task"),
            Self::TaskNotFound(id) => write!(f, "task not found: {id}"),
            Self::ActionNotFound(id) => write!(f, "action not found: {id}"),
            Self::ScreenshotNotFound(id) => write!(f, "screenshot not found: {id}"),
            Self::ImageNotFound(id) => write!(f, "generated image not found: {id}"),
            Self::InvalidTransition {
                entity,
                action,
                state,
            } => write!(f, "cannot {action} {entity} while it is {state}"),
            Self::VisionUnavailable => write!(f, "screenshot attachment requires available vision"),
            Self::InvalidScreenshotSha256 => {
                f.write_str("screenshot SHA-256 must be 64 lowercase hexadecimal characters")
            }
        }
    }
}

impl std::error::Error for TaskSessionError {}

fn validate_action_payload(payload: &serde_json::Value) -> Result<(), TaskSessionError> {
    let actual = payload.to_string().len();
    if actual > MAX_ACTION_PAYLOAD_BYTES {
        return Err(TaskSessionError::FieldTooLong {
            field: "action payload bytes",
            max: MAX_ACTION_PAYLOAD_BYTES,
            actual,
        });
    }
    Ok(())
}

fn validate_text(field: &'static str, value: &str, max: usize) -> Result<String, TaskSessionError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(TaskSessionError::EmptyField { field });
    }
    let actual = value.chars().count();
    if actual > max {
        return Err(TaskSessionError::FieldTooLong { field, max, actual });
    }
    Ok(value.to_string())
}

fn validate_optional_text(
    field: &'static str,
    value: Option<&str>,
    max: usize,
) -> Result<Option<String>, TaskSessionError> {
    value
        .map(|value| validate_text(field, value, max))
        .transpose()
}

fn invalid_transition(
    entity: &'static str,
    action: &'static str,
    state: impl Into<String>,
) -> TaskSessionError {
    TaskSessionError::InvalidTransition {
        entity,
        action,
        state: state.into(),
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskLifecycle {
    #[default]
    Active,
    Canceled,
    Completed,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectionState {
    #[default]
    Connected,
    Disconnected,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ValidationStatus {
    #[default]
    NotRun,
    Running,
    Passed {
        summary: String,
    },
    Failed {
        summary: String,
    },
}
impl ValidationStatus {
    pub fn is_passing(&self) -> bool {
        matches!(self, Self::Passed { .. })
    }
    pub fn is_running(&self) -> bool {
        matches!(self, Self::Running)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FocusedTestResult {
    pub passed: bool,
    pub summary: String,
}
impl FocusedTestResult {
    pub fn new(passed: bool, summary: impl Into<String>) -> Self {
        Self {
            passed,
            summary: summary.into(),
        }
    }
    pub fn passed(summary: impl Into<String>) -> Self {
        Self::new(true, summary)
    }
    pub fn failed(summary: impl Into<String>) -> Self {
        Self::new(false, summary)
    }
}
impl From<bool> for FocusedTestResult {
    fn from(passed: bool) -> Self {
        Self::new(passed, if passed { "passed" } else { "failed" })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskMetrics {
    pub elapsed_ms: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub estimated_cost_micros: u64,
    pub retry_count: u32,
    pub turn_count: u32,
}
impl TaskMetrics {
    pub fn record_turn(
        &mut self,
        elapsed_ms: u64,
        input_tokens: u64,
        output_tokens: u64,
        cost_micros: u64,
    ) {
        self.elapsed_ms = self.elapsed_ms.saturating_add(elapsed_ms);
        self.input_tokens = self.input_tokens.saturating_add(input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(output_tokens);
        self.estimated_cost_micros = self.estimated_cost_micros.saturating_add(cost_micros);
        self.turn_count = self.turn_count.saturating_add(1);
    }
    pub fn record_retry(&mut self) {
        self.retry_count = self.retry_count.saturating_add(1);
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RoutingState {
    #[default]
    Unassigned,
    Assigned {
        route: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FallbackState {
    #[default]
    Unconfigured,
    Ready {
        provider: String,
        model: Option<String>,
        route: Option<String>,
    },
    Active {
        provider: String,
        model: Option<String>,
        route: Option<String>,
    },
    Exhausted,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderState {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub routing: RoutingState,
    pub fallback: FallbackState,
}
impl ProviderState {
    pub fn new(
        provider: Option<String>,
        model: Option<String>,
        routing: RoutingState,
        fallback: FallbackState,
    ) -> Result<Self, TaskSessionError> {
        let routing = match routing {
            RoutingState::Unassigned => RoutingState::Unassigned,
            RoutingState::Assigned { route } => RoutingState::Assigned {
                route: validate_text("route", &route, MAX_ID_CHARS)?,
            },
        };
        let fallback = match fallback {
            FallbackState::Unconfigured | FallbackState::Exhausted => fallback,
            FallbackState::Ready {
                provider,
                model,
                route,
            } => FallbackState::Ready {
                provider: validate_text("fallback provider", &provider, MAX_ID_CHARS)?,
                model: validate_optional_text("fallback model", model.as_deref(), MAX_ID_CHARS)?,
                route: validate_optional_text("fallback route", route.as_deref(), MAX_ID_CHARS)?,
            },
            FallbackState::Active {
                provider,
                model,
                route,
            } => FallbackState::Active {
                provider: validate_text("fallback provider", &provider, MAX_ID_CHARS)?,
                model: validate_optional_text("fallback model", model.as_deref(), MAX_ID_CHARS)?,
                route: validate_optional_text("fallback route", route.as_deref(), MAX_ID_CHARS)?,
            },
        };
        Ok(Self {
            provider: validate_optional_text("provider", provider.as_deref(), MAX_ID_CHARS)?,
            model: validate_optional_text("model", model.as_deref(), MAX_ID_CHARS)?,
            routing,
            fallback,
        })
    }
    fn validate(self) -> Result<Self, TaskSessionError> {
        Self::new(self.provider, self.model, self.routing, self.fallback)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThreadEntryKind {
    Reply,
    Result,
    HostResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadEntry {
    pub sequence: u64,
    pub kind: ThreadEntryKind,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActionKind {
    #[default]
    Edit,
    Create,
    Delete,
    Test,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActionState {
    Proposed,
    Accepted,
    Applied,
    Rejected { reason: String },
    NeedsRepair { reason: String },
}
impl ActionState {
    fn label(&self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::Accepted => "accepted",
            Self::Applied => "applied",
            Self::Rejected { .. } => "rejected",
            Self::NeedsRepair { .. } => "needs repair",
        }
    }
    pub fn is_pending(&self) -> bool {
        matches!(
            self,
            Self::Proposed | Self::Accepted | Self::NeedsRepair { .. }
        )
    }
    pub fn is_accepted(&self) -> bool {
        matches!(self, Self::Accepted | Self::Applied)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionRevision {
    /// Number of thread entries preceding this proposal revision.
    #[serde(default)]
    pub thread_position: usize,
    pub description: String,
    pub state: ActionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskAction {
    /// Number of thread entries preceding this proposal revision.
    #[serde(default)]
    pub thread_position: usize,
    pub id: ActionId,
    pub kind: ActionKind,
    pub description: String,
    pub state: ActionState,
    pub revisions: Vec<ActionRevision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
}
impl TaskAction {
    pub fn was_accepted(&self) -> bool {
        self.state.is_accepted() || self.revisions.iter().any(|r| r.state.is_accepted())
    }
    pub fn is_pending(&self) -> bool {
        self.state.is_pending()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum VisionCapability {
    #[default]
    Unavailable,
    Available,
}
impl From<bool> for VisionCapability {
    fn from(available: bool) -> Self {
        if available {
            Self::Available
        } else {
            Self::Unavailable
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UploadState {
    Pending,
    Uploaded,
    Failed { reason: String },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScreenshotAnalysisState {
    #[default]
    Pending,
    Completed,
    Failed {
        reason: String,
    },
    Canceled,
}
impl UploadState {
    fn is_pending(&self) -> bool {
        matches!(self, Self::Pending | Self::Failed { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskProvenance {
    pub task_id: TaskId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenshotAttachment {
    pub id: ScreenshotId,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_sha256: Option<String>,
    pub provenance: TaskProvenance,
    /// Most recent provider request that admitted this attachment, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<u64>,
    pub vision: VisionCapability,
    pub upload: UploadState,
    #[serde(default)]
    pub analysis: ScreenshotAnalysisState,
    /// Ephemeral selection for the next provider request. Selections never survive recovery.
    #[serde(skip)]
    pub selected_for_request: bool,
    /// Explicit, one-use consent to send this attachment's pixels.
    #[serde(skip)]
    pub consent_to_send: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageAttribution {
    pub provider: String,
    pub model: Option<String>,
    pub credit: Option<String>,
}
impl ImageAttribution {
    pub fn new(
        provider: impl Into<String>,
        model: Option<String>,
        credit: Option<String>,
    ) -> Result<Self, TaskSessionError> {
        Ok(Self {
            provider: validate_text("image provider", &provider.into(), MAX_ATTRIBUTION_CHARS)?,
            model: validate_optional_text("image model", model.as_deref(), MAX_ATTRIBUTION_CHARS)?,
            credit: validate_optional_text(
                "image credit",
                credit.as_deref(),
                MAX_ATTRIBUTION_CHARS,
            )?,
        })
    }

    fn validate(self) -> Result<Self, TaskSessionError> {
        Self::new(self.provider, self.model, self.credit)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageReviewState {
    Pending,
    Approved,
    Rejected { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageHandoffState {
    Pending,
    Imported,
    Rejected { reason: String },
}
impl ImageHandoffState {
    fn is_pending(&self) -> bool {
        matches!(self, Self::Pending)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeneratedImageArtifact {
    pub id: GeneratedImageId,
    pub source: String,
    pub provenance: TaskProvenance,
    pub attribution: ImageAttribution,
    pub review: ImageReviewState,
    pub handoff: ImageHandoffState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActivityKind {
    UserMessage {
        thread_sequence: u64,
    },
    AiReply {
        thread_sequence: u64,
    },
    Attachment {
        screenshot_id: ScreenshotId,
        upload: UploadState,
        analysis: ScreenshotAnalysisState,
    },
    SemanticAction {
        action_id: ActionId,
        kind: ActionKind,
        description: String,
        state: ActionState,
    },
    GeneratedAsset {
        image_id: GeneratedImageId,
        review: ImageReviewState,
        handoff: ImageHandoffState,
    },
    HostResult {
        thread_sequence: u64,
    },
    FocusedTest {
        run_id: u64,
        status: ValidationStatus,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityEntry {
    pub sequence: u64,
    /// False for a deterministic snapshot synthesized from state saved before activity logging.
    pub recorded: bool,
    pub kind: ActivityKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderSelection {
    Codex,
    OpenRouter,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    pub objective: String,
    pub project_summary: String,
    pub relevant_files: Vec<String>,
    pub relevant_symbols: Vec<String>,
    pub relevant_tests: Vec<String>,
    pub thread: Vec<ThreadEntry>,
    pub actions: BTreeMap<ActionId, TaskAction>,
    pub validation: ValidationStatus,
    #[serde(default)]
    pub validation_run_id: u64,
    pub provider: ProviderState,
    #[serde(default)]
    pub selected_provider: Option<ProviderSelection>,
    pub metrics: TaskMetrics,
    pub screenshots: BTreeMap<ScreenshotId, ScreenshotAttachment>,
    pub generated_images: BTreeMap<GeneratedImageId, GeneratedImageArtifact>,
    #[serde(default)]
    pub activity: Vec<ActivityEntry>,
    pub vision_capability: VisionCapability,
    pub lifecycle: TaskLifecycle,
    pub connection: ConnectionState,
}

impl Task {
    pub fn new(
        id: impl Into<TaskId>,
        objective: impl Into<String>,
        project_summary: impl Into<String>,
    ) -> Result<Self, TaskSessionError> {
        let id = TaskId::try_new(id.into().into_inner())?;
        Ok(Self {
            id,
            objective: validate_text("objective", &objective.into(), MAX_OBJECTIVE_CHARS)?,
            project_summary: validate_text(
                "project summary",
                &project_summary.into(),
                MAX_PROJECT_SUMMARY_CHARS,
            )?,
            relevant_files: Vec::new(),
            relevant_symbols: Vec::new(),
            relevant_tests: Vec::new(),
            thread: Vec::new(),
            actions: BTreeMap::new(),
            validation: ValidationStatus::default(),
            validation_run_id: 0,
            provider: ProviderState::default(),
            selected_provider: None,
            metrics: TaskMetrics::default(),
            screenshots: BTreeMap::new(),
            generated_images: BTreeMap::new(),
            activity: Vec::new(),
            vision_capability: VisionCapability::default(),
            lifecycle: TaskLifecycle::default(),
            connection: ConnectionState::default(),
        })
    }

    fn record_activity(&mut self, kind: ActivityKind) {
        let sequence = self
            .activity
            .last()
            .map_or(1, |entry| entry.sequence.saturating_add(1));
        self.activity.push(ActivityEntry {
            sequence,
            recorded: true,
            kind,
        });
    }

    pub(crate) fn start_activity_recording(&mut self) {
        if self.activity.is_empty() {
            self.activity = self.legacy_activity_snapshot();
        }
    }

    fn legacy_activity_snapshot(&self) -> Vec<ActivityEntry> {
        let mut kinds = Vec::new();
        for entry in &self.thread {
            kinds.push(match entry.kind {
                ThreadEntryKind::Reply => ActivityKind::UserMessage {
                    thread_sequence: entry.sequence,
                },
                ThreadEntryKind::Result => ActivityKind::AiReply {
                    thread_sequence: entry.sequence,
                },
                ThreadEntryKind::HostResult => ActivityKind::HostResult {
                    thread_sequence: entry.sequence,
                },
            });
        }
        kinds.extend(
            self.screenshots
                .values()
                .map(|screenshot| ActivityKind::Attachment {
                    screenshot_id: screenshot.id.clone(),
                    upload: screenshot.upload.clone(),
                    analysis: screenshot.analysis.clone(),
                }),
        );
        kinds.extend(
            self.actions
                .values()
                .map(|action| ActivityKind::SemanticAction {
                    action_id: action.id.clone(),
                    kind: action.kind.clone(),
                    description: action.description.clone(),
                    state: action.state.clone(),
                }),
        );
        kinds.extend(
            self.generated_images
                .values()
                .map(|image| ActivityKind::GeneratedAsset {
                    image_id: image.id.clone(),
                    review: image.review.clone(),
                    handoff: image.handoff.clone(),
                }),
        );
        if !matches!(self.validation, ValidationStatus::NotRun) {
            kinds.push(ActivityKind::FocusedTest {
                run_id: self.validation_run_id,
                status: self.validation.clone(),
            });
        }
        kinds
            .into_iter()
            .enumerate()
            .map(|(index, kind)| ActivityEntry {
                sequence: u64::try_from(index).unwrap_or(u64::MAX).saturating_add(1),
                recorded: false,
                kind,
            })
            .collect()
    }

    /// Returns the recorded chronological log, or stable typed snapshots for legacy tasks.
    /// Snapshot entries have `recorded == false` because their cross-type chronology is unknown.
    pub fn activity_timeline(&self) -> Vec<ActivityEntry> {
        if !self.activity.is_empty() {
            return self.activity.clone();
        }

        self.legacy_activity_snapshot()
    }

    fn ensure_open(&self, action: &'static str) -> Result<(), TaskSessionError> {
        match self.lifecycle {
            TaskLifecycle::Active => Ok(()),
            TaskLifecycle::Canceled => Err(invalid_transition("task", action, "canceled")),
            TaskLifecycle::Completed => Err(invalid_transition("task", action, "completed")),
        }
    }

    fn append_thread(
        &mut self,
        kind: ThreadEntryKind,
        text: impl Into<String>,
    ) -> Result<(), TaskSessionError> {
        self.ensure_open("append thread history to")?;
        self.start_activity_recording();
        if self.thread.len() >= MAX_THREAD_ENTRIES {
            return Err(TaskSessionError::FieldTooLong {
                field: "thread history",
                max: MAX_THREAD_ENTRIES,
                actual: self.thread.len() + 1,
            });
        }
        let text = validate_text("thread entry", &text.into(), MAX_THREAD_TEXT_CHARS)?;
        let sequence = u64::try_from(self.thread.len())
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        self.thread.push(ThreadEntry {
            sequence,
            kind,
            text,
        });
        self.record_activity(match kind {
            ThreadEntryKind::Reply => ActivityKind::UserMessage {
                thread_sequence: sequence,
            },
            ThreadEntryKind::Result => ActivityKind::AiReply {
                thread_sequence: sequence,
            },
            ThreadEntryKind::HostResult => ActivityKind::HostResult {
                thread_sequence: sequence,
            },
        });
        Ok(())
    }

    pub fn append_user_message(&mut self, text: impl Into<String>) -> Result<(), TaskSessionError> {
        self.append_reply(text)
    }
    pub fn append_reply(&mut self, text: impl Into<String>) -> Result<(), TaskSessionError> {
        self.append_thread(ThreadEntryKind::Reply, text)
    }
    pub fn append_result(&mut self, text: impl Into<String>) -> Result<(), TaskSessionError> {
        self.append_thread(ThreadEntryKind::Result, text)
    }
    pub fn append_host_result(&mut self, text: impl Into<String>) -> Result<(), TaskSessionError> {
        self.append_thread(ThreadEntryKind::HostResult, text)
    }

    fn add_reference(
        list: &mut Vec<String>,
        value: impl Into<String>,
        field: &'static str,
        max_items: usize,
    ) -> Result<(), TaskSessionError> {
        if list.len() >= max_items {
            return Err(TaskSessionError::FieldTooLong {
                field,
                max: max_items,
                actual: list.len() + 1,
            });
        }
        let value = validate_text(field, &value.into(), MAX_REFERENCE_CHARS)?;
        if list.iter().any(|existing| existing == &value) {
            return Err(invalid_transition("reference", "add", "already present"));
        }
        list.push(value);
        Ok(())
    }

    pub fn add_relevant_file(&mut self, path: impl Into<String>) -> Result<(), TaskSessionError> {
        self.ensure_open("add a relevant file to")?;
        Self::add_reference(
            &mut self.relevant_files,
            path,
            "relevant file",
            MAX_RELEVANT_FILES,
        )
    }
    pub fn add_relevant_symbol(
        &mut self,
        symbol: impl Into<String>,
    ) -> Result<(), TaskSessionError> {
        self.ensure_open("add a relevant symbol to")?;
        Self::add_reference(
            &mut self.relevant_symbols,
            symbol,
            "relevant symbol",
            MAX_RELEVANT_SYMBOLS,
        )
    }
    pub fn add_relevant_test(&mut self, test: impl Into<String>) -> Result<(), TaskSessionError> {
        self.ensure_open("add a relevant test to")?;
        Self::add_reference(
            &mut self.relevant_tests,
            test,
            "relevant test",
            MAX_RELEVANT_TESTS,
        )?;
        self.validation = ValidationStatus::NotRun;
        Ok(())
    }

    pub fn set_provider_state(&mut self, state: ProviderState) -> Result<(), TaskSessionError> {
        self.ensure_open("set provider state for")?;
        self.provider = state.validate()?;
        Ok(())
    }

    pub fn select_provider(&mut self, provider: ProviderSelection) -> Result<(), TaskSessionError> {
        self.ensure_open("select provider for")?;
        if self.selected_provider != Some(provider) {
            for screenshot in self.screenshots.values_mut() {
                screenshot.selected_for_request = false;
                screenshot.consent_to_send = false;
            }
        }
        self.selected_provider = Some(provider);
        Ok(())
    }
    pub fn record_turn(
        &mut self,
        elapsed_ms: u64,
        input_tokens: u64,
        output_tokens: u64,
        cost_micros: u64,
    ) -> Result<(), TaskSessionError> {
        self.ensure_open("record a turn for")?;
        self.metrics
            .record_turn(elapsed_ms, input_tokens, output_tokens, cost_micros);
        Ok(())
    }

    pub fn propose_action(
        &mut self,
        id: impl Into<ActionId>,
        description: impl Into<String>,
    ) -> Result<(), TaskSessionError> {
        self.propose_action_with_kind(id, ActionKind::default(), description)
    }
    pub fn propose_action_with_kind(
        &mut self,
        id: impl Into<ActionId>,
        kind: ActionKind,
        description: impl Into<String>,
    ) -> Result<(), TaskSessionError> {
        self.ensure_open("propose an action for")?;
        if self.actions.len() >= MAX_ACTIONS {
            return Err(TaskSessionError::FieldTooLong {
                field: "actions",
                max: MAX_ACTIONS,
                actual: self.actions.len() + 1,
            });
        }
        let id = ActionId::try_new(id.into().into_inner())?;
        if self.actions.contains_key(&id) {
            return Err(TaskSessionError::DuplicateActionId(id));
        }
        let description = validate_text(
            "action description",
            &description.into(),
            MAX_ACTION_TEXT_CHARS,
        )?;
        self.start_activity_recording();
        self.actions.insert(
            id.clone(),
            TaskAction {
                thread_position: self.thread.len(),
                id: id.clone(),
                kind,
                description,
                state: ActionState::Proposed,
                revisions: Vec::new(),
                payload: None,
            },
        );
        self.record_action_activity(&id);
        Ok(())
    }

    pub fn propose_action_with_payload(
        &mut self,
        id: impl Into<ActionId>,
        kind: ActionKind,
        description: impl Into<String>,
        payload: serde_json::Value,
    ) -> Result<(), TaskSessionError> {
        validate_action_payload(&payload)?;
        let id = ActionId::try_new(id.into().into_inner())?;
        self.propose_action_with_kind(id.clone(), kind, description)?;
        self.action_mut(id)?.payload = Some(payload);
        Ok(())
    }

    fn action_mut(&mut self, id: impl AsRef<str>) -> Result<&mut TaskAction, TaskSessionError> {
        let id = ActionId::new(id.as_ref());
        self.actions
            .get_mut(&id)
            .ok_or(TaskSessionError::ActionNotFound(id))
    }

    fn record_action_activity(&mut self, id: &ActionId) {
        let action = &self.actions[id];
        self.record_activity(ActivityKind::SemanticAction {
            action_id: action.id.clone(),
            kind: action.kind.clone(),
            description: action.description.clone(),
            state: action.state.clone(),
        });
    }

    pub fn accept_action(&mut self, id: impl AsRef<str>) -> Result<(), TaskSessionError> {
        self.ensure_open("accept an action on")?;
        self.start_activity_recording();
        let id = ActionId::new(id.as_ref());
        let action = self.action_mut(&id)?;
        match &action.state {
            ActionState::Proposed => {
                action.state = ActionState::Accepted;
                self.record_action_activity(&id);
                Ok(())
            }
            state => Err(invalid_transition("action", "accept", state.label())),
        }
    }

    pub fn reject_action(
        &mut self,
        id: impl AsRef<str>,
        reason: impl Into<String>,
    ) -> Result<(), TaskSessionError> {
        self.ensure_open("reject an action on")?;
        let reason = validate_text(
            "action rejection reason",
            &reason.into(),
            MAX_ACTION_TEXT_CHARS,
        )?;
        self.start_activity_recording();
        let id = ActionId::new(id.as_ref());
        let action = self.action_mut(&id)?;
        match &action.state {
            ActionState::Proposed | ActionState::NeedsRepair { .. } => {
                action.state = ActionState::Rejected { reason };
                self.record_action_activity(&id);
                Ok(())
            }
            state => Err(invalid_transition("action", "reject", state.label())),
        }
    }

    pub fn apply_action(&mut self, id: impl AsRef<str>) -> Result<(), TaskSessionError> {
        self.ensure_open("apply an action on")?;
        self.start_activity_recording();
        let id = ActionId::new(id.as_ref());
        let action = self.action_mut(&id)?;
        match &action.state {
            ActionState::Accepted => {
                action.state = ActionState::Applied;
                self.validation = ValidationStatus::NotRun;
                self.record_action_activity(&id);
                Ok(())
            }
            state => Err(invalid_transition("action", "apply", state.label())),
        }
    }
    pub fn mark_action_for_repair(
        &mut self,
        id: impl AsRef<str>,
        reason: impl Into<String>,
    ) -> Result<(), TaskSessionError> {
        self.ensure_open("mark an action for repair on")?;
        let reason = validate_text("repair reason", &reason.into(), MAX_ACTION_TEXT_CHARS)?;
        self.start_activity_recording();
        let id = ActionId::new(id.as_ref());
        let action = self.action_mut(&id)?;
        match &action.state {
            ActionState::Proposed
            | ActionState::Accepted
            | ActionState::Applied
            | ActionState::Rejected { .. } => {
                if action.revisions.len() >= MAX_ACTION_REVISIONS {
                    return Err(TaskSessionError::FieldTooLong {
                        max: MAX_ACTION_REVISIONS,
                        actual: action.revisions.len() + 1,
                        field: "action revisions",
                    });
                }
                action.revisions.push(ActionRevision {
                    thread_position: action.thread_position,
                    description: action.description.clone(),
                    state: action.state.clone(),
                    payload: action.payload.clone(),
                });
                action.state = ActionState::NeedsRepair { reason };
                self.validation = ValidationStatus::NotRun;
                self.record_action_activity(&id);
                Ok(())
            }
            state => Err(invalid_transition("action", "repair", state.label())),
        }
    }

    pub fn repair_action(
        &mut self,
        id: impl AsRef<str>,
        description: impl Into<String>,
    ) -> Result<(), TaskSessionError> {
        self.ensure_open("repair an action on")?;
        let description = validate_text(
            "action description",
            &description.into(),
            MAX_ACTION_TEXT_CHARS,
        )?;
        let thread_position = self.thread.len();
        self.start_activity_recording();
        let id = ActionId::new(id.as_ref());
        let action = self.action_mut(&id)?;
        if action.revisions.len() >= MAX_ACTION_REVISIONS {
            return Err(TaskSessionError::FieldTooLong {
                field: "action revisions",
                max: MAX_ACTION_REVISIONS,
                actual: action.revisions.len() + 1,
            });
        }
        match &action.state {
            ActionState::NeedsRepair { .. }
            | ActionState::Rejected { .. }
            | ActionState::Accepted
            | ActionState::Applied => {
                action.revisions.push(ActionRevision {
                    thread_position: action.thread_position,
                    description: action.description.clone(),
                    state: action.state.clone(),
                    payload: action.payload.clone(),
                });
                action.thread_position = thread_position;
                action.description = description;
                action.payload = None;
                action.state = ActionState::Proposed;
                self.validation = ValidationStatus::NotRun;
                self.record_action_activity(&id);
                Ok(())
            }
            state => Err(invalid_transition("action", "repair", state.label())),
        }
    }

    pub fn pending_actions(&self) -> impl Iterator<Item = &TaskAction> {
        self.actions.values().filter(|action| action.is_pending())
    }

    pub fn repair_action_with_payload(
        &mut self,
        id: impl AsRef<str>,
        description: impl Into<String>,
        payload: serde_json::Value,
    ) -> Result<(), TaskSessionError> {
        validate_action_payload(&payload)?;
        self.repair_action(id.as_ref(), description)?;
        self.action_mut(id)?.payload = Some(payload);
        Ok(())
    }

    pub fn accepted_actions(&self) -> impl Iterator<Item = &TaskAction> {
        self.actions.values().filter(|action| action.was_accepted())
    }

    pub fn set_vision_capability(
        &mut self,
        capability: impl Into<VisionCapability>,
    ) -> Result<(), TaskSessionError> {
        self.ensure_open("set vision capability for")?;
        self.vision_capability = capability.into();
        Ok(())
    }

    pub fn attach_screenshot(
        &mut self,
        id: impl Into<ScreenshotId>,
        source: impl Into<String>,
    ) -> Result<(), TaskSessionError> {
        self.attach_screenshot_inner(id, source, None)
    }

    pub fn attach_screenshot_with_sha256(
        &mut self,
        id: impl Into<ScreenshotId>,
        source: impl Into<String>,
        content_sha256: impl Into<String>,
    ) -> Result<(), TaskSessionError> {
        let content_sha256 = content_sha256.into();
        if content_sha256.len() != 64
            || !content_sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(TaskSessionError::InvalidScreenshotSha256);
        }
        self.attach_screenshot_inner(id, source, Some(content_sha256))
    }

    fn attach_screenshot_inner(
        &mut self,
        id: impl Into<ScreenshotId>,
        source: impl Into<String>,
        content_sha256: Option<String>,
    ) -> Result<(), TaskSessionError> {
        self.ensure_open("attach a screenshot to")?;
        if self.vision_capability != VisionCapability::Available {
            return Err(TaskSessionError::VisionUnavailable);
        }
        if self.screenshots.len() >= MAX_SCREENSHOTS {
            return Err(TaskSessionError::FieldTooLong {
                field: "screenshots",
                max: MAX_SCREENSHOTS,
                actual: self.screenshots.len() + 1,
            });
        }
        let id = ScreenshotId::try_new(id.into().into_inner())?;
        if self.screenshots.contains_key(&id) {
            return Err(TaskSessionError::DuplicateScreenshotId(id));
        }
        let source = validate_text(
            "screenshot source",
            &source.into(),
            MAX_ARTIFACT_SOURCE_CHARS,
        )?;
        self.start_activity_recording();
        self.screenshots.insert(
            id.clone(),
            ScreenshotAttachment {
                id: id.clone(),
                source,
                content_sha256,
                provenance: TaskProvenance {
                    task_id: self.id.clone(),
                },
                request_id: None,
                vision: self.vision_capability,
                upload: UploadState::Pending,
                analysis: ScreenshotAnalysisState::Pending,
                selected_for_request: false,
                consent_to_send: false,
            },
        );
        self.record_screenshot_activity(&id);
        Ok(())
    }

    /// Selects this screenshot and grants consent to send its pixels once.
    pub fn select_screenshot_for_request(
        &mut self,
        id: impl AsRef<str>,
    ) -> Result<(), TaskSessionError> {
        self.ensure_open("select a screenshot on")?;
        let id = ScreenshotId::new(id.as_ref());
        let screenshot = self
            .screenshots
            .get_mut(&id)
            .ok_or_else(|| TaskSessionError::ScreenshotNotFound(id.clone()))?;
        if screenshot.provenance.task_id != self.id
            || screenshot.vision != VisionCapability::Available
        {
            return Err(TaskSessionError::VisionUnavailable);
        }
        screenshot.selected_for_request = true;
        screenshot.consent_to_send = true;
        Ok(())
    }

    /// Revokes any unconsumed selection and consent for this screenshot.
    pub fn unselect_screenshot_for_request(
        &mut self,
        id: impl AsRef<str>,
    ) -> Result<(), TaskSessionError> {
        self.ensure_open("unselect a screenshot on")?;
        let id = ScreenshotId::new(id.as_ref());
        let screenshot = self
            .screenshots
            .get_mut(&id)
            .ok_or_else(|| TaskSessionError::ScreenshotNotFound(id.clone()))?;
        screenshot.selected_for_request = false;
        screenshot.consent_to_send = false;
        Ok(())
    }

    pub fn remove_screenshot(
        &mut self,
        id: impl AsRef<str>,
    ) -> Result<ScreenshotAttachment, TaskSessionError> {
        self.ensure_open("remove a screenshot from")?;
        let id = ScreenshotId::new(id.as_ref());
        self.screenshots
            .remove(&id)
            .ok_or(TaskSessionError::ScreenshotNotFound(id))
    }

    pub(crate) fn take_consented_screenshots(
        &mut self,
        request_id: u64,
    ) -> Vec<ScreenshotAttachment> {
        let task_id = self.id.clone();
        self.screenshots
            .values_mut()
            .filter_map(|screenshot| {
                let admitted = screenshot.provenance.task_id == task_id
                    && screenshot.vision == VisionCapability::Available
                    && screenshot.selected_for_request
                    && screenshot.consent_to_send;
                if admitted {
                    screenshot.request_id = Some(request_id);
                }
                let selected = admitted.then(|| screenshot.clone());
                // Consent and selection are one-use even for malformed recovered state.
                screenshot.selected_for_request = false;
                screenshot.consent_to_send = false;
                selected
            })
            .collect()
    }

    pub(crate) fn clear_screenshot_request_selections(&mut self) {
        for screenshot in self.screenshots.values_mut() {
            screenshot.selected_for_request = false;
            screenshot.consent_to_send = false;
        }
    }

    pub(crate) fn record_screenshot_activity(&mut self, id: &ScreenshotId) {
        let screenshot = &self.screenshots[id];
        self.record_activity(ActivityKind::Attachment {
            screenshot_id: screenshot.id.clone(),
            upload: screenshot.upload.clone(),
            analysis: screenshot.analysis.clone(),
        });
    }

    pub fn mark_screenshot_uploaded(
        &mut self,
        id: impl AsRef<str>,
    ) -> Result<(), TaskSessionError> {
        self.ensure_open("mark a screenshot uploaded on")?;
        self.start_activity_recording();
        let id = ScreenshotId::new(id.as_ref());
        let screenshot = self
            .screenshots
            .get_mut(&id)
            .ok_or_else(|| TaskSessionError::ScreenshotNotFound(id.clone()))?;
        screenshot.upload = UploadState::Uploaded;
        self.record_screenshot_activity(&id);
        Ok(())
    }

    pub fn fail_screenshot_upload(
        &mut self,
        id: impl AsRef<str>,
        reason: impl Into<String>,
    ) -> Result<(), TaskSessionError> {
        self.ensure_open("record a screenshot upload failure on")?;
        let reason = validate_text(
            "screenshot upload failure",
            &reason.into(),
            MAX_ACTION_TEXT_CHARS,
        )?;
        self.start_activity_recording();
        let id = ScreenshotId::new(id.as_ref());
        let screenshot = self
            .screenshots
            .get_mut(&id)
            .ok_or_else(|| TaskSessionError::ScreenshotNotFound(id.clone()))?;
        screenshot.upload = UploadState::Failed { reason };
        self.record_screenshot_activity(&id);
        Ok(())
    }

    pub fn complete_screenshot_analysis(
        &mut self,
        id: impl AsRef<str>,
    ) -> Result<(), TaskSessionError> {
        self.ensure_open("complete screenshot analysis on")?;
        self.start_activity_recording();
        let id = ScreenshotId::new(id.as_ref());
        let screenshot = self
            .screenshots
            .get_mut(&id)
            .ok_or_else(|| TaskSessionError::ScreenshotNotFound(id.clone()))?;
        screenshot.upload = UploadState::Uploaded;
        screenshot.analysis = ScreenshotAnalysisState::Completed;
        self.record_screenshot_activity(&id);
        Ok(())
    }

    pub fn fail_screenshot_analysis(
        &mut self,
        id: impl AsRef<str>,
        reason: impl Into<String>,
    ) -> Result<(), TaskSessionError> {
        self.ensure_open("record a screenshot analysis failure on")?;
        let reason = validate_text(
            "screenshot analysis failure",
            &reason.into(),
            MAX_ACTION_TEXT_CHARS,
        )?;
        self.start_activity_recording();
        let id = ScreenshotId::new(id.as_ref());
        let screenshot = self
            .screenshots
            .get_mut(&id)
            .ok_or_else(|| TaskSessionError::ScreenshotNotFound(id.clone()))?;
        screenshot.upload = UploadState::Failed {
            reason: reason.clone(),
        };
        screenshot.analysis = ScreenshotAnalysisState::Failed { reason };
        self.record_screenshot_activity(&id);
        Ok(())
    }

    pub fn cancel_screenshot_analysis(
        &mut self,
        id: impl AsRef<str>,
    ) -> Result<(), TaskSessionError> {
        self.ensure_open("cancel screenshot analysis on")?;
        self.start_activity_recording();
        let id = ScreenshotId::new(id.as_ref());
        let screenshot = self
            .screenshots
            .get_mut(&id)
            .ok_or_else(|| TaskSessionError::ScreenshotNotFound(id.clone()))?;
        screenshot.analysis = ScreenshotAnalysisState::Canceled;
        self.record_screenshot_activity(&id);
        Ok(())
    }

    pub fn add_generated_image(
        &mut self,
        id: impl Into<GeneratedImageId>,
        source: impl Into<String>,
        attribution: ImageAttribution,
    ) -> Result<(), TaskSessionError> {
        self.ensure_open("add a generated image to")?;
        if self.generated_images.len() >= MAX_IMAGES {
            return Err(TaskSessionError::FieldTooLong {
                field: "generated images",
                max: MAX_IMAGES,
                actual: self.generated_images.len() + 1,
            });
        }
        let id = GeneratedImageId::try_new(id.into().into_inner())?;
        if self.generated_images.contains_key(&id) {
            return Err(TaskSessionError::DuplicateImageId(id));
        }
        let source = validate_text(
            "generated image source",
            &source.into(),
            MAX_ARTIFACT_SOURCE_CHARS,
        )?;
        let attribution = attribution.validate()?;
        self.start_activity_recording();
        self.generated_images.insert(
            id.clone(),
            GeneratedImageArtifact {
                id: id.clone(),
                source,
                provenance: TaskProvenance {
                    task_id: self.id.clone(),
                },
                attribution,
                review: ImageReviewState::Pending,
                handoff: ImageHandoffState::Pending,
            },
        );
        self.record_image_activity(&id);
        Ok(())
    }

    fn image_mut(
        &mut self,
        id: impl AsRef<str>,
    ) -> Result<&mut GeneratedImageArtifact, TaskSessionError> {
        let id = GeneratedImageId::new(id.as_ref());
        self.generated_images
            .get_mut(&id)
            .ok_or(TaskSessionError::ImageNotFound(id))
    }

    fn record_image_activity(&mut self, id: &GeneratedImageId) {
        let image = &self.generated_images[id];
        self.record_activity(ActivityKind::GeneratedAsset {
            image_id: image.id.clone(),
            review: image.review.clone(),
            handoff: image.handoff.clone(),
        });
    }

    pub fn approve_generated_image(&mut self, id: impl AsRef<str>) -> Result<(), TaskSessionError> {
        self.ensure_open("approve a generated image on")?;
        self.start_activity_recording();
        let id = GeneratedImageId::new(id.as_ref());
        let image = self.image_mut(&id)?;
        match &image.review {
            ImageReviewState::Pending => {
                image.review = ImageReviewState::Approved;
                self.record_image_activity(&id);
                Ok(())
            }
            state => Err(invalid_transition(
                "generated image",
                "approve",
                image_review_label(state),
            )),
        }
    }

    pub fn reject_generated_image(
        &mut self,
        id: impl AsRef<str>,
        reason: impl Into<String>,
    ) -> Result<(), TaskSessionError> {
        self.ensure_open("reject a generated image on")?;
        let reason = validate_text(
            "image rejection reason",
            &reason.into(),
            MAX_ACTION_TEXT_CHARS,
        )?;
        self.start_activity_recording();
        let id = GeneratedImageId::new(id.as_ref());
        let image = self.image_mut(&id)?;
        if matches!(image.handoff, ImageHandoffState::Imported) {
            return Err(invalid_transition("generated image", "reject", "imported"));
        }
        image.review = ImageReviewState::Rejected {
            reason: reason.clone(),
        };
        image.handoff = ImageHandoffState::Rejected { reason };
        self.record_image_activity(&id);
        Ok(())
    }

    pub fn import_generated_image(&mut self, id: impl AsRef<str>) -> Result<(), TaskSessionError> {
        self.ensure_open("import a generated image on")?;
        self.start_activity_recording();
        let id = GeneratedImageId::new(id.as_ref());
        let image = self.image_mut(&id)?;
        match (&image.review, &image.handoff) {
            (ImageReviewState::Approved, ImageHandoffState::Pending) => {
                image.handoff = ImageHandoffState::Imported;
                self.validation = ValidationStatus::NotRun;
                self.record_image_activity(&id);
                Ok(())
            }
            (_, ImageHandoffState::Imported) => {
                Err(invalid_transition("generated image", "import", "imported"))
            }
            (ImageReviewState::Pending, _) => Err(invalid_transition(
                "generated image",
                "import",
                "awaiting review",
            )),
            (ImageReviewState::Rejected { .. }, _) => {
                Err(invalid_transition("generated image", "import", "rejected"))
            }
            (_, ImageHandoffState::Rejected { .. }) => {
                Err(invalid_transition("generated image", "import", "rejected"))
            }
        }
    }

    pub fn review_generated_image(
        &mut self,
        id: impl AsRef<str>,
        approved: bool,
        reason: impl Into<String>,
    ) -> Result<(), TaskSessionError> {
        if approved {
            self.approve_generated_image(id)
        } else {
            self.reject_generated_image(id, reason)
        }
    }

    pub fn begin_focused_tests(&mut self) -> Result<(), TaskSessionError> {
        self.ensure_open("begin focused tests on")?;
        if self.validation.is_running() {
            return Err(invalid_transition("focused tests", "begin", "running"));
        }
        self.start_activity_recording();
        self.validation_run_id = self
            .validation_run_id
            .checked_add(1)
            .ok_or_else(|| invalid_transition("focused tests", "begin", "run IDs exhausted"))?;
        self.validation = ValidationStatus::Running;
        self.record_activity(ActivityKind::FocusedTest {
            run_id: self.validation_run_id,
            status: self.validation.clone(),
        });
        Ok(())
    }

    pub fn finish_focused_test_run<R: Into<FocusedTestResult>>(
        &mut self,
        run_id: u64,
        result: R,
    ) -> Result<(), TaskSessionError> {
        if run_id != self.validation_run_id {
            return Err(invalid_transition(
                "focused tests",
                "finish",
                "obsolete run",
            ));
        }
        self.finish_focused_tests(result)
    }

    /// Synchronous completion; background callers must use `finish_focused_test_run`.
    pub fn finish_focused_tests<R: Into<FocusedTestResult>>(
        &mut self,
        result: R,
    ) -> Result<(), TaskSessionError> {
        self.ensure_open("finish focused tests on")?;
        if !self.validation.is_running() {
            return Err(invalid_transition(
                "focused tests",
                "finish",
                validation_label(&self.validation),
            ));
        }
        let result = result.into();
        let summary = if result.summary.trim().is_empty() {
            if result.passed { "passed" } else { "failed" }.to_string()
        } else {
            validate_text("validation summary", &result.summary, MAX_THREAD_TEXT_CHARS)?
        };
        self.start_activity_recording();
        self.validation = if result.passed {
            ValidationStatus::Passed { summary }
        } else {
            ValidationStatus::Failed { summary }
        };
        self.record_activity(ActivityKind::FocusedTest {
            run_id: self.validation_run_id,
            status: self.validation.clone(),
        });
        Ok(())
    }

    pub fn retry(&mut self) -> Result<(), TaskSessionError> {
        self.ensure_open("retry")?;
        if !matches!(self.validation, ValidationStatus::Failed { .. }) {
            return Err(invalid_transition(
                "validation",
                "retry",
                validation_label(&self.validation),
            ));
        }
        self.start_activity_recording();
        self.metrics.record_retry();
        self.validation = ValidationStatus::NotRun;
        self.record_activity(ActivityKind::FocusedTest {
            run_id: self.validation_run_id,
            status: self.validation.clone(),
        });
        Ok(())
    }

    pub fn cancel(&mut self) -> Result<(), TaskSessionError> {
        match self.lifecycle {
            TaskLifecycle::Active => {
                if self.validation.is_running() {
                    self.start_activity_recording();
                }
                self.clear_screenshot_request_selections();
                self.lifecycle = TaskLifecycle::Canceled;
                if self.validation.is_running() {
                    self.validation = ValidationStatus::NotRun;
                    self.record_activity(ActivityKind::FocusedTest {
                        run_id: self.validation_run_id,
                        status: self.validation.clone(),
                    });
                }
                Ok(())
            }
            TaskLifecycle::Canceled => Err(invalid_transition("task", "cancel", "canceled")),
            TaskLifecycle::Completed => Err(invalid_transition("task", "cancel", "completed")),
        }
    }

    pub fn disconnect(&mut self) -> Result<(), TaskSessionError> {
        self.ensure_open("disconnect")?;
        match self.connection {
            ConnectionState::Connected => {
                self.clear_screenshot_request_selections();
                self.connection = ConnectionState::Disconnected;
                Ok(())
            }
            ConnectionState::Disconnected => Err(invalid_transition(
                "connection",
                "disconnect",
                "disconnected",
            )),
        }
    }

    pub fn reconnect(&mut self) -> Result<(), TaskSessionError> {
        self.ensure_open("reconnect")?;
        match self.connection {
            ConnectionState::Disconnected => {
                self.connection = ConnectionState::Connected;
                Ok(())
            }
            ConnectionState::Connected => {
                Err(invalid_transition("connection", "reconnect", "connected"))
            }
        }
    }

    pub fn mark_done(&mut self) -> Result<(), TaskSessionError> {
        self.ensure_open("mark done on")?;
        if !self.validation.is_passing() {
            return Err(invalid_transition(
                "task",
                "mark done",
                validation_label(&self.validation),
            ));
        }
        if self.pending_actions().next().is_some() {
            return Err(invalid_transition("task", "mark done", "pending actions"));
        }
        if self
            .screenshots
            .values()
            .any(|screenshot| screenshot.upload.is_pending())
        {
            return Err(invalid_transition(
                "task",
                "mark done",
                "pending screenshot upload",
            ));
        }
        if self
            .generated_images
            .values()
            .any(|image| image.handoff.is_pending())
        {
            return Err(invalid_transition(
                "task",
                "mark done",
                "pending image handoff",
            ));
        }
        self.lifecycle = TaskLifecycle::Completed;
        Ok(())
    }

    pub fn pending_generated_images(&self) -> impl Iterator<Item = &GeneratedImageArtifact> {
        self.generated_images
            .values()
            .filter(|image| image.handoff.is_pending())
    }
}

fn validation_label(status: &ValidationStatus) -> &'static str {
    match status {
        ValidationStatus::NotRun => "not run",
        ValidationStatus::Running => "running",
        ValidationStatus::Passed { .. } => "passed",
        ValidationStatus::Failed { .. } => "failed",
    }
}

fn image_review_label(review: &ImageReviewState) -> &'static str {
    match review {
        ImageReviewState::Pending => "pending",
        ImageReviewState::Approved => "approved",
        ImageReviewState::Rejected { .. } => "rejected",
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSession {
    pub tasks: BTreeMap<TaskId, Task>,
    pub active_task_id: Option<TaskId>,
}

impl TaskSession {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn new_task(
        &mut self,
        id: impl Into<TaskId>,
        objective: impl Into<String>,
        project_summary: impl Into<String>,
    ) -> Result<TaskId, TaskSessionError> {
        if self.tasks.len() >= MAX_TASKS {
            return Err(TaskSessionError::TaskLimitReached);
        }
        let id = TaskId::try_new(id.into().into_inner())?;
        if self.tasks.contains_key(&id) {
            return Err(TaskSessionError::DuplicateTaskId(id));
        }
        let task = Task::new(id.clone(), objective, project_summary)?;
        self.tasks.insert(id.clone(), task);
        self.active_task_id = Some(id.clone());
        Ok(id)
    }

    pub fn create_task(
        &mut self,
        id: impl Into<TaskId>,
        objective: impl Into<String>,
        project_summary: impl Into<String>,
    ) -> Result<TaskId, TaskSessionError> {
        self.new_task(id, objective, project_summary)
    }

    pub fn active_task_id(&self) -> Option<&TaskId> {
        self.active_task_id.as_ref()
    }

    pub fn active_task(&self) -> Result<&Task, TaskSessionError> {
        let id = self
            .active_task_id
            .as_ref()
            .ok_or(TaskSessionError::NoActiveTask)?;
        self.tasks
            .get(id)
            .ok_or_else(|| TaskSessionError::TaskNotFound(id.clone()))
    }

    pub fn active_task_mut(&mut self) -> Result<&mut Task, TaskSessionError> {
        let id = self
            .active_task_id
            .clone()
            .ok_or(TaskSessionError::NoActiveTask)?;
        self.tasks
            .get_mut(&id)
            .ok_or(TaskSessionError::TaskNotFound(id))
    }

    pub fn task(&self, id: impl AsRef<str>) -> Result<&Task, TaskSessionError> {
        let id = TaskId::new(id.as_ref());
        self.tasks
            .get(&id)
            .ok_or(TaskSessionError::TaskNotFound(id))
    }

    pub fn task_mut(&mut self, id: impl AsRef<str>) -> Result<&mut Task, TaskSessionError> {
        let id = TaskId::new(id.as_ref());
        self.tasks
            .get_mut(&id)
            .ok_or(TaskSessionError::TaskNotFound(id))
    }

    pub fn tasks(&self) -> impl Iterator<Item = &Task> {
        self.tasks.values()
    }

    pub fn task_count(&self) -> usize {
        self.tasks.len()
    }

    pub fn switch_task(&mut self, id: impl AsRef<str>) -> Result<(), TaskSessionError> {
        let id = TaskId::new(id.as_ref());
        if !self.tasks.contains_key(&id) {
            return Err(TaskSessionError::TaskNotFound(id));
        }
        self.active_task_id = Some(id);
        Ok(())
    }

    fn active_mut(&mut self) -> Result<&mut Task, TaskSessionError> {
        self.active_task_mut()
    }

    pub fn append_reply(&mut self, text: impl Into<String>) -> Result<(), TaskSessionError> {
        self.active_mut()?.append_reply(text)
    }

    pub fn append_user_message(&mut self, text: impl Into<String>) -> Result<(), TaskSessionError> {
        self.active_mut()?.append_user_message(text)
    }

    pub fn append_result(&mut self, text: impl Into<String>) -> Result<(), TaskSessionError> {
        self.active_mut()?.append_result(text)
    }

    pub fn append_host_result(&mut self, text: impl Into<String>) -> Result<(), TaskSessionError> {
        self.active_mut()?.append_host_result(text)
    }

    pub fn add_relevant_file(&mut self, path: impl Into<String>) -> Result<(), TaskSessionError> {
        self.active_mut()?.add_relevant_file(path)
    }

    pub fn add_relevant_symbol(
        &mut self,
        symbol: impl Into<String>,
    ) -> Result<(), TaskSessionError> {
        self.active_mut()?.add_relevant_symbol(symbol)
    }

    pub fn add_relevant_test(&mut self, test: impl Into<String>) -> Result<(), TaskSessionError> {
        self.active_mut()?.add_relevant_test(test)
    }

    pub fn set_provider_state(&mut self, state: ProviderState) -> Result<(), TaskSessionError> {
        self.active_mut()?.set_provider_state(state)
    }

    pub fn record_turn(
        &mut self,
        elapsed_ms: u64,
        input_tokens: u64,
        output_tokens: u64,
        cost_micros: u64,
    ) -> Result<(), TaskSessionError> {
        self.active_mut()?
            .record_turn(elapsed_ms, input_tokens, output_tokens, cost_micros)
    }

    pub fn propose_action(
        &mut self,
        id: impl Into<ActionId>,
        description: impl Into<String>,
    ) -> Result<(), TaskSessionError> {
        self.active_mut()?.propose_action(id, description)
    }

    pub fn propose_action_with_kind(
        &mut self,
        id: impl Into<ActionId>,
        kind: ActionKind,
        description: impl Into<String>,
    ) -> Result<(), TaskSessionError> {
        self.active_mut()?
            .propose_action_with_kind(id, kind, description)
    }

    pub fn accept_action(&mut self, id: impl AsRef<str>) -> Result<(), TaskSessionError> {
        self.active_mut()?.accept_action(id)
    }

    pub fn reject_action(
        &mut self,
        id: impl AsRef<str>,
        reason: impl Into<String>,
    ) -> Result<(), TaskSessionError> {
        self.active_mut()?.reject_action(id, reason)
    }

    pub fn apply_action(&mut self, id: impl AsRef<str>) -> Result<(), TaskSessionError> {
        self.active_mut()?.apply_action(id)
    }

    pub fn mark_action_for_repair(
        &mut self,
        id: impl AsRef<str>,
        reason: impl Into<String>,
    ) -> Result<(), TaskSessionError> {
        self.active_mut()?.mark_action_for_repair(id, reason)
    }

    pub fn repair_action(
        &mut self,
        id: impl AsRef<str>,
        description: impl Into<String>,
    ) -> Result<(), TaskSessionError> {
        self.active_mut()?.repair_action(id, description)
    }

    pub fn pending_actions(&self) -> Result<Vec<&TaskAction>, TaskSessionError> {
        Ok(self.active_task()?.pending_actions().collect())
    }

    pub fn set_vision_capability(
        &mut self,
        capability: impl Into<VisionCapability>,
    ) -> Result<(), TaskSessionError> {
        self.active_mut()?.set_vision_capability(capability)
    }

    pub fn attach_screenshot(
        &mut self,
        id: impl Into<ScreenshotId>,
        source: impl Into<String>,
    ) -> Result<(), TaskSessionError> {
        self.active_mut()?.attach_screenshot(id, source)
    }

    pub fn attach_screenshot_with_sha256(
        &mut self,
        id: impl Into<ScreenshotId>,
        source: impl Into<String>,
        content_sha256: impl Into<String>,
    ) -> Result<(), TaskSessionError> {
        self.active_mut()?
            .attach_screenshot_with_sha256(id, source, content_sha256)
    }

    pub fn select_screenshot_for_request(
        &mut self,
        id: impl AsRef<str>,
    ) -> Result<(), TaskSessionError> {
        self.active_mut()?.select_screenshot_for_request(id)
    }

    pub fn unselect_screenshot_for_request(
        &mut self,
        id: impl AsRef<str>,
    ) -> Result<(), TaskSessionError> {
        self.active_mut()?.unselect_screenshot_for_request(id)
    }

    pub fn remove_screenshot(
        &mut self,
        id: impl AsRef<str>,
    ) -> Result<ScreenshotAttachment, TaskSessionError> {
        self.active_mut()?.remove_screenshot(id)
    }

    pub fn mark_screenshot_uploaded(
        &mut self,
        id: impl AsRef<str>,
    ) -> Result<(), TaskSessionError> {
        self.active_mut()?.mark_screenshot_uploaded(id)
    }

    pub fn fail_screenshot_upload(
        &mut self,
        id: impl AsRef<str>,
        reason: impl Into<String>,
    ) -> Result<(), TaskSessionError> {
        self.active_mut()?.fail_screenshot_upload(id, reason)
    }

    pub fn complete_screenshot_analysis(
        &mut self,
        id: impl AsRef<str>,
    ) -> Result<(), TaskSessionError> {
        self.active_mut()?.complete_screenshot_analysis(id)
    }

    pub fn fail_screenshot_analysis(
        &mut self,
        id: impl AsRef<str>,
        reason: impl Into<String>,
    ) -> Result<(), TaskSessionError> {
        self.active_mut()?.fail_screenshot_analysis(id, reason)
    }

    pub fn cancel_screenshot_analysis(
        &mut self,
        id: impl AsRef<str>,
    ) -> Result<(), TaskSessionError> {
        self.active_mut()?.cancel_screenshot_analysis(id)
    }

    pub fn add_generated_image(
        &mut self,
        id: impl Into<GeneratedImageId>,
        source: impl Into<String>,
        attribution: ImageAttribution,
    ) -> Result<(), TaskSessionError> {
        self.active_mut()?
            .add_generated_image(id, source, attribution)
    }

    pub fn approve_generated_image(&mut self, id: impl AsRef<str>) -> Result<(), TaskSessionError> {
        self.active_mut()?.approve_generated_image(id)
    }

    pub fn reject_generated_image(
        &mut self,
        id: impl AsRef<str>,
        reason: impl Into<String>,
    ) -> Result<(), TaskSessionError> {
        self.active_mut()?.reject_generated_image(id, reason)
    }

    pub fn import_generated_image(&mut self, id: impl AsRef<str>) -> Result<(), TaskSessionError> {
        self.active_mut()?.import_generated_image(id)
    }

    pub fn review_generated_image(
        &mut self,
        id: impl AsRef<str>,
        approved: bool,
        reason: impl Into<String>,
    ) -> Result<(), TaskSessionError> {
        self.active_mut()?
            .review_generated_image(id, approved, reason)
    }

    pub fn begin_focused_tests(&mut self) -> Result<(), TaskSessionError> {
        self.active_mut()?.begin_focused_tests()
    }

    pub fn finish_focused_tests<R: Into<FocusedTestResult>>(
        &mut self,
        result: R,
    ) -> Result<(), TaskSessionError> {
        self.active_mut()?.finish_focused_tests(result)
    }

    pub fn retry(&mut self) -> Result<(), TaskSessionError> {
        self.active_mut()?.retry()
    }

    pub fn cancel(&mut self) -> Result<(), TaskSessionError> {
        self.active_mut()?.cancel()
    }

    pub fn disconnect(&mut self) -> Result<(), TaskSessionError> {
        self.active_mut()?.disconnect()
    }

    pub fn reconnect(&mut self) -> Result<(), TaskSessionError> {
        self.active_mut()?.reconnect()
    }

    pub fn mark_done(&mut self) -> Result<(), TaskSessionError> {
        self.active_mut()?.mark_done()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Key {
    Char(char),
    Digit(u8),
    Enter,
    Escape,
    Tab,
    Backspace,
    Up,
    Down,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
}

impl Modifiers {
    pub const NONE: Self = Self {
        ctrl: false,
        alt: false,
        shift: false,
        meta: false,
    };
    pub const CTRL: Self = Self {
        ctrl: true,
        ..Self::NONE
    };
    pub const CTRL_SHIFT: Self = Self {
        ctrl: true,
        shift: true,
        ..Self::NONE
    };
    pub const CTRL_ALT: Self = Self {
        ctrl: true,
        alt: true,
        ..Self::NONE
    };
}

impl Default for Modifiers {
    fn default() -> Self {
        Self::NONE
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct KeyChord {
    pub modifiers: Modifiers,
    pub key: Key,
}

impl KeyChord {
    pub const fn new(modifiers: Modifiers, key: Key) -> Self {
        Self { modifiers, key }
    }

    pub const fn ctrl(key: Key) -> Self {
        Self::new(Modifiers::CTRL, key)
    }

    pub const fn ctrl_shift(key: Key) -> Self {
        Self::new(Modifiers::CTRL_SHIFT, key)
    }

    pub const fn ctrl_alt(key: Key) -> Self {
        Self::new(Modifiers::CTRL_ALT, key)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskSessionCommand {
    OpenCommandPalette,
    Search,
    NewTask,
    SwitchNextTask,
    SwitchPreviousTask,
    SwitchTask(u8),
    FocusReply,
    SendReply,
    AcceptAction,
    RejectAction,
    ApplyAction,
    RunFocusedTests,
    Retry,
    AttachScreenshot,
    GenerateImage,
    ImportGeneratedImage,
    MarkDone,
    Cancel,
    Reconnect,
    FocusGame,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShortcutBinding {
    pub chord: KeyChord,
    pub command: TaskSessionCommand,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShortcutMapper {
    pub bindings: Vec<ShortcutBinding>,
}

impl Default for ShortcutMapper {
    fn default() -> Self {
        Self {
            bindings: vec![
                ShortcutBinding {
                    chord: KeyChord::ctrl(Key::Char('k')),
                    command: TaskSessionCommand::OpenCommandPalette,
                },
                ShortcutBinding {
                    chord: KeyChord::ctrl(Key::Char('f')),
                    command: TaskSessionCommand::Search,
                },
                ShortcutBinding {
                    chord: KeyChord::ctrl(Key::Char('n')),
                    command: TaskSessionCommand::NewTask,
                },
                ShortcutBinding {
                    chord: KeyChord::ctrl(Key::Tab),
                    command: TaskSessionCommand::SwitchNextTask,
                },
                ShortcutBinding {
                    chord: KeyChord::ctrl_shift(Key::Tab),
                    command: TaskSessionCommand::SwitchPreviousTask,
                },
                ShortcutBinding {
                    chord: KeyChord::ctrl(Key::Char('l')),
                    command: TaskSessionCommand::FocusReply,
                },
                ShortcutBinding {
                    chord: KeyChord::new(Modifiers::CTRL, Key::Enter),
                    command: TaskSessionCommand::SendReply,
                },
                ShortcutBinding {
                    chord: KeyChord::ctrl(Key::Char('y')),
                    command: TaskSessionCommand::AcceptAction,
                },
                ShortcutBinding {
                    chord: KeyChord::ctrl_shift(Key::Char('y')),
                    command: TaskSessionCommand::RejectAction,
                },
                ShortcutBinding {
                    chord: KeyChord::ctrl_alt(Key::Enter),
                    command: TaskSessionCommand::ApplyAction,
                },
                ShortcutBinding {
                    chord: KeyChord::ctrl(Key::Char('t')),
                    command: TaskSessionCommand::RunFocusedTests,
                },
                ShortcutBinding {
                    chord: KeyChord::ctrl(Key::Char('r')),
                    command: TaskSessionCommand::Retry,
                },
                ShortcutBinding {
                    chord: KeyChord::ctrl_shift(Key::Char('s')),
                    command: TaskSessionCommand::AttachScreenshot,
                },
                ShortcutBinding {
                    chord: KeyChord::ctrl(Key::Char('g')),
                    command: TaskSessionCommand::GenerateImage,
                },
                ShortcutBinding {
                    chord: KeyChord::ctrl_shift(Key::Char('i')),
                    command: TaskSessionCommand::ImportGeneratedImage,
                },
                ShortcutBinding {
                    chord: KeyChord::ctrl_shift(Key::Char('d')),
                    command: TaskSessionCommand::MarkDone,
                },
                ShortcutBinding {
                    chord: KeyChord::new(Modifiers::CTRL, Key::Escape),
                    command: TaskSessionCommand::Cancel,
                },
                ShortcutBinding {
                    chord: KeyChord::ctrl_shift(Key::Char('r')),
                    command: TaskSessionCommand::Reconnect,
                },
                ShortcutBinding {
                    chord: KeyChord::ctrl_alt(Key::Char('g')),
                    command: TaskSessionCommand::FocusGame,
                },
            ],
        }
    }
}

impl ShortcutMapper {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn command_for(&self, chord: KeyChord) -> Option<TaskSessionCommand> {
        if chord.modifiers == Modifiers::CTRL {
            if let Key::Digit(slot @ 1..=9) = chord.key {
                return Some(TaskSessionCommand::SwitchTask(slot));
            }
        }
        self.bindings
            .iter()
            .find(|binding| binding.chord == chord)
            .map(|binding| binding.command.clone())
    }

    pub fn map(&self, chord: KeyChord) -> Option<TaskSessionCommand> {
        self.command_for(chord)
    }

    pub fn lookup(&self, chord: KeyChord) -> Option<TaskSessionCommand> {
        self.command_for(chord)
    }

    pub fn bindings(&self) -> &[ShortcutBinding] {
        &self.bindings
    }

    pub fn bind(&mut self, chord: KeyChord, command: TaskSessionCommand) {
        if let Some(binding) = self
            .bindings
            .iter_mut()
            .find(|binding| binding.chord == chord)
        {
            binding.command = command;
        } else {
            self.bindings.push(ShortcutBinding { chord, command });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session_with_task() -> TaskSession {
        let mut session = TaskSession::new();
        session
            .new_task("task-1", "first objective", "compact project summary")
            .expect("first task");
        session
    }

    fn pass_tests(session: &mut TaskSession) {
        session.begin_focused_tests().expect("begin tests");
        session
            .finish_focused_tests(FocusedTestResult::passed("focused tests passed"))
            .expect("finish tests");
    }

    #[test]
    fn activity_records_mixed_types_in_mutation_order() {
        let mut session = session_with_task();
        session
            .append_user_message("make the player faster")
            .unwrap();
        session.set_vision_capability(true).unwrap();
        session.attach_screenshot("before", "before.png").unwrap();
        session
            .append_result("I found the movement setting.")
            .unwrap();
        session
            .propose_action_with_kind("speed", ActionKind::Edit, "Increase player speed")
            .unwrap();
        session.accept_action("speed").unwrap();
        session
            .add_generated_image(
                "preview",
                "preview.png",
                ImageAttribution::new("local", None, None).unwrap(),
            )
            .unwrap();
        session.append_host_result("Edit applied by host").unwrap();
        session.begin_focused_tests().unwrap();
        session
            .finish_focused_tests(FocusedTestResult::passed("movement test passed"))
            .unwrap();

        let activity = &session.active_task().unwrap().activity;
        assert_eq!(
            activity
                .iter()
                .map(|entry| entry.sequence)
                .collect::<Vec<_>>(),
            (1..=9).collect::<Vec<_>>()
        );
        assert!(activity.iter().all(|entry| entry.recorded));
        assert!(matches!(activity[0].kind, ActivityKind::UserMessage { .. }));
        assert!(matches!(activity[1].kind, ActivityKind::Attachment { .. }));
        assert!(matches!(activity[2].kind, ActivityKind::AiReply { .. }));
        assert!(matches!(
            activity[3].kind,
            ActivityKind::SemanticAction {
                state: ActionState::Proposed,
                ..
            }
        ));
        assert!(matches!(
            activity[4].kind,
            ActivityKind::SemanticAction {
                state: ActionState::Accepted,
                ..
            }
        ));
        assert!(matches!(
            activity[5].kind,
            ActivityKind::GeneratedAsset { .. }
        ));
        assert!(matches!(activity[6].kind, ActivityKind::HostResult { .. }));
        assert!(matches!(
            activity[7].kind,
            ActivityKind::FocusedTest {
                status: ValidationStatus::Running,
                ..
            }
        ));
        assert!(matches!(
            activity[8].kind,
            ActivityKind::FocusedTest {
                status: ValidationStatus::Passed { .. },
                ..
            }
        ));
    }

    #[test]
    fn failed_mutations_do_not_append_activity() {
        let mut session = session_with_task();
        session.propose_action("edit", "safe edit").unwrap();
        let before = session.active_task().unwrap().activity.clone();
        assert!(session.apply_action("edit").is_err());
        assert_eq!(session.active_task().unwrap().activity, before);
    }

    #[test]
    fn activity_is_task_local() {
        let mut session = session_with_task();
        session.append_user_message("first task").unwrap();
        session
            .new_task("task-2", "second objective", "second summary")
            .unwrap();
        session.append_result("second task").unwrap();

        let first = session.task("task-1").unwrap();
        let second = session.task("task-2").unwrap();
        assert_eq!(first.activity.len(), 1);
        assert_eq!(second.activity.len(), 1);
        assert!(matches!(
            first.activity[0].kind,
            ActivityKind::UserMessage { .. }
        ));
        assert!(matches!(
            second.activity[0].kind,
            ActivityKind::AiReply { .. }
        ));
    }

    #[test]
    fn legacy_task_builds_deterministic_unrecorded_typed_snapshots() {
        let mut task = Task::new("legacy", "edit", "project").unwrap();
        task.append_reply("existing reply").unwrap();
        task.propose_action("z-edit", "existing edit").unwrap();
        task.activity.clear();
        let mut encoded = serde_json::to_value(&task).unwrap();
        encoded.as_object_mut().unwrap().remove("activity");
        let decoded: Task = serde_json::from_value(encoded).unwrap();

        assert!(decoded.activity.is_empty());
        let first = decoded.activity_timeline();
        let second = decoded.activity_timeline();
        assert_eq!(first, second);
        assert_eq!(first.len(), 2);
        assert!(first.iter().all(|entry| !entry.recorded));
        assert!(matches!(first[0].kind, ActivityKind::UserMessage { .. }));
        assert!(matches!(first[1].kind, ActivityKind::SemanticAction { .. }));
    }

    #[test]
    fn first_new_activity_retains_legacy_snapshots_without_claiming_their_order() {
        let mut task = Task::new("legacy", "edit", "project").unwrap();
        task.append_reply("existing user request").unwrap();
        task.activity.clear();

        task.append_result("new AI response").unwrap();

        assert_eq!(task.activity.len(), 2);
        assert!(!task.activity[0].recorded);
        assert!(matches!(
            task.activity[0].kind,
            ActivityKind::UserMessage { .. }
        ));
        assert!(task.activity[1].recorded);
        assert!(matches!(
            task.activity[1].kind,
            ActivityKind::AiReply { .. }
        ));
        assert_eq!(task.activity[0].sequence, 1);
        assert_eq!(task.activity[1].sequence, 2);
    }

    #[test]
    fn tasks_are_isolated_and_new_tasks_start_fresh() {
        let mut session = session_with_task();
        session.append_reply("first-only reply").expect("reply");
        session.add_relevant_file("src/first.rs").expect("file");
        session
            .new_task("task-2", "second objective", "second summary")
            .expect("second task");

        let second = session.active_task().expect("active second task");
        assert_eq!(second.objective, "second objective");
        assert_eq!(second.project_summary, "second summary");
        assert!(second.thread.is_empty());
        assert!(second.relevant_files.is_empty());
        assert!(second.actions.is_empty());

        session.switch_task("task-1").expect("switch to first task");
        let first = session.active_task().expect("active first task");
        assert_eq!(first.thread.len(), 1);
        assert_eq!(first.thread[0].text, "first-only reply");
        assert_eq!(first.relevant_files, vec!["src/first.rs"]);
    }

    #[test]
    fn replies_and_results_only_touch_the_active_task() {
        let mut session = session_with_task();
        session
            .new_task("task-2", "second objective", "second summary")
            .expect("second task");
        session
            .append_result("second result")
            .expect("second result");
        session.switch_task("task-1").expect("switch to first");
        session.append_reply("first reply").expect("first reply");

        assert_eq!(session.task("task-1").expect("first").thread.len(), 1);
        assert_eq!(
            session.task("task-1").expect("first").thread[0].kind,
            ThreadEntryKind::Reply
        );
        assert_eq!(session.task("task-2").expect("second").thread.len(), 1);
        assert_eq!(
            session.task("task-2").expect("second").thread[0].kind,
            ThreadEntryKind::Result
        );
    }

    #[test]
    fn rejected_actions_can_be_repaired_without_losing_accepted_work() {
        let mut session = session_with_task();
        session
            .propose_action("good", "accepted edit")
            .expect("good action");
        session.accept_action("good").expect("accept good");
        session.apply_action("good").expect("apply good");

        session
            .propose_action("bad", "rejected edit")
            .expect("bad action");
        session
            .reject_action("bad", "needs a safer approach")
            .expect("reject bad");
        session
            .mark_action_for_repair("bad", "rewrite the edit")
            .expect("mark repair");
        session
            .repair_action("bad", "repaired edit")
            .expect("repair bad");
        session.accept_action("bad").expect("accept repaired");
        session.apply_action("bad").expect("apply repaired");

        assert_eq!(
            session.task("task-1").expect("task").actions[&ActionId::new("good")].state,
            ActionState::Applied
        );
        let bad = &session.task("task-1").expect("task").actions[&ActionId::new("bad")];
        assert_eq!(bad.state, ActionState::Applied);
        assert_eq!(bad.revisions.len(), 2);
        assert!(bad
            .revisions
            .iter()
            .any(|revision| matches!(revision.state, ActionState::Rejected { .. })));
        assert!(session
            .task("task-1")
            .expect("task")
            .accepted_actions()
            .any(|action| action.id == ActionId::new("good")));
    }

    #[test]
    fn repairing_accepted_and_applied_actions_preserves_acceptance_history() {
        let mut session = session_with_task();
        session
            .propose_action("accepted", "accepted edit")
            .expect("accepted action");
        session.accept_action("accepted").expect("accept action");
        session
            .mark_action_for_repair("accepted", "revise accepted edit")
            .expect("mark accepted action for repair");
        session
            .repair_action("accepted", "repaired accepted edit")
            .expect("repair accepted action");

        session
            .propose_action("applied", "applied edit")
            .expect("applied action");
        session
            .accept_action("applied")
            .expect("accept applied action");
        session.apply_action("applied").expect("apply action");
        session
            .mark_action_for_repair("applied", "revise applied edit")
            .expect("mark applied action for repair");
        session
            .repair_action("applied", "repaired applied edit")
            .expect("repair applied action");

        for id in ["accepted", "applied"] {
            let action = &session.active_task().expect("task").actions[&ActionId::new(id)];
            assert_eq!(action.state, ActionState::Proposed);
            assert!(action.was_accepted(), "{id} lost its accepted revision");
            assert!(session
                .active_task()
                .expect("task")
                .accepted_actions()
                .any(|candidate| candidate.id == ActionId::new(id)));
            assert!(action
                .revisions
                .iter()
                .any(|revision| revision.state.is_accepted()));
        }
    }

    #[test]
    fn completion_requires_passing_tests_and_resolved_actions() {
        let mut session = session_with_task();
        assert!(matches!(
            session.mark_done(),
            Err(TaskSessionError::InvalidTransition { .. })
        ));
        session.begin_focused_tests().expect("begin failed run");
        session
            .finish_focused_tests(FocusedTestResult::failed("compile failed"))
            .expect("finish failed run");
        session.retry().expect("retry");
        assert!(session.mark_done().is_err());

        session.propose_action("edit", "safe edit").expect("action");
        pass_tests(&mut session);
        assert!(session.mark_done().is_err());
        session.accept_action("edit").expect("accept");
        assert!(session.mark_done().is_err());
        session.apply_action("edit").expect("apply");
        assert!(session.mark_done().is_err(), "pre-edit validation is stale");
        pass_tests(&mut session);
        session.mark_done().expect("complete");
        assert_eq!(
            session.active_task().expect("task").lifecycle,
            TaskLifecycle::Completed
        );
        assert!(session.append_reply("too late").is_err());
    }

    #[test]
    fn changing_work_invalidates_in_flight_validation() {
        let mut session = session_with_task();
        session.propose_action("edit", "safe edit").unwrap();
        session.accept_action("edit").unwrap();
        session.begin_focused_tests().unwrap();
        session.apply_action("edit").unwrap();
        assert!(session
            .finish_focused_tests(FocusedTestResult::passed("old run"))
            .is_err());
        pass_tests(&mut session);
        session
            .mark_action_for_repair("edit", "fix the edge case")
            .unwrap();
        session
            .reject_action("edit", "keep the existing implementation")
            .unwrap();
        assert!(session.mark_done().is_err());
        pass_tests(&mut session);
        session.mark_done().unwrap();
    }

    #[test]
    fn obsolete_test_result_cannot_finish_a_newer_run() {
        let mut task = Task::new("task", "edit", "project").unwrap();
        task.begin_focused_tests().unwrap();
        let old_run = task.validation_run_id;
        task.add_relevant_test("tests/new.test.stasis").unwrap();
        task.begin_focused_tests().unwrap();
        let current_run = task.validation_run_id;
        assert!(task
            .finish_focused_test_run(old_run, FocusedTestResult::passed("obsolete"))
            .is_err());
        assert!(task.validation.is_running());
        task.finish_focused_test_run(current_run, FocusedTestResult::failed("current failure"))
            .unwrap();
        assert!(!task.validation.is_passing());
    }

    #[test]
    fn expanding_test_scope_requires_fresh_validation() {
        let mut session = session_with_task();
        session
            .add_relevant_test("tests/first.test.stasis")
            .unwrap();
        pass_tests(&mut session);
        assert!(session
            .add_relevant_test("tests/first.test.stasis")
            .is_err());
        assert!(session.active_task().unwrap().validation.is_passing());
        session
            .add_relevant_test("tests/second.test.stasis")
            .unwrap();
        assert!(session.mark_done().is_err());
    }

    #[test]
    fn action_revisions_preserve_their_chronological_thread_positions() {
        let mut task = Task::new("timeline", "Review revisions", "Project").unwrap();
        task.append_reply("First request").unwrap();
        task.propose_action("edit", "First proposal").unwrap();
        assert_eq!(task.actions["edit"].thread_position, 1);
        task.reject_action("edit", "Try again").unwrap();
        task.append_reply("Second request").unwrap();
        task.repair_action("edit", "Second proposal").unwrap();
        assert_eq!(task.actions["edit"].thread_position, 2);
        assert_eq!(task.actions["edit"].revisions[0].thread_position, 1);
        let restored: Task = serde_json::from_str(&serde_json::to_string(&task).unwrap()).unwrap();
        assert_eq!(restored.actions["edit"].thread_position, 2);
        assert_eq!(restored.actions["edit"].revisions[0].thread_position, 1);
    }

    #[test]
    fn payload_repairs_retain_the_accepted_revision_and_require_approval() {
        let mut task = Task::new("task", "edit", "project").unwrap();
        let original = serde_json::json!({"edits": ["original"]});
        let repaired = serde_json::json!({"edits": ["repaired"]});
        task.propose_action_with_payload(" edit ", ActionKind::Edit, "original", original.clone())
            .unwrap();
        task.accept_action("edit").unwrap();
        task.mark_action_for_repair("edit", "conflict").unwrap();
        task.repair_action_with_payload("edit", "repair", repaired.clone())
            .unwrap();
        let action = &task.actions["edit"];
        assert_eq!(action.payload, Some(repaired));
        assert_eq!(action.revisions[0].payload, Some(original));
        assert_eq!(action.revisions[0].state, ActionState::Accepted);
        assert!(task.apply_action("edit").is_err());
        task.accept_action("edit").unwrap();
        task.apply_action("edit").unwrap();
        task.repair_action("edit", "description only").unwrap();
        assert_eq!(task.actions["edit"].payload, None);
    }

    #[test]
    fn oversized_payload_does_not_mutate_task() {
        let mut task = Task::new("task", "edit", "project").unwrap();
        let before = task.clone();
        assert!(task
            .propose_action_with_payload(
                "edit",
                ActionKind::Edit,
                "large",
                serde_json::Value::String("x".repeat(MAX_ACTION_PAYLOAD_BYTES))
            )
            .is_err());
        assert_eq!(task, before);
    }

    #[test]
    fn legacy_actions_deserialize_without_executable_payloads() {
        let action: TaskAction = serde_json::from_value(serde_json::json!({
            "id": "old", "kind": "Edit", "description": "legacy action",
            "state": "Accepted", "revisions": [{"description": "old revision", "state": "Proposed"}]
        }))
        .unwrap();
        assert_eq!(action.payload, None);
        assert_eq!(action.revisions[0].payload, None);
    }

    #[test]
    fn screenshots_require_vision_and_keep_task_provenance() {
        let mut session = session_with_task();
        assert_eq!(
            session.attach_screenshot("shot-1", "capture.png"),
            Err(TaskSessionError::VisionUnavailable)
        );
        session.set_vision_capability(true).expect("enable vision");
        session
            .attach_screenshot("shot-1", "capture.png")
            .expect("attach screenshot");
        let screenshot =
            &session.active_task().expect("task").screenshots[&ScreenshotId::new("shot-1")];
        assert_eq!(screenshot.provenance.task_id, TaskId::new("task-1"));
        assert_eq!(screenshot.vision, VisionCapability::Available);
        assert_eq!(screenshot.upload, UploadState::Pending);
        session
            .fail_screenshot_upload("shot-1", "temporary upload error")
            .expect("record upload error");
        assert!(
            session.active_task().expect("task").screenshots[&ScreenshotId::new("shot-1")]
                .upload
                .is_pending()
        );
        session.mark_screenshot_uploaded("shot-1").expect("upload");

        assert_eq!(
            session.attach_screenshot_with_sha256("bad-hash", "bad.png", "ABC"),
            Err(TaskSessionError::InvalidScreenshotSha256)
        );
        session
            .attach_screenshot_with_sha256("hashed", "hashed.png", "a".repeat(64))
            .expect("attach screenshot with hash");
        assert_eq!(
            session.active_task().expect("task").screenshots[&ScreenshotId::new("hashed")]
                .content_sha256
                .as_deref(),
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        );
    }

    #[test]
    fn screenshot_consent_is_explicit_one_use_and_recovery_safe() {
        let mut task = Task::new("task", "inspect", "project").unwrap();
        task.set_vision_capability(true).unwrap();
        task.attach_screenshot("shot", "shot.png").unwrap();
        let shot = &task.screenshots[&ScreenshotId::new("shot")];
        assert!(!shot.selected_for_request);
        assert!(!shot.consent_to_send);
        assert!(task.take_consented_screenshots(1).is_empty());

        task.select_screenshot_for_request("shot").unwrap();
        let restored: Task = serde_json::from_str(&serde_json::to_string(&task).unwrap()).unwrap();
        let restored_shot = &restored.screenshots[&ScreenshotId::new("shot")];
        assert!(!restored_shot.selected_for_request);
        assert!(!restored_shot.consent_to_send);

        let selected = task.take_consented_screenshots(7);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].request_id, Some(7));
        assert!(selected[0].consent_to_send);
        let shot = &task.screenshots[&ScreenshotId::new("shot")];
        assert_eq!(shot.request_id, Some(7));
        assert!(!shot.selected_for_request);
        assert!(!shot.consent_to_send);
        assert!(task.take_consented_screenshots(8).is_empty());
    }

    #[test]
    fn provider_change_revokes_pending_screenshot_consent() {
        let mut task = Task::new("task", "inspect", "project").unwrap();
        task.set_vision_capability(true).unwrap();
        task.select_provider(ProviderSelection::Codex).unwrap();
        task.attach_screenshot("shot", "shot.png").unwrap();
        task.select_screenshot_for_request("shot").unwrap();
        task.select_provider(ProviderSelection::OpenRouter).unwrap();
        assert!(task.take_consented_screenshots(1).is_empty());
    }

    #[test]
    fn generated_images_require_explicit_review_and_import() {
        let mut session = session_with_task();
        let invalid_attribution = ImageAttribution {
            provider: String::new(),
            model: None,
            credit: None,
        };
        assert!(matches!(
            session.add_generated_image("invalid", "invalid.png", invalid_attribution),
            Err(TaskSessionError::EmptyField {
                field: "image provider"
            })
        ));
        let attribution = ImageAttribution::new(
            "image-provider",
            Some("model-x".to_string()),
            Some("credit".to_string()),
        )
        .expect("attribution");
        session
            .add_generated_image("image-1", "artifact.png", attribution.clone())
            .expect("image");
        assert!(session.import_generated_image("image-1").is_err());
        session.approve_generated_image("image-1").expect("approve");
        session.import_generated_image("image-1").expect("import");
        assert_eq!(
            session.active_task().expect("task").generated_images
                [&GeneratedImageId::new("image-1")]
                .handoff,
            ImageHandoffState::Imported
        );
        assert!(session
            .reject_generated_image("image-1", "too late")
            .is_err());

        session
            .add_generated_image("image-2", "other.png", attribution)
            .expect("second image");
        session
            .reject_generated_image("image-2", "not suitable")
            .expect("reject image");
        assert!(session.import_generated_image("image-2").is_err());
    }

    #[test]
    fn cancellation_and_connection_transitions_are_independent() {
        let mut canceled = session_with_task();
        canceled.set_vision_capability(true).unwrap();
        canceled.attach_screenshot("shot", "shot.png").unwrap();
        canceled.select_screenshot_for_request("shot").unwrap();
        canceled.cancel().expect("cancel");
        assert_eq!(
            canceled.active_task().expect("task").lifecycle,
            TaskLifecycle::Canceled
        );
        assert_eq!(
            canceled.active_task().expect("task").connection,
            ConnectionState::Connected
        );
        assert!(canceled.reconnect().is_err());
        assert!(canceled.disconnect().is_err());
        assert!(canceled.append_reply("blocked").is_err());
        assert!(!canceled.active_task().unwrap().screenshots["shot"].consent_to_send);

        let mut disconnected = session_with_task();
        disconnected.set_vision_capability(true).unwrap();
        disconnected.attach_screenshot("shot", "shot.png").unwrap();
        disconnected.select_screenshot_for_request("shot").unwrap();
        disconnected.disconnect().expect("disconnect");
        assert_eq!(
            disconnected.active_task().expect("task").lifecycle,
            TaskLifecycle::Active
        );
        assert_eq!(
            disconnected.active_task().expect("task").connection,
            ConnectionState::Disconnected
        );
        assert!(!disconnected.active_task().unwrap().screenshots["shot"].consent_to_send);
        disconnected.reconnect().expect("reconnect");
        assert_eq!(
            disconnected.active_task().expect("task").lifecycle,
            TaskLifecycle::Active
        );
        assert_eq!(
            disconnected.active_task().expect("task").connection,
            ConnectionState::Connected
        );
        disconnected.append_reply("after reconnect").expect("reply");
        assert!(disconnected.reconnect().is_err());
    }

    #[test]
    fn bounds_and_duplicate_ids_are_rejected() {
        let mut session = TaskSession::new();
        assert!(matches!(
            session.new_task("", "objective", "summary"),
            Err(TaskSessionError::EmptyField { field: "TaskId" })
        ));
        session_with_task();
        session
            .new_task("task-1", "objective", "summary")
            .expect("task");
        assert!(matches!(
            session.new_task("task-1", "other", "summary"),
            Err(TaskSessionError::DuplicateTaskId(_))
        ));
        session.propose_action("action", "edit").expect("action");
        assert!(matches!(
            session.propose_action("action", "duplicate"),
            Err(TaskSessionError::DuplicateActionId(_))
        ));
        let too_long = "x".repeat(MAX_OBJECTIVE_CHARS + 1);
        assert!(matches!(
            Task::new("task-long", too_long, "summary"),
            Err(TaskSessionError::FieldTooLong {
                field: "objective",
                ..
            })
        ));
        for index in 0..MAX_RELEVANT_FILES {
            session
                .add_relevant_file(format!("src/file-{index}.rs"))
                .expect("bounded file");
        }
        assert!(matches!(
            session.add_relevant_file("src/overflow.rs"),
            Err(TaskSessionError::FieldTooLong {
                field: "relevant file",
                ..
            })
        ));
    }

    #[test]
    fn shortcut_mapper_covers_keyboard_first_path_without_nul_keys() {
        let mapper = ShortcutMapper::default();
        assert_eq!(
            mapper.command_for(KeyChord::ctrl(Key::Char('k'))),
            Some(TaskSessionCommand::OpenCommandPalette)
        );
        assert_eq!(
            mapper.command_for(KeyChord::ctrl(Key::Char('n'))),
            Some(TaskSessionCommand::NewTask)
        );
        assert_eq!(
            mapper.command_for(KeyChord::ctrl(Key::Enter)),
            Some(TaskSessionCommand::SendReply)
        );
        assert_eq!(
            mapper.command_for(KeyChord::ctrl_alt(Key::Enter)),
            Some(TaskSessionCommand::ApplyAction)
        );
        assert_eq!(
            mapper.command_for(KeyChord::ctrl(Key::Digit(3))),
            Some(TaskSessionCommand::SwitchTask(3))
        );
        assert_eq!(
            mapper.command_for(KeyChord::ctrl_shift(Key::Char('s'))),
            Some(TaskSessionCommand::AttachScreenshot)
        );
        assert_eq!(
            mapper.command_for(KeyChord::ctrl_alt(Key::Char('g'))),
            Some(TaskSessionCommand::FocusGame)
        );
        assert_eq!(
            mapper.command_for(KeyChord::new(Modifiers::NONE, Key::Char('k'))),
            None
        );

        let mut custom = mapper.clone();
        custom.bind(KeyChord::ctrl(Key::Char('x')), TaskSessionCommand::Cancel);
        assert_eq!(
            custom.lookup(KeyChord::ctrl(Key::Char('x'))),
            Some(TaskSessionCommand::Cancel)
        );
    }

    #[test]
    fn session_types_round_trip_through_json() {
        let mut session = session_with_task();
        session.set_vision_capability(true).expect("vision");
        session
            .attach_screenshot("shot", "capture.png")
            .expect("shot");
        session.propose_action("edit", "safe edit").expect("action");
        let encoded = serde_json::to_string(&session).expect("serialize session");
        let decoded: TaskSession = serde_json::from_str(&encoded).expect("deserialize session");
        assert_eq!(decoded, session);
    }
}
