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
        }
    }
}

impl std::error::Error for TaskSessionError {}

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
    pub description: String,
    pub state: ActionState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskAction {
    pub id: ActionId,
    pub kind: ActionKind,
    pub description: String,
    pub state: ActionState,
    pub revisions: Vec<ActionRevision>,
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
    pub provenance: TaskProvenance,
    pub vision: VisionCapability,
    pub upload: UploadState,
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
    pub provider: ProviderState,
    pub metrics: TaskMetrics,
    pub screenshots: BTreeMap<ScreenshotId, ScreenshotAttachment>,
    pub generated_images: BTreeMap<GeneratedImageId, GeneratedImageArtifact>,
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
            provider: ProviderState::default(),
            metrics: TaskMetrics::default(),
            screenshots: BTreeMap::new(),
            generated_images: BTreeMap::new(),
            vision_capability: VisionCapability::default(),
            lifecycle: TaskLifecycle::default(),
            connection: ConnectionState::default(),
        })
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
        Ok(())
    }

    pub fn append_reply(&mut self, text: impl Into<String>) -> Result<(), TaskSessionError> {
        self.append_thread(ThreadEntryKind::Reply, text)
    }
    pub fn append_result(&mut self, text: impl Into<String>) -> Result<(), TaskSessionError> {
        self.append_thread(ThreadEntryKind::Result, text)
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
        )
    }

    pub fn set_provider_state(&mut self, state: ProviderState) -> Result<(), TaskSessionError> {
        self.ensure_open("set provider state for")?;
        self.provider = state.validate()?;
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
        self.actions.insert(
            id.clone(),
            TaskAction {
                id,
                kind,
                description,
                state: ActionState::Proposed,
                revisions: Vec::new(),
            },
        );
        Ok(())
    }

    fn action_mut(&mut self, id: impl AsRef<str>) -> Result<&mut TaskAction, TaskSessionError> {
        let id = ActionId::new(id.as_ref());
        self.actions
            .get_mut(&id)
            .ok_or(TaskSessionError::ActionNotFound(id))
    }

    pub fn accept_action(&mut self, id: impl AsRef<str>) -> Result<(), TaskSessionError> {
        self.ensure_open("accept an action on")?;
        let action = self.action_mut(id)?;
        match &action.state {
            ActionState::Proposed => {
                action.state = ActionState::Accepted;
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
        let action = self.action_mut(id)?;
        match &action.state {
            ActionState::Proposed | ActionState::NeedsRepair { .. } => {
                action.state = ActionState::Rejected { reason };
                Ok(())
            }
            state => Err(invalid_transition("action", "reject", state.label())),
        }
    }

    pub fn apply_action(&mut self, id: impl AsRef<str>) -> Result<(), TaskSessionError> {
        self.ensure_open("apply an action on")?;
        let action = self.action_mut(id)?;
        match &action.state {
            ActionState::Accepted => {
                action.state = ActionState::Applied;
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
        let action = self.action_mut(id)?;
        if matches!(&action.state, ActionState::Rejected { .. }) {
            if action.revisions.len() >= MAX_ACTION_REVISIONS {
                return Err(TaskSessionError::FieldTooLong {
                    max: MAX_ACTION_REVISIONS,
                    actual: action.revisions.len() + 1,
                    field: "action revisions",
                });
            }
            action.revisions.push(ActionRevision {
                description: action.description.clone(),
                state: action.state.clone(),
            });
        }
        match &action.state {
            ActionState::Proposed
            | ActionState::Accepted
            | ActionState::Applied
            | ActionState::Rejected { .. } => {
                action.state = ActionState::NeedsRepair { reason };
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
        let action = self.action_mut(id)?;
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
                    description: action.description.clone(),
                    state: action.state.clone(),
                });
                action.description = description;
                action.state = ActionState::Proposed;
                Ok(())
            }
            state => Err(invalid_transition("action", "repair", state.label())),
        }
    }

    pub fn pending_actions(&self) -> impl Iterator<Item = &TaskAction> {
        self.actions.values().filter(|action| action.is_pending())
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
        self.screenshots.insert(
            id.clone(),
            ScreenshotAttachment {
                id,
                source,
                provenance: TaskProvenance {
                    task_id: self.id.clone(),
                },
                vision: self.vision_capability,
                upload: UploadState::Pending,
            },
        );
        Ok(())
    }

    pub fn mark_screenshot_uploaded(
        &mut self,
        id: impl AsRef<str>,
    ) -> Result<(), TaskSessionError> {
        self.ensure_open("mark a screenshot uploaded on")?;
        let id = ScreenshotId::new(id.as_ref());
        let screenshot = self
            .screenshots
            .get_mut(&id)
            .ok_or_else(|| TaskSessionError::ScreenshotNotFound(id.clone()))?;
        screenshot.upload = UploadState::Uploaded;
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
        let id = ScreenshotId::new(id.as_ref());
        let screenshot = self
            .screenshots
            .get_mut(&id)
            .ok_or_else(|| TaskSessionError::ScreenshotNotFound(id.clone()))?;
        screenshot.upload = UploadState::Failed { reason };
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
        self.generated_images.insert(
            id.clone(),
            GeneratedImageArtifact {
                id,
                source,
                provenance: TaskProvenance {
                    task_id: self.id.clone(),
                },
                attribution,
                review: ImageReviewState::Pending,
                handoff: ImageHandoffState::Pending,
            },
        );
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

    pub fn approve_generated_image(&mut self, id: impl AsRef<str>) -> Result<(), TaskSessionError> {
        self.ensure_open("approve a generated image on")?;
        let image = self.image_mut(id)?;
        match &image.review {
            ImageReviewState::Pending => {
                image.review = ImageReviewState::Approved;
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
        let image = self.image_mut(id)?;
        if matches!(image.handoff, ImageHandoffState::Imported) {
            return Err(invalid_transition("generated image", "reject", "imported"));
        }
        image.review = ImageReviewState::Rejected {
            reason: reason.clone(),
        };
        image.handoff = ImageHandoffState::Rejected { reason };
        Ok(())
    }

    pub fn import_generated_image(&mut self, id: impl AsRef<str>) -> Result<(), TaskSessionError> {
        self.ensure_open("import a generated image on")?;
        let image = self.image_mut(id)?;
        match (&image.review, &image.handoff) {
            (ImageReviewState::Approved, ImageHandoffState::Pending) => {
                image.handoff = ImageHandoffState::Imported;
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
        self.validation = ValidationStatus::Running;
        Ok(())
    }

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
        self.validation = if result.passed {
            ValidationStatus::Passed { summary }
        } else {
            ValidationStatus::Failed { summary }
        };
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
        self.metrics.record_retry();
        self.validation = ValidationStatus::NotRun;
        Ok(())
    }

    pub fn cancel(&mut self) -> Result<(), TaskSessionError> {
        match self.lifecycle {
            TaskLifecycle::Active => {
                self.lifecycle = TaskLifecycle::Canceled;
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

    pub fn append_result(&mut self, text: impl Into<String>) -> Result<(), TaskSessionError> {
        self.active_mut()?.append_result(text)
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
        session.mark_done().expect("complete");
        assert_eq!(
            session.active_task().expect("task").lifecycle,
            TaskLifecycle::Completed
        );
        assert!(session.append_reply("too late").is_err());
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

        let mut disconnected = session_with_task();
        disconnected.disconnect().expect("disconnect");
        assert_eq!(
            disconnected.active_task().expect("task").lifecycle,
            TaskLifecycle::Active
        );
        assert_eq!(
            disconnected.active_task().expect("task").connection,
            ConnectionState::Disconnected
        );
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
