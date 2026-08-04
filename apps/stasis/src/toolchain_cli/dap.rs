use super::{compile_workspace_jit_with_debug, Workspace};
use serde_json::{json, Value};
use stasis_compiler::backend::jit::{JitDebugFunctionMetadata, JitProcess, JitScalarValue};
use stasis_compiler::frontend::types::{TYPE_ID_BOOL, TYPE_ID_I32, TYPE_ID_VOID};
use stasis_dynload::{
    disable_jit_debugger, drain_jit_output, enable_jit_debugger, pause_jit_debugger,
    resume_jit_debugger, set_jit_debug_breakpoints, set_jit_output_capture,
    wait_for_jit_debug_stop, JitDebugResume, JitDebugStop, JitDebugValue,
};
use std::collections::{BTreeMap, HashMap};
use std::io::{self, BufRead, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const THREAD_ID: i64 = 1;
const GLOBALS_REFERENCE: i64 = 2_000;
const LOCALS_REFERENCE_BASE: i64 = 1_000;

pub(super) fn run(workspace: &Workspace) -> Result<(), String> {
    let (input_tx, input_rx) = mpsc::channel();
    thread::Builder::new()
        .name("stasis-dap-input".to_string())
        .spawn(move || {
            let stdin = io::stdin();
            let result = read_messages(stdin.lock(), |message| {
                input_tx
                    .send(DapInput::Message(message))
                    .map_err(|_| "DAP session closed".to_string())
            });
            let _ = input_tx.send(match result {
                Ok(()) => DapInput::Eof,
                Err(error) => DapInput::Error(error),
            });
        })
        .map_err(|error| format!("failed to start DAP input reader: {error}"))?;

    let stdout = io::stdout();
    let mut output = DapOutput::new(stdout.lock());
    let mut session = DapSession::new(workspace.clone());
    loop {
        session.publish_runtime_events(&mut output)?;
        match input_rx.recv_timeout(Duration::from_millis(10)) {
            Ok(DapInput::Message(request)) => {
                if session.handle_request(request, &mut output)? {
                    break;
                }
            }
            Ok(DapInput::Eof) => break,
            Ok(DapInput::Error(error)) => return Err(error),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    session.shutdown();
    Ok(())
}

enum DapInput {
    Message(Value),
    Eof,
    Error(String),
}

fn read_messages<R: BufRead>(
    mut input: R,
    mut on_message: impl FnMut(Value) -> Result<(), String>,
) -> Result<(), String> {
    loop {
        let mut content_length = None;
        let mut saw_header = false;
        loop {
            let mut line = String::new();
            let count = input
                .read_line(&mut line)
                .map_err(|error| format!("failed reading DAP header: {error}"))?;
            if count == 0 {
                if saw_header {
                    return Err("DAP stream ended inside a message header".to_string());
                }
                return Ok(());
            }
            saw_header = true;
            if line == "\r\n" || line == "\n" {
                break;
            }
            let Some((name, value)) = line.trim_end().split_once(':') else {
                return Err(format!("invalid DAP header: {}", line.trim_end()));
            };
            if name.eq_ignore_ascii_case("Content-Length") {
                content_length = Some(
                    value
                        .trim()
                        .parse::<usize>()
                        .map_err(|_| "invalid DAP Content-Length".to_string())?,
                );
            }
        }
        let length =
            content_length.ok_or_else(|| "DAP message has no Content-Length".to_string())?;
        if length > 16 * 1024 * 1024 {
            return Err("DAP message exceeds the 16 MiB protocol limit".to_string());
        }
        let mut body = vec![0; length];
        input
            .read_exact(&mut body)
            .map_err(|error| format!("failed reading DAP message body: {error}"))?;
        let message = serde_json::from_slice(&body)
            .map_err(|error| format!("invalid DAP JSON payload: {error}"))?;
        on_message(message)?;
    }
}

struct DapOutput<W> {
    writer: W,
    sequence: i64,
}

impl<W: Write> DapOutput<W> {
    fn new(writer: W) -> Self {
        Self {
            writer,
            sequence: 1,
        }
    }

    fn response(&mut self, request: &Value, body: Value) -> Result<(), String> {
        self.response_with_success(request, true, None, body)
    }

    fn error_response(
        &mut self,
        request: &Value,
        message: impl Into<String>,
    ) -> Result<(), String> {
        self.response_with_success(request, false, Some(message.into()), json!({}))
    }

    fn response_with_success(
        &mut self,
        request: &Value,
        success: bool,
        message: Option<String>,
        body: Value,
    ) -> Result<(), String> {
        let mut response = json!({
            "seq": self.next_sequence(),
            "type": "response",
            "request_seq": request.get("seq").and_then(Value::as_i64).unwrap_or(0),
            "success": success,
            "command": request.get("command").and_then(Value::as_str).unwrap_or(""),
            "body": body,
        });
        if let Some(message) = message {
            response["message"] = Value::String(message);
        }
        self.write_message(&response)
    }

    fn event(&mut self, event: &str, body: Value) -> Result<(), String> {
        let sequence = self.next_sequence();
        self.write_message(&json!({
            "seq": sequence,
            "type": "event",
            "event": event,
            "body": body,
        }))
    }

    fn next_sequence(&mut self) -> i64 {
        let sequence = self.sequence;
        self.sequence = self.sequence.saturating_add(1);
        sequence
    }

    fn write_message(&mut self, message: &Value) -> Result<(), String> {
        let body = serde_json::to_vec(message)
            .map_err(|error| format!("failed serializing DAP message: {error}"))?;
        write!(self.writer, "Content-Length: {}\r\n\r\n", body.len())
            .and_then(|_| self.writer.write_all(&body))
            .and_then(|_| self.writer.flush())
            .map_err(|error| format!("failed writing DAP response: {error}"))
    }
}

struct DapSession {
    workspace: Workspace,
    runtime: Option<Arc<JitProcess>>,
    metadata: BTreeMap<u32, JitDebugFunctionMetadata>,
    sources: HashMap<String, String>,
    breakpoints: HashMap<String, Vec<(u32, u32)>>,
    current_stop: Option<JitDebugStop>,
    last_stop_sequence: u64,
    cancel: Arc<AtomicBool>,
    worker: Option<JoinHandle<Result<(), String>>>,
    terminated_sent: bool,
    launched: bool,
    started: bool,
    stop_on_entry: bool,
    next_stop_reason: &'static str,
}

impl DapSession {
    fn new(workspace: Workspace) -> Self {
        Self {
            workspace,
            runtime: None,
            metadata: BTreeMap::new(),
            sources: HashMap::new(),
            breakpoints: HashMap::new(),
            current_stop: None,
            last_stop_sequence: 0,
            cancel: Arc::new(AtomicBool::new(false)),
            worker: None,
            terminated_sent: false,
            launched: false,
            started: false,
            stop_on_entry: false,
            next_stop_reason: "breakpoint",
        }
    }

    fn handle_request<W: Write>(
        &mut self,
        request: Value,
        output: &mut DapOutput<W>,
    ) -> Result<bool, String> {
        if request.get("type").and_then(Value::as_str) != Some("request") {
            return Ok(false);
        }
        let command = request.get("command").and_then(Value::as_str).unwrap_or("");
        let result = (|| match command {
            "initialize" => {
                output.response(
                    &request,
                    json!({
                        "supportsConfigurationDoneRequest": true,
                        "supportsTerminateRequest": true,
                        "supportsEvaluateForHovers": true,
                        "supportsCancelRequest": false,
                        "supportsStepBack": false,
                        "supportsSetVariable": false,
                        "supportTerminateDebuggee": true,
                    }),
                )?;
                Ok(false)
            }
            "launch" => self.launch(&request, output),
            "setBreakpoints" => self.set_breakpoints(&request, output),
            "configurationDone" => self.configuration_done(&request, output),
            "threads" => {
                output.response(
                    &request,
                    json!({"threads": [{"id": THREAD_ID, "name": "Stasis runtime"}]}),
                )?;
                Ok(false)
            }
            "stackTrace" => self.stack_trace(&request, output),
            "scopes" => self.scopes(&request, output),
            "variables" => self.variables(&request, output),
            "evaluate" => self.evaluate(&request, output),
            "continue" => self.resume(&request, output, JitDebugResume::Continue),
            "next" => self.resume(&request, output, JitDebugResume::StepOver),
            "stepIn" => self.resume(&request, output, JitDebugResume::StepIn),
            "stepOut" => self.resume(&request, output, JitDebugResume::StepOut),
            "pause" => {
                self.require_runtime()?;
                pause_jit_debugger()?;
                self.next_stop_reason = "pause";
                output.response(&request, json!({}))?;
                Ok(false)
            }
            "terminate" => {
                output.response(&request, json!({}))?;
                self.stop_runtime();
                if !self.terminated_sent {
                    output.event("terminated", json!({}))?;
                    self.terminated_sent = true;
                }
                Ok(false)
            }
            "disconnect" => {
                output.response(&request, json!({}))?;
                Ok(true)
            }
            _ => Err(format!("unsupported DAP request '{command}'")),
        })();
        match result {
            Ok(exit) => Ok(exit),
            Err(error) => {
                output.error_response(&request, error)?;
                Ok(false)
            }
        }
    }

    fn launch<W: Write>(
        &mut self,
        request: &Value,
        output: &mut DapOutput<W>,
    ) -> Result<bool, String> {
        if self.launched {
            return Err("the Stasis debug session is already launched".to_string());
        }
        let jit = compile_workspace_jit_with_debug(&self.workspace, true)?;
        jit.activate_staged_runtime()?;
        self.metadata = jit.debug_metadata().clone();
        self.sources = jit
            .program_snapshot()
            .into_iter()
            .flat_map(|snapshot| snapshot.files())
            .map(|file| {
                (
                    normalize_workspace_path(&self.workspace.root, &file.path),
                    file.content.clone(),
                )
            })
            .collect();
        self.stop_on_entry = request
            .get("arguments")
            .and_then(|arguments| arguments.get("stopOnEntry"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        self.runtime = Some(Arc::new(jit));
        self.launched = true;
        set_jit_output_capture(true);
        enable_jit_debugger([]);
        output.response(request, json!({}))?;
        output.event("initialized", json!({}))?;
        output.event(
            "process",
            json!({
                "name": self.workspace.manifest.name,
                "isLocalProcess": true,
                "startMethod": "launch"
            }),
        )?;
        Ok(false)
    }

    fn set_breakpoints<W: Write>(
        &mut self,
        request: &Value,
        output: &mut DapOutput<W>,
    ) -> Result<bool, String> {
        self.require_runtime()?;
        let arguments = request.get("arguments").unwrap_or(&Value::Null);
        let source_path = arguments
            .get("source")
            .and_then(|source| source.get("path"))
            .and_then(Value::as_str)
            .ok_or_else(|| "setBreakpoints requires source.path".to_string())?;
        let source_key = normalize_workspace_path(&self.workspace.root, source_path);
        let requested = arguments
            .get("breakpoints")
            .and_then(Value::as_array)
            .map(|breakpoints| {
                breakpoints
                    .iter()
                    .filter_map(|breakpoint| breakpoint.get("line").and_then(Value::as_u64))
                    .filter_map(|line| u32::try_from(line).ok())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let mut accepted = Vec::new();
        let mut response = Vec::new();
        for requested_line in requested {
            if let Some((function_id, site_id, line, column)) =
                self.resolve_breakpoint(&source_key, requested_line)
            {
                accepted.push((function_id, site_id));
                response.push(json!({
                    "verified": true,
                    "line": line,
                    "column": column,
                    "source": {"path": source_path}
                }));
            } else {
                response.push(json!({
                    "verified": false,
                    "line": requested_line,
                    "message": "No executable Stasis statement is available at or after this line"
                }));
            }
        }
        self.breakpoints.insert(source_key, accepted);
        set_jit_debug_breakpoints(
            self.breakpoints
                .values()
                .flat_map(|breakpoints| breakpoints.iter().copied()),
        );
        output.response(request, json!({"breakpoints": response}))?;
        Ok(false)
    }

    fn configuration_done<W: Write>(
        &mut self,
        request: &Value,
        output: &mut DapOutput<W>,
    ) -> Result<bool, String> {
        self.require_runtime()?;
        if self.started {
            return Err("the Stasis debug runtime has already started".to_string());
        }
        if self.stop_on_entry {
            pause_jit_debugger()?;
            self.next_stop_reason = "entry";
        }
        let runtime = Arc::clone(self.runtime.as_ref().expect("runtime was checked"));
        let cancel = Arc::clone(&self.cancel);
        self.worker = Some(
            thread::Builder::new()
                .name("stasis-dap-runtime".to_string())
                .spawn(move || run_program(runtime, cancel))
                .map_err(|error| format!("failed to start Stasis debug runtime: {error}"))?,
        );
        self.started = true;
        output.response(request, json!({}))?;
        Ok(false)
    }

    fn resolve_breakpoint(
        &self,
        source_key: &str,
        requested_line: u32,
    ) -> Option<(u32, u32, u32, u32)> {
        let source = self.sources.get(source_key)?;
        self.metadata
            .values()
            .filter(|function| {
                normalize_workspace_path(&self.workspace.root, &function.file) == source_key
            })
            .flat_map(|function| {
                function.sites.iter().filter_map(move |site| {
                    let (line, column) = line_column(source, site.source_offset as usize)?;
                    (line >= requested_line).then_some((
                        function.function_id,
                        site.site_id,
                        line,
                        column,
                    ))
                })
            })
            .min_by_key(|(_, _, line, column)| (*line, *column))
    }

    fn stack_trace<W: Write>(
        &self,
        request: &Value,
        output: &mut DapOutput<W>,
    ) -> Result<bool, String> {
        let stop = self
            .current_stop
            .as_ref()
            .ok_or_else(|| "the Stasis runtime is not stopped".to_string())?;
        let mut frames = Vec::new();
        for (dap_index, frame) in stop.frames.iter().rev().enumerate() {
            let Some(metadata) = self.metadata.get(&frame.function_id) else {
                continue;
            };
            let absolute_file = self.workspace.root.join(&metadata.file);
            let source = self.sources.get(&normalize_workspace_path(
                &self.workspace.root,
                &metadata.file,
            ));
            let source_offset = metadata
                .sites
                .iter()
                .find(|site| site.site_id == frame.site_id)
                .map_or(metadata.source_range.start as usize, |site| {
                    site.source_offset as usize
                });
            let (line, column) = source
                .and_then(|source| line_column(source, source_offset))
                .unwrap_or((1, 1));
            frames.push(json!({
                "id": dap_index as i64 + 1,
                "name": metadata.name,
                "source": {"name": Path::new(&metadata.file).file_name().and_then(|name| name.to_str()).unwrap_or(&metadata.file), "path": absolute_file},
                "line": line,
                "column": column,
            }));
        }
        let total_frames = frames.len();
        output.response(
            request,
            json!({"stackFrames": frames, "totalFrames": total_frames}),
        )?;
        Ok(false)
    }

    fn scopes<W: Write>(&self, request: &Value, output: &mut DapOutput<W>) -> Result<bool, String> {
        let frame_id = request
            .get("arguments")
            .and_then(|arguments| arguments.get("frameId"))
            .and_then(Value::as_i64)
            .ok_or_else(|| "scopes requires frameId".to_string())?;
        self.frame_for_id(frame_id)?;
        output.response(
            request,
            json!({"scopes": [
                {"name": "Locals", "presentationHint": "locals", "variablesReference": LOCALS_REFERENCE_BASE + frame_id, "expensive": false},
                {"name": "Globals", "presentationHint": "globals", "variablesReference": GLOBALS_REFERENCE, "expensive": false}
            ]}),
        )?;
        Ok(false)
    }

    fn variables<W: Write>(
        &self,
        request: &Value,
        output: &mut DapOutput<W>,
    ) -> Result<bool, String> {
        let reference = request
            .get("arguments")
            .and_then(|arguments| arguments.get("variablesReference"))
            .and_then(Value::as_i64)
            .ok_or_else(|| "variables requires variablesReference".to_string())?;
        let runtime = self.require_runtime()?;
        let mut variables = if reference == GLOBALS_REFERENCE {
            runtime
                .snapshot_global_scalars()
                .into_iter()
                .map(|(name, value)| {
                    json!({
                        "name": name,
                        "value": format_scalar(value),
                        "type": value.type_name(),
                        "evaluateName": name,
                        "variablesReference": 0
                    })
                })
                .collect::<Vec<_>>()
        } else if reference > LOCALS_REFERENCE_BASE {
            let frame_id = reference - LOCALS_REFERENCE_BASE;
            let frame = self.frame_for_id(frame_id)?;
            let metadata = self
                .metadata
                .get(&frame.function_id)
                .ok_or_else(|| "debug metadata is missing for the selected frame".to_string())?;
            frame
                .values
                .iter()
                .filter_map(|(slot, value)| {
                    let name = metadata.variables.get(slot)?;
                    let (value_text, type_name) = format_debug_value(runtime, *value);
                    Some(json!({
                        "name": name,
                        "value": value_text,
                        "type": type_name,
                        "evaluateName": name,
                        "variablesReference": 0
                    }))
                })
                .collect::<Vec<_>>()
        } else {
            return Err(format!("unknown variablesReference {reference}"));
        };
        variables.sort_by(|left, right| left["name"].as_str().cmp(&right["name"].as_str()));
        output.response(request, json!({"variables": variables}))?;
        Ok(false)
    }

    fn evaluate<W: Write>(
        &self,
        request: &Value,
        output: &mut DapOutput<W>,
    ) -> Result<bool, String> {
        let arguments = request.get("arguments").unwrap_or(&Value::Null);
        let expression = arguments
            .get("expression")
            .and_then(Value::as_str)
            .ok_or_else(|| "evaluate requires an expression".to_string())?
            .trim();
        let runtime = self.require_runtime()?;
        let frame = if let Some(frame_id) = arguments.get("frameId").and_then(Value::as_i64) {
            Some(self.frame_for_id(frame_id)?)
        } else {
            self.current_stop
                .as_ref()
                .and_then(|stop| stop.frames.last())
        };
        if let Some(frame) = frame {
            if let Some(metadata) = self.metadata.get(&frame.function_id) {
                if let Some((_, value)) = frame.values.iter().find(|(slot, _)| {
                    metadata
                        .variables
                        .get(slot)
                        .is_some_and(|name| name == expression)
                }) {
                    let (value, type_name) = format_debug_value(runtime, *value);
                    output.response(
                        request,
                        json!({"result": value, "type": type_name, "variablesReference": 0}),
                    )?;
                    return Ok(false);
                }
            }
        }
        let inspected = runtime.inspect_state_query(expression)?;
        let value = inspected
            .pointer("/value/value")
            .cloned()
            .unwrap_or_else(|| inspected.clone());
        let result = value
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| value.to_string());
        let type_name = inspected
            .get("static_type")
            .and_then(Value::as_str)
            .unwrap_or("state query");
        output.response(
            request,
            json!({"result": result, "type": type_name, "variablesReference": 0}),
        )?;
        Ok(false)
    }

    fn resume<W: Write>(
        &mut self,
        request: &Value,
        output: &mut DapOutput<W>,
        mode: JitDebugResume,
    ) -> Result<bool, String> {
        self.require_runtime()?;
        if self.current_stop.is_none() {
            return Err("the Stasis runtime is not stopped".to_string());
        }
        resume_jit_debugger(mode)?;
        self.next_stop_reason = match mode {
            JitDebugResume::Continue => "breakpoint",
            JitDebugResume::StepIn | JitDebugResume::StepOver | JitDebugResume::StepOut => "step",
        };
        self.current_stop = None;
        output.response(request, json!({"allThreadsContinued": true}))?;
        output.event(
            "continued",
            json!({"threadId": THREAD_ID, "allThreadsContinued": true}),
        )?;
        Ok(false)
    }

    fn frame_for_id(&self, frame_id: i64) -> Result<&stasis_dynload::JitDebugFrame, String> {
        let stop = self
            .current_stop
            .as_ref()
            .ok_or_else(|| "the Stasis runtime is not stopped".to_string())?;
        let dap_index = usize::try_from(frame_id.saturating_sub(1))
            .map_err(|_| format!("invalid frameId {frame_id}"))?;
        stop.frames
            .len()
            .checked_sub(dap_index + 1)
            .and_then(|index| stop.frames.get(index))
            .ok_or_else(|| format!("invalid frameId {frame_id}"))
    }

    fn publish_runtime_events<W: Write>(
        &mut self,
        output: &mut DapOutput<W>,
    ) -> Result<(), String> {
        let captured = drain_jit_output();
        if !captured.is_empty() {
            output.event("output", json!({"category": "stdout", "output": captured}))?;
        }
        if self.started && self.current_stop.is_none() {
            if let Some(stop) = wait_for_jit_debug_stop(self.last_stop_sequence, Duration::ZERO) {
                self.last_stop_sequence = stop.sequence;
                let at_breakpoint = self
                    .breakpoints
                    .values()
                    .flatten()
                    .any(|site| *site == (stop.function_id, stop.site_id));
                let reason = if at_breakpoint {
                    "breakpoint"
                } else {
                    self.next_stop_reason
                };
                self.current_stop = Some(stop);
                output.event(
                    "stopped",
                    json!({
                        "reason": reason,
                        "threadId": THREAD_ID,
                        "allThreadsStopped": true
                    }),
                )?;
            }
        }
        let finished = self
            .worker
            .as_ref()
            .is_some_and(|worker| worker.is_finished());
        if finished {
            let result = self.worker.take().expect("finished worker exists").join();
            let result = result.map_err(|_| "Stasis debug runtime panicked".to_string())?;
            if let Err(error) = result {
                output.event(
                    "output",
                    json!({"category": "stderr", "output": format!("{error}\n")}),
                )?;
            }
            if !self.terminated_sent {
                output.event("terminated", json!({}))?;
                self.terminated_sent = true;
            }
        }
        Ok(())
    }

    fn require_runtime(&self) -> Result<&Arc<JitProcess>, String> {
        self.runtime
            .as_ref()
            .ok_or_else(|| "launch the Stasis debug runtime first".to_string())
    }

    fn stop_runtime(&mut self) {
        self.cancel.store(true, Ordering::Release);
        disable_jit_debugger();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        self.current_stop = None;
    }

    fn shutdown(&mut self) {
        self.stop_runtime();
        set_jit_output_capture(false);
    }
}

fn run_program(runtime: Arc<JitProcess>, cancel: Arc<AtomicBool>) -> Result<(), String> {
    execute_host_function(&runtime, "main", false)?;
    let snapshot = runtime
        .program_snapshot()
        .ok_or_else(|| "debug runtime has no accepted program snapshot".to_string())?;
    let has_tick = snapshot
        .functions()
        .iter()
        .any(|function| function.name == "tick");
    let has_render = snapshot
        .functions()
        .iter()
        .any(|function| function.name == "render");
    if !has_tick && !has_render {
        return Ok(());
    }
    while !cancel.load(Ordering::Acquire) {
        if has_tick {
            execute_host_function(&runtime, "tick", true)?;
        }
        if has_render {
            execute_host_function(&runtime, "render", true)?;
        }
        thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}

fn execute_host_function(runtime: &JitProcess, name: &str, optional: bool) -> Result<(), String> {
    let Some(snapshot) = runtime.program_snapshot() else {
        return Err("debug runtime has no accepted program snapshot".to_string());
    };
    let Some(function) = snapshot
        .functions()
        .iter()
        .find(|function| function.name == name)
    else {
        return if optional {
            Ok(())
        } else {
            Err(format!("required host function '{name}' was not found"))
        };
    };
    if !function.params.is_empty() {
        return Err(format!(
            "debug host function '{name}' must not accept parameters"
        ));
    }
    match function.return_type {
        TYPE_ID_VOID => runtime.execute_void_noarg_by_name(name),
        TYPE_ID_I32 => runtime.execute_i32_noarg_by_name(name).map(|_| ()),
        TYPE_ID_BOOL => runtime.execute_bool_noarg_by_name(name).map(|_| ()),
        return_type => Err(format!(
            "debug host function '{name}' has unsupported return type id {return_type}"
        )),
    }
}

fn format_debug_value(runtime: &JitProcess, value: JitDebugValue) -> (String, String) {
    let type_tag = match value {
        JitDebugValue::I64 { type_tag, .. } | JitDebugValue::F64 { type_tag, .. } => type_tag,
    };
    let type_name = runtime
        .program_snapshot()
        .and_then(|snapshot| snapshot.type_info(type_tag as u16))
        .map(|info| info.name.clone())
        .unwrap_or_else(|| format!("type#{type_tag}"));
    let text = match value {
        JitDebugValue::I64 { value, .. } if type_name == "bool" => (value != 0).to_string(),
        JitDebugValue::I64 { value, .. } if matches!(type_name.as_str(), "u8" | "u16" | "u32") => {
            (value as u64).to_string()
        }
        JitDebugValue::I64 { value, .. } => value.to_string(),
        JitDebugValue::F64 { value, .. } => value.to_string(),
    };
    (text, type_name)
}

fn format_scalar(value: JitScalarValue) -> String {
    match value {
        JitScalarValue::I32(value) => value.to_string(),
        JitScalarValue::F32(value) => value.to_string(),
        JitScalarValue::F64(value) => value.to_string(),
        JitScalarValue::Bool(value) => value.to_string(),
        JitScalarValue::U8(value) => value.to_string(),
        JitScalarValue::U16(value) => value.to_string(),
        JitScalarValue::U32(value) => value.to_string(),
    }
}

fn normalize_path(path: &str) -> String {
    let resolved = Path::new(path)
        .canonicalize()
        .unwrap_or_else(|_| Path::new(path).to_path_buf());
    let normalized = resolved.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        normalized.to_ascii_lowercase()
    } else {
        normalized
    }
}

fn normalize_workspace_path(root: &Path, path: &str) -> String {
    let path = Path::new(path);
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    normalize_path(&resolved.to_string_lossy())
}

fn line_column(source: &str, offset: usize) -> Option<(u32, u32)> {
    if offset > source.len() || !source.is_char_boundary(offset) {
        return None;
    }
    let prefix = &source[..offset];
    let line_start = prefix.rfind('\n').map_or(0, |index| index + 1);
    let line = u32::try_from(prefix.bytes().filter(|byte| *byte == b'\n').count())
        .ok()?
        .checked_add(1)?;
    let column = u32::try_from(source[line_start..offset].encode_utf16().count())
        .ok()?
        .checked_add(1)?;
    Some((line, column))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufReader, Cursor};
    use std::time::{Instant, SystemTime, UNIX_EPOCH};

    #[test]
    fn dap_framing_reads_multiple_messages_and_writes_content_lengths() {
        let first = br#"{"seq":1,"type":"request","command":"initialize"}"#;
        let second = br#"{"seq":2,"type":"request","command":"threads"}"#;
        let input = format!(
            "Content-Length: {}\r\n\r\n{}Content-Length: {}\r\n\r\n{}",
            first.len(),
            String::from_utf8_lossy(first),
            second.len(),
            String::from_utf8_lossy(second)
        );
        let mut messages = Vec::new();
        read_messages(BufReader::new(Cursor::new(input.into_bytes())), |message| {
            messages.push(message);
            Ok(())
        })
        .expect("read messages");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1]["command"], "threads");

        let mut bytes = Vec::new();
        let mut output = DapOutput::new(&mut bytes);
        output.event("initialized", json!({})).expect("write event");
        let text = String::from_utf8(bytes).expect("utf8 output");
        let (header, body) = text.split_once("\r\n\r\n").expect("framed output");
        let declared = header
            .strip_prefix("Content-Length: ")
            .expect("content length")
            .parse::<usize>()
            .expect("length");
        assert_eq!(declared, body.len());
        assert_eq!(
            serde_json::from_str::<Value>(body).unwrap()["event"],
            "initialized"
        );
    }

    #[test]
    fn source_positions_are_one_based_utf16() {
        assert_eq!(line_column("a\n😀x", 6), Some((2, 3)));
    }

    #[test]
    fn dap_session_stops_in_real_jit_with_stack_locals_globals_and_stepping() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "stasis-dap-session-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join("src")).expect("create source directory");
        let source = "global score: i32;\nfunction helper(base: i32): i32 {\n    let doubled: i32 = base * 2;\n    score = doubled;\n    return doubled;\n}\nfunction main(): i32 {\n    let base: i32 = 3;\n    return helper(base);\n}\n";
        let source_path = root.join("src/main.stasis");
        std::fs::write(&source_path, source).expect("write source");
        let manifest = super::super::ProjectManifest::new("dap_fixture".to_string());
        std::fs::write(
            root.join(super::super::MANIFEST_NAME),
            serde_json::to_vec(&manifest).expect("manifest json"),
        )
        .expect("write manifest");
        let workspace = Workspace {
            root: root.clone(),
            manifest,
        };
        let mut session = DapSession::new(workspace);
        let mut bytes = Vec::new();
        {
            let mut output = DapOutput::new(&mut bytes);
            session
                .handle_request(
                    json!({"seq": 1, "type": "request", "command": "launch", "arguments": {}}),
                    &mut output,
                )
                .expect("launch request");
            session
                .handle_request(
                    json!({
                        "seq": 2,
                        "type": "request",
                        "command": "setBreakpoints",
                        "arguments": {"source": {"path": source_path}, "breakpoints": [{"line": 3}]}
                    }),
                    &mut output,
                )
                .expect("breakpoint request");
            assert_eq!(
                session
                    .breakpoints
                    .get(&normalize_workspace_path(
                        &root,
                        &source_path.to_string_lossy()
                    ))
                    .map(Vec::len),
                Some(1),
                "breakpoint was not resolved; metadata={:?}, sources={:?}",
                session.metadata,
                session.sources.keys().collect::<Vec<_>>()
            );
            session
                .handle_request(
                    json!({"seq": 3, "type": "request", "command": "configurationDone"}),
                    &mut output,
                )
                .expect("configuration request");

            wait_until(Duration::from_secs(2), || {
                session
                    .publish_runtime_events(&mut output)
                    .expect("runtime event");
                session.current_stop.is_some()
            });
            let stop = session.current_stop.as_ref().expect("breakpoint stop");
            assert_eq!(stop.frames.len(), 2);
            let helper = stop.frames.last().expect("helper frame");
            let helper_metadata = session
                .metadata
                .get(&helper.function_id)
                .expect("helper metadata");
            assert_eq!(helper_metadata.name, "helper");
            let base_slot = helper_metadata
                .variables
                .iter()
                .find_map(|(slot, name)| (name == "base").then_some(*slot))
                .expect("base slot");
            let doubled_slot = helper_metadata
                .variables
                .iter()
                .find_map(|(slot, name)| (name == "doubled").then_some(*slot))
                .expect("doubled slot");
            assert_eq!(
                helper.values.get(&base_slot),
                Some(&JitDebugValue::I64 {
                    type_tag: i32::from(TYPE_ID_I32),
                    value: 3,
                })
            );
            session
                .handle_request(
                    json!({"seq": 4, "type": "request", "command": "stackTrace", "arguments": {"threadId": THREAD_ID}}),
                    &mut output,
                )
                .expect("stack request");
            session
                .handle_request(
                    json!({"seq": 5, "type": "request", "command": "scopes", "arguments": {"frameId": 1}}),
                    &mut output,
                )
                .expect("scopes request");
            session
                .handle_request(
                    json!({"seq": 6, "type": "request", "command": "variables", "arguments": {"variablesReference": LOCALS_REFERENCE_BASE + 1}}),
                    &mut output,
                )
                .expect("variables request");
            session
                .handle_request(
                    json!({"seq": 7, "type": "request", "command": "next", "arguments": {"threadId": THREAD_ID}}),
                    &mut output,
                )
                .expect("next request");
            wait_until(Duration::from_secs(2), || {
                session
                    .publish_runtime_events(&mut output)
                    .expect("step event");
                session.current_stop.is_some()
            });
            let helper = session
                .current_stop
                .as_ref()
                .unwrap()
                .frames
                .last()
                .unwrap();
            assert_eq!(
                helper.values.get(&doubled_slot),
                Some(&JitDebugValue::I64 {
                    type_tag: i32::from(TYPE_ID_I32),
                    value: 6,
                })
            );
            session
                .handle_request(
                    json!({"seq": 8, "type": "request", "command": "continue", "arguments": {"threadId": THREAD_ID}}),
                    &mut output,
                )
                .expect("continue request");
            wait_until(Duration::from_secs(2), || {
                session
                    .publish_runtime_events(&mut output)
                    .expect("termination event");
                session.terminated_sent
            });
        }
        session.shutdown();
        let output = String::from_utf8(bytes).expect("DAP output utf8");
        assert!(output.contains("\"event\":\"stopped\""), "{output}");
        assert!(output.contains("\"stackFrames\""), "{output}");
        assert!(output.contains("\"name\":\"base\""), "{output}");
        assert!(output.contains("\"event\":\"terminated\""), "{output}");
        std::fs::remove_dir_all(root).expect("remove fixture");
    }

    fn wait_until(timeout: Duration, mut predicate: impl FnMut() -> bool) {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if predicate() {
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("condition did not become true within {timeout:?}");
    }
}
