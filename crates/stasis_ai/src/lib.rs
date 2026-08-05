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

pub const DEFAULT_AGENT_TURNS: usize = 15;
pub const MAX_AGENT_TURNS: usize = 48;
pub const MAX_TOOL_CALLS_PER_TURN: usize = 50;
pub const MAX_WORKING_NOTES_CHARS: usize = 2_000;
pub const DEFAULT_CODEX_MODEL: &str = "gpt-5.6-sol";
pub const DEFAULT_REASONING_EFFORT: &str = "medium";
pub const MAX_OBSERVATION_BYTES: usize = 1024 * 1024;
pub const MIN_COMPACTION_BYTES: usize = 256 * 1024;
pub const MAX_COMPACTION_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_COMPACTION_RETAINED_TURNS: usize = 16;
const MAX_COMPLETION_REJECTIONS: usize = 3;
const AGENT_INSTRUCTION: &str = "Use only the supplied Stasis tools. These are host-mediated virtual tools described by tool_specs in the immutable request header, not native Codex registry tools. Invoke them by returning mode=tool_calls with the requested calls in the structured response contract; never search for them in or reject them because of the native callable-tool registry. The first JSONL record is the immutable request header; every following record is the authoritative append-only transcript of an earlier model response and its tool observations. Do not repeat completed inspection. Start with initial_context.initial_symbols, which is the completed compact default list_symbols result for the entry file and its direct imports. Also use initial_context.stdlib_api as the completed catalog of public standard-library signatures; add the listed canonical_import when a needed module is not already imported, and do not spend turns rediscovering stdlib implementation files unless the catalog is ambiguous. Treat every listed project function whose name directly contains the requested behavior noun as a candidate: batch read_symbol and find_references for all of them before editing. Do not skip update, movement, collision, or render candidates merely because one function exposes the visible value. If relevant project symbols are missing, batch multiple narrow list_symbols searches directly suggested by the request, such as the behavior noun plus render or update terms; never enumerate the whole project. A reference lookup does not require a prior source read. Call find_references for behavior-bearing project symbols before writing. For collision or geometry changes, use rendered rectangle bounds as the coordinate source of truth and derive contact test inputs after the update function's movement order instead of copying old collision constants. Put all related source and requested durable-test changes in one contiguous atomic write batch. The write compiles the batch and runs project tests; if it succeeds, return done immediately without a separate test call. If it fails, correct only the reported defect and retry atomically. Return exactly one JSON object matching the response contract.";

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
pub struct ToolCall {
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
    pub purpose: String,
    #[serde(default)]
    pub required_args: Vec<String>,
    #[serde(default)]
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
    user_prompt: &'a str,
    initial_context: &'a Value,
    tool_specs: &'a [ToolSpec],
    response_contract: &'a Value,
}

pub trait ModelProvider {
    fn respond(&mut self, request: &str, canceled: &AtomicBool) -> Result<ModelResponse, String>;

    fn take_usage(&mut self) -> Option<Value> {
        None
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
    let known_tools = tool_specs
        .iter()
        .map(|spec| spec.tool.as_str())
        .collect::<BTreeSet<_>>();
    let response_contract = response_contract();
    let header = serde_json::to_string(&ModelRequestHeader {
        record: "request",
        schema_version: 1,
        role: &profile.role,
        instruction: &profile.instruction,
        user_prompt,
        initial_context: &initial_context,
        tool_specs: &tool_specs,
        response_contract: &response_contract,
    })
    .map_err(|error| format!("failed encoding append-only AI request header: {error}"))?;
    let mut transcript = AgentTranscript::new(header);
    let mut completion_rejections = 0_usize;
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
            ModelResponse::ToolCalls { tool_calls, .. } => {
                if tool_calls.is_empty() {
                    return Err("model returned an empty tool-call batch".to_string());
                }
                if tool_calls.len() > MAX_TOOL_CALLS_PER_TURN {
                    return Err(format!(
                        "model returned {} tool calls; limit is {MAX_TOOL_CALLS_PER_TURN}",
                        tool_calls.len()
                    ));
                }
                for call in &tool_calls {
                    if !known_tools.contains(call.tool.as_str()) {
                        return Err(format!("unsupported AI tool: {}", call.tool));
                    }
                }
                let validation_errors = tool_calls
                    .iter()
                    .filter_map(|call| validate_tool_call(call, &tool_specs, &known_tools).err())
                    .collect::<Vec<_>>();
                if !validation_errors.is_empty() {
                    let detail = validation_errors.join("; ");
                    let observations = tool_calls
                        .iter()
                        .map(|call| {
                            ToolObservation::error(
                                &call.tool,
                                format!(
                                    "tool-call batch rejected before execution: {detail}; correct the arguments and retry"
                                ),
                            )
                        })
                        .collect::<Vec<_>>();
                    emit(AgentEvent::Observations(observations.clone()));
                    transcript.append(&response_record, &observations)?;
                    compact_transcript(&mut transcript, profile.compaction.as_ref(), &mut emit)?;
                    continue;
                }
                emit(AgentEvent::ToolBatch(tool_calls.clone()));
                let observations = bound_observations(executor.execute(&tool_calls, canceled));
                emit(AgentEvent::Observations(observations.clone()));
                if let Some(error) = executor.terminal_failure() {
                    return Err(error);
                }
                transcript.append(&response_record, &observations)?;
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
        while self.entries.len() > policy.retain_recent_turns
            && self.render()?.len() > policy.max_request_bytes
        {
            let entry = self.entries.remove(0);
            self.compacted.push(entry.compact);
            turns_compacted = turns_compacted.saturating_add(1);
        }
        // The retained-turn count is a target, not permission to exceed the hard byte ceiling.
        while !self.entries.is_empty() && self.render()?.len() > policy.max_request_bytes {
            let entry = self.entries.remove(0);
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
                        "tool": call.get("tool").cloned().unwrap_or(Value::Null),
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

fn validate_tool_call(
    call: &ToolCall,
    specs: &[ToolSpec],
    known_tools: &BTreeSet<&str>,
) -> Result<(), String> {
    if !known_tools.contains(call.tool.as_str()) {
        return Err(format!("unsupported AI tool: {}", call.tool));
    }
    let args = call
        .args
        .as_object()
        .ok_or_else(|| format!("AI tool {} requires an object args value", call.tool))?;
    let spec = specs
        .iter()
        .find(|spec| spec.tool == call.tool)
        .expect("known tool has spec");
    for required in &spec.required_args {
        if !args.contains_key(required) {
            return Err(format!("AI tool {} requires arg: {required}", call.tool));
        }
    }
    Ok(())
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
        "tool_calls": {"maximum": MAX_TOOL_CALLS_PER_TURN, "shape": {"tool": "name", "args": "JSON object encoded as a string"}},
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
                    "required": ["tool", "args"],
                    "properties": {"tool": {"type": "string"}, "args": {"type": "string"}},
                    "additionalProperties": false
                }
            }
        },
        "additionalProperties": false
    })
}

pub fn workshop_tool_specs() -> Vec<ToolSpec> {
    vec![
        spec("list_symbols", "Search compact editable Stasis symbols within explicit starting files. Without files, the project entry file and its direct imports are searched. The response maps every searched file to its direct imports. Pass files as an array of up to 16 project-relative paths to choose a different scope. Symbol items exclude imports and empty global groups, default to 32 items, and never include source or source hashes.", &[], &["files", "query", "kind", "owner", "page", "limit"]),
        spec("find_references", "Find compact compiler-owned definitions, reads, writes, and calls for a function, global, or dot-qualified field.", &["symbol"], &["limit"]),
        spec("list_owner_symbols", "List compact symbols owned by one type or group.", &["owner"], &[]),
        spec("read_symbol", "Read one Stasis symbol. Up to 50 deliberate symbol reads may be batched as separate tool calls in one turn.", &["name"], &["kind", "file", "owner", "signature"]),
        spec("write_symbol", "Atomically add or replace a symbol; set operation=add for a new symbol. A write batch compiles and tests together.", &["file", "name", "new_source"], &["operation", "kind", "owner", "signature", "expected_source_hash"]),
        spec("delete_symbol", "Atomically delete a symbol.", &["name"], &["file", "kind", "owner", "signature", "expected_source_hash"]),
        spec("read_imports", "Read one source file's imports.", &["file"], &[]),
        spec("write_imports", "Atomically replace one source file's imports.", &["file", "imports"], &[]),
        spec("get_diagnostics", "Read the latest compiler diagnostics.", &[], &[]),
        spec("set_input_state", "Set simulated input state.", &[], &["x", "y", "active", "screen_w", "screen_h"]),
        spec("inspect_runtime_state", "Read bounded live scalar state.", &[], &[]),
        spec("run_frame", "Advance the live runtime by one deterministic tick.", &[], &[]),
        spec("take_screenshot", "Capture a logical render snapshot and runtime state.", &[], &[]),
        spec("list_tests", "List Stasis test files.", &[], &[]),
        spec("read_test_file", "Read one Stasis test file.", &["file"], &[]),
        spec("write_test_file", "Create or replace one Stasis test file.", &["file", "source"], &[]),
        spec("delete_test_file", "Delete one Stasis test file.", &["file"], &[]),
        spec("run_tests", "Confirm the latest atomic write batch compiled and passed tests.", &[], &[]),
    ]
}

pub fn live_tool_specs() -> Vec<ToolSpec> {
    const LIVE_TOOLS: &[&str] = &[
        "list_symbols",
        "find_references",
        "read_symbol",
        "write_symbol",
        "delete_symbol",
        "inspect_runtime_state",
        "run_frame",
    ];
    workshop_tool_specs()
        .into_iter()
        .filter(|spec| LIVE_TOOLS.contains(&spec.tool.as_str()))
        .collect()
}

fn spec(tool: &str, purpose: &str, required: &[&str], optional: &[&str]) -> ToolSpec {
    ToolSpec {
        tool: tool.to_string(),
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

pub fn project_ai_tool_specs() -> Vec<ToolSpec> {
    let mut tools = live_tool_specs();
    tools.extend([
        spec("request_imagegen_asset", "Request one host-generated PNG containing one isolated asset or subject, not an atlas. The master defaults to 1024x1024; request up to 2048x2048 only when extra detail or crop latitude is needed. Keep it in the controlled ImageGen inbox and derive the game copy later with import_png_asset crop/background-removal options. The host persists the prompt and this call waits for the PNG, then returns its source_path. Use only when generated bitmap art materially improves the task; request before the atomic asset/source write batch.", &["filename", "prompt", "purpose"], &["width", "height"]),
        spec("write_svg_asset", "Stage one bounded SVG under assets/generated and derive its v2 manifest entry. Prefer this for basic UI, simple icons, markers, and overlays; do not use primitive vector construction for characters or units by default. It must be in the same tool batch immediately before source writes that load or use it.", &["id", "path", "source", "width", "height"], &[]),
        spec("write_png_asset", "Generate and stage one deterministic PNG from bounded rect/circle/line shapes, then derive its v2 manifest entry. Prefer it for basic UI and deterministic overlays or a capability fallback; do not use primitive-shape characters or units by default. It must be in the same tool batch immediately before source writes that load or use it.", &["id", "path", "width", "height", "background", "shapes"], &[]),
        spec("import_png_asset", "Copy a host-generated PNG from build/ai-assets/imagegen or build/gauntlet/imagegen into assets/generated, optionally crop it and remove a flat background color, validate the result, and derive its v2 manifest entry. Supply all four crop fields together. transparent_color is #RRGGBB; transparent_tolerance defaults to 12. It must be in the same tool batch immediately before source writes that load or use it.", &["id", "path", "source_path"], &["crop_x", "crop_y", "crop_width", "crop_height", "transparent_color", "transparent_tolerance"]),
        spec("write_data_asset", "Stage bounded JSON or CSV data under assets/generated. It must be in the same tool batch immediately before related source writes.", &["path", "source"], &[]),
        spec("write_procedural_wav", "Stage deterministic mono PCM audio under assets/generated and derive its v2 manifest entry. It must be in the same tool batch immediately before related source writes.", &["id", "path", "frequency_hz", "duration_ms"], &[]),
    ]);
    tools
}

pub fn gauntlet_tool_specs() -> Vec<ToolSpec> {
    let mut tools = project_ai_tool_specs();
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
        let mut child = command.spawn().map_err(|error| {
            format!(
                "failed starting Codex; install/sign in to Codex or set STASIS_CODEX_EXE: {error}"
            )
        })?;
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
        let source = self.run_codex(request, &model_response_schema(), canceled)?;
        decode_codex_response(&source)
    }

    fn take_usage(&mut self) -> Option<Value> {
        self.last_usage.take()
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

fn decode_codex_response(source: &str) -> Result<ModelResponse, String> {
    let mut value: Value = serde_json::from_str(source)
        .map_err(|error| format!("Codex returned invalid agent JSON: {error}"))?;
    if let Some(calls) = value.get_mut("tool_calls").and_then(Value::as_array_mut) {
        for call in calls {
            let Some(args) = call.get_mut("args") else {
                continue;
            };
            if let Some(encoded) = args.as_str() {
                if let Ok(decoded) = serde_json::from_str(encoded) {
                    *args = decoded;
                }
            }
        }
    }
    serde_json::from_value(value)
        .map_err(|error| format!("Codex returned invalid agent response: {error}"))
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

        assert_eq!(DEFAULT_AGENT_TURNS, 15);
        assert_eq!(MAX_AGENT_TURNS, 48);
        assert_eq!(tools.0, 50);
        assert_eq!(contract_json()["limits"]["default_agent_turns"], 15);
        assert_eq!(contract_json()["limits"]["maximum_profile_turns"], 48);
        assert_eq!(contract_json()["limits"]["tool_calls_per_turn"], 50);
    }

    #[test]
    fn explicit_profiles_can_exceed_the_live_ai_default() {
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
                max_turns: 20,
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
    fn rejects_unknown_tools_before_execution() {
        let mut provider = Responses(vec![ModelResponse::ToolCalls {
            working_notes: "Intent: act. Observed: none. Next: shell. Blocker: none.".to_string(),
            summary: String::new(),
            tool_calls: vec![ToolCall {
                tool: "shell".to_string(),
                args: json!({}),
            }],
        }]);
        let error = run_agent(
            &mut provider,
            &mut Tools::default(),
            "change",
            json!({}),
            workshop_tool_specs(),
            &AtomicBool::new(false),
            |_| {},
        )
        .expect_err("unknown tool");
        assert_eq!(error, "unsupported AI tool: shell");
    }

    #[test]
    fn live_contract_is_a_strict_subset_of_the_workshop_contract() {
        let workshop = workshop_tool_specs();
        let live = live_tool_specs();
        assert!(!live.is_empty());
        assert!(live.len() < workshop.len());
        assert!(live
            .iter()
            .all(|tool| workshop.iter().any(|candidate| candidate.tool == tool.tool)));
        assert!(live.iter().any(|tool| tool.tool == "write_symbol"));
        assert!(live.iter().any(|tool| tool.tool == "find_references"));
        assert!(!live.iter().any(|tool| tool.tool == "run_tests"));
        assert!(!workshop
            .iter()
            .any(|tool| tool.tool == "validate_runtime_state"));
        assert!(!live.iter().any(|tool| tool.tool == "capture_screenshot"));
    }

    #[test]
    fn project_ai_gets_assets_without_gauntlet_decision_memory() {
        let project = project_ai_tool_specs();
        for expected in [
            "request_imagegen_asset",
            "write_svg_asset",
            "write_png_asset",
            "import_png_asset",
            "write_data_asset",
            "write_procedural_wav",
        ] {
            assert!(project.iter().any(|tool| tool.tool == expected));
        }
        assert!(!project.iter().any(|tool| tool.tool == "record_decision"));
        assert!(!project.iter().any(|tool| tool.tool == "report_blocked"));
        assert!(gauntlet_tool_specs()
            .iter()
            .any(|tool| tool.tool == "record_decision"));
        assert!(gauntlet_tool_specs()
            .iter()
            .any(|tool| tool.tool == "report_blocked"));
    }

    #[test]
    fn codex_transport_decodes_json_encoded_tool_args() {
        let response = decode_codex_response(
            r#"{"mode":"tool_calls","working_notes":"Inspect next.","summary":"","tool_calls":[{"tool":"read_symbol","args":"{\"name\":\"tick\"}"}]}"#,
        )
        .expect("response");
        let ModelResponse::ToolCalls { tool_calls, .. } = response else {
            panic!("tool calls");
        };
        assert_eq!(tool_calls[0].args, json!({"name": "tick"}));
    }

    #[test]
    fn malformed_json_encoded_tool_args_are_returned_for_correction() {
        let malformed = decode_codex_response(
            r#"{"mode":"tool_calls","working_notes":"Correct the malformed call next.","summary":"","tool_calls":[{"tool":"read_symbol","args":"{\"name\":\"tick\"} {\"extra\":true}"}]}"#,
        )
        .expect("transport preserves malformed args for the agent loop");
        let mut provider = Responses(vec![
            malformed,
            ModelResponse::Done {
                working_notes: "The rejected batch was corrected without executing it.".to_string(),
                summary: "corrected".to_string(),
            },
        ]);
        let mut tools = Tools::default();
        let mut errors = Vec::new();

        let result = run_agent(
            &mut provider,
            &mut tools,
            "correct malformed tool arguments",
            json!({}),
            workshop_tool_specs(),
            &AtomicBool::new(false),
            |event| {
                if let AgentEvent::Observations(observations) = event {
                    errors.extend(
                        observations
                            .into_iter()
                            .filter_map(|observation| observation.error),
                    );
                }
            },
        )
        .expect("agent retries after rejected transport args");

        assert_eq!(result, "corrected");
        assert_eq!(tools.0, 0);
        assert!(errors
            .iter()
            .any(|error| error.contains("batch rejected before execution")));
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
