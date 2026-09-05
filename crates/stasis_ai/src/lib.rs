use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

mod openrouter;
pub mod task_controller;
pub mod task_session;

pub use openrouter::{
    ConfiguredProvider, OpenRouterConfig, OpenRouterProvider, PreferredThroughputPolicy,
    ProviderConfig, ProviderKind, RoutingConfig, RoutingSort,
};

pub use task_controller::{
    ProviderActionContext, ProviderActionProposal, ProviderReply, ProviderRequest, ProviderUsage,
    RequestId, TaskController, TaskControllerConfig, TaskControllerError, TaskControllerEvent,
    TaskRequestSnapshot, TaskRequestState,
};

pub use task_session::{
    ActionId, ActionKind, ActionRevision, ActionState, ConnectionState, FallbackState,
    FocusedTestResult, GeneratedImageArtifact, GeneratedImageId, ImageAttribution,
    ImageHandoffState, ImageReviewState, Key, KeyChord, Modifiers, ProviderState, RoutingState,
    ScreenshotAttachment, ScreenshotId, ShortcutBinding, ShortcutMapper, Task, TaskAction, TaskId,
    TaskLifecycle, TaskMetrics, TaskProvenance, TaskSession, TaskSessionCommand, TaskSessionError,
    ThreadEntry, ThreadEntryKind, UploadState, ValidationStatus, VisionCapability,
};

pub const DEFAULT_AGENT_TURNS: usize = 50;
pub const MAX_AGENT_TURNS: usize = 50;
pub const MAX_TOOL_CALLS_PER_TURN: usize = 50;
pub const MAX_WORKING_NOTES_CHARS: usize = 2_000;
pub const DEFAULT_CODEX_MODEL: &str = "gpt-5.6-sol";
pub const DEFAULT_REASONING_EFFORT: &str = "medium";
pub const MAX_OBSERVATION_BYTES: usize = 1024 * 1024;
pub const MIN_COMPACTION_BYTES: usize = 256 * 1024;
pub const MAX_COMPACTION_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_COMPACTION_RETAINED_TURNS: usize = 16;
const MAX_COMPLETION_REJECTIONS: usize = 3;
const AGENT_INSTRUCTION: &str = "Stasis is statically typed and C-like. Declarations use import, struct, global, function, and test `name`(): bool. Receivers put self first: function damage(self: Enemy, amount: i32): void; call enemy.damage(5). Read exact local syntax before editing. Use host-mediated tools through structured tool_calls, not native tools. Later JSONL records are completed; do not repeat them. initial_context start actions are lexical leads, not proof; refine them or use list_symbols. Its options expose stdlib discovery and optional baseline tests. Use canonical_import with read_imports/write_imports. Before behavior writes, batch relevant reads and find_references. Submit related symbols/imports/tests in one contiguous atomic tested write batch; its successful receipt proves completion. Tests are default evidence; get runtime/assets capability only when necessary. Return one response-contract object.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentProfile {
    pub role: String,
    pub instruction: String,
    pub max_turns: usize,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub request_timeout: Option<Duration>,
    pub compaction: Option<AgentCompactionPolicy>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentCompactionPolicy {
    pub max_request_bytes: usize,
    pub retain_recent_turns: usize,
}

impl Default for AgentProfile {
    fn default() -> Self {
        Self {
            role: "Stasis live-workspace coding agent".to_string(),
            instruction: AGENT_INSTRUCTION.to_string(),
            max_turns: DEFAULT_AGENT_TURNS,
            model: None,
            reasoning_effort: None,
            request_timeout: None,
            compaction: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolCall {
    #[serde(rename = "action_id")]
    pub tool: String,
    #[serde(default)]
    pub args: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolObservation {
    pub tool: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ToolObservation {
    pub fn result(tool: impl Into<String>, result: Value) -> Self {
        Self {
            tool: tool.into(),
            result: Some(result),
            error: None,
        }
    }

    pub fn error(tool: impl Into<String>, error: impl Into<String>) -> Self {
        Self {
            tool: tool.into(),
            result: None,
            error: Some(error.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub tool: String,
    pub action_id: String,
    #[serde(rename = "use", alias = "purpose")]
    pub purpose: String,
    #[serde(
        default,
        rename = "required",
        alias = "required_args",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub required_args: Vec<String>,
    #[serde(
        default,
        rename = "optional",
        alias = "optional_args",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub optional_args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ModelResponse {
    ToolCalls {
        working_notes: String,
        #[serde(default)]
        summary: String,
        tool_calls: Vec<ToolCall>,
    },
    Done {
        working_notes: String,
        #[serde(default)]
        summary: String,
    },
}

impl ModelResponse {
    pub fn working_notes(&self) -> &str {
        match self {
            Self::ToolCalls { working_notes, .. } | Self::Done { working_notes, .. } => {
                working_notes
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum AgentEvent {
    Turn {
        current: usize,
        maximum: usize,
    },
    ProviderUsage(Value),
    WorkingNotes(String),
    ToolBatch(Vec<ToolCall>),
    Observations(Vec<ToolObservation>),
    ContextCompacted {
        turns_compacted: usize,
        before_bytes: usize,
        after_bytes: usize,
    },
    Completed(String),
}

#[derive(Serialize)]
struct ModelRequestHeader<'a> {
    record: &'static str,
    schema_version: u32,
    role: &'a str,
    instruction: &'a str,
    tool_specs: &'a [ToolSpec],
    response_contract: &'a Value,
    user_prompt: &'a str,
    initial_context: &'a Value,
}

pub trait ModelProvider {
    fn respond(&mut self, request: &str, canceled: &AtomicBool) -> Result<ModelResponse, String>;

    fn take_usage(&mut self) -> Option<Value> {
        None
    }

    fn requires_action_ids(&self) -> bool {
        false
    }
}

pub trait ToolExecutor {
    fn execute(&mut self, calls: &[ToolCall], canceled: &AtomicBool) -> Vec<ToolObservation>;

    fn validate_completion(&self) -> Result<(), String> {
        Ok(())
    }

    fn terminal_failure(&self) -> Option<String> {
        None
    }
}

pub fn run_agent<P, T, E>(
    provider: &mut P,
    executor: &mut T,
    user_prompt: &str,
    initial_context: Value,
    tool_specs: Vec<ToolSpec>,
    canceled: &AtomicBool,
    emit: E,
) -> Result<String, String>
where
    P: ModelProvider,
    T: ToolExecutor,
    E: FnMut(AgentEvent),
{
    run_agent_with_profile(
        provider,
        executor,
        &AgentProfile::default(),
        user_prompt,
        initial_context,
        tool_specs,
        canceled,
        emit,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn run_agent_with_profile<P, T, E>(
    provider: &mut P,
    executor: &mut T,
    profile: &AgentProfile,
    user_prompt: &str,
    initial_context: Value,
    tool_specs: Vec<ToolSpec>,
    canceled: &AtomicBool,
    mut emit: E,
) -> Result<String, String>
where
    P: ModelProvider,
    T: ToolExecutor,
    E: FnMut(AgentEvent),
{
    if user_prompt.trim().is_empty() {
        return Err("AI request must not be empty".to_string());
    }
    if profile.role.trim().is_empty() || profile.instruction.trim().is_empty() {
        return Err("AI agent profile requires a role and instruction".to_string());
    }
    if profile.max_turns == 0 || profile.max_turns > MAX_AGENT_TURNS {
        return Err(format!(
            "AI agent profile max_turns must be between 1 and {MAX_AGENT_TURNS}"
        ));
    }
    for (field, value) in [
        ("model", profile.model.as_deref()),
        ("reasoning_effort", profile.reasoning_effort.as_deref()),
    ] {
        if value.is_some_and(|value| value.trim().is_empty() || value.len() > 128) {
            return Err(format!(
                "AI agent profile {field} must contain 1..=128 characters when set"
            ));
        }
    }
    if let Some(compaction) = &profile.compaction {
        if !(MIN_COMPACTION_BYTES..=MAX_COMPACTION_BYTES).contains(&compaction.max_request_bytes)
            || compaction.retain_recent_turns == 0
            || compaction.retain_recent_turns > MAX_COMPACTION_RETAINED_TURNS
        {
            return Err(format!(
                "AI compaction requires max_request_bytes between {MIN_COMPACTION_BYTES} and {MAX_COMPACTION_BYTES} and retain_recent_turns between 1 and {MAX_COMPACTION_RETAINED_TURNS}"
            ));
        }
    }
    let mut known_tools = tool_specs
        .iter()
        .map(|spec| spec.tool.clone())
        .collect::<BTreeSet<_>>();
    let mut validation_tool_specs = tool_specs.clone();
    if known_tools.contains("get_capability") {
        validation_tool_specs.extend(asset_tool_specs());
        validation_tool_specs.extend(runtime_tool_specs());
    }
    let mut active_tool_specs = tool_specs.clone();
    let response_contract = response_contract();
    let header = serde_json::to_string(&ModelRequestHeader {
        record: "request",
        schema_version: 1,
        role: &profile.role,
        instruction: &profile.instruction,
        tool_specs: &tool_specs,
        response_contract: &response_contract,
        user_prompt,
        initial_context: &initial_context,
    })
    .map_err(|error| format!("failed encoding append-only AI request header: {error}"))?;
    let mut transcript = AgentTranscript::new(header);
    let mut completion_rejections = 0_usize;
    let require_action_ids = provider.requires_action_ids();
    for turn in 1..=profile.max_turns {
        if canceled.load(Ordering::Acquire) {
            return Err("AI request canceled".to_string());
        }
        emit(AgentEvent::Turn {
            current: turn,
            maximum: profile.max_turns,
        });
        let request = transcript.render()?;
        let response = provider.respond(&request, canceled)?;
        if let Some(usage) = provider.take_usage() {
            emit(AgentEvent::ProviderUsage(usage));
        }
        validate_working_notes(response.working_notes())?;
        emit(AgentEvent::WorkingNotes(
            response.working_notes().to_string(),
        ));
        let response_record = serde_json::to_value(&response)
            .map_err(|error| format!("failed recording AI response: {error}"))?;
        match response {
            ModelResponse::Done { summary, .. } => {
                if let Err(error) = executor.validate_completion() {
                    completion_rejections = completion_rejections.saturating_add(1);
                    let observations = vec![ToolObservation::error("completion_gate", error)];
                    emit(AgentEvent::Observations(observations.clone()));
                    transcript.append(&response_record, &observations)?;
                    if completion_rejections >= MAX_COMPLETION_REJECTIONS {
                        let reason = observations[0]
                            .error
                            .as_deref()
                            .unwrap_or("completion validation failed");
                        return Err(format!(
                            "AI agent repeated an invalid completion {completion_rejections} times: {reason}"
                        ));
                    }
                    compact_transcript(&mut transcript, profile.compaction.as_ref(), &mut emit)?;
                    continue;
                }
                let summary = if summary.trim().is_empty() {
                    "AI request completed".to_string()
                } else {
                    summary
                };
                emit(AgentEvent::Completed(summary.clone()));
                return Ok(summary);
            }
            ModelResponse::ToolCalls { mut tool_calls, .. } => {
                if tool_calls.is_empty() {
                    return Err("model returned an empty tool-call batch".to_string());
                }
                if tool_calls.len() > MAX_TOOL_CALLS_PER_TURN {
                    return Err(format!(
                        "model returned {} tool calls; limit is {MAX_TOOL_CALLS_PER_TURN}",
                        tool_calls.len()
                    ));
                }
                for call in &mut tool_calls {
                    normalize_optional_nulls(call, &validation_tool_specs, require_action_ids);
                }
                let validation_errors = tool_calls
                    .iter()
                    .filter_map(|call| {
                        validate_tool_call(
                            call,
                            &validation_tool_specs,
                            &known_tools,
                            require_action_ids,
                        )
                        .err()
                        .map(|error| (call.tool.clone(), error))
                    })
                    .collect::<Vec<_>>();
                if !validation_errors.is_empty() {
                    let observations = validation_errors
                        .into_iter()
                        .map(|(action_id, error)| {
                            ToolObservation::error(
                                action_id,
                                format!(
                                    "action rejected before execution: {error}; replace only this rejected action ID"
                                ),
                            )
                        })
                        .collect::<Vec<_>>();
                    emit(AgentEvent::Observations(observations.clone()));
                    transcript.append(&response_record, &observations)?;
                    compact_transcript(&mut transcript, profile.compaction.as_ref(), &mut emit)?;
                    continue;
                }
                for call in &mut tool_calls {
                    let spec = resolve_tool_spec(call, &validation_tool_specs, require_action_ids)
                        .expect("validated action has a tool spec");
                    call.tool.clone_from(&spec.tool);
                }
                emit(AgentEvent::ToolBatch(tool_calls.clone()));
                let observations = bound_observations(executor.execute(&tool_calls, canceled));
                emit(AgentEvent::Observations(observations.clone()));
                let mut newly_active_specs = Vec::new();
                for (call, observation) in tool_calls.iter().zip(&observations) {
                    if call.tool != "get_capability" || observation.error.is_some() {
                        continue;
                    }
                    let Some(capability) = call.args.get("name").and_then(Value::as_str) else {
                        continue;
                    };
                    let fallback = match capability {
                        "assets" => asset_tool_specs(),
                        "runtime" => runtime_tool_specs(),
                        _ => Vec::new(),
                    };
                    let discovered = observation
                        .result
                        .as_ref()
                        .and_then(|result| result.get("tool_specs"))
                        .and_then(|specs| {
                            serde_json::from_value::<Vec<ToolSpec>>(specs.clone()).ok()
                        })
                        .filter(|specs| !specs.is_empty())
                        .unwrap_or(fallback);
                    newly_active_specs.extend(merge_tool_specs(&mut active_tool_specs, discovered));
                    known_tools.extend(newly_active_specs.iter().map(|spec| spec.tool.clone()));
                    validation_tool_specs.extend(newly_active_specs.iter().cloned());
                }
                if let Some(error) = executor.terminal_failure() {
                    return Err(error);
                }
                transcript.append(&response_record, &observations)?;
                transcript.append_active_capabilities(&newly_active_specs)?;
                compact_transcript(&mut transcript, profile.compaction.as_ref(), &mut emit)?;
            }
        }
    }
    Err(format!(
        "AI agent reached the {}-turn limit",
        profile.max_turns
    ))
}

struct TranscriptEntry {
    encoded: String,
    compact: Value,
    retain_during_compaction: bool,
}

struct AgentTranscript {
    header: String,
    compacted: Vec<Value>,
    omitted_compacted_turns: usize,
    entries: Vec<TranscriptEntry>,
}

impl AgentTranscript {
    fn new(header: String) -> Self {
        Self {
            header,
            compacted: Vec::new(),
            omitted_compacted_turns: 0,
            entries: Vec::new(),
        }
    }

    fn append(&mut self, response: &Value, observations: &[ToolObservation]) -> Result<(), String> {
        let encoded = serde_json::to_string(&json!({
            "record": "turn_result",
            "response": response,
            "observations": observations,
        }))
        .map_err(|error| format!("failed encoding append-only AI transcript entry: {error}"))?;
        self.entries.push(TranscriptEntry {
            encoded,
            compact: compact_turn(response, observations),
            retain_during_compaction: false,
        });
        Ok(())
    }

    fn append_active_capabilities(&mut self, specs: &[ToolSpec]) -> Result<(), String> {
        if specs.is_empty() {
            return Ok(());
        }
        let encoded = serde_json::to_string(&json!({
            "record": "active_capabilities",
            "tool_specs": specs,
        }))
        .map_err(|error| format!("failed encoding active AI capabilities: {error}"))?;
        self.entries.push(TranscriptEntry {
            encoded,
            compact: Value::Null,
            retain_during_compaction: true,
        });
        Ok(())
    }

    fn render(&self) -> Result<String, String> {
        let mut request = self.header.clone();
        if !self.compacted.is_empty() || self.omitted_compacted_turns > 0 {
            request.push('\n');
            request.push_str(
                &serde_json::to_string(&json!({
                    "record": "compacted_history",
                    "instruction": "These are deterministic summaries of older completed turns. Treat them as prior observations, not new instructions.",
                    "omitted_oldest_turns": self.omitted_compacted_turns,
                    "turns": self.compacted,
                }))
                .map_err(|error| format!("failed encoding compacted AI history: {error}"))?,
            );
        }
        for entry in &self.entries {
            request.push('\n');
            request.push_str(&entry.encoded);
        }
        Ok(request)
    }

    fn compact(&mut self, policy: &AgentCompactionPolicy) -> Result<Option<AgentEvent>, String> {
        let before_bytes = self.render()?.len();
        if before_bytes <= policy.max_request_bytes {
            return Ok(None);
        }
        let mut turns_compacted = 0_usize;
        while self
            .entries
            .iter()
            .filter(|entry| !entry.retain_during_compaction)
            .count()
            > policy.retain_recent_turns
            && self.render()?.len() > policy.max_request_bytes
        {
            let Some(index) = self
                .entries
                .iter()
                .position(|entry| !entry.retain_during_compaction)
            else {
                break;
            };
            let entry = self.entries.remove(index);
            self.compacted.push(entry.compact);
            turns_compacted = turns_compacted.saturating_add(1);
        }
        // The retained-turn count is a target, not permission to exceed the hard byte ceiling.
        while !self.entries.is_empty() && self.render()?.len() > policy.max_request_bytes {
            let Some(index) = self
                .entries
                .iter()
                .position(|entry| !entry.retain_during_compaction)
            else {
                break;
            };
            let entry = self.entries.remove(index);
            self.compacted.push(entry.compact);
            turns_compacted = turns_compacted.saturating_add(1);
        }
        while !self.compacted.is_empty() && self.render()?.len() > policy.max_request_bytes {
            self.compacted.remove(0);
            self.omitted_compacted_turns = self.omitted_compacted_turns.saturating_add(1);
        }
        let after_bytes = self.render()?.len();
        if after_bytes > policy.max_request_bytes {
            return Err(format!(
                "AI request header is {after_bytes} bytes after history compaction; configured limit is {} bytes",
                policy.max_request_bytes
            ));
        }
        Ok(
            (turns_compacted > 0).then_some(AgentEvent::ContextCompacted {
                turns_compacted,
                before_bytes,
                after_bytes,
            }),
        )
    }
}

fn compact_transcript<E: FnMut(AgentEvent)>(
    transcript: &mut AgentTranscript,
    policy: Option<&AgentCompactionPolicy>,
    emit: &mut E,
) -> Result<(), String> {
    if let Some(policy) = policy {
        if let Some(event) = transcript.compact(policy)? {
            emit(event);
        }
    }
    Ok(())
}

fn compact_turn(response: &Value, observations: &[ToolObservation]) -> Value {
    let calls = response
        .get("tool_calls")
        .and_then(Value::as_array)
        .map(|calls| {
            calls
                .iter()
                .take(24)
                .map(|call| {
                    let args = call.get("args").and_then(Value::as_object);
                    let selected_args = args
                        .map(|args| {
                            [
                                "name",
                                "kind",
                                "file",
                                "owner",
                                "signature",
                                "operation",
                                "path",
                                "id",
                                "source_path",
                                "query",
                                "summary",
                                "rationale",
                                "evidence",
                                "next_step",
                            ]
                            .into_iter()
                            .filter_map(|key| {
                                args.get(key)
                                    .map(|value| (key.to_string(), bounded_json_value(value, 1000)))
                            })
                            .collect::<serde_json::Map<String, Value>>()
                        })
                        .unwrap_or_default();
                    json!({
                        "action_id": call.get("action_id").cloned().unwrap_or(Value::Null),
                        "args": selected_args,
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let compact_observations = observations
        .iter()
        .take(24)
        .map(|observation| {
            let result = observation.result.as_ref().map(compact_observation_result);
            json!({
                "tool": observation.tool,
                "ok": observation.error.is_none(),
                "error": observation.error.as_deref().map(|value| bounded_chars(value, 500)),
                "result": result,
            })
        })
        .collect::<Vec<_>>();
    json!({
        "working_notes": bounded_chars(response.get("working_notes").and_then(Value::as_str).unwrap_or(""), 1000),
        "summary": bounded_chars(response.get("summary").and_then(Value::as_str).unwrap_or(""), 500),
        "tool_calls": calls,
        "tool_calls_omitted": response.get("tool_calls").and_then(Value::as_array).map_or(0, |calls| calls.len().saturating_sub(24)),
        "observations": compact_observations,
        "observations_omitted": observations.len().saturating_sub(24),
    })
}

fn compact_observation_result(result: &Value) -> Value {
    let Some(object) = result.as_object() else {
        return json!({"summary": bounded_chars(&result.to_string(), 500)});
    };
    let selected = [
        "status",
        "name",
        "kind",
        "file",
        "receipt",
        "tests",
        "changed_symbols",
        "expected_reload",
        "state_layout_compatible",
    ]
    .into_iter()
    .filter_map(|key| {
        object
            .get(key)
            .map(|value| (key.to_string(), bounded_json_value(value, 1000)))
    })
    .collect::<serde_json::Map<String, Value>>();
    if selected.is_empty() {
        json!({"summary": bounded_chars(&result.to_string(), 500)})
    } else {
        Value::Object(selected)
    }
}

fn bounded_json_value(value: &Value, limit: usize) -> Value {
    let encoded = value.to_string();
    if encoded.chars().count() <= limit {
        value.clone()
    } else {
        json!({
            "summary": bounded_chars(&encoded, limit),
            "truncated": true,
        })
    }
}

fn bounded_chars(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

fn validate_working_notes(notes: &str) -> Result<(), String> {
    let count = notes.chars().count();
    if notes.trim().is_empty() || count > MAX_WORKING_NOTES_CHARS {
        return Err(format!(
            "working_notes must contain 1..={MAX_WORKING_NOTES_CHARS} characters"
        ));
    }
    Ok(())
}

fn resolve_tool_spec<'a>(
    call: &ToolCall,
    specs: &'a [ToolSpec],
    require_action_id: bool,
) -> Option<&'a ToolSpec> {
    specs
        .iter()
        .find(|spec| spec.action_id == call.tool || (!require_action_id && spec.tool == call.tool))
}

fn normalize_optional_nulls(call: &mut ToolCall, specs: &[ToolSpec], require_action_id: bool) {
    let Some(spec) = resolve_tool_spec(call, specs, require_action_id) else {
        return;
    };
    let Some(args) = call.args.as_object_mut() else {
        return;
    };
    for optional in &spec.optional_args {
        if args.get(optional).is_some_and(Value::is_null) {
            args.remove(optional);
        }
    }
}

fn validate_tool_call(
    call: &ToolCall,
    specs: &[ToolSpec],
    known_tools: &BTreeSet<String>,
    require_action_id: bool,
) -> Result<(), String> {
    let spec = resolve_tool_spec(call, specs, require_action_id)
        .filter(|spec| known_tools.contains(spec.tool.as_str()))
        .ok_or_else(|| format!("unsupported or invented AI action ID: {}", call.tool))?;
    let args = call
        .args
        .as_object()
        .ok_or_else(|| format!("AI action {} requires an object args value", call.tool))?;
    for required in &spec.required_args {
        if !args.contains_key(required) {
            return Err(format!("AI action {} requires arg: {required}", call.tool));
        }
    }
    let allowed = spec
        .required_args
        .iter()
        .chain(&spec.optional_args)
        .collect::<BTreeSet<_>>();
    if let Some(unknown) = args.keys().find(|name| !allowed.contains(name)) {
        return Err(format!(
            "AI action {} does not accept arg: {unknown}",
            call.tool
        ));
    }
    Ok(())
}

fn merge_tool_specs(target: &mut Vec<ToolSpec>, additions: Vec<ToolSpec>) -> Vec<ToolSpec> {
    let mut added = Vec::new();
    for spec in additions {
        if target
            .iter()
            .any(|existing| existing.action_id == spec.action_id || existing.tool == spec.tool)
        {
            continue;
        }
        target.push(spec.clone());
        added.push(spec);
    }
    added
}

fn bound_observations(observations: Vec<ToolObservation>) -> Vec<ToolObservation> {
    if observation_bytes(&observations) <= MAX_OBSERVATION_BYTES {
        return observations;
    }
    let mut bounded = observations
        .iter()
        .map(omitted_observation)
        .collect::<Vec<_>>();
    for (index, observation) in observations.into_iter().enumerate() {
        let omitted = std::mem::replace(&mut bounded[index], observation);
        if observation_bytes(&bounded) > MAX_OBSERVATION_BYTES {
            bounded[index] = omitted;
        }
    }
    bounded
}

fn omitted_observation(observation: &ToolObservation) -> ToolObservation {
    let bytes = serde_json::to_vec(observation).map_or(0, |encoded| encoded.len());
    ToolObservation::error(
        &observation.tool,
        format!(
            "observation omitted because its {bytes} bytes would exceed the {MAX_OBSERVATION_BYTES}-byte turn budget; narrow or page the request"
        ),
    )
}

fn observation_bytes(observations: &[ToolObservation]) -> usize {
    serde_json::to_vec(observations).map_or(usize::MAX, |encoded| encoded.len())
}

pub fn response_contract() -> Value {
    json!({
        "accepted_modes": ["tool_calls", "done"],
        "working_notes": format!("required non-empty string, maximum {MAX_WORKING_NOTES_CHARS} characters"),
        "tool_calls": {"maximum": MAX_TOOL_CALLS_PER_TURN, "shape": {"action_id": "host-offered opaque action ID", "args": "native JSON object"}},
    })
}

pub fn model_response_schema_for(tool_specs: &[ToolSpec]) -> Value {
    let mut schema = model_response_schema();
    let action_ids = tool_specs
        .iter()
        .map(|spec| Value::String(spec.action_id.clone()))
        .collect::<Vec<_>>();
    if !action_ids.is_empty() {
        let variants = tool_specs
            .iter()
            .map(|spec| {
                json!({
                    "type": "object",
                    "required": ["action_id", "args"],
                    "properties": {
                        "action_id": {"type": "string", "enum": [spec.action_id]},
                        "args": tool_args_schema(spec),
                    },
                    "additionalProperties": false,
                })
            })
            .collect::<Vec<_>>();
        schema["properties"]["tool_calls"]["items"] = json!({"anyOf": variants});
    }
    schema
}

fn model_response_schema_for_request(request: &str) -> Result<Value, String> {
    let mut lines = request.lines();
    let header: Value = serde_json::from_str(lines.next().unwrap_or_default())
        .map_err(|error| format!("AI request header is not valid JSON: {error}"))?;
    let mut specs: Vec<ToolSpec> = serde_json::from_value(
        header
            .get("tool_specs")
            .cloned()
            .ok_or_else(|| "AI request header omitted tool_specs".to_string())?,
    )
    .map_err(|error| format!("AI request tool_specs are invalid: {error}"))?;
    for line in lines {
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if record.get("record").and_then(Value::as_str) != Some("active_capabilities") {
            continue;
        }
        let Some(active) = record.get("tool_specs") else {
            continue;
        };
        let Ok(active) = serde_json::from_value::<Vec<ToolSpec>>(active.clone()) else {
            continue;
        };
        merge_tool_specs(&mut specs, active);
    }
    Ok(model_response_schema_for(&specs))
}

fn tool_args_schema(spec: &ToolSpec) -> Value {
    match spec.tool.as_str() {
        "propose_semantic_edit" | "repair_semantic_edit" => object_schema(
            &[
                ("proposal_id", string_schema()),
                ("description", string_schema()),
                ("batch", semantic_edit_batch_schema()),
            ],
            &["proposal_id", "description", "batch"],
        ),
        "list_symbols" => object_schema(
            &[
                ("files", array_schema(string_schema(), Some(16))),
                ("query", string_schema()),
                ("kind", string_schema()),
                ("owner", string_schema()),
                ("page", integer_schema(Some(0), None)),
                ("limit", integer_schema(Some(1), Some(64))),
            ],
            &[],
        ),
        "get_stdlib_api" => object_schema(
            &[
                ("module", string_schema()),
                ("query", string_schema()),
                ("kind", string_schema()),
                ("page", integer_schema(Some(0), None)),
                ("limit", integer_schema(Some(1), Some(64))),
            ],
            &[],
        ),
        "find_references" => object_schema(
            &[
                ("symbol", string_schema()),
                ("limit", integer_schema(Some(1), Some(256))),
            ],
            &["symbol"],
        ),
        "list_owner_symbols" => object_schema(&[("owner", string_schema())], &["owner"]),
        "read_symbol" => object_schema(
            &[
                ("name", string_schema()),
                ("kind", string_schema()),
                ("file", string_schema()),
                ("owner", string_schema()),
                ("signature", string_schema()),
            ],
            &["name"],
        ),
        "write_symbol" => object_schema(
            &[
                ("file", string_schema()),
                ("name", string_schema()),
                ("new_source", string_schema()),
                ("operation", enum_schema(&["add", "replace"])),
                ("kind", string_schema()),
                ("owner", string_schema()),
                ("signature", string_schema()),
                ("expected_source_hash", string_schema()),
            ],
            &["file", "name", "new_source"],
        ),
        "delete_symbol" => object_schema(
            &[
                ("name", string_schema()),
                ("file", string_schema()),
                ("kind", string_schema()),
                ("owner", string_schema()),
                ("signature", string_schema()),
                ("expected_source_hash", string_schema()),
            ],
            &["name"],
        ),
        "read_imports" => object_schema(&[("file", string_schema())], &["file"]),
        "write_imports" => object_schema(
            &[
                ("file", string_schema()),
                ("imports", array_schema(string_schema(), None)),
            ],
            &["file", "imports"],
        ),
        "get_diagnostics"
        | "inspect_runtime_state"
        | "run_frame"
        | "take_screenshot"
        | "list_tests"
        | "run_tests" => object_schema(&[], &[]),
        "set_input_state" => object_schema(
            &[
                ("x", number_schema()),
                ("y", number_schema()),
                ("active", json!({"type": "boolean"})),
                ("screen_w", number_schema()),
                ("screen_h", number_schema()),
            ],
            &[],
        ),
        "read_test_file" => object_schema(&[("file", string_schema())], &["file"]),
        "write_test_file" => object_schema(
            &[("file", string_schema()), ("source", string_schema())],
            &["file", "source"],
        ),
        "delete_test_file" => object_schema(&[("file", string_schema())], &["file"]),
        "get_capability" => {
            object_schema(&[("name", enum_schema(&["assets", "runtime"]))], &["name"])
        }
        "request_imagegen_asset" => object_schema(
            &[
                ("filename", string_schema()),
                ("prompt", string_schema()),
                ("purpose", string_schema()),
                ("width", integer_schema(Some(1), Some(2048))),
                ("height", integer_schema(Some(1), Some(2048))),
            ],
            &["filename", "prompt", "purpose"],
        ),
        "write_svg_asset" => object_schema(
            &[
                ("id", string_schema()),
                ("path", string_schema()),
                ("source", string_schema()),
                ("width", integer_schema(Some(1), Some(4096))),
                ("height", integer_schema(Some(1), Some(4096))),
            ],
            &["id", "path", "source", "width", "height"],
        ),
        "write_png_asset" => object_schema(
            &[
                ("id", string_schema()),
                ("path", string_schema()),
                ("width", integer_schema(Some(1), Some(2048))),
                ("height", integer_schema(Some(1), Some(2048))),
                ("background", string_schema()),
                ("shapes", array_schema(png_shape_schema(), Some(512))),
            ],
            &["id", "path", "width", "height", "background", "shapes"],
        ),
        "import_png_asset" => object_schema(
            &[
                ("id", string_schema()),
                ("path", string_schema()),
                ("source_path", string_schema()),
                ("crop_x", integer_schema(Some(0), None)),
                ("crop_y", integer_schema(Some(0), None)),
                ("crop_width", integer_schema(Some(1), None)),
                ("crop_height", integer_schema(Some(1), None)),
                ("transparent_color", string_schema()),
                ("transparent_tolerance", integer_schema(Some(0), Some(255))),
            ],
            &["id", "path", "source_path"],
        ),
        "delete_asset" => object_schema(
            &[("path", string_schema()), ("id", string_schema())],
            &["path"],
        ),
        "write_data_asset" => object_schema(
            &[("path", string_schema()), ("source", string_schema())],
            &["path", "source"],
        ),
        "write_procedural_wav" => object_schema(
            &[
                ("id", string_schema()),
                ("path", string_schema()),
                ("frequency_hz", integer_schema(Some(20), Some(8000))),
                ("duration_ms", integer_schema(Some(20), Some(5000))),
            ],
            &["id", "path", "frequency_hz", "duration_ms"],
        ),
        "record_decision" => object_schema(
            &[
                ("kind", string_schema()),
                ("summary", string_schema()),
                ("rationale", string_schema()),
                ("evidence", string_schema()),
                ("next_step", string_schema()),
            ],
            &["kind", "summary", "rationale", "evidence", "next_step"],
        ),
        "report_blocked" => object_schema(
            &[
                ("reason", string_schema()),
                ("evidence", string_schema()),
                ("next_step", string_schema()),
            ],
            &["reason", "evidence", "next_step"],
        ),
        _ => {
            let mut properties = Vec::new();
            for name in spec.required_args.iter().chain(&spec.optional_args) {
                properties.push((name.as_str(), string_schema()));
            }
            let required = spec
                .required_args
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>();
            object_schema(&properties, &required)
        }
    }
}

fn semantic_edit_batch_schema() -> Value {
    let target = object_schema(
        &[
            ("file", string_schema()),
            (
                "kind",
                enum_schema(&["imports", "globals", "struct", "function", "test"]),
            ),
            ("name", string_schema()),
            ("owner", string_schema()),
            ("signature", string_schema()),
        ],
        &["file", "kind", "name"],
    );
    let edit = object_schema(
        &[
            ("operation", enum_schema(&["add", "update", "delete"])),
            ("target", target),
            ("new_source", string_schema()),
            ("expected_source_hash", string_schema()),
        ],
        &["operation", "target"],
    );
    object_schema(
        &[
            ("schema_version", integer_schema(Some(1), Some(2))),
            ("edits", array_schema(edit, Some(64))),
        ],
        &["schema_version", "edits"],
    )
}

fn string_schema() -> Value {
    json!({"type": "string"})
}

fn number_schema() -> Value {
    json!({"type": "number"})
}

fn integer_schema(minimum: Option<u64>, maximum: Option<u64>) -> Value {
    let mut schema = json!({"type": "integer"});
    if let Some(minimum) = minimum {
        schema["minimum"] = json!(minimum);
    }
    if let Some(maximum) = maximum {
        schema["maximum"] = json!(maximum);
    }
    schema
}

fn enum_schema(values: &[&str]) -> Value {
    json!({"type": "string", "enum": values})
}

fn array_schema(items: Value, max_items: Option<usize>) -> Value {
    let mut schema = json!({"type": "array", "items": items});
    if let Some(max_items) = max_items {
        schema["maxItems"] = json!(max_items);
    }
    schema
}

fn object_schema(properties: &[(&str, Value)], required: &[&str]) -> Value {
    let properties = properties
        .iter()
        .map(|(name, schema)| {
            let schema = if required.contains(name) {
                schema.clone()
            } else {
                json!({"anyOf": [schema, {"type": "null"}]})
            };
            ((*name).to_string(), schema)
        })
        .collect::<serde_json::Map<_, _>>();
    let required = properties.keys().cloned().collect::<Vec<_>>();
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

fn png_shape_schema() -> Value {
    let common = |kind: &str, properties: &[(&str, Value)], required: &[&str]| {
        let mut all = vec![("kind", enum_schema(&[kind])), ("color", string_schema())];
        all.extend(
            properties
                .iter()
                .map(|(name, schema)| (*name, schema.clone())),
        );
        object_schema(&all, required)
    };
    json!({
        "anyOf": [
            common(
                "rect",
                &[
                    ("x", integer_schema(None, None)),
                    ("y", integer_schema(None, None)),
                    ("width", integer_schema(Some(1), Some(4096))),
                    ("height", integer_schema(Some(1), Some(4096))),
                ],
                &["kind", "color", "x", "y", "width", "height"],
            ),
            common(
                "circle",
                &[
                    ("x", integer_schema(None, None)),
                    ("y", integer_schema(None, None)),
                    ("radius", integer_schema(Some(1), Some(2048))),
                ],
                &["kind", "color", "x", "y", "radius"],
            ),
            common(
                "line",
                &[
                    ("x1", integer_schema(None, None)),
                    ("y1", integer_schema(None, None)),
                    ("x2", integer_schema(None, None)),
                    ("y2", integer_schema(None, None)),
                    ("thickness", integer_schema(Some(1), Some(128))),
                ],
                &["kind", "color", "x1", "y1", "x2", "y2", "thickness"],
            ),
        ]
    })
}

pub fn model_response_schema() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "required": ["mode", "working_notes", "summary", "tool_calls"],
        "properties": {
            "mode": {"type": "string", "enum": ["tool_calls", "done"]},
            "working_notes": {"type": "string", "minLength": 1, "maxLength": MAX_WORKING_NOTES_CHARS},
            "summary": {"type": "string"},
            "tool_calls": {
                "type": "array",
                "maxItems": MAX_TOOL_CALLS_PER_TURN,
                "items": {
                    "type": "object",
                    "required": ["action_id", "args"],
                    "properties": {
                        "action_id": {"type": "string", "pattern": "^a_[0-9a-f]{16}$"},
                        "args": {"type": "object", "properties": {}, "required": [], "additionalProperties": false}
                    },
                    "additionalProperties": false
                }
            }
        },
        "additionalProperties": false
    })
}

pub fn workshop_tool_specs() -> Vec<ToolSpec> {
    vec![
        spec("list_symbols", "List symbols. Defaults to entry plus direct imports; kind=test with no files selects known tests. files accepts 16 paths. Returns 32 items by default and a next action when more exist.", &[], &["files", "query", "kind", "owner", "page", "limit"]),
        spec("get_stdlib_api", "No module lists valid modules; module returns filtered/paged public signatures, externs, and canonical_import (64 max).", &[], &["module", "query", "kind", "page", "limit"]),
        spec("find_references", "Group compiler-owned definition/read/write/call uses by containing symbol.", &["symbol"], &["limit"]),
        spec("list_owner_symbols", "List compact symbols owned by one type or group.", &["owner"], &[]),
        spec("read_symbol", "Read one symbol's full source and reusable expected_source_hash.", &["name"], &["kind", "file", "owner", "signature"]),
        spec("write_symbol", "Atomically add or replace a symbol; operation=add creates it. The batch compiles and tests.", &["file", "name", "new_source"], &["operation", "kind", "owner", "signature", "expected_source_hash"]),
        spec("delete_symbol", "Atomically delete a symbol.", &["name"], &["file", "kind", "owner", "signature", "expected_source_hash"]),
        spec("read_imports", "Read one source file's imports group.", &["file"], &[]),
        spec("write_imports", "Atomically replace imports from path strings, including canonical_import.", &["file", "imports"], &[]),
        spec("get_diagnostics", "Read the latest compiler diagnostics.", &[], &[]),
        spec("set_input_state", "Set simulated input state.", &[], &["x", "y", "active", "screen_w", "screen_h"]),
        spec("inspect_runtime_state", "Read bounded live scalar state.", &[], &[]),
        spec("run_frame", "Advance the live runtime by one deterministic tick.", &[], &[]),
        spec("take_screenshot", "Capture a logical render snapshot and runtime state.", &[], &[]),
        spec("list_tests", "List Stasis test files.", &[], &[]),
        spec("read_test_file", "Read one Stasis test file.", &["file"], &[]),
        spec("write_test_file", "Create or replace one Stasis test file.", &["file", "source"], &[]),
        spec("delete_test_file", "Delete one Stasis test file.", &["file"], &[]),
        spec("run_tests", "Run the optional baseline/current suite; writes compile and test automatically.", &[], &[]),
    ]
}

pub fn live_tool_specs() -> Vec<ToolSpec> {
    const LIVE_TOOLS: &[&str] = &[
        "list_symbols",
        "get_stdlib_api",
        "find_references",
        "read_symbol",
        "write_symbol",
        "delete_symbol",
        "read_imports",
        "write_imports",
        "run_tests",
    ];
    let mut tools = workshop_tool_specs()
        .into_iter()
        .filter(|spec| LIVE_TOOLS.contains(&spec.tool.as_str()))
        .collect::<Vec<_>>();
    tools.push(spec(
        "get_capability",
        "Load tools and policy; name must be assets or runtime.",
        &["name"],
        &[],
    ));
    tools
}

pub fn action_id_for_tool(tool: &str) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in b"stasis-action-v1:".iter().chain(tool.as_bytes()) {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("a_{hash:016x}")
}
pub fn offered_action(tool: &str, purpose: &str, args: Value) -> Value {
    json!({
        "action_id": action_id_for_tool(tool),
        "purpose": purpose,
        "args": args,
    })
}
fn spec(tool: &str, purpose: &str, required: &[&str], optional: &[&str]) -> ToolSpec {
    ToolSpec {
        tool: tool.to_string(),
        action_id: action_id_for_tool(tool),
        purpose: purpose.to_string(),
        required_args: required.iter().map(|value| (*value).to_string()).collect(),
        optional_args: optional.iter().map(|value| (*value).to_string()).collect(),
    }
}

pub struct CodexExecProvider {
    executable: PathBuf,
    model: String,
    reasoning_effort: String,
    last_usage: Option<Value>,
    run: Option<TemporaryRun>,
    images: Vec<PathBuf>,
    web_search: bool,
    call_count: u32,
    request_timeout: Option<Duration>,
}

impl Default for CodexExecProvider {
    fn default() -> Self {
        Self {
            executable: std::env::var_os("STASIS_CODEX_EXE")
                .map(PathBuf::from)
                .unwrap_or_else(default_codex_executable),
            model: std::env::var("STASIS_AI_MODEL")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_CODEX_MODEL.to_string()),
            reasoning_effort: std::env::var("STASIS_AI_REASONING_EFFORT")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_REASONING_EFFORT.to_string()),
            last_usage: None,
            run: None,
            images: Vec::new(),
            web_search: false,
            call_count: 0,
            request_timeout: None,
        }
    }
}

pub fn asset_tool_specs() -> Vec<ToolSpec> {
    vec![
        spec("request_imagegen_asset", "Persist a host ImageGen request, wait for one PNG, and return source_path. Dimensions default to 1024x1024 and may be at most 2048x2048.", &["filename", "prompt", "purpose"], &["width", "height"]),
        spec("write_svg_asset", "Stage one bounded SVG under assets/generated and derive its v2 manifest entry.", &["id", "path", "source", "width", "height"], &[]),
        spec("write_png_asset", "Stage a deterministic filled-shape PNG and derive its v2 manifest entry. Colors are #RRGGBB or #RRGGBBAA. Shapes: rect: {kind,color,x,y,width,height}; circle: {kind,color,x,y,radius} with center x/y; line: {kind,color,x1,y1,x2,y2,thickness}. fill, stroke, stroke_width, cx/cy, and line width are not supported.", &["id", "path", "width", "height", "background", "shapes"], &[]),
        spec("import_png_asset", "Validate and stage an ImageGen PNG under assets/generated and derive its v2 manifest entry. Supply all four crop fields together. transparent_color is #RRGGBB; tolerance defaults to 12. Background removal fails if the border stays opaque or the subject is nearly erased.", &["id", "path", "source_path"], &["crop_x", "crop_y", "crop_width", "crop_height", "transparent_color", "transparent_tolerance"]),
        spec("delete_asset", "Stage deletion of a generated asset and matching manifest entry. id is required for sprites/audio and omitted for manifest-free JSON/CSV.", &["path"], &["id"]),
        spec("write_data_asset", "Stage bounded JSON or CSV under assets/generated.", &["path", "source"], &[]),
        spec("write_procedural_wav", "Stage deterministic mono PCM audio and derive its v2 manifest entry.", &["id", "path", "frequency_hz", "duration_ms"], &[]),
    ]
}

pub fn runtime_tool_specs() -> Vec<ToolSpec> {
    workshop_tool_specs()
        .into_iter()
        .filter(|spec| matches!(spec.tool.as_str(), "inspect_runtime_state" | "run_frame"))
        .collect()
}

pub fn project_ai_tool_specs() -> Vec<ToolSpec> {
    live_tool_specs()
}

pub fn gauntlet_tool_specs() -> Vec<ToolSpec> {
    let mut tools = live_tool_specs();
    tools.retain(|spec| spec.tool != "get_capability");
    tools.extend(runtime_tool_specs());
    tools.extend(asset_tool_specs());
    tools.push(spec("record_decision", "Persist one concise Gauntlet decision, rationale, evidence summary, and next step for future fresh agents. Record conclusions and tradeoffs, never hidden chain-of-thought.", &["kind", "summary", "rationale", "evidence", "next_step"], &[]));
    tools.push(spec("report_blocked", "Immediately terminate this builder attempt when a non-recoverable environment, harness, permission, or missing-capability condition makes completion impossible with the supplied tools. Do not use this for an ordinary code/test failure that can be corrected.", &["reason", "evidence", "next_step"], &[]));
    tools
}

impl CodexExecProvider {
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    pub fn with_reasoning_effort(mut self, reasoning_effort: impl Into<String>) -> Self {
        self.reasoning_effort = reasoning_effort.into();
        self
    }

    pub fn with_images(mut self, images: Vec<PathBuf>) -> Self {
        self.images = images;
        self
    }

    pub fn with_web_search(mut self, enabled: bool) -> Self {
        self.web_search = enabled;
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = Some(timeout);
        self
    }

    pub fn call_count(&self) -> u32 {
        self.call_count
    }

    pub fn respond_structured<T: DeserializeOwned>(
        &mut self,
        request: &str,
        schema: &Value,
        canceled: &AtomicBool,
    ) -> Result<T, String> {
        let source = self.run_codex(request, schema, canceled)?;
        serde_json::from_str(&source)
            .map_err(|error| format!("Codex returned invalid structured JSON: {error}"))
    }

    fn run_codex(
        &mut self,
        request: &str,
        schema: &Value,
        canceled: &AtomicBool,
    ) -> Result<String, String> {
        self.last_usage = None;
        self.call_count = self.call_count.saturating_add(1);
        if self.run.is_none() {
            self.run = Some(TemporaryRun::create()?);
        }
        let run = self.run.as_ref().expect("AI temporary run initialized");
        fs::write(
            &run.schema,
            serde_json::to_vec_pretty(schema).map_err(|error| error.to_string())?,
        )
        .map_err(|error| format!("failed writing Codex output schema: {error}"))?;
        for image in &self.images {
            if !image.is_file() {
                return Err(format!("Codex image does not exist: {}", image.display()));
            }
        }
        let mut command = Command::new(&self.executable);
        let stderr = fs::File::create(&run.stderr)
            .map_err(|error| format!("failed creating Codex error capture: {error}"))?;
        if self.web_search {
            command.arg("--search");
        }
        command
            .arg("exec")
            .arg("--ephemeral")
            .arg("--ignore-user-config")
            .arg("--ignore-rules")
            .arg("--sandbox")
            .arg("read-only")
            .arg("--skip-git-repo-check")
            .arg("--color")
            .arg("never")
            .arg("--json")
            .arg("--cd")
            .arg(&run.root)
            .arg("--output-schema")
            .arg(&run.schema)
            .arg("--output-last-message")
            .arg(&run.output);
        for image in &self.images {
            command.arg("--image").arg(image);
        }
        command.arg("--model").arg(&self.model);
        command.arg("--config").arg(format!(
            "model_reasoning_effort=\"{}\"",
            self.reasoning_effort
        ));
        command
            .arg("-")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(stderr));
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let provider_job = ProviderProcessJob::new()?;
        let mut child = command.spawn().map_err(|error| {
            format!(
                "failed starting Codex; install/sign in to Codex or set STASIS_CODEX_EXE: {error}"
            )
        })?;
        if let Err(error) = provider_job.assign(&child) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "Codex stdout was unavailable".to_string())?;
        let usage_worker = std::thread::spawn(move || read_codex_usage(stdout));
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| "Codex stdin was unavailable".to_string())?;
        let request = request.as_bytes().to_vec();
        let input_worker = std::thread::spawn(move || {
            stdin
                .write_all(&request)
                .map_err(|error| format!("failed sending Codex request: {error}"))
        });
        let started = std::time::Instant::now();
        loop {
            if canceled.load(Ordering::Acquire) {
                let _ = child.kill();
                let _ = child.wait();
                let _ = input_worker.join();
                let _ = usage_worker.join();
                return Err("AI request canceled".to_string());
            }
            if let Some(timeout) = self.request_timeout {
                if started.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = input_worker.join();
                    let _ = usage_worker.join();
                    return Err(format!(
                        "AI request exceeded its {} second timeout",
                        timeout.as_secs()
                    ));
                }
            }
            match child
                .try_wait()
                .map_err(|error| format!("failed waiting for Codex: {error}"))?
            {
                Some(status) if status.success() => break,
                Some(status) => {
                    let _ = input_worker.join();
                    let _ = usage_worker.join();
                    return Err(codex_failure_message(&run.stderr, status));
                }
                None => std::thread::sleep(Duration::from_millis(50)),
            }
        }
        input_worker
            .join()
            .map_err(|_| "Codex input writer panicked".to_string())??;
        self.last_usage = usage_worker
            .join()
            .map_err(|_| "Codex usage reader panicked".to_string())??;
        fs::read_to_string(&run.output)
            .map_err(|error| format!("Codex did not produce a final response: {error}"))
    }
}

#[cfg(not(windows))]
struct ProviderProcessJob;

#[cfg(not(windows))]
impl ProviderProcessJob {
    fn new() -> Result<Self, String> {
        Ok(Self)
    }

    fn assign(&self, _child: &std::process::Child) -> Result<(), String> {
        Ok(())
    }
}

#[cfg(windows)]
struct ProviderProcessJob(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl ProviderProcessJob {
    fn new() -> Result<Self, String> {
        use std::mem::{size_of, zeroed};
        use windows_sys::Win32::System::JobObjects::{
            CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };

        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(format!(
                "failed creating Codex provider job object: {}",
                std::io::Error::last_os_error()
            ));
        }
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let configured = unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if configured == 0 {
            let error = std::io::Error::last_os_error();
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(handle);
            }
            return Err(format!(
                "failed configuring Codex provider job object: {error}"
            ));
        }
        Ok(Self(handle))
    }

    fn assign(&self, child: &std::process::Child) -> Result<(), String> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;

        let assigned = unsafe {
            AssignProcessToJobObject(
                self.0,
                child.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE,
            )
        };
        if assigned == 0 {
            return Err(format!(
                "failed assigning Codex provider to its cleanup job: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }
}

#[cfg(windows)]
impl Drop for ProviderProcessJob {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

fn default_codex_executable() -> PathBuf {
    #[cfg(windows)]
    if let Some(app_data) = std::env::var_os("APPDATA") {
        let npm = PathBuf::from(app_data).join(
            "npm/node_modules/@openai/codex/node_modules/@openai/codex-win32-x64/\
             vendor/x86_64-pc-windows-msvc/bin/codex.exe",
        );
        if npm.is_file() {
            return npm;
        }
    }
    PathBuf::from("codex")
}

impl ModelProvider for CodexExecProvider {
    fn respond(&mut self, request: &str, canceled: &AtomicBool) -> Result<ModelResponse, String> {
        let schema = model_response_schema_for_request(request)?;
        let source = self.run_codex(request, &schema, canceled)?;
        decode_codex_response(&source)
    }

    fn take_usage(&mut self) -> Option<Value> {
        self.last_usage.take()
    }

    fn requires_action_ids(&self) -> bool {
        true
    }
}

fn read_codex_usage(stdout: impl Read) -> Result<Option<Value>, String> {
    let mut usage = None;
    for line in BufReader::new(stdout).lines() {
        let line = line.map_err(|error| format!("failed reading Codex JSON events: {error}"))?;
        let Ok(event) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if event.get("type").and_then(Value::as_str) == Some("turn.completed") {
            usage = event.get("usage").cloned();
        }
    }
    Ok(usage)
}

fn decode_model_response(source: &str, provider: &str) -> Result<ModelResponse, String> {
    let value: Value = serde_json::from_str(source)
        .map_err(|error| format!("{provider} returned invalid agent JSON: {error}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| format!("{provider} returned a non-object agent response"))?;
    let allowed = ["mode", "working_notes", "summary", "tool_calls"]
        .into_iter()
        .collect::<BTreeSet<_>>();
    if let Some(field) = object
        .keys()
        .find(|field| !allowed.contains(field.as_str()))
    {
        return Err(format!(
            "{provider} returned unknown response field: {field}"
        ));
    }
    let response: ModelResponse = serde_json::from_value(value)
        .map_err(|error| format!("{provider} returned invalid agent response: {error}"))?;
    if let ModelResponse::ToolCalls { tool_calls, .. } = &response {
        if tool_calls.iter().any(|call| !call.args.is_object()) {
            return Err(format!(
                "{provider} returned tool args that were not native JSON objects"
            ));
        }
    }
    Ok(response)
}

fn decode_codex_response(source: &str) -> Result<ModelResponse, String> {
    decode_model_response(source, "Codex")
}
struct TemporaryRun {
    root: PathBuf,
    schema: PathBuf,
    output: PathBuf,
    stderr: PathBuf,
}

impl TemporaryRun {
    fn create() -> Result<Self, String> {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("stasis_ai_{}_{}", std::process::id(), stamp));
        fs::create_dir(&root)
            .map_err(|error| format!("failed creating isolated AI directory: {error}"))?;
        Ok(Self {
            schema: root.join("response.schema.json"),
            output: root.join("response.json"),
            stderr: root.join("stderr.log"),
            root,
        })
    }
}

fn codex_failure_message(stderr: &Path, status: std::process::ExitStatus) -> String {
    let detail = fs::read_to_string(stderr).ok().and_then(|text| {
        let start = text.rfind("ERROR:").or_else(|| text.rfind("error:"))?;
        let detail = text[start..]
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        Some(detail.chars().take(500).collect::<String>())
    });
    match detail {
        Some(detail) => format!("Codex exited with status {status}: {detail}"),
        None => format!("Codex exited with status {status}; verify `codex login` and the model"),
    }
}

impl Drop for TemporaryRun {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

pub fn contract_json() -> Value {
    json!({
        "schema_version": 1,
        "limits": {
            "default_agent_turns": DEFAULT_AGENT_TURNS,
            "maximum_profile_turns": MAX_AGENT_TURNS,
            "tool_calls_per_turn": MAX_TOOL_CALLS_PER_TURN,
            "working_notes_characters": MAX_WORKING_NOTES_CHARS,
            "compaction_minimum_bytes": MIN_COMPACTION_BYTES,
            "compaction_maximum_bytes": MAX_COMPACTION_BYTES,
            "compaction_maximum_retained_turns": MAX_COMPACTION_RETAINED_TURNS,
        },
        "tool_specs": workshop_tool_specs(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_proposal_schema_exposes_a_native_batch_and_hash_guard() {
        for tool in ["propose_semantic_edit", "repair_semantic_edit"] {
            let spec = ToolSpec {
                tool: tool.into(),
                action_id: action_id_for_tool(tool),
                purpose: "review edits".into(),
                required_args: vec!["proposal_id".into(), "description".into(), "batch".into()],
                optional_args: Vec::new(),
            };
            let schema = tool_args_schema(&spec);
            let batch = &schema["properties"]["batch"];
            assert_eq!(batch["type"], "object");
            let edit = &batch["properties"]["edits"]["items"];
            assert_eq!(edit["properties"]["target"]["type"], "object");
            assert_eq!(
                edit["properties"]["operation"]["enum"],
                json!(["add", "update", "delete"])
            );
            assert_eq!(
                edit["properties"]["expected_source_hash"]["anyOf"][0]["type"],
                "string"
            );
            assert_eq!(edit["additionalProperties"], false);
        }
    }

    #[cfg(windows)]
    #[test]
    fn provider_job_terminates_an_assigned_child_when_dropped() {
        let mut child = Command::new("cmd.exe")
            .args(["/C", "ping -n 30 127.0.0.1 >NUL"])
            .spawn()
            .expect("long-lived child");
        let job = ProviderProcessJob::new().expect("provider job");
        job.assign(&child).expect("assign child");

        drop(job);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if child.try_wait().expect("child status").is_some() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "assigned child survived job closure"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }
    use std::sync::atomic::AtomicBool;

    struct Responses(Vec<ModelResponse>);
    impl ModelProvider for Responses {
        fn respond(
            &mut self,
            _request: &str,
            _canceled: &AtomicBool,
        ) -> Result<ModelResponse, String> {
            Ok(self.0.remove(0))
        }
    }

    #[derive(Default)]
    struct Tools(usize);
    impl ToolExecutor for Tools {
        fn execute(&mut self, calls: &[ToolCall], _canceled: &AtomicBool) -> Vec<ToolObservation> {
            self.0 += calls.len();
            calls
                .iter()
                .map(|call| ToolObservation::result(&call.tool, json!({"ok": true})))
                .collect()
        }
    }

    #[test]
    fn bounded_loop_executes_tools_then_finishes() {
        let mut provider = Responses(vec![
            ModelResponse::ToolCalls {
                working_notes: "Intent: inspect. Observed: none. Next: list. Blocker: none."
                    .to_string(),
                summary: String::new(),
                tool_calls: vec![ToolCall {
                    tool: "list_symbols".to_string(),
                    args: json!({}),
                }],
            },
            ModelResponse::Done {
                working_notes: "Intent: finish. Observed: verified. Next: none. Blocker: none."
                    .to_string(),
                summary: "verified".to_string(),
            },
        ]);
        let mut tools = Tools::default();
        let result = run_agent(
            &mut provider,
            &mut tools,
            "inspect",
            json!({}),
            workshop_tool_specs(),
            &AtomicBool::new(false),
            |_| {},
        )
        .expect("agent");
        assert_eq!(result, "verified");
        assert_eq!(tools.0, 1);
    }

    #[test]
    fn fifty_tool_calls_execute_in_one_turn() {
        let calls = (0..MAX_TOOL_CALLS_PER_TURN)
            .map(|index| ToolCall {
                tool: "read_symbol".to_string(),
                args: json!({"name": format!("function_{index}")}),
            })
            .collect();
        let mut provider = Responses(vec![
            ModelResponse::ToolCalls {
                working_notes: "Read the explicitly selected related functions together."
                    .to_string(),
                summary: String::new(),
                tool_calls: calls,
            },
            ModelResponse::Done {
                working_notes: "The requested symbol batch is available for review.".to_string(),
                summary: "verified".to_string(),
            },
        ]);
        let mut tools = Tools::default();

        run_agent(
            &mut provider,
            &mut tools,
            "inspect selected functions",
            json!({}),
            workshop_tool_specs(),
            &AtomicBool::new(false),
            |_| {},
        )
        .expect("fifty-call batch");

        assert_eq!(DEFAULT_AGENT_TURNS, 50);
        assert_eq!(MAX_AGENT_TURNS, 50);
        assert_eq!(tools.0, 50);
        assert_eq!(contract_json()["limits"]["default_agent_turns"], 50);
        assert_eq!(contract_json()["limits"]["maximum_profile_turns"], 50);
        assert_eq!(contract_json()["limits"]["tool_calls_per_turn"], 50);
    }

    #[test]
    fn explicit_profiles_accept_the_maximum_turn_limit() {
        let mut responses = (0..16)
            .map(|index| ModelResponse::ToolCalls {
                working_notes: format!("Inspect bounded decision input {index}."),
                summary: String::new(),
                tool_calls: vec![ToolCall {
                    tool: "list_symbols".to_string(),
                    args: json!({"query": format!("symbol-{index}")}),
                }],
            })
            .collect::<Vec<_>>();
        responses.push(ModelResponse::Done {
            working_notes: "The extended bounded profile completed.".to_string(),
            summary: "extended".to_string(),
        });
        let mut provider = Responses(responses);
        let mut tools = Tools::default();
        let result = run_agent_with_profile(
            &mut provider,
            &mut tools,
            &AgentProfile {
                role: "Gauntlet builder".to_string(),
                instruction: "Complete the bounded workstream.".to_string(),
                max_turns: 50,
                model: None,
                reasoning_effort: None,
                request_timeout: None,
                compaction: None,
            },
            "extended task",
            json!({}),
            workshop_tool_specs(),
            &AtomicBool::new(false),
            |_| {},
        )
        .expect("extended profile");
        assert_eq!(result, "extended");
        assert_eq!(tools.0, 16);
    }

    #[test]
    fn profiles_reject_turn_limits_above_fifty() {
        let mut profile = AgentProfile::default();
        profile.max_turns = 51;
        let mut provider = Responses(vec![]);
        let mut tools = Tools::default();

        let error = run_agent_with_profile(
            &mut provider,
            &mut tools,
            &profile,
            "invalid turn limit",
            json!({}),
            workshop_tool_specs(),
            &AtomicBool::new(false),
            |_| {},
        )
        .expect_err("51 turns exceeds the shared maximum");

        assert_eq!(error, "AI agent profile max_turns must be between 1 and 50");
    }

    #[test]
    fn substantial_full_source_observations_fit_the_batch_budget() {
        let source = "x".repeat(18 * 1024);
        let observations = (0..MAX_TOOL_CALLS_PER_TURN)
            .map(|index| {
                ToolObservation::result(
                    "read_symbol",
                    json!({"name": format!("function_{index}"), "source": source}),
                )
            })
            .collect::<Vec<_>>();
        assert!(
            serde_json::to_vec(&observations)
                .expect("observations")
                .len()
                > 64 * 1024
        );

        let bounded = bound_observations(observations);

        assert_eq!(bounded.len(), 50);
        assert!(bounded
            .iter()
            .all(|observation| observation.error.is_none()));
    }

    #[test]
    fn oversized_observation_batches_preserve_results_that_fit() {
        let observations = vec![
            ToolObservation::result("large_a", json!({"source": "a".repeat(700 * 1024)})),
            ToolObservation::result("large_b", json!({"source": "b".repeat(700 * 1024)})),
            ToolObservation::result("small", json!({"value": 7})),
        ];

        let bounded = bound_observations(observations);

        assert_eq!(bounded.len(), 3);
        assert!(observation_bytes(&bounded) <= MAX_OBSERVATION_BYTES);
        assert_eq!(
            bounded[0].result.as_ref().unwrap()["source"]
                .as_str()
                .unwrap()
                .len(),
            700 * 1024
        );
        assert!(bounded[1]
            .error
            .as_deref()
            .is_some_and(|error| error.contains("omitted")));
        assert_eq!(bounded[2].result.as_ref().unwrap()["value"], 7);
    }

    #[test]
    fn later_turns_receive_prior_calls_and_observations() {
        struct RecordingResponses {
            responses: Vec<ModelResponse>,
            requests: Vec<String>,
        }
        impl ModelProvider for RecordingResponses {
            fn respond(
                &mut self,
                request: &str,
                _canceled: &AtomicBool,
            ) -> Result<ModelResponse, String> {
                self.requests.push(request.to_string());
                Ok(self.responses.remove(0))
            }
        }

        let mut provider = RecordingResponses {
            responses: vec![
                ModelResponse::ToolCalls {
                    working_notes: "Inspect the relevant symbol once.".to_string(),
                    summary: String::new(),
                    tool_calls: vec![ToolCall {
                        tool: "read_symbol".to_string(),
                        args: json!({"name": "tick"}),
                    }],
                },
                ModelResponse::ToolCalls {
                    working_notes: "Inspect one related symbol in the next batch.".to_string(),
                    summary: String::new(),
                    tool_calls: vec![ToolCall {
                        tool: "read_symbol".to_string(),
                        args: json!({"name": "render"}),
                    }],
                },
                ModelResponse::Done {
                    working_notes: "The prior inspection is visible and complete.".to_string(),
                    summary: "verified".to_string(),
                },
            ],
            requests: Vec::new(),
        };

        run_agent(
            &mut provider,
            &mut Tools::default(),
            "inspect",
            json!({}),
            workshop_tool_specs(),
            &AtomicBool::new(false),
            |_| {},
        )
        .expect("agent");

        assert_eq!(provider.requests.len(), 3);
        let header = serde_json::from_str::<Value>(&provider.requests[0]).expect("request header");
        assert_eq!(header["record"], "request");
        assert!(header.get("observations").is_none());
        let first_entry = serde_json::from_str::<Value>(
            provider.requests[1]
                .lines()
                .nth(1)
                .expect("first transcript entry"),
        )
        .expect("transcript JSON");
        assert_eq!(
            first_entry["response"]["tool_calls"][0]["args"]["name"],
            "tick"
        );
        assert_eq!(first_entry["observations"][0]["result"]["ok"], true);
        assert_eq!(provider.requests[2].lines().count(), 3);
        assert!(provider.requests[1].starts_with(&format!("{}\n", provider.requests[0])));
        assert!(provider.requests[2].starts_with(&format!("{}\n", provider.requests[1])));
    }

    #[test]
    fn request_header_serializes_stable_fields_before_dynamic_content() {
        struct RecordingProvider(Option<String>);
        impl ModelProvider for RecordingProvider {
            fn respond(
                &mut self,
                request: &str,
                _canceled: &AtomicBool,
            ) -> Result<ModelResponse, String> {
                self.0 = Some(request.to_string());
                Ok(ModelResponse::Done {
                    working_notes: "The request header order was captured.".to_string(),
                    summary: "captured".to_string(),
                })
            }
        }
        let mut provider = RecordingProvider(None);
        run_agent(
            &mut provider,
            &mut Tools::default(),
            "dynamic user prompt",
            json!({"dynamic": "initial context"}),
            live_tool_specs(),
            &AtomicBool::new(false),
            |_| {},
        )
        .expect("agent");

        let header = provider.0.expect("request header");
        let positions = [
            "\"role\":",
            "\"instruction\":",
            "\"tool_specs\":",
            "\"response_contract\":",
            "\"user_prompt\":",
            "\"initial_context\":",
        ]
        .map(|field| header.find(field).expect("serialized field"));
        assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn compaction_replaces_old_payloads_with_deterministic_history() {
        struct RecordingResponses {
            responses: Vec<ModelResponse>,
            requests: Vec<String>,
        }
        impl ModelProvider for RecordingResponses {
            fn respond(
                &mut self,
                request: &str,
                _canceled: &AtomicBool,
            ) -> Result<ModelResponse, String> {
                self.requests.push(request.to_string());
                Ok(self.responses.remove(0))
            }
        }
        struct LargeTools;
        impl ToolExecutor for LargeTools {
            fn execute(
                &mut self,
                calls: &[ToolCall],
                _canceled: &AtomicBool,
            ) -> Vec<ToolObservation> {
                calls
                    .iter()
                    .map(|call| {
                        ToolObservation::result(
                            &call.tool,
                            json!({"name": call.args["name"], "source": "x".repeat(110 * 1024)}),
                        )
                    })
                    .collect()
            }
        }
        let mut responses = (0..4)
            .map(|index| ModelResponse::ToolCalls {
                working_notes: format!("Inspected large symbol {index}; preserve the conclusion."),
                summary: String::new(),
                tool_calls: vec![ToolCall {
                    tool: "read_symbol".to_string(),
                    args: json!({"name": format!("symbol-{index}")}),
                }],
            })
            .collect::<Vec<_>>();
        responses.push(ModelResponse::Done {
            working_notes: "The compacted inspection is sufficient.".to_string(),
            summary: "compacted".to_string(),
        });
        let mut provider = RecordingResponses {
            responses,
            requests: Vec::new(),
        };
        let mut compacted_events = 0;
        let result = run_agent_with_profile(
            &mut provider,
            &mut LargeTools,
            &AgentProfile {
                role: "Gauntlet builder".to_string(),
                instruction: "Inspect bounded symbols.".to_string(),
                max_turns: 8,
                model: None,
                reasoning_effort: None,
                request_timeout: None,
                compaction: Some(AgentCompactionPolicy {
                    max_request_bytes: MIN_COMPACTION_BYTES,
                    retain_recent_turns: 4,
                }),
            },
            "inspect large symbols",
            json!({}),
            live_tool_specs(),
            &AtomicBool::new(false),
            |event| {
                compacted_events +=
                    usize::from(matches!(event, AgentEvent::ContextCompacted { .. }));
            },
        )
        .expect("compacted agent");
        assert_eq!(result, "compacted");
        assert!(compacted_events > 0);
        let final_request = provider.requests.last().expect("final compact request");
        assert!(final_request.contains("\"record\":\"compacted_history\""));
        let records = final_request
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).expect("request record"))
            .collect::<Vec<_>>();
        let compacted = records
            .iter()
            .find(|record| record["record"] == "compacted_history")
            .expect("compacted history record");
        assert!(!compacted.to_string().contains(&"x".repeat(2_000)));
        assert_eq!(
            records
                .iter()
                .filter(|record| record["record"] == "turn_result")
                .count(),
            2
        );
        assert!(final_request.len() <= MIN_COMPACTION_BYTES);
    }

    #[test]
    fn completion_gate_returns_evidence_to_the_agent_before_finishing() {
        struct RecordingResponses {
            responses: Vec<ModelResponse>,
            requests: Vec<String>,
        }
        impl ModelProvider for RecordingResponses {
            fn respond(
                &mut self,
                request: &str,
                _canceled: &AtomicBool,
            ) -> Result<ModelResponse, String> {
                self.requests.push(request.to_string());
                Ok(self.responses.remove(0))
            }
        }

        #[derive(Default)]
        struct GatedTools {
            ready: bool,
        }
        impl ToolExecutor for GatedTools {
            fn execute(
                &mut self,
                calls: &[ToolCall],
                _canceled: &AtomicBool,
            ) -> Vec<ToolObservation> {
                self.ready = true;
                calls
                    .iter()
                    .map(|call| ToolObservation::result(&call.tool, json!({"ok": true})))
                    .collect()
            }

            fn validate_completion(&self) -> Result<(), String> {
                self.ready
                    .then_some(())
                    .ok_or_else(|| "successful tool execution required".to_string())
            }
        }

        let mut provider = RecordingResponses {
            responses: vec![
                ModelResponse::Done {
                    working_notes: "Intent: finish. Observed: edit. Next: none. Blocker: none."
                        .to_string(),
                    summary: "too early".to_string(),
                },
                ModelResponse::ToolCalls {
                    working_notes: "Intent: validate. Observed: gate. Next: check. Blocker: none."
                        .to_string(),
                    summary: String::new(),
                    tool_calls: vec![ToolCall {
                        tool: "run_tests".to_string(),
                        args: json!({}),
                    }],
                },
                ModelResponse::Done {
                    working_notes: "Intent: finish. Observed: success. Next: none. Blocker: none."
                        .to_string(),
                    summary: "verified".to_string(),
                },
            ],
            requests: Vec::new(),
        };
        let mut tools = GatedTools::default();
        let mut saw_gate = false;

        let result = run_agent(
            &mut provider,
            &mut tools,
            "change",
            json!({}),
            workshop_tool_specs(),
            &AtomicBool::new(false),
            |event| {
                if let AgentEvent::Observations(observations) = event {
                    saw_gate |= observations
                        .iter()
                        .any(|observation| observation.tool == "completion_gate");
                }
            },
        )
        .expect("agent");

        assert_eq!(result, "verified");
        assert!(saw_gate);
        let gate_entry = serde_json::from_str::<Value>(
            provider.requests[1]
                .lines()
                .nth(1)
                .expect("completion gate entry"),
        )
        .expect("completion gate JSON");
        assert_eq!(gate_entry["observations"][0]["tool"], "completion_gate");
        let header = serde_json::from_str::<Value>(provider.requests[1].lines().next().unwrap())
            .expect("request header");
        assert!(header.get("observations").is_none());
    }

    #[test]
    fn repeated_invalid_completions_end_the_attempt_for_escalation() {
        struct PrematureProvider {
            calls: usize,
        }
        impl ModelProvider for PrematureProvider {
            fn respond(
                &mut self,
                _request: &str,
                _canceled: &AtomicBool,
            ) -> Result<ModelResponse, String> {
                self.calls += 1;
                Ok(ModelResponse::Done {
                    working_notes:
                        "Intent: finish. Observed: no write. Next: retry. Blocker: completion gate."
                            .to_string(),
                    summary: "finished without evidence".to_string(),
                })
            }
        }

        struct NeverReady;
        impl ToolExecutor for NeverReady {
            fn execute(
                &mut self,
                _calls: &[ToolCall],
                _canceled: &AtomicBool,
            ) -> Vec<ToolObservation> {
                Vec::new()
            }

            fn validate_completion(&self) -> Result<(), String> {
                Err("successful atomic write required".to_string())
            }
        }

        let mut provider = PrematureProvider { calls: 0 };
        let error = run_agent(
            &mut provider,
            &mut NeverReady,
            "change",
            json!({}),
            workshop_tool_specs(),
            &AtomicBool::new(false),
            |_| {},
        )
        .expect_err("repeated premature completion");

        assert_eq!(provider.calls, MAX_COMPLETION_REJECTIONS);
        assert!(error.contains("repeated an invalid completion 3 times"));
        assert!(error.contains("successful atomic write required"));
    }

    #[test]
    fn terminal_tool_failure_ends_the_agent_without_another_turn() {
        struct OneResponse {
            calls: usize,
        }
        impl ModelProvider for OneResponse {
            fn respond(
                &mut self,
                _request: &str,
                _canceled: &AtomicBool,
            ) -> Result<ModelResponse, String> {
                self.calls += 1;
                Ok(ModelResponse::ToolCalls {
                    working_notes:
                        "Intent: stop. Observed: terminal blocker. Next: escalate. Blocker: environment."
                            .to_string(),
                    summary: String::new(),
                    tool_calls: vec![ToolCall {
                        tool: "report_blocked".to_string(),
                        args: json!({
                            "reason": "missing executable",
                            "evidence": "staged test gate failed",
                            "next_step": "provision the host"
                        }),
                    }],
                })
            }
        }

        #[derive(Default)]
        struct BlockedTools {
            failure: Option<String>,
        }
        impl ToolExecutor for BlockedTools {
            fn execute(
                &mut self,
                calls: &[ToolCall],
                _canceled: &AtomicBool,
            ) -> Vec<ToolObservation> {
                self.failure = Some("builder reported blocked: missing executable".to_string());
                calls
                    .iter()
                    .map(|call| ToolObservation::result(&call.tool, json!({"status":"terminated"})))
                    .collect()
            }

            fn terminal_failure(&self) -> Option<String> {
                self.failure.clone()
            }
        }

        let mut provider = OneResponse { calls: 0 };
        let mut tools = BlockedTools::default();
        let error = run_agent(
            &mut provider,
            &mut tools,
            "attempt the change",
            json!({}),
            gauntlet_tool_specs(),
            &AtomicBool::new(false),
            |_| {},
        )
        .expect_err("terminal blocker");

        assert_eq!(provider.calls, 1);
        assert_eq!(error, "builder reported blocked: missing executable");
    }

    #[test]
    fn external_provider_concrete_tool_name_cannot_execute() {
        struct ExternalResponses(Vec<ModelResponse>);
        impl ModelProvider for ExternalResponses {
            fn respond(
                &mut self,
                _request: &str,
                _canceled: &AtomicBool,
            ) -> Result<ModelResponse, String> {
                Ok(self.0.remove(0))
            }

            fn requires_action_ids(&self) -> bool {
                true
            }
        }
        let mut provider = ExternalResponses(vec![
            ModelResponse::ToolCalls {
                working_notes: "Attempt a concrete tool name.".to_string(),
                summary: String::new(),
                tool_calls: vec![ToolCall {
                    tool: "read_symbol".to_string(),
                    args: json!({"name":"tick"}),
                }],
            },
            ModelResponse::Done {
                working_notes: "The concrete name was rejected before execution.".to_string(),
                summary: "rejected".to_string(),
            },
        ]);
        let mut tools = Tools::default();
        let mut observations = Vec::new();
        run_agent(
            &mut provider,
            &mut tools,
            "inspect",
            json!({}),
            workshop_tool_specs(),
            &AtomicBool::new(false),
            |event| {
                if let AgentEvent::Observations(values) = event {
                    observations.extend(values);
                }
            },
        )
        .expect("provider receives replacement feedback");
        assert_eq!(tools.0, 0);
        assert_eq!(observations.len(), 1);
        assert_eq!(observations[0].tool, "read_symbol");
        assert!(observations[0]
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("invented AI action ID"));
    }
    #[test]
    fn rejects_only_invented_action_id_and_retains_accepted_selection() {
        struct RecordingResponses {
            responses: Vec<ModelResponse>,
            requests: Vec<String>,
        }
        impl ModelProvider for RecordingResponses {
            fn respond(
                &mut self,
                request: &str,
                _canceled: &AtomicBool,
            ) -> Result<ModelResponse, String> {
                self.requests.push(request.to_string());
                Ok(self.responses.remove(0))
            }
        }
        let read_id = workshop_tool_specs()
            .into_iter()
            .find(|spec| spec.tool == "read_symbol")
            .expect("read spec")
            .action_id;
        let rejected_id = "a_0000000000000000";
        let mut provider = RecordingResponses {
            responses: vec![
                ModelResponse::ToolCalls {
                    working_notes: "Select one valid read and one invalid action.".to_string(),
                    summary: String::new(),
                    tool_calls: vec![
                        ToolCall {
                            tool: read_id.clone(),
                            args: json!({"name":"tick"}),
                        },
                        ToolCall {
                            tool: rejected_id.to_string(),
                            args: json!({}),
                        },
                    ],
                },
                ModelResponse::Done {
                    working_notes: "Retain the valid selection and replace the rejected ID."
                        .to_string(),
                    summary: "rejected safely".to_string(),
                },
            ],
            requests: Vec::new(),
        };
        let mut tools = Tools::default();
        let mut rejected = Vec::new();
        let result = run_agent(
            &mut provider,
            &mut tools,
            "change",
            json!({}),
            workshop_tool_specs(),
            &AtomicBool::new(false),
            |event| {
                if let AgentEvent::Observations(observations) = event {
                    rejected.extend(observations);
                }
            },
        )
        .expect("agent receives localized rejection");
        assert_eq!(result, "rejected safely");
        assert_eq!(tools.0, 0, "invalid batches remain atomic");
        assert_eq!(rejected.len(), 1);
        assert_eq!(rejected[0].tool, rejected_id);
        assert!(rejected[0]
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("replace only this rejected action ID"));
        assert!(provider.requests[1].contains(&read_id));
        assert!(provider.requests[1].contains(rejected_id));
    }
    #[test]
    fn live_contract_is_compact_and_lazily_exposes_rare_tools() {
        let workshop = workshop_tool_specs();
        let live = live_tool_specs();
        assert!(!live.is_empty());
        assert!(live.len() < workshop.len());
        assert!(live
            .iter()
            .filter(|tool| tool.tool != "get_capability")
            .all(|tool| workshop.iter().any(|candidate| candidate.tool == tool.tool)));
        assert!(live.iter().any(|tool| tool.tool == "write_symbol"));
        assert!(live.iter().any(|tool| tool.tool == "find_references"));
        assert!(live.iter().any(|tool| tool.tool == "get_stdlib_api"));
        assert!(live.iter().any(|tool| tool.tool == "run_tests"));
        assert!(!live.iter().any(|tool| tool.tool == "inspect_runtime_state"));
        assert!(!live.iter().any(|tool| tool.tool == "run_frame"));
        assert!(live.iter().any(|tool| tool.tool == "get_capability"));
        assert!(!workshop
            .iter()
            .any(|tool| tool.tool == "validate_runtime_state"));
        assert!(!live.iter().any(|tool| tool.tool == "capture_screenshot"));
        let instruction = AgentProfile::default().instruction;
        assert!(instruction.contains("successful receipt proves completion"));
        for syntax in [
            "import",
            "struct",
            "global",
            "function",
            "test `name`(): bool",
        ] {
            assert!(instruction.contains(syntax));
        }
        assert!(instruction.contains("function damage(self: Enemy, amount: i32): void"));
        assert!(instruction.contains("enemy.damage(5)"));
        assert!(instruction.contains("runtime/assets capability only when necessary"));
        assert!(instruction.len() <= 1_000);
    }

    #[test]
    fn project_ai_discovers_assets_while_gauntlet_gets_them_eagerly() {
        let live = live_tool_specs();
        let project = project_ai_tool_specs();
        assert!(project.iter().any(|tool| tool.tool == "get_capability"));
        for hidden in [
            "request_imagegen_asset",
            "write_svg_asset",
            "write_png_asset",
            "import_png_asset",
            "delete_asset",
            "write_data_asset",
            "write_procedural_wav",
        ] {
            assert!(!project.iter().any(|tool| tool.tool == hidden));
        }
        assert!(!project.iter().any(|tool| tool.tool == "record_decision"));
        assert!(!project.iter().any(|tool| tool.tool == "report_blocked"));
        assert_eq!(project, live);
        let assets = asset_tool_specs();
        for shared_policy in [
            "including early versions",
            "preserves submission order",
            "contiguous asset-tool",
            "manifest directly",
            "visibly drawn",
        ] {
            assert!(assets
                .iter()
                .all(|tool| !tool.purpose.contains(shared_policy)));
        }
        let png = assets
            .iter()
            .find(|tool| tool.tool == "write_png_asset")
            .expect("PNG tool");
        for signature in [
            "rect: {kind,color,x,y,width,height}",
            "circle: {kind,color,x,y,radius}",
            "line: {kind,color,x1,y1,x2,y2,thickness}",
            "fill, stroke, stroke_width, cx/cy, and line width are not supported",
        ] {
            assert!(png.purpose.contains(signature));
        }
        assert!(gauntlet_tool_specs()
            .iter()
            .any(|tool| tool.tool == "record_decision"));
        assert!(gauntlet_tool_specs()
            .iter()
            .any(|tool| tool.tool == "report_blocked"));
        for asset in &assets {
            assert!(gauntlet_tool_specs()
                .iter()
                .any(|tool| tool.tool == asset.tool));
        }
        for runtime in runtime_tool_specs() {
            assert!(gauntlet_tool_specs()
                .iter()
                .any(|tool| tool.tool == runtime.tool));
        }
        assert!(!gauntlet_tool_specs()
            .iter()
            .any(|tool| tool.tool == "get_capability"));

        let live_bytes = serde_json::to_vec(&live).expect("live specs JSON").len();
        let project_bytes = serde_json::to_vec(&project)
            .expect("project specs JSON")
            .len();
        eprintln!(
            "serialized context contracts: instruction={} bytes, live_tools={live_bytes} bytes, project_tools={project_bytes} bytes",
            AgentProfile::default().instruction.len()
        );
        assert!(
            live_bytes <= 2_100,
            "live tool specs grew to {live_bytes} bytes"
        );
        assert!(
            project_bytes <= 2_500,
            "project tool specs grew to {project_bytes} bytes"
        );
        let write = project
            .iter()
            .find(|spec| spec.tool == "write_symbol")
            .expect("write spec");
        let encoded = serde_json::to_value(write).expect("serialized tool spec");
        assert!(encoded.get("use").is_some());
        assert!(encoded.get("required").is_some());
        assert!(encoded.get("optional").is_some());
        assert!(encoded.get("purpose").is_none());
        assert!(encoded.get("required_args").is_none());
        let decoded: ToolSpec = serde_json::from_value(encoded).expect("compact tool spec");
        assert_eq!(decoded, *write);
        let capability = serde_json::to_value(
            project
                .iter()
                .find(|spec| spec.tool == "get_capability")
                .expect("capability spec"),
        )
        .expect("serialized capability");
        assert!(capability.get("required").is_some());
        assert!(capability.get("optional").is_none());
    }

    #[test]
    fn rare_tools_activate_only_after_their_capability_discovery() {
        let mut provider = Responses(vec![
            ModelResponse::ToolCalls {
                working_notes: "Discover the authored-presentation capability.".to_string(),
                summary: String::new(),
                tool_calls: vec![ToolCall {
                    tool: "get_capability".to_string(),
                    args: json!({"name":"assets"}),
                }],
            },
            ModelResponse::ToolCalls {
                working_notes: "Use the discovered bounded SVG capability.".to_string(),
                summary: String::new(),
                tool_calls: vec![ToolCall {
                    tool: "write_svg_asset".to_string(),
                    args: json!({
                        "id":"marker", "path":"assets/generated/marker.svg",
                        "source":"<svg/>", "width":16, "height":16
                    }),
                }],
            },
            ModelResponse::ToolCalls {
                working_notes: "Load runtime observation only after asset work.".to_string(),
                summary: String::new(),
                tool_calls: vec![ToolCall {
                    tool: "get_capability".to_string(),
                    args: json!({"name":"runtime"}),
                }],
            },
            ModelResponse::ToolCalls {
                working_notes: "Use the discovered bounded runtime capability.".to_string(),
                summary: String::new(),
                tool_calls: vec![ToolCall {
                    tool: "run_frame".to_string(),
                    args: json!({}),
                }],
            },
            ModelResponse::Done {
                working_notes: "The discovered capability was accepted.".to_string(),
                summary: "done".to_string(),
            },
        ]);
        let mut tools = Tools::default();
        run_agent(
            &mut provider,
            &mut tools,
            "add authored marker art",
            json!({}),
            project_ai_tool_specs(),
            &AtomicBool::new(false),
            |_| {},
        )
        .expect("discovered asset tool");
        assert_eq!(tools.0, 4);
    }

    #[test]
    fn external_provider_schema_tracks_capability_records() {
        struct RecordingProvider {
            responses: Vec<ModelResponse>,
            requests: Vec<String>,
            schemas: Vec<Value>,
        }

        impl ModelProvider for RecordingProvider {
            fn respond(
                &mut self,
                request: &str,
                _canceled: &AtomicBool,
            ) -> Result<ModelResponse, String> {
                self.requests.push(request.to_string());
                self.schemas
                    .push(model_response_schema_for_request(request)?);
                Ok(self.responses.remove(0))
            }

            fn requires_action_ids(&self) -> bool {
                true
            }
        }

        struct CapabilityExecutor {
            calls: Vec<String>,
        }

        impl ToolExecutor for CapabilityExecutor {
            fn execute(
                &mut self,
                calls: &[ToolCall],
                _canceled: &AtomicBool,
            ) -> Vec<ToolObservation> {
                self.calls
                    .extend(calls.iter().map(|call| call.tool.clone()));
                calls
                    .iter()
                    .map(|call| {
                        if call.tool == "get_capability"
                            && call.args.get("name").and_then(Value::as_str) == Some("assets")
                        {
                            ToolObservation::result(
                                &call.tool,
                                json!({"name":"assets", "tool_specs": asset_tool_specs()}),
                            )
                        } else {
                            ToolObservation::result(&call.tool, json!({"ok": true}))
                        }
                    })
                    .collect()
            }
        }

        let capability_id = project_ai_tool_specs()
            .into_iter()
            .find(|spec| spec.tool == "get_capability")
            .expect("capability spec")
            .action_id;
        let svg_id = asset_tool_specs()
            .into_iter()
            .find(|spec| spec.tool == "write_svg_asset")
            .expect("SVG spec")
            .action_id;
        let mut provider = RecordingProvider {
            responses: vec![
                ModelResponse::ToolCalls {
                    working_notes: "Discover the asset capability before using it.".to_string(),
                    summary: String::new(),
                    tool_calls: vec![ToolCall {
                        tool: capability_id,
                        args: json!({"name":"assets"}),
                    }],
                },
                ModelResponse::ToolCalls {
                    working_notes: "Use the newly advertised SVG action.".to_string(),
                    summary: String::new(),
                    tool_calls: vec![ToolCall {
                        tool: svg_id.clone(),
                        args: json!({
                            "id":"marker", "path":"assets/generated/marker.svg",
                            "source":"<svg/>", "width":16, "height":16
                        }),
                    }],
                },
                ModelResponse::Done {
                    working_notes: "The capability action completed.".to_string(),
                    summary: "done".to_string(),
                },
            ],
            requests: Vec::new(),
            schemas: Vec::new(),
        };
        let mut executor = CapabilityExecutor { calls: Vec::new() };

        run_agent(
            &mut provider,
            &mut executor,
            "discover and use assets",
            json!({}),
            project_ai_tool_specs(),
            &AtomicBool::new(false),
            |_| {},
        )
        .expect("external provider capability flow");

        assert_eq!(executor.calls, ["get_capability", "write_svg_asset"]);
        assert_eq!(provider.schemas.len(), 3);
        let initial_variants = provider.schemas[0]
            .pointer("/properties/tool_calls/items/anyOf")
            .and_then(Value::as_array)
            .expect("initial variants");
        assert!(!initial_variants.iter().any(|variant| {
            variant.pointer("/properties/action_id/enum/0") == Some(&json!(svg_id))
        }));
        let discovered_variants = provider.schemas[1]
            .pointer("/properties/tool_calls/items/anyOf")
            .and_then(Value::as_array)
            .expect("discovered variants");
        assert!(discovered_variants.iter().any(|variant| {
            variant.pointer("/properties/action_id/enum/0") == Some(&json!(svg_id))
        }));
        let active_record = provider.requests[1]
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).expect("transcript record"))
            .find(|record| record["record"] == "active_capabilities")
            .expect("active capability record");
        assert!(active_record["tool_specs"]
            .as_array()
            .expect("active tool specs")
            .iter()
            .any(|spec| spec["tool"] == "write_svg_asset"));
        assert!(provider.requests[1].starts_with(&format!("{}\n", provider.requests[0])));
    }

    #[test]
    fn action_schemas_close_args_and_nested_png_shapes() {
        let specs = [
            workshop_tool_specs()
                .into_iter()
                .find(|spec| spec.tool == "read_symbol")
                .expect("read spec"),
            workshop_tool_specs()
                .into_iter()
                .find(|spec| spec.tool == "write_symbol")
                .expect("write spec"),
            live_tool_specs()
                .into_iter()
                .find(|spec| spec.tool == "get_capability")
                .expect("capability spec"),
            asset_tool_specs()
                .into_iter()
                .find(|spec| spec.tool == "write_png_asset")
                .expect("PNG spec"),
        ];
        let schema = model_response_schema_for(&specs);
        let variants = schema["properties"]["tool_calls"]["items"]["anyOf"]
            .as_array()
            .expect("action variants");
        let args = |tool: &str| {
            variants
                .iter()
                .find(|variant| {
                    variant["properties"]["action_id"]["enum"][0] == json!(action_id_for_tool(tool))
                })
                .expect("tool variant")["properties"]["args"]
                .clone()
        };
        for tool in [
            "read_symbol",
            "write_symbol",
            "get_capability",
            "write_png_asset",
        ] {
            assert_eq!(args(tool)["additionalProperties"], json!(false));
            assert!(args(tool)["properties"].is_object());
            assert_eq!(
                variants
                    .iter()
                    .find(|variant| {
                        variant["properties"]["action_id"]["enum"][0]
                            == json!(action_id_for_tool(tool))
                    })
                    .expect("tool variant")["additionalProperties"],
                json!(false)
            );
        }
        for tool in [
            "read_symbol",
            "write_symbol",
            "get_capability",
            "write_png_asset",
        ] {
            let args = args(tool);
            let properties = args["properties"].as_object().expect("arg properties");
            let required = args["required"].as_array().expect("required args");
            assert_eq!(required.len(), properties.len());
            assert!(properties
                .keys()
                .all(|name| required.contains(&json!(name))));
        }
        assert_eq!(
            args("read_symbol")["properties"]["file"],
            json!({"anyOf": [{"type": "string"}, {"type": "null"}]})
        );
        assert_eq!(
            args("write_png_asset")["properties"]["shapes"]["items"]["anyOf"][0]
                ["additionalProperties"],
            json!(false)
        );
    }

    #[test]
    fn codex_transport_rejects_json_encoded_tool_args() {
        let error = decode_codex_response(
            r#"{"mode":"tool_calls","working_notes":"Inspect next.","summary":"","tool_calls":[{"action_id":"a_0000000000000000","args":"{\"name\":\"tick\"}"}]}"#,
        )
        .expect_err("string args must be rejected");
        assert!(error.contains("native JSON objects"));
    }

    #[test]
    fn host_rejects_unknown_tool_arguments() {
        let specs = workshop_tool_specs();
        let known = specs
            .iter()
            .map(|spec| spec.tool.clone())
            .collect::<BTreeSet<_>>();
        let error = validate_tool_call(
            &ToolCall {
                tool: "read_symbol".into(),
                args: json!({"name":"tick", "invented":true}),
            },
            &specs,
            &known,
            false,
        )
        .expect_err("unknown argument");
        assert!(error.contains("does not accept arg: invented"));
    }

    #[test]
    fn strict_nullable_optional_args_execute_as_omitted() {
        let specs = workshop_tool_specs();
        let read = specs
            .iter()
            .find(|spec| spec.tool == "read_symbol")
            .expect("read spec");
        let mut call = ToolCall {
            tool: read.action_id.clone(),
            args: json!({"name": "tick", "file": null}),
        };
        normalize_optional_nulls(&mut call, &specs, true);
        assert_eq!(call.args, json!({"name": "tick"}));
        validate_tool_call(&call, &specs, &BTreeSet::from([read.tool.clone()]), true)
            .expect("normalized strict action");
    }
    #[test]
    fn mismatched_action_id_cannot_select_another_tool() {
        let specs = workshop_tool_specs();
        let known = specs
            .iter()
            .map(|spec| spec.tool.clone())
            .collect::<BTreeSet<_>>();
        let write_id = specs
            .iter()
            .find(|spec| spec.tool == "write_symbol")
            .expect("write spec")
            .action_id
            .clone();
        let error = validate_tool_call(
            &ToolCall {
                tool: write_id,
                args: json!({"name":"tick"}),
            },
            &specs,
            &known,
            true,
        )
        .expect_err("write ID cannot invoke read arguments");
        assert!(error.contains("requires arg: file"));
    }

    #[test]
    fn provider_decoder_rejects_unknown_response_fields() {
        let error = decode_codex_response(
            r#"{"mode":"done","working_notes":"Complete.","summary":"done","invented":true}"#,
        )
        .expect_err("unknown response field");
        assert!(error.contains("unknown response field: invented"));
        let error = decode_codex_response(
            r#"{"mode":"tool_calls","working_notes":"Read.","summary":"","tool_calls":[{"action_id":"a_0000000000000000","args":{},"invented":true}]}"#,
        )
        .expect_err("unknown call field");
        assert!(error.contains("unknown field"));
    }
    #[test]
    fn response_schema_requires_native_object_args() {
        assert_eq!(
            model_response_schema().pointer("/properties/tool_calls/items/properties/args/type"),
            Some(&json!("object"))
        );
    }
    #[test]
    fn codex_json_stream_keeps_only_the_exact_reported_usage() {
        let stream = concat!(
            "{\"type\":\"thread.started\",\"thread_id\":\"hidden\"}\n",
            "{\"type\":\"item.completed\",\"item\":{\"text\":\"not retained\"}}\n",
            "{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":24763,\"cached_input_tokens\":24448,\"output_tokens\":122,\"reasoning_output_tokens\":7}}\n"
        );

        let usage = read_codex_usage(std::io::Cursor::new(stream))
            .expect("stream")
            .expect("usage");

        assert_eq!(
            usage,
            json!({
                "input_tokens": 24763,
                "cached_input_tokens": 24448,
                "output_tokens": 122,
                "reasoning_output_tokens": 7,
            })
        );
        assert!(!usage.to_string().contains("not retained"));
        assert!(!usage.to_string().contains("hidden"));
    }
}
