use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const MAX_AGENT_TURNS: usize = 15;
pub const MAX_TOOL_CALLS_PER_TURN: usize = 50;
pub const MAX_WORKING_NOTES_CHARS: usize = 2_000;
pub const DEFAULT_CODEX_MODEL: &str = "gpt-5.6-sol";
pub const DEFAULT_REASONING_EFFORT: &str = "medium";
pub const MAX_OBSERVATION_BYTES: usize = 1024 * 1024;
const AGENT_INSTRUCTION: &str = "Use only the supplied Stasis tools. The first JSONL record is the immutable request header; every following record is the authoritative append-only transcript of an earlier model response and its tool observations. Do not repeat completed inspection or validation. Start with initial_context.initial_symbols, which is the completed compact default list_symbols result for the entry file and its direct imports. When initial_context.targeted_symbols is present, use those prompt-matched priority candidates without repeating their discovery queries. Treat every listed function whose name directly contains the requested behavior noun as a candidate: batch read_symbol and find_references for all of them before editing. Do not skip update, movement, collision, or render candidates merely because one function exposes the visible value. If relevant symbols are missing, batch multiple narrow list_symbols searches directly suggested by the request, such as the behavior noun plus render or update terms; never enumerate the whole project. A reference lookup does not require a prior source read. Call find_references for behavior-bearing symbols before writing. For geometry, treat rendered rectangles as the observable contract and keep movement, collision, wall, scoring, and reset extents consistent with them. Durable behavior tests must drive the public update or tick path at the exact boundary and adjacent values; direct helper tests are supplementary and never establish integration behavior by themselves. For an observable requested behavior, call validate_runtime_state with the target requirements and expected_outcome=fail before writing. Never guess setup, tick, or render names: omit optional entrypoints unless a tool observation established the exact function. When source edits are ready, place red validation immediately before one contiguous atomic write batch in the same turn; writes run only when the red contract is accepted. The runtime automatically applies the identical green validation after a successful write, so do not request it again. Use baseline=fresh for startup/reset or integration-style behavior and baseline=live when the current running state matters. If the before check already passes, report that the request is already satisfied without rewriting it. Do not claim an affected behavior is consistent unless you inspected its source or validated it. Return done after the edit reports compilation/tests passed and automatic green validation passes. Return exactly one JSON object matching the response contract.";

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
    Turn { current: usize, maximum: usize },
    ProviderUsage(Value),
    WorkingNotes(String),
    ToolBatch(Vec<ToolCall>),
    Observations(Vec<ToolObservation>),
    Completed(String),
}

#[derive(Serialize)]
struct ModelRequestHeader<'a> {
    record: &'static str,
    schema_version: u32,
    role: &'static str,
    instruction: &'static str,
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
}

pub fn run_agent<P, T, E>(
    provider: &mut P,
    executor: &mut T,
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
    let known_tools = tool_specs
        .iter()
        .map(|spec| spec.tool.as_str())
        .collect::<BTreeSet<_>>();
    let response_contract = response_contract();
    let mut request = serde_json::to_string(&ModelRequestHeader {
        record: "request",
        schema_version: 1,
        role: "Stasis live-workspace coding agent",
        instruction: AGENT_INSTRUCTION,
        user_prompt,
        initial_context: &initial_context,
        tool_specs: &tool_specs,
        response_contract: &response_contract,
    })
    .map_err(|error| format!("failed encoding append-only AI request header: {error}"))?;
    for turn in 1..=MAX_AGENT_TURNS {
        if canceled.load(Ordering::Acquire) {
            return Err("AI request canceled".to_string());
        }
        emit(AgentEvent::Turn {
            current: turn,
            maximum: MAX_AGENT_TURNS,
        });
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
                    let observations = vec![ToolObservation::error("completion_gate", error)];
                    emit(AgentEvent::Observations(observations.clone()));
                    append_transcript_entry(&mut request, &response_record, &observations)?;
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
                    validate_tool_call(call, &tool_specs, &known_tools)?;
                }
                emit(AgentEvent::ToolBatch(tool_calls.clone()));
                let observations = bound_observations(executor.execute(&tool_calls, canceled));
                emit(AgentEvent::Observations(observations.clone()));
                append_transcript_entry(&mut request, &response_record, &observations)?;
            }
        }
    }
    Err(format!("AI agent reached the {MAX_AGENT_TURNS}-turn limit"))
}

fn append_transcript_entry(
    request: &mut String,
    response: &Value,
    observations: &[ToolObservation],
) -> Result<(), String> {
    let entry = serde_json::to_string(&json!({
        "record": "turn_result",
        "response": response,
        "observations": observations,
    }))
    .map_err(|error| format!("failed encoding append-only AI transcript entry: {error}"))?;
    request.push('\n');
    request.push_str(&entry);
    Ok(())
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
        .map(|observation| omitted_observation(observation))
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
        spec("validate_runtime_state", "Evaluate 1..=16 scalar requirements shaped as {path, op, value}, where op is eq, ne, lt, lte, gt, or gte. baseline=live pauses and restores the running game; baseline=fresh boots an isolated child process for an integration-style check. Fresh defaults to main/tick/render. Omit setup, tick, or render unless an earlier tool result established the exact game-level function; never infer an entrypoint name. Use expected_outcome=fail immediately before a contiguous write batch; the runtime automatically repeats the identical contract as green validation after a successful write.", &["requirements", "expected_outcome"], &["frames", "baseline", "setup", "tick", "render"]),
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
        "validate_runtime_state",
        "run_frame",
        "run_tests",
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
        self.last_usage = None;
        if self.run.is_none() {
            self.run = Some(TemporaryRun::create()?);
        }
        let run = self.run.as_ref().expect("AI temporary run initialized");
        fs::write(
            &run.schema,
            serde_json::to_vec_pretty(&model_response_schema())
                .map_err(|error| error.to_string())?,
        )
        .map_err(|error| format!("failed writing Codex output schema: {error}"))?;
        let mut command = Command::new(&self.executable);
        let stderr = fs::File::create(&run.stderr)
            .map_err(|error| format!("failed creating Codex error capture: {error}"))?;
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
        child
            .stdin
            .take()
            .ok_or_else(|| "Codex stdin was unavailable".to_string())?
            .write_all(request.as_bytes())
            .map_err(|error| format!("failed sending Codex request: {error}"))?;
        loop {
            if canceled.load(Ordering::Acquire) {
                let _ = child.kill();
                let _ = child.wait();
                let _ = usage_worker.join();
                return Err("AI request canceled".to_string());
            }
            match child
                .try_wait()
                .map_err(|error| format!("failed waiting for Codex: {error}"))?
            {
                Some(status) if status.success() => break,
                Some(status) => {
                    let _ = usage_worker.join();
                    return Err(codex_failure_message(&run.stderr, status));
                }
                None => std::thread::sleep(Duration::from_millis(50)),
            }
        }
        self.last_usage = usage_worker
            .join()
            .map_err(|_| "Codex usage reader panicked".to_string())??;
        let source = fs::read_to_string(&run.output)
            .map_err(|error| format!("Codex did not produce a final response: {error}"))?;
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
                *args = serde_json::from_str(encoded)
                    .map_err(|error| format!("Codex returned invalid tool args JSON: {error}"))?;
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
            "agent_turns": MAX_AGENT_TURNS,
            "tool_calls_per_turn": MAX_TOOL_CALLS_PER_TURN,
            "working_notes_characters": MAX_WORKING_NOTES_CHARS,
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

        assert_eq!(MAX_AGENT_TURNS, 15);
        assert_eq!(tools.0, 50);
        assert_eq!(contract_json()["limits"]["agent_turns"], 15);
        assert_eq!(contract_json()["limits"]["tool_calls_per_turn"], 50);
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
                    .ok_or_else(|| "green validation required".to_string())
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
                    working_notes: "Intent: finish. Observed: green. Next: none. Blocker: none."
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
        assert!(live
            .iter()
            .any(|tool| tool.tool == "validate_runtime_state"));
        assert!(!live.iter().any(|tool| tool.tool == "capture_screenshot"));
    }

    #[test]
    fn agent_instruction_requires_complete_threshold_coverage() {
        assert!(AGENT_INSTRUCTION.contains("scoring, and reset extents"));
        assert!(AGENT_INSTRUCTION.contains("public update or tick path"));
        assert!(AGENT_INSTRUCTION.contains("direct helper tests are supplementary"));
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

    #[test]
    #[ignore = "requires an installed, signed-in Codex CLI"]
    fn installed_codex_provider_accepts_the_shared_response_schema() {
        let mut provider = CodexExecProvider::default();
        let response = provider
            .respond(
                r#"{"system":"Return a done response without calling tools.","user_prompt":"Confirm the Stasis AI provider is connected.","tool_specs":[],"transcript":[]}"#,
                &AtomicBool::new(false),
            )
            .expect("signed-in Codex response");
        assert!(matches!(response, ModelResponse::Done { .. }));
    }
}
