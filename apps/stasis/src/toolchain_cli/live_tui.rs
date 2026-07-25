use super::{format_live_response, scalar_text};
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::queue;
use crossterm::style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor};
use crossterm::terminal::{
    self, disable_raw_mode, enable_raw_mode, BeginSynchronizedUpdate, Clear, ClearType,
    EndSynchronizedUpdate, EnterAlternateScreen, LeaveAlternateScreen,
};
use serde_json::Value;
use stasis_ai::{
    live_tool_specs, run_agent, AgentEvent, CodexExecProvider, ToolCall, ToolExecutor,
    ToolObservation, DEFAULT_CODEX_MODEL, DEFAULT_REASONING_EFFORT,
};
use stasis_compiler::frontend::parser::completion_expected_type;
use stasis_compiler::frontend::workshop::{
    workshop_source_hash, WorkshopSourceItem, WorkshopSourceItemKind,
};
use stasis_runner::live::{
    CompletionContext, CompletionItem, CompletionQuery, LiveCommand, LiveEdit, LiveEditOperation,
    LiveRequest, LiveResponse, LiveSessionClient, LiveSymbolTarget, TerminalBuffer, TerminalInput,
};
use std::collections::{BTreeMap, VecDeque};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const MAX_TRANSCRIPT_LINES: usize = 500;
const MAX_UNDO_STATES: usize = 100;
const COMPLETION_LIMIT: usize = 64;
const TUI_REQUEST_START: u64 = 1u64 << 61;
const AI_REQUEST_START: u64 = 1u64 << 62;

enum AiUiEvent {
    InitialContext(Value),
    Progress(AgentEvent),
    Finished(Result<String, String>),
}

pub(super) fn run_scripted_ai(
    client: &LiveSessionClient,
    project_root: &Path,
    prompt: &str,
) -> Result<(String, PathBuf, PathBuf), String> {
    run_scripted_ai_with_cancel(client, project_root, prompt, &AtomicBool::new(false))
}

pub(super) fn run_scripted_ai_with_cancel(
    client: &LiveSessionClient,
    project_root: &Path,
    prompt: &str,
    canceled: &AtomicBool,
) -> Result<(String, PathBuf, PathBuf), String> {
    let mut audit = AiAuditLog::create(project_root, prompt)?;
    let trace_path = audit.path.clone();
    let usage_path = audit.usage_path.clone();
    let mut provider = CodexExecProvider::default();
    let mut tools = LiveAiTools::new(client.clone());
    let initial_context = load_ai_initial_context(&mut tools, canceled)?;
    audit.write(serde_json::json!({
        "event": "initial_context",
        "value": initial_context.clone(),
    }))?;
    let mut audit_error = None;
    let result = run_agent(
        &mut provider,
        &mut tools,
        prompt,
        initial_context,
        live_tool_specs(),
        canceled,
        |event| {
            if audit_error.is_none() {
                audit_error = match &event {
                    AgentEvent::ProviderUsage(usage) => audit.write_usage(usage).err(),
                    _ => audit.write(audit_agent_event(&event)).err(),
                };
            }
        },
    );
    if let Some(error) = audit_error {
        return Err(error);
    }
    match result {
        Ok(summary) => {
            audit.write(serde_json::json!({
                "event": "finished",
                "ok": true,
                "summary": summary,
            }))?;
            Ok((summary, trace_path, usage_path))
        }
        Err(error) => {
            audit.write(serde_json::json!({
                "event": "finished",
                "ok": false,
                "error": error,
            }))?;
            Err(error)
        }
    }
}

fn ai_initial_context(initial_symbols: Value) -> Value {
    serde_json::json!({
        "language": "Stasis",
        "runtime": "live in-process JIT",
        "commit_boundary": "between deterministic ticks",
        "write_policy": "all writes in one model batch compile, test, and commit atomically",
        "initial_symbols": initial_symbols,
        "initial_symbols_instruction": "This is the completed default list_symbols result. Use it before requesting filtered or paged follow-up discovery.",
    })
}

fn load_ai_initial_context(
    tools: &mut LiveAiTools,
    canceled: &AtomicBool,
) -> Result<Value, String> {
    let observations = tools.execute(
        &[ToolCall {
            tool: "list_symbols".to_string(),
            args: serde_json::json!({}),
        }],
        canceled,
    );
    let observation = observations
        .into_iter()
        .next()
        .ok_or_else(|| "initial symbol discovery returned no observation".to_string())?;
    if let Some(error) = observation.error {
        return Err(format!("initial symbol discovery failed: {error}"));
    }
    Ok(ai_initial_context(
        observation.result.unwrap_or(Value::Null),
    ))
}

fn audit_agent_event(event: &AgentEvent) -> Value {
    match event {
        AgentEvent::Turn { current, maximum } => {
            serde_json::json!({"event": "turn", "current": current, "maximum": maximum})
        }
        AgentEvent::ProviderUsage(_) => unreachable!("provider usage has a separate log"),
        AgentEvent::WorkingNotes(notes) => {
            serde_json::json!({"event": "working_notes", "text": notes})
        }
        AgentEvent::ToolBatch(calls) => serde_json::json!({
            "event": "tool_calls",
            "calls": calls.iter().map(audit_tool_call).collect::<Vec<_>>(),
        }),
        AgentEvent::Observations(observations) => serde_json::json!({
            "event": "tool_observations",
            "observations": observations.iter().map(audit_observation).collect::<Vec<_>>(),
        }),
        AgentEvent::Completed(summary) => {
            serde_json::json!({"event": "model_completed", "summary": summary})
        }
    }
}

struct AiRun {
    canceled: Arc<AtomicBool>,
    events: mpsc::Receiver<AiUiEvent>,
    worker: Option<thread::JoinHandle<()>>,
}

struct AiAuditLog {
    path: PathBuf,
    file: fs::File,
    usage_path: PathBuf,
    usage_file: fs::File,
    timing_path: PathBuf,
    timing_file: fs::File,
    started_at: Instant,
    last_event_at: Instant,
}

impl AiAuditLog {
    fn create(project_root: &Path, prompt: &str) -> Result<Self, String> {
        let directory = project_root.join("build/ai-traces");
        fs::create_dir_all(&directory)
            .map_err(|error| format!("failed creating AI trace directory: {error}"))?;
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let path = directory.join(format!("tui-ai-{stamp}.jsonl"));
        let usage_path = directory.join(format!("tui-ai-{stamp}.usage.jsonl"));
        let timing_path = directory.join(format!("tui-ai-{stamp}.timing.jsonl"));
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .map_err(|error| format!("failed creating AI trace: {error}"))?;
        let usage_file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&usage_path)
            .map_err(|error| format!("failed creating AI usage trace: {error}"))?;
        let timing_file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&timing_path)
            .map_err(|error| format!("failed creating AI timing trace: {error}"))?;
        let now = Instant::now();
        let mut log = Self {
            path,
            file,
            usage_path,
            usage_file,
            timing_path,
            timing_file,
            started_at: now,
            last_event_at: now,
        };
        log.write(serde_json::json!({
            "event": "request",
            "provider": "installed_codex_subscription",
            "model": std::env::var("STASIS_AI_MODEL")
                .unwrap_or_else(|_| DEFAULT_CODEX_MODEL.to_string()),
            "reasoning_effort": std::env::var("STASIS_AI_REASONING_EFFORT")
                .unwrap_or_else(|_| DEFAULT_REASONING_EFFORT.to_string()),
            "prompt": prompt,
            "payload_logging": "exact agent tool calls and observations; Codex transport envelope omitted",
        }))?;
        Ok(log)
    }

    fn write(&mut self, value: Value) -> Result<(), String> {
        let now = Instant::now();
        let timing = serde_json::json!({
            "event": value.get("event").cloned().unwrap_or(Value::Null),
            "elapsed_ms": duration_millis(now.duration_since(self.started_at)),
            "since_previous_ms": duration_millis(now.duration_since(self.last_event_at)),
        });
        self.last_event_at = now;
        serde_json::to_writer(&mut self.file, &value)
            .map_err(|error| format!("failed encoding AI trace: {error}"))?;
        self.file
            .write_all(b"\n")
            .map_err(|error| format!("failed writing AI trace: {error}"))?;
        self.file
            .flush()
            .map_err(|error| format!("failed flushing AI trace: {error}"))?;
        serde_json::to_writer(&mut self.timing_file, &timing)
            .map_err(|error| format!("failed encoding AI timing trace: {error}"))?;
        self.timing_file
            .write_all(b"\n")
            .map_err(|error| format!("failed writing AI timing trace: {error}"))?;
        self.timing_file
            .flush()
            .map_err(|error| format!("failed flushing AI timing trace: {error}"))
    }

    fn write_usage(&mut self, usage: &Value) -> Result<(), String> {
        serde_json::to_writer(&mut self.usage_file, usage)
            .map_err(|error| format!("failed encoding AI usage trace: {error}"))?;
        self.usage_file
            .write_all(b"\n")
            .map_err(|error| format!("failed writing AI usage trace: {error}"))?;
        self.usage_file
            .flush()
            .map_err(|error| format!("failed flushing AI usage trace: {error}"))
    }
}

fn duration_millis(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

impl Drop for AiRun {
    fn drop(&mut self) {
        self.canceled.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub(super) fn run(client: &LiveSessionClient, project_root: &Path) -> Result<bool, String> {
    let _guard = TerminalGuard::enter()?;
    let mut app = LiveTui::new(client.clone(), project_root.to_path_buf());
    app.request_default_inspection()?;
    app.refresh_completion(false);
    loop {
        app.maybe_refresh_default_inspection()?;
        app.drain_responses()?;
        app.render()?;
        if app.quit {
            return Ok(true);
        }
        if !event::poll(Duration::from_millis(33))
            .map_err(|error| format!("failed polling live TUI input: {error}"))?
        {
            continue;
        }
        match event::read().map_err(|error| format!("failed reading live TUI input: {error}"))? {
            Event::Key(key) if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
                app.handle_key(key)?;
            }
            Event::Resize(_, _) => {}
            _ => {}
        }
    }
}

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> Result<Self, String> {
        enable_raw_mode().map_err(|error| format!("failed enabling live TUI input: {error}"))?;
        let mut stdout = io::stdout();
        if let Err(error) = execute!(stdout, EnterAlternateScreen, Hide, Clear(ClearType::All)) {
            let _ = execute!(stdout, Show, LeaveAlternateScreen);
            let _ = disable_raw_mode();
            return Err(format!("failed entering live TUI screen: {error}"));
        }
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let mut stdout = io::stdout();
        let _ = execute!(
            stdout,
            EndSynchronizedUpdate,
            ResetColor,
            SetAttribute(Attribute::Reset),
            Show,
            LeaveAlternateScreen
        );
        let _ = disable_raw_mode();
    }
}

#[derive(Clone)]
struct BufferSnapshot {
    text: String,
    cursor: usize,
}

#[derive(Default)]
struct EditBuffer {
    text: String,
    cursor: usize,
    scroll_top: usize,
    last_rendered_cursor_line: usize,
    selection_anchor: Option<usize>,
    undo: Vec<BufferSnapshot>,
    redo: Vec<BufferSnapshot>,
    revision: u64,
}

impl EditBuffer {
    fn from_text(text: String) -> Self {
        let cursor = text.len();
        Self {
            text,
            cursor,
            ..Self::default()
        }
    }

    fn snapshot(&self) -> BufferSnapshot {
        BufferSnapshot {
            text: self.text.clone(),
            cursor: self.cursor,
        }
    }

    fn remember(&mut self) {
        self.undo.push(self.snapshot());
        if self.undo.len() > MAX_UNDO_STATES {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.revision = self.revision.saturating_add(1);
    }

    fn restore(&mut self, snapshot: BufferSnapshot) {
        self.text = snapshot.text;
        self.cursor = snapshot.cursor.min(self.text.len());
        self.selection_anchor = None;
        self.revision = self.revision.saturating_add(1);
    }

    fn undo(&mut self) {
        if let Some(snapshot) = self.undo.pop() {
            self.redo.push(self.snapshot());
            self.restore(snapshot);
        }
    }

    fn redo(&mut self) {
        if let Some(snapshot) = self.redo.pop() {
            self.undo.push(self.snapshot());
            self.restore(snapshot);
        }
    }

    fn selected_range(&self) -> Option<std::ops::Range<usize>> {
        let anchor = self.selection_anchor?;
        (anchor != self.cursor).then(|| anchor.min(self.cursor)..anchor.max(self.cursor))
    }

    fn selected_or_token(&self) -> String {
        if let Some(range) = self.selected_range() {
            return self.text[range].trim().to_string();
        }
        let is_token = |ch: char| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.');
        let mut start = self.cursor;
        while start > 0 {
            let prior = previous_boundary(&self.text, start);
            if !self.text[prior..start].chars().all(is_token) {
                break;
            }
            start = prior;
        }
        let mut end = self.cursor;
        while end < self.text.len() {
            let next = next_boundary(&self.text, end);
            if !self.text[end..next].chars().all(is_token) {
                break;
            }
            end = next;
        }
        self.text[start..end].trim().to_string()
    }

    fn delete_selection(&mut self) -> bool {
        let Some(range) = self.selected_range() else {
            return false;
        };
        self.text.replace_range(range.clone(), "");
        self.cursor = range.start;
        self.selection_anchor = None;
        true
    }

    fn insert_char(&mut self, ch: char) {
        self.remember();
        self.delete_selection();
        self.text.insert(self.cursor, ch);
        self.cursor += ch.len_utf8();
    }

    fn insert_newline(&mut self) {
        self.remember();
        self.delete_selection();
        let line_start = self.text[..self.cursor]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        let line = &self.text[line_start..self.cursor];
        let mut indent = line.chars().take_while(|ch| *ch == ' ').count();
        if line.trim_end().ends_with('{') {
            indent += 4;
        }
        let insertion = format!("\n{}", " ".repeat(indent));
        self.text.insert_str(self.cursor, &insertion);
        self.cursor += insertion.len();
    }

    fn backspace(&mut self) {
        if self.cursor == 0 && self.selected_range().is_none() {
            return;
        }
        self.remember();
        if self.delete_selection() {
            return;
        }
        let prior = previous_boundary(&self.text, self.cursor);
        self.text.replace_range(prior..self.cursor, "");
        self.cursor = prior;
    }

    fn delete(&mut self) {
        if self.cursor == self.text.len() && self.selected_range().is_none() {
            return;
        }
        self.remember();
        if self.delete_selection() {
            return;
        }
        let next = next_boundary(&self.text, self.cursor);
        self.text.replace_range(self.cursor..next, "");
    }

    fn set_cursor(&mut self, cursor: usize, select: bool) {
        if select {
            self.selection_anchor.get_or_insert(self.cursor);
        } else {
            self.selection_anchor = None;
        }
        self.cursor = cursor.min(self.text.len());
    }

    fn move_left(&mut self, select: bool) {
        self.set_cursor(previous_boundary(&self.text, self.cursor), select);
    }

    fn move_right(&mut self, select: bool) {
        self.set_cursor(next_boundary(&self.text, self.cursor), select);
    }

    fn move_home(&mut self, select: bool) {
        let line_start = self.text[..self.cursor]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        self.set_cursor(line_start, select);
    }

    fn move_end(&mut self, select: bool) {
        let line_end = self.text[self.cursor..]
            .find('\n')
            .map_or(self.text.len(), |index| self.cursor + index);
        self.set_cursor(line_end, select);
    }

    fn move_vertical(&mut self, delta: isize, select: bool) {
        let (line, column) = line_column(&self.text, self.cursor);
        let lines = line_ranges(&self.text);
        let target = line
            .saturating_add_signed(delta)
            .min(lines.len().saturating_sub(1));
        let range = &lines[target];
        let cursor = self.text[range.clone()]
            .char_indices()
            .nth(column)
            .map_or(range.end, |(offset, _)| range.start + offset);
        self.set_cursor(cursor, select);
    }

    fn replace_completion(&mut self, start: usize, end: usize, text: &str) {
        if start > end || end > self.text.len() {
            return;
        }
        self.remember();
        self.text.replace_range(start..end, text);
        self.cursor = start + text.len();
        self.selection_anchor = None;
    }
}

fn previous_boundary(text: &str, cursor: usize) -> usize {
    text[..cursor]
        .char_indices()
        .last()
        .map_or(0, |(index, _)| index)
}

fn next_boundary(text: &str, cursor: usize) -> usize {
    text[cursor..]
        .chars()
        .next()
        .map_or(cursor, |ch| cursor + ch.len_utf8())
}

fn line_ranges(text: &str) -> Vec<std::ops::Range<usize>> {
    let mut ranges = Vec::new();
    let mut start = 0;
    for (index, ch) in text.char_indices() {
        if ch == '\n' {
            ranges.push(start..index);
            start = index + 1;
        }
    }
    ranges.push(start..text.len());
    ranges
}

fn line_column(text: &str, cursor: usize) -> (usize, usize) {
    let before = &text[..cursor];
    let line = before.chars().filter(|ch| *ch == '\n').count();
    let line_start = before.rfind('\n').map_or(0, |index| index + 1);
    (line, text[line_start..cursor].chars().count())
}

struct EditSession {
    id: u64,
    target: LiveSymbolTarget,
    expected_source_hash: String,
    accepted_source: String,
    source_start: usize,
    buffer: EditBuffer,
    discard_confirm: bool,
}

enum InputMode {
    Prompt(EditBuffer),
    Definition(EditSession),
}

impl InputMode {
    fn buffer(&self) -> &EditBuffer {
        match self {
            Self::Prompt(buffer) => buffer,
            Self::Definition(edit) => &edit.buffer,
        }
    }

    fn buffer_mut(&mut self) -> &mut EditBuffer {
        match self {
            Self::Prompt(buffer) => buffer,
            Self::Definition(edit) => &mut edit.buffer,
        }
    }
}

#[derive(Default)]
struct CompletionState {
    items: Vec<CompletionItem>,
    selected: usize,
    replacement_start: usize,
    replacement_end: usize,
    armed: bool,
    truncated: bool,
}

#[derive(Clone)]
enum PendingAction {
    OpenEdit,
    ApplyEdit {
        source: String,
        revision: u64,
        session_id: u64,
        target: LiveSymbolTarget,
        submitted_at: Instant,
    },
    DefaultInspect,
    Completion {
        generation: u64,
        arm: bool,
        selected_key: Option<(String, String, String)>,
    },
    InspectorWatchLifecycle(InspectorWatchOperation),
}

#[derive(Clone)]
enum InspectorWatchOperation {
    Watch(String),
    Unwatch(String),
}

struct QueuedCompletion {
    generation: u64,
    arm: bool,
    selected_key: Option<(String, String, String)>,
    command: LiveCommand,
}

struct InspectorState {
    title: String,
    lines: Vec<String>,
    pinned: bool,
}

impl Default for InspectorState {
    fn default() -> Self {
        Self {
            title: "Inspect / details".to_string(),
            lines: vec!["Loading default live state...".to_string()],
            pinned: false,
        }
    }
}

struct LiveTui {
    client: LiveSessionClient,
    project_root: PathBuf,
    terminal: TerminalBuffer,
    input: InputMode,
    command_bar: Option<EditBuffer>,
    completion: CompletionState,
    transcript: VecDeque<String>,
    history: Vec<String>,
    history_cursor: usize,
    pending: BTreeMap<u64, PendingAction>,
    next_request_id: u64,
    next_edit_session_id: u64,
    completion_generation: u64,
    completion_in_flight: Option<u64>,
    queued_completion: Option<QueuedCompletion>,
    inspector: InspectorState,
    inspector_watch: Option<String>,
    inspector_watch_target: Option<String>,
    last_default_inspection: Instant,
    status: String,
    queued_ai_prompt: Option<String>,
    ai_run: Option<AiRun>,
    ai_audit: Option<AiAuditLog>,
    prompt: &'static str,
    quit: bool,
    last_regions: [Vec<u8>; 5],
    last_size: Option<(u16, u16)>,
}

impl LiveTui {
    fn new(client: LiveSessionClient, project_root: PathBuf) -> Self {
        Self {
            client,
            project_root,
            terminal: TerminalBuffer::new(),
            input: InputMode::Prompt(EditBuffer::default()),
            command_bar: None,
            completion: CompletionState::default(),
            transcript: VecDeque::from([
                "Stasis live workspace".to_string(),
                "Type :help, :edit SYMBOL, or :inspect PATH. Ctrl+Space arms completion."
                    .to_string(),
            ]),
            history: Vec::new(),
            history_cursor: 0,
            pending: BTreeMap::new(),
            next_request_id: TUI_REQUEST_START,
            next_edit_session_id: 0,
            completion_generation: 0,
            completion_in_flight: None,
            queued_completion: None,
            inspector: InspectorState::default(),
            inspector_watch: None,
            inspector_watch_target: None,
            last_default_inspection: Instant::now() - Duration::from_secs(1),
            status: "running".to_string(),
            queued_ai_prompt: None,
            ai_run: None,
            ai_audit: None,
            prompt: "stasis> ",
            quit: false,
            last_regions: std::array::from_fn(|_| Vec::new()),
            last_size: None,
        }
    }

    fn active_buffer(&self) -> &EditBuffer {
        self.command_bar
            .as_ref()
            .unwrap_or_else(|| self.input.buffer())
    }

    fn active_buffer_mut(&mut self) -> &mut EditBuffer {
        if self.command_bar.is_none() {
            if let InputMode::Definition(edit) = &mut self.input {
                edit.discard_confirm = false;
            }
        }
        self.active_buffer_for_render_mut()
    }

    fn active_buffer_for_render_mut(&mut self) -> &mut EditBuffer {
        self.command_bar
            .as_mut()
            .unwrap_or_else(|| self.input.buffer_mut())
    }

    fn next_request(&mut self) -> u64 {
        let request = self.next_request_id;
        self.next_request_id = self.next_request_id.saturating_add(1);
        request
    }

    fn completion_context(&self) -> CompletionContext {
        if self.command_bar.is_some() {
            return CompletionContext::default();
        }
        match &self.input {
            InputMode::Prompt(_) => self.terminal.completion_context(),
            InputMode::Definition(edit) => CompletionContext {
                owner: Some(edit.target.name.clone()),
                file: edit.target.file.clone(),
                owner_signature: edit.target.signature.clone(),
                source_offset: Some(edit.source_start.saturating_add(edit.buffer.cursor)),
                expected_type: None,
            },
        }
    }

    fn refresh_completion(&mut self, arm: bool) {
        if self.queued_ai_prompt.is_some() || self.ai_run.is_some() {
            return;
        }
        let buffer = self.active_buffer().text.clone();
        let cursor = self.active_buffer().cursor;
        let selected_key = self
            .completion
            .items
            .get(self.completion.selected)
            .map(completion_key);
        let mut context = self.completion_context();
        if context.expected_type.is_none() {
            context.expected_type = completion_expected_type(&buffer, cursor).unwrap_or_default();
        }
        self.completion_generation = self.completion_generation.saturating_add(1);
        self.queued_completion = Some(QueuedCompletion {
            generation: self.completion_generation,
            arm,
            selected_key,
            command: LiveCommand::Complete {
                buffer,
                cursor,
                limit: COMPLETION_LIMIT,
                context,
            },
        });
        self.dispatch_completion();
    }

    fn dispatch_completion(&mut self) {
        if self.queued_ai_prompt.is_some() || self.ai_run.is_some() {
            return;
        }
        if self.completion_in_flight.is_some() {
            return;
        }
        let Some(queued) = self.queued_completion.take() else {
            return;
        };
        let request_id = self.next_request();
        let action = PendingAction::Completion {
            generation: queued.generation,
            arm: queued.arm,
            selected_key: queued.selected_key,
        };
        match self
            .client
            .submit(LiveRequest::new(request_id, queued.command))
        {
            Ok(()) => {
                self.pending.insert(request_id, action);
                self.completion_in_flight = Some(request_id);
            }
            Err(error) => {
                self.completion.armed = false;
                self.status = format!("completion unavailable: {error}");
            }
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<(), String> {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);

        if control && key.code == KeyCode::Char('c') {
            if let Some(run) = &self.ai_run {
                run.canceled.store(true, Ordering::Release);
                self.status = "canceling AI request...".to_string();
                return Ok(());
            }
            if self.queued_ai_prompt.take().is_some() {
                self.status = "queued AI request canceled".to_string();
                return Ok(());
            }
            self.cancel_input();
            return Ok(());
        }
        if control && key.code == KeyCode::Char('k') {
            self.command_bar = Some(EditBuffer::from_text(":".to_string()));
            self.refresh_completion(true);
            return Ok(());
        }
        if control && matches!(key.code, KeyCode::Char(' ') | KeyCode::Char('p')) {
            self.refresh_completion(true);
            return Ok(());
        }
        if alt && matches!(key.code, KeyCode::Char('d' | 'p' | 'i')) {
            return self.run_workspace_action(key.code);
        }
        if control && key.code == KeyCode::Char('z') {
            self.active_buffer_mut().undo();
            self.refresh_completion(false);
            return Ok(());
        }
        if control && key.code == KeyCode::Char('y') {
            self.active_buffer_mut().redo();
            self.refresh_completion(false);
            return Ok(());
        }
        if control && key.code == KeyCode::Char('w') {
            self.close_definition();
            return Ok(());
        }
        if control && key.code == KeyCode::Enter {
            self.apply_definition()?;
            return Ok(());
        }

        match key.code {
            KeyCode::Esc => {
                if self.command_bar.take().is_some() {
                    self.status = "command bar closed".to_string();
                } else {
                    self.completion.armed = false;
                }
            }
            KeyCode::Tab => self.accept_completion(),
            KeyCode::Up if self.completion.armed => self.move_completion(-1),
            KeyCode::Down if self.completion.armed => self.move_completion(1),
            KeyCode::PageUp if self.completion.armed => self.move_completion(-8),
            KeyCode::PageDown if self.completion.armed => self.move_completion(8),
            KeyCode::Up => self.move_up(shift),
            KeyCode::Down => self.move_down(shift),
            KeyCode::Left => {
                self.active_buffer_mut().move_left(shift);
                self.completion.armed = false;
            }
            KeyCode::Right => {
                self.active_buffer_mut().move_right(shift);
                self.completion.armed = false;
            }
            KeyCode::Home => {
                self.active_buffer_mut().move_home(shift);
                self.completion.armed = false;
            }
            KeyCode::End => {
                self.active_buffer_mut().move_end(shift);
                self.completion.armed = false;
            }
            KeyCode::Backspace => {
                self.active_buffer_mut().backspace();
                let arm = !current_token(self.active_buffer()).is_empty();
                self.refresh_completion(arm);
            }
            KeyCode::Delete => {
                self.active_buffer_mut().delete();
                self.refresh_completion(false);
            }
            KeyCode::Enter => self.enter()?,
            KeyCode::Char(ch) if !control && !alt => {
                self.active_buffer_mut().insert_char(ch);
                let arm = ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | ':');
                self.refresh_completion(arm);
            }
            _ => {}
        }
        Ok(())
    }

    fn cancel_input(&mut self) {
        if self.command_bar.take().is_some() {
            self.status = "command bar canceled".to_string();
        } else if self.terminal.cancel_pending() {
            self.input = InputMode::Prompt(EditBuffer::default());
            self.prompt = "stasis> ";
            self.status = "multiline input canceled".to_string();
        } else {
            self.completion.armed = false;
            self.status = "nothing pending to cancel".to_string();
        }
        self.refresh_completion(false);
    }

    fn enter(&mut self) -> Result<(), String> {
        if self.command_bar.is_some() {
            let command = self.command_bar.take().expect("command bar").text;
            self.submit_line(command)?;
            return Ok(());
        }
        match &mut self.input {
            InputMode::Definition(_) => {
                if let InputMode::Definition(edit) = &mut self.input {
                    edit.discard_confirm = false;
                }
                self.input.buffer_mut().insert_newline();
                self.refresh_completion(false);
            }
            InputMode::Prompt(_) => {
                let line = std::mem::take(&mut self.input.buffer_mut().text);
                self.input.buffer_mut().cursor = 0;
                self.submit_line(line)?;
                if self.queued_ai_prompt.is_none() && self.ai_run.is_none() {
                    self.refresh_completion(false);
                }
            }
        }
        Ok(())
    }

    fn submit_line(&mut self, line: String) -> Result<(), String> {
        let line = line.trim_end().to_string();
        if line.trim().is_empty() {
            return Ok(());
        }
        self.push_transcript(format!("{}{}", self.prompt, line));
        self.history.push(line.clone());
        self.history_cursor = self.history.len();
        if line.trim_start().starts_with(":ai") {
            return self.handle_ai_command(&line);
        }
        if line.trim() == ":inspect" {
            self.replace_inspector_watch(None);
            self.inspector.pinned = false;
            self.request_default_inspection()?;
            return Ok(());
        }
        if line.trim_start().starts_with(":edit") {
            return self.open_definition(&line);
        }
        match self.terminal.feed_line(&line) {
            Ok(TerminalInput::Continue { prompt }) => {
                self.prompt = prompt;
                self.status = "multiline compatibility input; finish with :end".to_string();
            }
            Ok(TerminalInput::Request(request)) => {
                self.prompt = "stasis> ";
                if let Err(error) = self.client.submit(request) {
                    self.status = format!("live command queued failed: {error}");
                    self.push_transcript(format!("error: {error}"));
                }
            }
            Err(error) => self.push_transcript(format!("error: {error}")),
        }
        Ok(())
    }

    fn open_definition(&mut self, line: &str) -> Result<(), String> {
        let args = line.split_ascii_whitespace().collect::<Vec<_>>();
        if args.len() != 2 || args[0] != ":edit" {
            self.push_transcript("error: use :edit SYMBOL".to_string());
            return Ok(());
        }
        let target = edit_target_from_completion(args[1], &self.completion);
        let request_id = self.next_request();
        self.pending.insert(request_id, PendingAction::OpenEdit);
        if let Err(error) = self.client.submit(LiveRequest::new(
            request_id,
            LiveCommand::Read {
                name: target.name,
                kind: target.kind,
                file: target.file,
                owner: target.owner,
                signature: target.signature,
            },
        )) {
            self.pending.remove(&request_id);
            self.status = format!("definition open could not be queued: {error}");
            return Ok(());
        }
        self.status = format!("opening {}...", args[1]);
        Ok(())
    }

    fn apply_definition(&mut self) -> Result<(), String> {
        let (source, revision, session_id, target, expected_source_hash) = match &self.input {
            InputMode::Definition(edit) => (
                edit.buffer.text.clone(),
                edit.buffer.revision,
                edit.id,
                edit.target.clone(),
                edit.expected_source_hash.clone(),
            ),
            InputMode::Prompt(_) => {
                self.status = "Ctrl+Enter applies an open definition".to_string();
                return Ok(());
            }
        };
        let request_id = self.next_request();
        let command = LiveCommand::Edit {
            operation: LiveEditOperation::Update,
            target: target.clone(),
            source: Some(source.clone()),
            expected_source_hash: Some(expected_source_hash),
            preview: false,
            run_tests: true,
        };
        self.pending.insert(
            request_id,
            PendingAction::ApplyEdit {
                source,
                revision,
                session_id,
                target,
                submitted_at: Instant::now(),
            },
        );
        if let Err(error) = self.client.submit(LiveRequest::new(request_id, command)) {
            self.pending.remove(&request_id);
            self.status = format!("definition apply could not be queued: {error}");
            return Ok(());
        }
        self.status = "validating snapshot in background; editing remains available".to_string();
        Ok(())
    }

    fn close_definition(&mut self) {
        let InputMode::Definition(edit) = &mut self.input else {
            return;
        };
        if edit.buffer.text != edit.accepted_source && !edit.discard_confirm {
            edit.discard_confirm = true;
            self.status = "dirty definition: press Ctrl+W again to discard".to_string();
            return;
        }
        self.input = InputMode::Prompt(EditBuffer::default());
        self.status = "definition editor closed".to_string();
        self.refresh_completion(false);
    }

    fn run_workspace_action(&mut self, code: KeyCode) -> Result<(), String> {
        let expression = self.active_buffer().selected_or_token();
        if expression.is_empty() {
            self.status = "select text or place the cursor on an expression".to_string();
            return Ok(());
        }
        let command = match code {
            KeyCode::Char('d') => LiveCommand::Do {
                code: expression,
                preview: false,
            },
            KeyCode::Char('p') => LiveCommand::Print { expression },
            KeyCode::Char('i') => LiveCommand::Inspect { path: expression },
            _ => return Ok(()),
        };
        let request_id = self.next_request();
        if let Err(error) = self.client.submit(LiveRequest::new(request_id, command)) {
            self.status = format!("workspace action could not be queued: {error}");
        }
        Ok(())
    }

    fn move_completion(&mut self, delta: isize) {
        if self.completion.items.is_empty() {
            return;
        }
        self.completion.selected = self
            .completion
            .selected
            .saturating_add_signed(delta)
            .min(self.completion.items.len() - 1);
    }

    fn accept_completion(&mut self) {
        if !self.completion.armed
            || !suffix_is_whitespace(&self.active_buffer().text, self.active_buffer().cursor)
        {
            return;
        }
        let Some(item) = self.completion.items.get(self.completion.selected) else {
            return;
        };
        let text = item.text.clone();
        let start = self.completion.replacement_start;
        let end = self.completion.replacement_end;
        self.active_buffer_mut()
            .replace_completion(start, end, &text);
        self.completion.armed = false;
        self.refresh_completion(false);
    }

    fn move_up(&mut self, select: bool) {
        match &mut self.input {
            InputMode::Definition(edit) if self.command_bar.is_none() => {
                edit.buffer.move_vertical(-1, select)
            }
            _ if self.command_bar.is_none() => self.history_move(-1),
            _ => self.active_buffer_mut().move_vertical(-1, select),
        }
    }

    fn move_down(&mut self, select: bool) {
        match &mut self.input {
            InputMode::Definition(edit) if self.command_bar.is_none() => {
                edit.buffer.move_vertical(1, select)
            }
            _ if self.command_bar.is_none() => self.history_move(1),
            _ => self.active_buffer_mut().move_vertical(1, select),
        }
    }

    fn history_move(&mut self, delta: isize) {
        if self.history.is_empty() {
            return;
        }
        self.history_cursor = self
            .history_cursor
            .saturating_add_signed(delta)
            .min(self.history.len());
        let text = self
            .history
            .get(self.history_cursor)
            .cloned()
            .unwrap_or_default();
        self.input = InputMode::Prompt(EditBuffer::from_text(text));
        self.refresh_completion(false);
    }

    fn drain_responses(&mut self) -> Result<(), String> {
        self.drain_ai_events();
        if self.ai_run.is_some() {
            return Ok(());
        }
        while let Some(response) = self.client.try_receive()? {
            self.handle_response(response);
        }
        self.maybe_start_queued_ai();
        Ok(())
    }

    fn handle_ai_command(&mut self, line: &str) -> Result<(), String> {
        let argument = line
            .trim_start()
            .strip_prefix(":ai")
            .unwrap_or_default()
            .trim();
        if argument.eq_ignore_ascii_case("cancel") {
            if let Some(run) = &self.ai_run {
                run.canceled.store(true, Ordering::Release);
                self.status = "canceling AI request...".to_string();
            } else if self.queued_ai_prompt.take().is_some() {
                self.status = "queued AI request canceled".to_string();
            } else {
                self.status = "no AI request is active".to_string();
            }
            return Ok(());
        }
        if argument.eq_ignore_ascii_case("status") {
            self.push_transcript(if self.ai_run.is_some() {
                "AI: running through the installed Codex subscription".to_string()
            } else if self.queued_ai_prompt.is_some() {
                "AI: waiting for outstanding live responses".to_string()
            } else {
                "AI: idle; use :ai PROMPT".to_string()
            });
            return Ok(());
        }
        if argument.is_empty() {
            self.push_transcript("error: use :ai PROMPT, :ai status, or :ai cancel".to_string());
            return Ok(());
        }
        if self.ai_run.is_some() || self.queued_ai_prompt.is_some() {
            self.push_transcript("error: one AI request is already active".to_string());
            return Ok(());
        }
        self.completion.armed = false;
        self.queued_completion = None;
        self.queued_ai_prompt = Some(argument.to_string());
        self.status = "AI request queued; draining live UI responses...".to_string();
        self.maybe_start_queued_ai();
        Ok(())
    }

    fn maybe_start_queued_ai(&mut self) {
        if self.ai_run.is_some() || !self.pending.is_empty() {
            return;
        }
        let Some(prompt) = self.queued_ai_prompt.take() else {
            return;
        };
        match AiAuditLog::create(&self.project_root, &prompt) {
            Ok(log) => {
                self.push_transcript(format!("AI trace: {}", log.path.display()));
                self.push_transcript(format!("AI usage: {}", log.usage_path.display()));
                self.ai_audit = Some(log);
            }
            Err(error) => {
                self.status = format!("AI trace unavailable: {error}");
                self.push_transcript(format!("error: {error}"));
                return;
            }
        }
        let client = self.client.clone();
        let canceled = Arc::new(AtomicBool::new(false));
        let worker_canceled = canceled.clone();
        let (events_tx, events_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            let mut provider = CodexExecProvider::default();
            let mut tools = LiveAiTools::new(client);
            let progress = events_tx.clone();
            let result =
                load_ai_initial_context(&mut tools, &worker_canceled).and_then(|initial_context| {
                    let _ = progress.send(AiUiEvent::InitialContext(initial_context.clone()));
                    run_agent(
                        &mut provider,
                        &mut tools,
                        &prompt,
                        initial_context,
                        live_tool_specs(),
                        &worker_canceled,
                        move |event| {
                            let _ = progress.send(AiUiEvent::Progress(event));
                        },
                    )
                });
            let _ = events_tx.send(AiUiEvent::Finished(result));
        });
        self.ai_run = Some(AiRun {
            canceled,
            events: events_rx,
            worker: Some(worker),
        });
        self.status = "AI: starting installed Codex... Ctrl+C cancels".to_string();
        self.inspector.title = "AI working notes".to_string();
        self.inspector.lines = vec!["Starting a subscription-backed Codex turn...".to_string()];
        self.inspector.pinned = true;
    }

    fn drain_ai_events(&mut self) {
        let mut events = Vec::new();
        if let Some(run) = &self.ai_run {
            while let Ok(event) = run.events.try_recv() {
                events.push(event);
            }
        }
        let mut finished = None;
        for event in events {
            match event {
                AiUiEvent::InitialContext(initial_context) => {
                    if let Some(audit) = self.ai_audit.as_mut() {
                        if let Err(error) = audit.write(serde_json::json!({
                            "event": "initial_context",
                            "value": initial_context,
                        })) {
                            self.status = format!("AI trace failed: {error}");
                        }
                    }
                }
                AiUiEvent::Progress(AgentEvent::Turn { current, maximum }) => {
                    self.status = format!("AI turn {current}/{maximum}; Ctrl+C cancels");
                    self.audit(serde_json::json!({"event": "turn", "current": current, "maximum": maximum}));
                }
                AiUiEvent::Progress(AgentEvent::ProviderUsage(usage)) => {
                    if let Some(log) = &mut self.ai_audit {
                        if let Err(error) = log.write_usage(&usage) {
                            self.status = format!("AI usage trace disabled: {error}");
                        }
                    }
                }
                AiUiEvent::Progress(AgentEvent::WorkingNotes(notes)) => {
                    self.inspector.title = "AI working notes".to_string();
                    self.inspector.lines = notes.lines().map(str::to_string).collect();
                    if self.inspector.lines.is_empty() {
                        self.inspector
                            .lines
                            .push("AI supplied no displayable notes".to_string());
                    }
                    self.audit(serde_json::json!({"event": "working_notes", "text": notes}));
                }
                AiUiEvent::Progress(AgentEvent::ToolBatch(calls)) => {
                    let tools = calls
                        .iter()
                        .map(|call| call.tool.as_str())
                        .collect::<Vec<_>>();
                    self.status = format!("AI tools: {}", tools.join(", "));
                    self.push_transcript(format!("AI tools: {}", tools.join(", ")));
                    self.audit(serde_json::json!({
                        "event": "tool_calls",
                        "calls": calls.iter().map(audit_tool_call).collect::<Vec<_>>(),
                    }));
                }
                AiUiEvent::Progress(AgentEvent::Observations(observations)) => {
                    self.audit(serde_json::json!({
                        "event": "tool_observations",
                        "observations": observations.iter().map(audit_observation).collect::<Vec<_>>(),
                    }));
                }
                AiUiEvent::Progress(AgentEvent::Completed(summary)) => {
                    self.push_transcript(format!("AI: {summary}"));
                    self.audit(serde_json::json!({"event": "model_completed", "summary": summary}));
                }
                AiUiEvent::Finished(result) => finished = Some(result),
            }
        }
        if let Some(result) = finished {
            let mut run = self.ai_run.take().expect("finished AI run exists");
            if let Some(worker) = run.worker.take() {
                let _ = worker.join();
            }
            match result {
                Ok(summary) => {
                    self.status = "AI request completed and verified".to_string();
                    self.push_transcript(format!("AI complete: {summary}"));
                    self.audit(
                        serde_json::json!({"event": "finished", "ok": true, "summary": summary}),
                    );
                }
                Err(error) => {
                    self.status = "AI request failed".to_string();
                    self.push_transcript(format!("AI error: {error}"));
                    self.audit(
                        serde_json::json!({"event": "finished", "ok": false, "error": error}),
                    );
                }
            }
            self.ai_audit = None;
            self.inspector.pinned = false;
            let _ = self.request_default_inspection();
            self.refresh_completion(false);
        }
    }

    fn audit(&mut self, value: Value) {
        if let Some(log) = &mut self.ai_audit {
            if let Err(error) = log.write(value) {
                self.status = error;
                self.ai_audit = None;
            }
        }
    }

    fn request_default_inspection(&mut self) -> Result<(), String> {
        if self.queued_ai_prompt.is_some() || self.ai_run.is_some() {
            return Ok(());
        }
        if self.inspector.pinned
            || self
                .pending
                .values()
                .any(|action| matches!(action, PendingAction::DefaultInspect))
        {
            return Ok(());
        }
        let request_id = self.next_request();
        self.pending
            .insert(request_id, PendingAction::DefaultInspect);
        self.last_default_inspection = Instant::now();
        if let Err(error) = self.client.submit(LiveRequest::new(
            request_id,
            LiveCommand::InspectAll {
                limit: 32,
                concise: true,
            },
        )) {
            self.pending.remove(&request_id);
            self.status = format!("default inspection delayed by backpressure: {error}");
        }
        Ok(())
    }

    fn maybe_refresh_default_inspection(&mut self) -> Result<(), String> {
        if !self.inspector.pinned
            && self.last_default_inspection.elapsed() >= Duration::from_millis(250)
        {
            self.request_default_inspection()?;
        }
        Ok(())
    }

    fn handle_response(&mut self, response: LiveResponse) {
        if let Some(action) = self.pending.get(&response.request_id).cloned() {
            if matches!(
                response.kind.as_str(),
                "edit_preparing" | "completion_preparing"
            ) {
                if response.kind == "edit_preparing" {
                    self.status = "snapshot compiling and testing...".to_string();
                }
                return;
            }
            match action {
                PendingAction::OpenEdit => self.finish_open(response),
                PendingAction::ApplyEdit {
                    source,
                    revision,
                    session_id,
                    target,
                    submitted_at,
                } => {
                    self.finish_apply(response, source, revision, session_id, target, submitted_at);
                }
                PendingAction::DefaultInspect => self.finish_default_inspection(response),
                PendingAction::Completion {
                    generation,
                    arm,
                    selected_key,
                } => self.finish_completion(response, generation, arm, selected_key),
                PendingAction::InspectorWatchLifecycle(operation) => {
                    self.pending.remove(&response.request_id);
                    if response.ok {
                        match operation {
                            InspectorWatchOperation::Watch(path)
                                if response.kind == "watch_added" =>
                            {
                                self.inspector_watch = Some(path);
                            }
                            InspectorWatchOperation::Unwatch(path)
                                if response.kind == "watch_removed"
                                    && self.inspector_watch.as_deref() == Some(path.as_str()) =>
                            {
                                self.inspector_watch = None;
                            }
                            _ => {
                                self.inspector_watch_target = self.inspector_watch.clone();
                                self.status = format!(
                                    "inspector live refresh returned an unexpected response: {}",
                                    format_live_response(&response)
                                );
                                return;
                            }
                        }
                        self.advance_inspector_watch();
                    } else {
                        self.inspector_watch_target = self.inspector_watch.clone();
                        self.status = format_live_response(&response);
                    }
                }
            }
            return;
        }

        if response.kind == "quitting" {
            self.quit = true;
        }
        if response.kind == "inspection" && response.ok {
            let path = response
                .data
                .as_ref()
                .and_then(|data| data.get("path"))
                .and_then(|value| value.as_str())
                .unwrap_or("inspection")
                .to_string();
            self.inspector.title = path.clone();
            self.inspector.lines = vec![format_live_response(&response)];
            self.inspector.pinned = true;
            self.status = "live inspection pinned".to_string();
            self.replace_inspector_watch(Some(path));
            return;
        }
        if response.kind == "watch" && response.ok {
            let path = response
                .data
                .as_ref()
                .and_then(|data| data.get("path"))
                .and_then(Value::as_str);
            if path == self.inspector_watch.as_deref() {
                self.inspector.lines = vec![format_live_response(&response)];
                return;
            }
        }
        if response.kind == "watch_removed" && response.ok {
            let still_watched = response
                .data
                .as_ref()
                .and_then(|data| data.get("watches"))
                .and_then(Value::as_array)
                .is_some_and(|paths| {
                    paths
                        .iter()
                        .any(|path| path.as_str() == self.inspector_watch.as_deref())
                });
            if !still_watched && self.inspector_watch.take().is_some() {
                self.inspector.pinned = false;
                if let Err(error) = self.request_default_inspection() {
                    self.status = format!("default inspection unavailable: {error}");
                }
            }
        }
        self.push_transcript(format_live_response(&response));
    }

    fn replace_inspector_watch(&mut self, next: Option<String>) {
        self.inspector_watch_target = next;
        self.advance_inspector_watch();
    }

    fn advance_inspector_watch(&mut self) {
        if self
            .pending
            .values()
            .any(|action| matches!(action, PendingAction::InspectorWatchLifecycle(_)))
            || self.inspector_watch == self.inspector_watch_target
        {
            return;
        }
        if let Some(previous) = self.inspector_watch.clone() {
            self.submit_inspector_watch_command(
                LiveCommand::Unwatch {
                    path: Some(previous.clone()),
                },
                InspectorWatchOperation::Unwatch(previous),
            );
        } else if let Some(path) = self.inspector_watch_target.clone() {
            self.submit_inspector_watch_command(
                LiveCommand::Watch { path: path.clone() },
                InspectorWatchOperation::Watch(path),
            );
        }
    }

    fn submit_inspector_watch_command(
        &mut self,
        command: LiveCommand,
        operation: InspectorWatchOperation,
    ) {
        let request_id = self.next_request();
        match self.client.submit(LiveRequest::new(request_id, command)) {
            Ok(()) => {
                self.pending.insert(
                    request_id,
                    PendingAction::InspectorWatchLifecycle(operation),
                );
            }
            Err(error) => {
                self.status = format!("inspector live refresh unavailable: {error}");
            }
        }
    }

    fn finish_completion(
        &mut self,
        response: LiveResponse,
        generation: u64,
        arm: bool,
        selected_key: Option<(String, String, String)>,
    ) {
        self.pending.remove(&response.request_id);
        if self.completion_in_flight == Some(response.request_id) {
            self.completion_in_flight = None;
        }
        if generation == self.completion_generation {
            if !response.ok || response.kind != "completion" || response.truncated {
                self.completion.armed = false;
                self.status = format!(
                    "completion unavailable: {}",
                    format_live_response(&response)
                );
            } else {
                match serde_json::from_value::<CompletionQuery>(
                    response.data.unwrap_or(Value::Null),
                ) {
                    Ok(query) => {
                        self.completion.replacement_start = query.replacement_start;
                        self.completion.replacement_end = query.replacement_end;
                        self.completion.truncated = query.truncated;
                        self.completion.items = query.items;
                        self.completion.selected = selected_key
                            .and_then(|key| {
                                self.completion
                                    .items
                                    .iter()
                                    .position(|item| completion_key(item) == key)
                            })
                            .unwrap_or(0);
                        self.completion.armed =
                            arm && suffix_is_whitespace(
                                &self.active_buffer().text,
                                self.active_buffer().cursor,
                            ) && !self.completion.items.is_empty();
                    }
                    Err(error) => {
                        self.completion.armed = false;
                        self.status = format!("completion unavailable: {error}");
                    }
                }
            }
        }
        self.dispatch_completion();
    }

    fn finish_default_inspection(&mut self, response: LiveResponse) {
        self.pending.remove(&response.request_id);
        if self.inspector.pinned {
            return;
        }
        if !response.ok || response.kind != "state_inspection" {
            self.inspector.lines = vec![format_live_response(&response)];
            return;
        }
        let Some(items) = response
            .data
            .as_ref()
            .and_then(|data| data.get("items"))
            .and_then(|items| items.as_array())
        else {
            self.inspector.lines = vec!["Default state response omitted items.".to_string()];
            return;
        };
        self.inspector.title = "Live state (default)".to_string();
        self.inspector.lines = items.iter().map(format_state_item).collect();
        if let Some(data) = response.data.as_ref() {
            append_state_tree_lines(data, &mut self.inspector.lines);
        }
        if self.inspector.lines.is_empty() {
            self.inspector.lines = vec!["No concise state values; use :inspect PATH.".to_string()];
        }
        if response
            .data
            .as_ref()
            .and_then(|data| data.get("truncated"))
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
        {
            self.inspector
                .lines
                .push("... bounded view; use :inspect PATH".to_string());
        }
    }

    fn finish_open(&mut self, response: LiveResponse) {
        self.pending.remove(&response.request_id);
        if !response.ok {
            self.push_transcript(format_live_response(&response));
            self.status = "definition open failed".to_string();
            return;
        }
        let item = match response
            .data
            .clone()
            .ok_or_else(|| "symbol response omitted data".to_string())
            .and_then(|data| {
                serde_json::from_value::<WorkshopSourceItem>(data)
                    .map_err(|error| error.to_string())
            }) {
            Ok(item) => item,
            Err(error) => {
                self.push_transcript(format!("error: {error}"));
                return;
            }
        };
        let kind = match item.kind {
            WorkshopSourceItemKind::Function => "function",
            WorkshopSourceItemKind::Struct => "struct",
            WorkshopSourceItemKind::Test => "test",
            WorkshopSourceItemKind::Imports => "imports",
            WorkshopSourceItemKind::Globals => "globals",
        };
        let source_start = item
            .source_spans
            .first()
            .map_or(0, |span| span.start as usize);
        let target = LiveSymbolTarget {
            name: item.name.clone(),
            kind: Some(kind.to_string()),
            file: Some(item.file.clone()),
            owner: item.owner.clone(),
            signature: Some(item.signature.clone()),
        };
        self.next_edit_session_id = self.next_edit_session_id.saturating_add(1);
        self.input = InputMode::Definition(EditSession {
            id: self.next_edit_session_id,
            target,
            expected_source_hash: item.source_hash,
            accepted_source: item.source.clone(),
            source_start,
            buffer: EditBuffer::from_text(item.source),
            discard_confirm: false,
        });
        self.status = format!("editing {} [{}]", item.name, item.file);
        self.refresh_completion(false);
    }

    fn finish_apply(
        &mut self,
        response: LiveResponse,
        source: String,
        revision: u64,
        session_id: u64,
        target: LiveSymbolTarget,
        submitted_at: Instant,
    ) {
        self.pending.remove(&response.request_id);
        if !response.ok {
            self.status = match &self.input {
                InputMode::Definition(edit) if edit.id == session_id && edit.target == target => {
                    "apply failed; buffer remains open".to_string()
                }
                _ => format!("apply failed for {}; current editor unchanged", target.name),
            };
            self.push_transcript(format_apply_error(&response));
            return;
        }
        let mut close_editor = false;
        let same_editor = matches!(
            &self.input,
            InputMode::Definition(edit) if edit.id == session_id && edit.target == target
        );
        if same_editor {
            let InputMode::Definition(edit) = &mut self.input else {
                unreachable!("same_editor requires a definition");
            };
            edit.accepted_source = source.clone();
            edit.expected_source_hash = workshop_source_hash(&source);
            edit.discard_confirm = false;
            close_editor = edit.buffer.revision == revision && edit.buffer.text == source;
            self.status = if close_editor {
                "snapshot applied; editor closed".to_string()
            } else {
                "snapshot applied; later edits remain dirty".to_string()
            };
        } else {
            self.status = format!(
                "snapshot applied for {}; current editor unchanged",
                target.name
            );
        }
        if close_editor {
            self.input = InputMode::Prompt(EditBuffer::default());
        }
        self.push_transcript(format_apply_confirmation(&response, submitted_at.elapsed()));
        self.refresh_completion(false);
    }

    fn push_transcript(&mut self, line: String) {
        for line in line.lines() {
            self.transcript.push_back(line.to_string());
        }
        while self.transcript.len() > MAX_TRANSCRIPT_LINES {
            self.transcript.pop_front();
        }
    }

    fn render(&mut self) -> Result<(), String> {
        let (width, height) = terminal::size().unwrap_or((120, 40));
        let layout = Layout::new(
            width,
            height,
            matches!(self.input, InputMode::Definition(_)),
        );
        let mut regions: [Vec<u8>; 5] = std::array::from_fn(|_| Vec::new());
        draw_box(&mut regions[0], layout.transcript, "Transcript")?;
        draw_transcript(&mut regions[0], layout.transcript, &self.transcript)?;
        draw_box(&mut regions[1], layout.editor, self.editor_title().as_str())?;
        let ghost = self.ghost_text();
        let command_bar = self.command_bar.is_some();
        let definition = matches!(self.input, InputMode::Definition(_));
        let prompt = self.prompt;
        let cursor = draw_editor(
            &mut regions[1],
            layout.editor,
            self.active_buffer_for_render_mut(),
            ghost,
            command_bar,
            definition,
            prompt,
        )?;
        draw_box(&mut regions[2], layout.right, "Completions / inspect")?;
        draw_right_panel(
            &mut regions[2],
            layout.right,
            &self.completion,
            &self.inspector,
        )?;
        let status = format!(
            "{} | Tab complete | Ctrl+Space arm | Ctrl+K commands | Ctrl+Enter apply",
            self.status
        );
        write_at(
            &mut regions[3],
            0,
            height.saturating_sub(1),
            width,
            &format!("{status:<width$}", width = width as usize),
            Color::DarkGrey,
        )?;
        if let Some((x, y)) = cursor {
            queue!(regions[4], MoveTo(x, y), Show)
                .map_err(|error| format!("failed positioning live TUI cursor: {error}"))?;
        } else {
            queue!(regions[4], Hide)
                .map_err(|error| format!("failed hiding live TUI cursor: {error}"))?;
        }
        let resized = self.last_size != Some((width, height));
        let changed: [bool; 5] =
            std::array::from_fn(|index| resized || regions[index] != self.last_regions[index]);
        if !changed.iter().any(|changed| *changed) {
            return Ok(());
        }
        let mut update = Vec::new();
        queue!(update, BeginSynchronizedUpdate, Hide)
            .map_err(|error| format!("failed starting live TUI update: {error}"))?;
        if resized {
            queue!(update, Clear(ClearType::All))
                .map_err(|error| format!("failed resizing live TUI: {error}"))?;
        }
        for index in 0..4 {
            if changed[index] {
                update.extend_from_slice(&regions[index]);
            }
        }
        update.extend_from_slice(&regions[4]);
        queue!(update, EndSynchronizedUpdate)
            .map_err(|error| format!("failed ending live TUI update: {error}"))?;
        let mut stdout = io::stdout();
        if let Err(error) = stdout.write_all(&update) {
            let _ = execute!(stdout, EndSynchronizedUpdate);
            return Err(format!("failed drawing live TUI update: {error}"));
        }
        if let Err(error) = stdout.flush() {
            let _ = execute!(stdout, EndSynchronizedUpdate);
            return Err(format!("failed flushing live TUI: {error}"));
        }
        self.last_regions = regions;
        self.last_size = Some((width, height));
        Ok(())
    }

    fn editor_title(&self) -> String {
        if self.command_bar.is_some() {
            return "Command bar".to_string();
        }
        match &self.input {
            InputMode::Prompt(_) => "Input".to_string(),
            InputMode::Definition(edit) => format!(
                "Edit {}{}",
                edit.target.name,
                if edit.buffer.text == edit.accepted_source {
                    ""
                } else {
                    " *"
                }
            ),
        }
    }

    fn ghost_text(&self) -> String {
        ghost_suffix(self.active_buffer(), &self.completion).unwrap_or_default()
    }
}

fn format_apply_confirmation(response: &LiveResponse, elapsed: Duration) -> String {
    let elapsed_ms = elapsed.as_nanos().saturating_add(999_999) / 1_000_000;
    let Some(data) = response.data.as_ref() else {
        return format!(
            "Hot swapped <= {elapsed_ms} ms | {}",
            format_live_response(response)
        );
    };
    let tests = data
        .get("tests")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    format!("Hot swapped <= {elapsed_ms} ms | tests {tests}")
}

fn format_apply_error(response: &LiveResponse) -> String {
    let Some(error) = response.error.as_deref() else {
        return format_live_response(response);
    };
    let message = error
        .lines()
        .next()
        .unwrap_or(error)
        .rsplit_once(": ")
        .map_or(error, |(_, message)| message);
    format!("error: {message}")
}

fn completion_key(item: &CompletionItem) -> (String, String, String) {
    (item.text.clone(), item.kind.clone(), item.detail.clone())
}

struct LiveAiTools {
    client: LiveSessionClient,
    next_request_id: u64,
    last_write: Option<LiveResponse>,
    reference_search_ready: bool,
}

impl LiveAiTools {
    fn new(client: LiveSessionClient) -> Self {
        Self {
            client,
            next_request_id: AI_REQUEST_START,
            last_write: None,
            reference_search_ready: false,
        }
    }

    fn request(
        &mut self,
        command: LiveCommand,
        canceled: &AtomicBool,
    ) -> Result<LiveResponse, String> {
        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.saturating_add(1);
        self.client.submit(LiveRequest::new(request_id, command))?;
        loop {
            if canceled.load(Ordering::Acquire) {
                let _ = self.client.submit(LiveRequest::new(
                    self.next_request_id,
                    LiveCommand::Cancel { request_id },
                ));
                return Err("AI request canceled".to_string());
            }
            let response = self.client.receive_timeout(Duration::from_millis(100));
            let response = match response {
                Ok(response) => response,
                Err(error) if error.contains("timed out") => continue,
                Err(error) => return Err(error),
            };
            if response.request_id != request_id {
                continue;
            }
            if matches!(
                response.kind.as_str(),
                "edit_preparing" | "completion_preparing"
            ) {
                continue;
            }
            return Ok(response);
        }
    }

    fn execute_read(&mut self, call: &ToolCall, canceled: &AtomicBool) -> ToolObservation {
        let args = call.args.as_object().expect("validated tool args");
        let command = match call.tool.as_str() {
            "list_symbols" => LiveCommand::Symbols {
                query: string_arg(args, "query"),
                kind: string_arg(args, "kind"),
                files: match string_array_arg(args, "files") {
                    Ok(files) => files,
                    Err(error) => return ToolObservation::error(&call.tool, error),
                },
                owner: string_arg(args, "owner"),
                page: usize_arg(args, "page").unwrap_or(0).min(u32::MAX as usize) as u32,
                limit: usize_arg(args, "limit").unwrap_or(32).min(64),
            },
            "read_symbol" => LiveCommand::Read {
                name: string_arg(args, "name").unwrap_or_default(),
                kind: string_arg(args, "kind"),
                file: string_arg(args, "file"),
                owner: string_arg(args, "owner"),
                signature: string_arg(args, "signature"),
            },
            "find_references" => LiveCommand::References {
                symbol: string_arg(args, "symbol").unwrap_or_default(),
                limit: usize_arg(args, "limit").unwrap_or(128).min(256),
            },
            "inspect_runtime_state" => LiveCommand::InspectAll {
                limit: 64,
                concise: true,
            },
            "run_frame" => LiveCommand::Step { ticks: 1 },
            _ => return ToolObservation::error(&call.tool, "tool is not a read operation"),
        };
        match self.request(command, canceled) {
            Ok(response) if response.ok => {
                if call.tool == "find_references" {
                    self.reference_search_ready = true;
                }
                ToolObservation::result(&call.tool, response.data.unwrap_or(Value::Null))
            }
            Ok(response) => ToolObservation::error(&call.tool, format_live_response(&response)),
            Err(error) => ToolObservation::error(&call.tool, error),
        }
    }

    fn execute_writes(
        &mut self,
        calls: &[&ToolCall],
        canceled: &AtomicBool,
    ) -> Vec<ToolObservation> {
        if !self.reference_search_ready {
            return calls
                .iter()
                .map(|call| {
                    ToolObservation::error(
                        &call.tool,
                        "run find_references for a behavior-bearing symbol before a live AI write",
                    )
                })
                .collect();
        }
        let edits = calls
            .iter()
            .map(|call| {
                let args = call.args.as_object().expect("validated tool args");
                let operation = if call.tool == "delete_symbol" {
                    LiveEditOperation::Delete
                } else if string_arg(args, "operation").as_deref() == Some("add") {
                    LiveEditOperation::Add
                } else {
                    LiveEditOperation::Update
                };
                LiveEdit {
                    operation,
                    target: LiveSymbolTarget {
                        name: string_arg(args, "name").unwrap_or_default(),
                        kind: string_arg(args, "kind"),
                        file: string_arg(args, "file"),
                        owner: string_arg(args, "owner"),
                        signature: string_arg(args, "signature"),
                    },
                    source: string_arg(args, "new_source"),
                    expected_source_hash: string_arg(args, "expected_source_hash"),
                }
            })
            .collect();
        match self.request(
            LiveCommand::EditBatch {
                edits,
                preview: false,
                run_tests: true,
            },
            canceled,
        ) {
            Ok(response) => {
                let applied = response.ok && response.kind == "edit_applied";
                if applied {
                    self.last_write = Some(response.clone());
                    self.reference_search_ready = false;
                }
                if applied {
                    calls
                        .iter()
                        .enumerate()
                        .map(|(index, call)| {
                            applied_write_observation(call, index, calls.len(), &response.data)
                        })
                        .collect()
                } else {
                    let error = if response.ok && response.kind == "edit_preview" {
                        "layout-changing AI edit was validated but requires explicit user :apply approval"
                            .to_string()
                    } else {
                        format_live_response(&response)
                    };
                    failed_write_observations(calls, error)
                }
            }
            Err(error) => failed_write_observations(calls, error),
        }
    }
}

fn failed_write_observations(calls: &[&ToolCall], error: String) -> Vec<ToolObservation> {
    calls
        .iter()
        .enumerate()
        .map(|(index, call)| {
            ToolObservation::error(
                &call.tool,
                if index == 0 {
                    error.clone()
                } else {
                    "atomic write batch failed; see the first write observation".to_string()
                },
            )
        })
        .collect()
}

fn applied_write_observation(
    call: &ToolCall,
    index: usize,
    batch_size: usize,
    transaction: &Option<Value>,
) -> ToolObservation {
    let transaction = compact_write_transaction(transaction.as_ref());
    let result = if index == 0 {
        serde_json::json!({
            "status": "compiled_tested_applied",
            "batch_size": batch_size,
            "write": transaction,
        })
    } else {
        serde_json::json!({
            "status": "compiled_tested_applied",
            "batch_size": batch_size,
            "write_receipt": transaction.get("receipt").cloned().unwrap_or(Value::Null),
        })
    };
    ToolObservation::result(&call.tool, result)
}

fn compact_write_transaction(transaction: Option<&Value>) -> Value {
    let Some(transaction) = transaction else {
        return Value::Null;
    };
    serde_json::json!({
        "receipt": transaction.get("receipt").cloned().unwrap_or(Value::Null),
        "tests": transaction.get("tests").cloned().unwrap_or(Value::Null),
        "changed_symbols": transaction.pointer("/plan/reload/changed_symbols").cloned().unwrap_or_else(|| serde_json::json!([])),
        "expected_reload": transaction.pointer("/plan/reload/expected_reload").cloned().unwrap_or(Value::Null),
        "state_layout_compatible": transaction.pointer("/swap/state_layout_compatible").cloned().unwrap_or(Value::Null),
        "requires_explicit_apply": transaction.pointer("/swap/requires_explicit_apply").cloned().unwrap_or(Value::Null),
        "warnings": transaction.pointer("/swap/warnings").cloned().unwrap_or_else(|| serde_json::json!([])),
    })
}

fn contiguous_write_range(calls: &[ToolCall]) -> Result<Option<std::ops::Range<usize>>, String> {
    let write_indexes = calls
        .iter()
        .enumerate()
        .filter_map(|(index, call)| {
            matches!(call.tool.as_str(), "write_symbol" | "delete_symbol").then_some(index)
        })
        .collect::<Vec<_>>();
    let Some(first) = write_indexes.first().copied() else {
        return Ok(None);
    };
    let last = write_indexes.last().copied().expect("write index");
    if write_indexes.len() != last - first + 1 {
        return Err(
            "write_symbol and delete_symbol calls must be contiguous so their atomic order is unambiguous"
                .to_string(),
        );
    }
    Ok(Some(first..last + 1))
}

impl ToolExecutor for LiveAiTools {
    fn execute(&mut self, calls: &[ToolCall], canceled: &AtomicBool) -> Vec<ToolObservation> {
        let write_range = match contiguous_write_range(calls) {
            Ok(range) => range,
            Err(error) => {
                return calls
                    .iter()
                    .map(|call| ToolObservation::error(&call.tool, error.clone()))
                    .collect()
            }
        };
        let mut observations = Vec::with_capacity(calls.len());
        let mut index = 0;
        while index < calls.len() {
            if write_range
                .as_ref()
                .is_some_and(|range| range.start == index)
            {
                let range = write_range.as_ref().expect("write range");
                let writes = calls[range.clone()].iter().collect::<Vec<_>>();
                observations.extend(self.execute_writes(&writes, canceled));
                index = range.end;
            } else {
                observations.push(self.execute_read(&calls[index], canceled));
                index += 1;
            }
        }
        observations
    }

    fn validate_completion(&self) -> Result<(), String> {
        match &self.last_write {
            Some(response)
                if response.ok
                    && response.kind == "edit_applied"
                    && response
                        .data
                        .as_ref()
                        .and_then(|data| data.get("tests"))
                        .and_then(Value::as_str)
                        == Some("passed") =>
            {
                Ok(())
            }
            _ => Err(
                "complete the requested change with one atomic write that compiles and passes project tests"
                    .to_string(),
            ),
        }
    }
}

fn string_arg(args: &serde_json::Map<String, Value>, name: &str) -> Option<String> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|value| !value.trim().is_empty())
}

fn usize_arg(args: &serde_json::Map<String, Value>, name: &str) -> Option<usize> {
    args.get(name)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
}

fn string_array_arg(
    args: &serde_json::Map<String, Value>,
    name: &str,
) -> Result<Vec<String>, String> {
    let Some(value) = args.get(name) else {
        return Ok(Vec::new());
    };
    let values = value
        .as_array()
        .ok_or_else(|| format!("{name} must be an array of project-relative paths"))?;
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .ok_or_else(|| format!("{name} must contain non-empty strings"))
        })
        .collect()
}

fn audit_tool_call(call: &ToolCall) -> Value {
    serde_json::to_value(call).expect("tool calls serialize")
}

fn audit_observation(observation: &ToolObservation) -> Value {
    serde_json::to_value(observation).expect("tool observations serialize")
}

fn edit_target_from_completion(symbol: &str, completion: &CompletionState) -> LiveSymbolTarget {
    let supported = |item: &&CompletionItem| {
        item.text == symbol && matches!(item.kind.as_str(), "function" | "struct" | "test")
    };
    let matches = completion
        .items
        .iter()
        .filter(supported)
        .collect::<Vec<_>>();
    let selected = completion
        .items
        .get(completion.selected)
        .filter(|item| supported(item));
    let item = selected.or_else(|| (matches.len() == 1).then_some(matches[0]));
    let Some(item) = item else {
        return LiveSymbolTarget {
            name: symbol.to_string(),
            kind: None,
            file: None,
            owner: None,
            signature: None,
        };
    };
    if let Some(selector) = item.selector.as_ref() {
        return selector.clone();
    }
    LiveSymbolTarget {
        name: symbol.to_string(),
        kind: Some(item.kind.clone()),
        file: item.source.clone(),
        owner: None,
        signature: None,
    }
}

fn format_state_item(item: &Value) -> String {
    let path = item.get("path").and_then(Value::as_str).unwrap_or("value");
    let depth = path.matches('.').count();
    let name = path.rsplit('.').next().unwrap_or(path);
    format!(
        "{}{} = {}",
        "  ".repeat(depth),
        name,
        scalar_text(item.get("value").unwrap_or(&Value::Null))
    )
}

fn append_state_tree_lines(data: &Value, lines: &mut Vec<String>) {
    if let Some(collections) = data.get("collections").and_then(Value::as_array) {
        for collection in collections {
            let path = collection
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or("collection");
            let active = collection
                .get("active_count")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let capacity = collection
                .get("capacity")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            lines.push(format!("{path} [{active}/{capacity}]"));
            if let Some(fields) = collection.get("fields").and_then(Value::as_array) {
                for field in fields {
                    let name = field
                        .get("field")
                        .and_then(Value::as_str)
                        .filter(|name| !name.is_empty())
                        .unwrap_or("element");
                    let static_type = field
                        .get("type_name")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown");
                    lines.push(format!("  {name}: {static_type}"));
                }
            }
        }
    }
    if let Some(structs) = data.get("structs").and_then(Value::as_array) {
        for state_struct in structs {
            let path = state_struct
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or("struct");
            let name = state_struct
                .get("type_name")
                .and_then(Value::as_str)
                .unwrap_or("struct");
            lines.push(format!("{path}: {name}"));
        }
    }
    if let Some(memory) = data.get("memory") {
        let bytes = memory
            .get("total_capacity_bytes")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let snapshot = memory
            .get("snapshot_bytes")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        lines.push(format!("memory: {bytes} bytes; snapshot: {snapshot} bytes"));
    }
}

fn current_token(buffer: &EditBuffer) -> &str {
    let start = buffer.text[..buffer.cursor]
        .rfind(|ch: char| !(ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | ':')))
        .map_or(0, |index| index + 1);
    &buffer.text[start..buffer.cursor]
}

fn suffix_is_whitespace(text: &str, cursor: usize) -> bool {
    text.get(cursor..)
        .is_some_and(|suffix| suffix.chars().all(char::is_whitespace))
}

fn ghost_suffix(buffer: &EditBuffer, completion: &CompletionState) -> Option<String> {
    if !completion.armed || !suffix_is_whitespace(&buffer.text, buffer.cursor) {
        return None;
    }
    let item = completion.items.get(completion.selected)?;
    let typed = buffer
        .text
        .get(completion.replacement_start..buffer.cursor)?;
    item.text
        .strip_prefix(typed)
        .filter(|suffix| !suffix.is_empty())
        .map(str::to_string)
}

#[derive(Clone, Copy)]
struct Rect {
    x: u16,
    y: u16,
    width: u16,
    height: u16,
}

struct Layout {
    transcript: Rect,
    editor: Rect,
    right: Rect,
}

impl Layout {
    fn new(width: u16, height: u16, editing: bool) -> Self {
        let usable_height = height.saturating_sub(1);
        if usable_height < 6 || width < 20 {
            return Self {
                transcript: Rect {
                    x: 0,
                    y: 0,
                    width: 0,
                    height: 0,
                },
                editor: Rect {
                    x: 0,
                    y: 0,
                    width,
                    height: usable_height,
                },
                right: Rect {
                    x: 0,
                    y: usable_height,
                    width: 0,
                    height: 0,
                },
            };
        }
        if width >= 60 {
            let left_width = (width * 3 / 5).max(40);
            let preferred_editor_height = if editing {
                (usable_height * 3 / 5).max(4)
            } else {
                5
            };
            let editor_height = preferred_editor_height.min(usable_height.saturating_sub(2));
            let transcript_height = usable_height.saturating_sub(editor_height);
            Self {
                transcript: Rect {
                    x: 0,
                    y: 0,
                    width: left_width,
                    height: transcript_height,
                },
                editor: Rect {
                    x: 0,
                    y: transcript_height,
                    width: left_width,
                    height: editor_height,
                },
                right: Rect {
                    x: left_width,
                    y: 0,
                    width: width.saturating_sub(left_width),
                    height: usable_height,
                },
            }
        } else {
            let preferred_left_height = if editing {
                usable_height * 2 / 3
            } else {
                usable_height / 2
            };
            let left_height = preferred_left_height.clamp(3, usable_height);
            let preferred_editor_height = if editing {
                (left_height * 3 / 5).max(3)
            } else {
                3
            };
            let editor_height = preferred_editor_height.min(left_height);
            let transcript_height = left_height.saturating_sub(editor_height);
            Self {
                transcript: Rect {
                    x: 0,
                    y: 0,
                    width,
                    height: transcript_height,
                },
                editor: Rect {
                    x: 0,
                    y: left_height.saturating_sub(editor_height),
                    width,
                    height: editor_height,
                },
                right: Rect {
                    x: 0,
                    y: left_height,
                    width,
                    height: usable_height.saturating_sub(left_height),
                },
            }
        }
    }
}

fn draw_box(stdout: &mut impl Write, rect: Rect, title: &str) -> Result<(), String> {
    if rect.width < 2 || rect.height < 2 {
        return Ok(());
    }
    let horizontal = "-".repeat(rect.width.saturating_sub(2) as usize);
    write_at(
        stdout,
        rect.x,
        rect.y,
        rect.width,
        &format!("+{horizontal}+"),
        Color::DarkGrey,
    )?;
    for row in 1..rect.height.saturating_sub(1) {
        write_at(
            stdout,
            rect.x,
            rect.y + row,
            rect.width,
            &format!("|{}|", " ".repeat(rect.width.saturating_sub(2) as usize)),
            Color::DarkGrey,
        )?;
    }
    write_at(
        stdout,
        rect.x,
        rect.y + rect.height.saturating_sub(1),
        rect.width,
        &format!("+{horizontal}+"),
        Color::DarkGrey,
    )?;
    write_at(
        stdout,
        rect.x + 2,
        rect.y,
        rect.width.saturating_sub(4),
        title,
        Color::Cyan,
    )
}

fn draw_transcript(
    stdout: &mut impl Write,
    rect: Rect,
    transcript: &VecDeque<String>,
) -> Result<(), String> {
    let rows = rect.height.saturating_sub(2) as usize;
    let start = transcript.len().saturating_sub(rows);
    for (row, line) in transcript.iter().skip(start).take(rows).enumerate() {
        write_at(
            stdout,
            rect.x + 1,
            rect.y + 1 + row as u16,
            rect.width.saturating_sub(2),
            line,
            if line.starts_with("error:") {
                Color::Red
            } else {
                Color::White
            },
        )?;
    }
    Ok(())
}

fn draw_editor(
    stdout: &mut impl Write,
    rect: Rect,
    buffer: &mut EditBuffer,
    ghost: String,
    command_bar: bool,
    definition: bool,
    prompt: &str,
) -> Result<Option<(u16, u16)>, String> {
    if rect.width < 3 || rect.height < 3 {
        return Ok(None);
    }
    let ranges = line_ranges(&buffer.text);
    let (cursor_line, cursor_column) = line_column(&buffer.text, buffer.cursor);
    let visible_rows = rect.height.saturating_sub(2) as usize;
    let start_line = update_editor_scroll(buffer, cursor_line, ranges.len(), visible_rows);
    let prefix = if definition || command_bar {
        ""
    } else {
        prompt
    };
    for (screen_row, line_index) in (start_line..ranges.len()).take(visible_rows).enumerate() {
        let range = ranges[line_index].clone();
        let line = &buffer.text[range];
        let y = rect.y + 1 + screen_row as u16;
        if definition {
            draw_syntax_line(stdout, rect.x + 1, y, rect.width.saturating_sub(2), line)?;
        } else {
            write_at(
                stdout,
                rect.x + 1,
                y,
                rect.width.saturating_sub(2),
                &format!("{prefix}{line}"),
                Color::White,
            )?;
        }
        if line_index == cursor_line && !ghost.is_empty() {
            write_at(
                stdout,
                rect.x + 1 + prefix.chars().count() as u16 + cursor_column as u16,
                y,
                rect.width.saturating_sub(2 + cursor_column as u16),
                &ghost,
                Color::DarkGrey,
            )?;
        }
    }
    let cursor_y = rect.y + 1 + cursor_line.saturating_sub(start_line) as u16;
    let cursor_x = rect.x + 1 + prefix.chars().count() as u16 + cursor_column as u16;
    Ok((cursor_x < rect.x + rect.width.saturating_sub(1)
        && cursor_y < rect.y + rect.height.saturating_sub(1))
    .then_some((cursor_x, cursor_y)))
}

fn update_editor_scroll(
    buffer: &mut EditBuffer,
    cursor_line: usize,
    total_lines: usize,
    visible_rows: usize,
) -> usize {
    if visible_rows == 0 {
        return 0;
    }
    let max_start = total_lines.saturating_sub(visible_rows);
    buffer.scroll_top = buffer.scroll_top.min(max_start);
    let margin = (visible_rows / 4)
        .max(1)
        .min(visible_rows.saturating_sub(1) / 2);

    if cursor_line < buffer.last_rendered_cursor_line {
        let upper_trigger = buffer.scroll_top.saturating_add(margin);
        if cursor_line <= upper_trigger && buffer.scroll_top > 0 {
            buffer.scroll_top = cursor_line.saturating_sub(margin);
        }
    } else if cursor_line > buffer.last_rendered_cursor_line {
        let lower_trigger = buffer
            .scroll_top
            .saturating_add(visible_rows.saturating_sub(1 + margin));
        if cursor_line >= lower_trigger && buffer.scroll_top < max_start {
            buffer.scroll_top = cursor_line
                .saturating_add(1 + margin)
                .saturating_sub(visible_rows)
                .min(max_start);
        }
    }

    if cursor_line < buffer.scroll_top {
        buffer.scroll_top = cursor_line;
    } else if cursor_line >= buffer.scroll_top.saturating_add(visible_rows) {
        buffer.scroll_top = cursor_line
            .saturating_add(1)
            .saturating_sub(visible_rows)
            .min(max_start);
    }
    buffer.last_rendered_cursor_line = cursor_line;
    buffer.scroll_top
}

fn draw_syntax_line(
    stdout: &mut impl Write,
    x: u16,
    y: u16,
    width: u16,
    line: &str,
) -> Result<(), String> {
    let mut column = 0u16;
    for (text, color) in syntax_segments(line) {
        if column >= width {
            break;
        }
        write_at(stdout, x + column, y, width - column, &text, color)?;
        column = column.saturating_add(text.chars().count() as u16);
    }
    Ok(())
}

fn syntax_segments(line: &str) -> Vec<(String, Color)> {
    let keywords = [
        "function", "struct", "enum", "global", "const", "let", "return", "if", "else", "for",
        "foreach", "while", "true", "false",
    ];
    let mut segments = Vec::<(String, Color)>::new();
    let mut chars = line.char_indices().peekable();
    while let Some((start, ch)) = chars.next() {
        let end = if ch.is_ascii_alphabetic() || ch == '_' {
            while chars
                .peek()
                .is_some_and(|(_, next)| next.is_ascii_alphanumeric() || *next == '_')
            {
                chars.next();
            }
            chars.peek().map_or(line.len(), |(index, _)| *index)
        } else if ch.is_ascii_digit() {
            while chars
                .peek()
                .is_some_and(|(_, next)| next.is_ascii_digit() || *next == '.')
            {
                chars.next();
            }
            chars.peek().map_or(line.len(), |(index, _)| *index)
        } else {
            chars.peek().map_or(line.len(), |(index, _)| *index)
        };
        let text = &line[start..end];
        let color = if keywords.contains(&text) {
            Color::Cyan
        } else if ch.is_ascii_digit() {
            Color::Yellow
        } else {
            Color::White
        };
        if let Some((prior, prior_color)) = segments.last_mut() {
            if *prior_color == color {
                prior.push_str(text);
                continue;
            }
        }
        segments.push((text.to_string(), color));
    }
    segments
}

fn draw_right_panel(
    stdout: &mut impl Write,
    rect: Rect,
    completion: &CompletionState,
    inspector: &InspectorState,
) -> Result<(), String> {
    if rect.width < 3 || rect.height < 3 {
        return Ok(());
    }
    let inner_height = rect.height.saturating_sub(2);
    let completion_rows = (inner_height / 2).max(2) as usize;
    let page_start = completion
        .selected
        .saturating_sub(completion_rows.saturating_sub(1));
    for (row, (index, item)) in completion
        .items
        .iter()
        .enumerate()
        .skip(page_start)
        .take(completion_rows)
        .enumerate()
    {
        let marker = if completion.armed && index == completion.selected {
            ">"
        } else {
            " "
        };
        write_at(
            stdout,
            rect.x + 1,
            rect.y + 1 + row as u16,
            rect.width.saturating_sub(2),
            &format!("{marker} {} [{}]", item.text, item.kind),
            if index == completion.selected {
                Color::Green
            } else {
                Color::White
            },
        )?;
    }
    let detail_y = rect.y + 1 + completion_rows as u16;
    write_at(
        stdout,
        rect.x + 1,
        detail_y,
        rect.width.saturating_sub(2),
        &format!("-- {} --", inspector.title),
        Color::Cyan,
    )?;
    let detail = completion
        .items
        .get(completion.selected)
        .map(|item| item.detail.clone());
    let lines = detail.into_iter().chain(inspector.lines.iter().cloned());
    for (row, line) in lines
        .take(inner_height.saturating_sub(completion_rows as u16 + 1) as usize)
        .enumerate()
    {
        write_at(
            stdout,
            rect.x + 1,
            detail_y + 1 + row as u16,
            rect.width.saturating_sub(2),
            &line,
            Color::DarkGrey,
        )?;
    }
    Ok(())
}

fn write_at(
    stdout: &mut impl Write,
    x: u16,
    y: u16,
    width: u16,
    text: &str,
    color: Color,
) -> Result<(), String> {
    if width == 0 {
        return Ok(());
    }
    let clipped = text.chars().take(width as usize).collect::<String>();
    queue!(
        stdout,
        MoveTo(x, y),
        SetForegroundColor(color),
        Print(clipped),
        ResetColor
    )
    .map_err(|error| format!("failed drawing live TUI: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn live_ai_write_is_rejected_until_references_were_checked() {
        let (client, _server) = stasis_runner::live::live_session(1);
        let mut tools = LiveAiTools::new(client);

        let observations = tools.execute(
            &[ToolCall {
                tool: "write_symbol".into(),
                args: json!({
                    "file": "src/main.stasis",
                    "name": "tick",
                    "new_source": "function tick(): void { return; }"
                }),
            }],
            &AtomicBool::new(false),
        );

        assert!(observations[0]
            .error
            .as_deref()
            .is_some_and(|error| error.contains("find_references")));
    }

    #[test]
    fn atomic_write_batch_returns_one_compact_receipt() {
        let calls = [
            ToolCall {
                tool: "write_symbol".into(),
                args: json!({"name": "tick"}),
            },
            ToolCall {
                tool: "write_symbol".into(),
                args: json!({"name": "render"}),
            },
        ];
        let transaction = Some(json!({
            "receipt": "build/live-edits/receipt.json",
            "tests": "passed",
            "plan": {"reload": {
                "changed_symbols": [{"name": "tick", "kind": "function"}],
                "expected_reload": "FastReload"
            }},
            "swap": {
                "state_layout_compatible": true,
                "requires_explicit_apply": false,
                "warnings": []
            },
            "large_source": "must not be returned"
        }));

        let first = applied_write_observation(&calls[0], 0, calls.len(), &transaction);
        let second = applied_write_observation(&calls[1], 1, calls.len(), &transaction);

        assert_eq!(
            first.result.as_ref().unwrap()["write"]["receipt"],
            "build/live-edits/receipt.json"
        );
        assert!(first.result.as_ref().unwrap()["write"]
            .get("large_source")
            .is_none());
        assert_eq!(
            second.result.as_ref().unwrap()["write_receipt"],
            "build/live-edits/receipt.json"
        );
    }

    #[test]
    fn failed_atomic_batch_reports_details_once() {
        let calls = [
            ToolCall {
                tool: "write_symbol".into(),
                args: json!({"name": "tick"}),
            },
            ToolCall {
                tool: "write_symbol".into(),
                args: json!({"name": "render"}),
            },
        ];
        let call_refs = calls.iter().collect::<Vec<_>>();

        let observations = failed_write_observations(&call_refs, "compile failed at line 4".into());

        assert_eq!(
            observations[0].error.as_deref(),
            Some("compile failed at line 4")
        );
        assert_eq!(
            observations[1].error.as_deref(),
            Some("atomic write batch failed; see the first write observation")
        );
    }

    #[test]
    fn live_ai_completion_requires_a_tested_atomic_write() {
        let (client, _server) = stasis_runner::live::live_session(1);
        let mut tools = LiveAiTools::new(client);
        assert!(tools
            .validate_completion()
            .expect_err("write required")
            .contains("compiles and passes project tests"));

        tools.last_write = Some(LiveResponse::success(
            1,
            0,
            "edit_applied",
            json!({"tests": "skipped"}),
        ));
        assert!(tools.validate_completion().is_err());

        tools.last_write = Some(LiveResponse::success(
            2,
            0,
            "edit_applied",
            json!({"tests": "passed"}),
        ));
        tools.validate_completion().expect("tested write completes");
    }

    #[test]
    fn write_batches_must_be_contiguous_to_preserve_call_order() {
        let calls = [
            ToolCall {
                tool: "write_symbol".into(),
                args: json!({}),
            },
            ToolCall {
                tool: "read_symbol".into(),
                args: json!({}),
            },
            ToolCall {
                tool: "write_symbol".into(),
                args: json!({}),
            },
        ];

        assert!(contiguous_write_range(&calls)
            .expect_err("split writes")
            .contains("must be contiguous"));
    }

    #[test]
    fn ai_audit_records_the_exact_tool_payload_seen_by_the_agent() {
        let call = ToolCall {
            tool: "write_symbol".into(),
            args: serde_json::json!({
                "file": "src/main.stasis",
                "name": "tick",
                "new_source": "function tick(): void { return; }"
            }),
        };
        let logged = audit_tool_call(&call);
        assert_eq!(
            logged,
            serde_json::json!({
                "tool": "write_symbol",
                "args": {
                    "file": "src/main.stasis",
                    "name": "tick",
                    "new_source": "function tick(): void { return; }"
                }
            })
        );

        let observation = ToolObservation::result(
            "read_symbol",
            serde_json::json!({
                "name": "tick",
                "source": "function tick(): void { return; }"
            }),
        );
        assert_eq!(
            audit_observation(&observation),
            serde_json::to_value(observation).unwrap()
        );

        let root = std::env::temp_dir().join(format!(
            "stasis_ai_usage_log_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let usage = serde_json::json!({
            "input_tokens": 100,
            "cached_input_tokens": 80,
            "output_tokens": 20,
        });
        let mut log = AiAuditLog::create(&root, "test prompt").expect("audit log");
        let trace_path = log.path.clone();
        let usage_path = log.usage_path.clone();
        let timing_path = log.timing_path.clone();
        log.write(serde_json::json!({"event": "test"}))
            .expect("timed trace");
        log.write_usage(&usage).expect("usage log");
        drop(log);
        let trace = fs::read_to_string(trace_path).expect("trace contents");
        let records = trace
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).expect("trace record"))
            .collect::<Vec<_>>();
        assert_eq!(records.len(), 2);
        assert!(records
            .iter()
            .all(|record| record.get("elapsed_ms").is_none()));
        let timings = fs::read_to_string(timing_path).expect("timing contents");
        let timings = timings
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).expect("timing record"))
            .collect::<Vec<_>>();
        assert_eq!(timings.len(), 2);
        assert!(timings
            .iter()
            .all(|record| record["elapsed_ms"].is_u64() && record["since_previous_ms"].is_u64()));
        assert_eq!(
            fs::read_to_string(&usage_path).expect("usage contents"),
            format!("{}\n", serde_json::to_string(&usage).unwrap())
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn multiline_editor_autoindents_and_moves_vertically() {
        let mut buffer = EditBuffer::from_text("function tick(): i32 {".to_string());
        buffer.insert_newline();
        buffer.insert_char('r');
        assert_eq!(buffer.text, "function tick(): i32 {\n    r");
        buffer.move_vertical(-1, false);
        assert_eq!(line_column(&buffer.text, buffer.cursor).0, 0);
    }

    #[test]
    fn editor_scrolls_down_inside_the_lower_quarter_margin() {
        let mut buffer =
            EditBuffer::from_text((0..20).map(|line| format!("line {line}\n")).collect());
        buffer.set_cursor(0, false);
        assert_eq!(update_editor_scroll(&mut buffer, 0, 21, 8), 0);

        for line in 1..=6 {
            buffer.move_vertical(1, false);
            let cursor_line = line_column(&buffer.text, buffer.cursor).0;
            update_editor_scroll(&mut buffer, cursor_line, 21, 8);
            if line < 5 {
                assert_eq!(buffer.scroll_top, 0);
            }
        }

        assert_eq!(buffer.scroll_top, 1);
    }

    #[test]
    fn editor_scrolls_up_inside_the_upper_quarter_margin() {
        let mut buffer =
            EditBuffer::from_text((0..20).map(|line| format!("line {line}\n")).collect());
        buffer.scroll_top = 8;
        let line_ten = line_ranges(&buffer.text)[10].start;
        buffer.set_cursor(line_ten, false);
        buffer.last_rendered_cursor_line = 11;
        assert_eq!(update_editor_scroll(&mut buffer, 10, 21, 8), 8);

        buffer.move_vertical(-1, false);
        let cursor_line = line_column(&buffer.text, buffer.cursor).0;
        assert_eq!(update_editor_scroll(&mut buffer, cursor_line, 21, 8), 7);
    }

    #[test]
    fn completion_requires_whitespace_document_tail() {
        assert!(suffix_is_whitespace("hero.da\n   ", 7));
        assert!(!suffix_is_whitespace("hero.da later", 7));
    }

    #[test]
    fn ghost_is_only_a_real_prefix_suffix() {
        let buffer = EditBuffer::from_text("hero.da".to_string());
        let completion = CompletionState {
            items: vec![CompletionItem {
                text: "hero.damage".to_string(),
                kind: "method".to_string(),
                detail: String::new(),
                type_name: None,
                source: None,
                selector: None,
                scope: None,
            }],
            selected: 0,
            replacement_start: 0,
            replacement_end: 7,
            armed: true,
            truncated: false,
        };
        assert_eq!(ghost_suffix(&buffer, &completion).as_deref(), Some("mage"));
    }

    #[test]
    fn very_narrow_layout_stacks_the_right_panel() {
        let layout = Layout::new(50, 30, true);
        assert_eq!(layout.right.x, 0);
        assert!(layout.right.y > layout.editor.y);
    }

    #[test]
    fn compact_desktop_keeps_the_right_panel_beside_the_editor() {
        let layout = Layout::new(68, 30, true);
        assert!(layout.right.x > 0);
        assert_eq!(layout.right.y, 0);
    }

    #[test]
    fn tiny_layout_never_draws_below_the_real_viewport() {
        for (width, height) in [(80u16, 3u16), (40, 1), (10, 10)] {
            let usable_height = height.saturating_sub(1);
            let layout = Layout::new(width, height, true);
            for rect in [layout.transcript, layout.editor, layout.right] {
                assert!(rect.x.saturating_add(rect.width) <= width);
                assert!(rect.y.saturating_add(rect.height) <= usable_height);
            }
        }
    }

    #[test]
    fn syntax_segments_highlight_keywords_and_numbers() {
        let segments = syntax_segments("let score: i32 = 12;");
        assert!(segments
            .iter()
            .any(|(text, color)| text == "let" && *color == Color::Cyan));
        assert!(segments
            .iter()
            .any(|(text, color)| text == "12" && *color == Color::Yellow));
    }

    #[test]
    fn default_state_items_are_concise_and_indent_dotted_paths() {
        let item = serde_json::json!({
            "path": "player.stats.score",
            "static_type": "f32",
            "value": {"type": "f32", "value": 12.5}
        });
        assert_eq!(format_state_item(&item), "    score = 12.5");
    }

    #[test]
    fn default_state_tree_includes_collections_structs_and_memory() {
        let data = serde_json::json!({
            "collections": [{
                "path": "state.enemies",
                "active_count": 2,
                "capacity": 8,
                "fields": [{"field": "hp", "type_name": "i32"}]
            }],
            "structs": [{"path": "state.player", "type_name": "Player"}],
            "memory": {"total_capacity_bytes": 96, "snapshot_bytes": 96}
        });
        let mut lines = Vec::new();

        append_state_tree_lines(&data, &mut lines);

        assert_eq!(
            lines,
            vec![
                "state.enemies [2/8]",
                "  hp: i32",
                "state.player: Player",
                "memory: 96 bytes; snapshot: 96 bytes",
            ]
        );
    }

    #[test]
    fn apply_confirmation_reports_active_confirmation_bound_in_transcript() {
        let response = LiveResponse::success(
            8,
            43,
            "edit_applied",
            serde_json::json!({
                "plan": {
                    "changed_files": [{"file": "src/main.stasis"}],
                    "reload": {
                        "expected_reload": "FastReload",
                        "changed_symbols": [{"name": "tick"}]
                    }
                },
                "tests": "passed"
            }),
        );

        assert_eq!(
            format_apply_confirmation(&response, Duration::from_millis(18)),
            "Hot swapped <= 18 ms | tests passed"
        );
        assert!(
            format_apply_confirmation(&response, Duration::from_micros(900))
                .starts_with("Hot swapped <= 1 ms")
        );
        assert!(
            format_apply_confirmation(&response, Duration::from_micros(18_900))
                .starts_with("Hot swapped <= 19 ms")
        );
    }

    #[test]
    fn apply_error_omits_the_workspace_path_on_narrow_transcripts() {
        let response = LiveResponse::failure(
            8,
            43,
            r"C:\workspace\src\main.stasis:4514-4544: unknown identifier 'missing_symbol'",
        );

        assert_eq!(
            format_apply_error(&response),
            "error: unknown identifier 'missing_symbol'"
        );
    }

    #[test]
    fn successful_clean_apply_closes_definition_editor() {
        let (client, _server) = stasis_runner::live::live_session(8);
        let mut app = LiveTui::new(client, std::env::temp_dir());
        let target = LiveSymbolTarget {
            name: "tick".to_string(),
            kind: Some("function".to_string()),
            file: Some("src/main.stasis".to_string()),
            owner: None,
            signature: Some("tick(): i32".to_string()),
        };
        app.input = InputMode::Definition(EditSession {
            id: 1,
            target: target.clone(),
            expected_source_hash: workshop_source_hash("accepted"),
            accepted_source: "accepted".to_string(),
            source_start: 0,
            buffer: EditBuffer::from_text("accepted".to_string()),
            discard_confirm: false,
        });
        let response = LiveResponse::success(
            8,
            43,
            "edit_applied",
            serde_json::json!({
                "plan": {
                    "changed_files": [{"file": "src/main.stasis"}],
                    "reload": {"expected_reload": "FastReload"}
                },
                "tests": "passed"
            }),
        );

        app.finish_apply(
            response,
            "accepted".to_string(),
            0,
            1,
            target,
            Instant::now(),
        );

        assert!(matches!(app.input, InputMode::Prompt(_)));
        assert_eq!(app.status, "snapshot applied; editor closed");
    }

    #[test]
    fn completed_apply_does_not_mutate_a_different_open_definition() {
        let (client, _server) = stasis_runner::live::live_session(8);
        let mut app = LiveTui::new(client, std::env::temp_dir());
        let submitted_target = LiveSymbolTarget {
            name: "tick".to_string(),
            kind: Some("function".to_string()),
            file: Some("src/main.stasis".to_string()),
            owner: None,
            signature: Some("tick(): i32".to_string()),
        };
        let current_target = LiveSymbolTarget {
            name: "render".to_string(),
            signature: Some("render(): i32".to_string()),
            ..submitted_target.clone()
        };
        app.input = InputMode::Definition(EditSession {
            id: 2,
            target: current_target.clone(),
            expected_source_hash: workshop_source_hash("render source"),
            accepted_source: "render source".to_string(),
            source_start: 0,
            buffer: EditBuffer::from_text("render source".to_string()),
            discard_confirm: false,
        });
        let response = LiveResponse::success(
            8,
            43,
            "edit_applied",
            serde_json::json!({
                "plan": {
                    "changed_files": [{"file": "src/main.stasis"}],
                    "reload": {"expected_reload": "FastReload"}
                },
                "tests": "passed"
            }),
        );

        app.finish_apply(
            response,
            "tick source".to_string(),
            0,
            1,
            submitted_target,
            Instant::now(),
        );

        let InputMode::Definition(edit) = &app.input else {
            panic!("current definition must remain open");
        };
        assert_eq!(edit.target, current_target);
        assert_eq!(edit.accepted_source, "render source");
        assert_eq!(
            app.status,
            "snapshot applied for tick; current editor unchanged"
        );
    }

    #[test]
    fn completed_apply_does_not_mutate_a_reopened_same_definition() {
        let (client, _server) = stasis_runner::live::live_session(8);
        let mut app = LiveTui::new(client, std::env::temp_dir());
        let target = LiveSymbolTarget {
            name: "obstacle_enabled".to_string(),
            kind: Some("function".to_string()),
            file: Some("src/main.stasis".to_string()),
            owner: None,
            signature: Some("obstacle_enabled(): bool".to_string()),
        };
        app.input = InputMode::Definition(EditSession {
            id: 2,
            target: target.clone(),
            expected_source_hash: workshop_source_hash("reopened source"),
            accepted_source: "reopened source".to_string(),
            source_start: 0,
            buffer: EditBuffer::from_text("reopened source".to_string()),
            discard_confirm: false,
        });
        let response = LiveResponse::success(
            8,
            43,
            "edit_applied",
            serde_json::json!({
                "plan": {
                    "changed_files": [{"file": "src/main.stasis"}],
                    "reload": {"expected_reload": "FastReload"}
                },
                "tests": "passed"
            }),
        );

        app.finish_apply(
            response,
            "old submitted source".to_string(),
            0,
            1,
            target,
            Instant::now(),
        );

        let InputMode::Definition(edit) = &app.input else {
            panic!("reopened definition must remain open");
        };
        assert_eq!(edit.id, 2);
        assert_eq!(edit.accepted_source, "reopened source");
        assert_eq!(
            app.status,
            "snapshot applied for obstacle_enabled; current editor unchanged"
        );
    }

    #[test]
    fn completion_requests_coalesce_and_stale_results_are_discarded() {
        let (client, server) = stasis_runner::live::live_session(8);
        let mut app = LiveTui::new(client, std::env::temp_dir());
        app.refresh_completion(false);
        app.active_buffer_mut().insert_char('s');
        app.refresh_completion(true);

        let first = server.drain(8);
        assert_eq!(first.len(), 1);
        let first_id = first[0].request_id;
        server
            .respond(LiveResponse::success(
                first_id,
                1,
                "completion",
                serde_json::json!({
                    "replacement_start": 0,
                    "replacement_end": 0,
                    "page": 0,
                    "truncated": false,
                    "items": [{"text": "stale", "kind": "global", "detail": "stale"}]
                }),
            ))
            .expect("stale completion response");
        app.drain_responses().expect("drain stale completion");
        assert!(app.completion.items.is_empty());

        let second = server.drain(8);
        assert_eq!(second.len(), 1);
        let second_id = second[0].request_id;
        server
            .respond(LiveResponse::success(
                second_id,
                2,
                "completion",
                serde_json::json!({
                    "replacement_start": 0,
                    "replacement_end": 1,
                    "page": 0,
                    "truncated": false,
                    "items": [{"text": "score", "kind": "global", "detail": "i32"}]
                }),
            ))
            .expect("current completion response");
        app.drain_responses().expect("drain current completion");
        assert_eq!(app.completion.items[0].text, "score");
        assert!(app.completion.armed);
    }

    #[test]
    fn editing_after_discard_warning_requires_a_fresh_confirmation() {
        let (client, _server) = stasis_runner::live::live_session(8);
        let mut app = LiveTui::new(client, std::env::temp_dir());
        app.input = InputMode::Definition(EditSession {
            id: 1,
            target: LiveSymbolTarget {
                name: "tick".to_string(),
                kind: Some("function".to_string()),
                file: Some("src/main.stasis".to_string()),
                owner: None,
                signature: Some("tick(): i32".to_string()),
            },
            expected_source_hash: workshop_source_hash("accepted"),
            accepted_source: "accepted".to_string(),
            source_start: 0,
            buffer: EditBuffer::from_text("dirty".to_string()),
            discard_confirm: false,
        });
        app.close_definition();
        assert!(matches!(
            &app.input,
            InputMode::Definition(edit) if edit.discard_confirm
        ));
        app.active_buffer_mut().insert_char('!');
        app.close_definition();
        assert!(matches!(
            &app.input,
            InputMode::Definition(edit) if edit.discard_confirm
        ));
    }

    #[test]
    fn rendering_buffer_access_preserves_discard_confirmation() {
        let (client, _server) = stasis_runner::live::live_session(8);
        let mut app = LiveTui::new(client, std::env::temp_dir());
        app.input = InputMode::Definition(EditSession {
            id: 1,
            target: LiveSymbolTarget {
                name: "tick".to_string(),
                kind: Some("function".to_string()),
                file: Some("src/main.stasis".to_string()),
                owner: None,
                signature: Some("tick(): i32".to_string()),
            },
            expected_source_hash: workshop_source_hash("accepted"),
            accepted_source: "accepted".to_string(),
            source_start: 0,
            buffer: EditBuffer::from_text("dirty".to_string()),
            discard_confirm: false,
        });

        app.close_definition();
        let _ = app.active_buffer_for_render_mut();
        app.close_definition();

        assert!(matches!(app.input, InputMode::Prompt(_)));
    }

    #[test]
    fn edit_uses_the_selected_overload_selector() {
        let long_signature = format!("tick({}): i32", "value: i32, ".repeat(30));
        let completion = CompletionState {
            items: vec![
                CompletionItem {
                    text: "tick".to_string(),
                    kind: "function".to_string(),
                    detail: "tick(i32): i32 [src/a.stasis]".to_string(),
                    type_name: None,
                    source: Some("src/a.stasis".to_string()),
                    selector: Some(LiveSymbolTarget {
                        name: "tick".to_string(),
                        kind: Some("function".to_string()),
                        file: Some("src/a.stasis".to_string()),
                        owner: None,
                        signature: Some("tick(i32): i32".to_string()),
                    }),
                    scope: None,
                },
                CompletionItem {
                    text: "tick".to_string(),
                    kind: "function".to_string(),
                    detail: "tick(...) [display text intentionally truncated]".to_string(),
                    type_name: None,
                    source: Some("src/b.stasis".to_string()),
                    selector: Some(LiveSymbolTarget {
                        name: "tick".to_string(),
                        kind: Some("function".to_string()),
                        file: Some("src/b.stasis".to_string()),
                        owner: None,
                        signature: Some(long_signature.clone()),
                    }),
                    scope: None,
                },
            ],
            selected: 1,
            ..CompletionState::default()
        };
        let target = edit_target_from_completion("tick", &completion);
        assert_eq!(target.file.as_deref(), Some("src/b.stasis"));
        assert_eq!(target.signature.as_deref(), Some(long_signature.as_str()));
    }

    #[test]
    fn default_inspection_backpressure_does_not_exit_or_leave_pending_work() {
        let (client, _server) = stasis_runner::live::live_session(1);
        client
            .submit(LiveRequest::new(99, LiveCommand::Pause))
            .expect("fill request queue");
        let mut app = LiveTui::new(client, std::env::temp_dir());

        app.request_default_inspection()
            .expect("backpressure stays inside the TUI");

        assert!(!app
            .pending
            .values()
            .any(|action| matches!(action, PendingAction::DefaultInspect)));
        assert!(app.status.contains("delayed by backpressure"));
    }

    #[test]
    fn replacing_a_pinned_inspection_unwatches_the_previous_path() {
        let (client, server) = stasis_runner::live::live_session(8);
        let mut app = LiveTui::new(client, std::env::temp_dir());
        app.handle_response(LiveResponse::success(
            10,
            1,
            "inspection",
            serde_json::json!({"path": "score", "static_type": "i32", "value": 1}),
        ));
        let first = server.drain(8);
        assert!(matches!(
            &first[0].command,
            LiveCommand::Watch { path } if path == "score"
        ));
        server
            .respond(LiveResponse::success(
                first[0].request_id,
                2,
                "watch_added",
                serde_json::json!({"path": "score", "value": 1}),
            ))
            .expect("confirm score watch");
        app.drain_responses().expect("drain score watch");
        assert_eq!(app.inspector_watch.as_deref(), Some("score"));
        app.handle_response(LiveResponse::success(
            11,
            3,
            "inspection",
            serde_json::json!({"path": "speed", "static_type": "f32", "value": 2.0}),
        ));
        let replaced = server.drain(8);
        assert_eq!(replaced.len(), 1);
        assert!(matches!(
            &replaced[0].command,
            LiveCommand::Unwatch { path: Some(path) } if path == "score"
        ));
        server
            .respond(LiveResponse::success(
                replaced[0].request_id,
                4,
                "watch_removed",
                serde_json::json!({"watches": []}),
            ))
            .expect("confirm score unwatch");
        app.drain_responses().expect("drain score unwatch");
        let watch = server.drain(8);
        assert!(matches!(
            &watch[0].command,
            LiveCommand::Watch { path } if path == "speed"
        ));
        server
            .respond(LiveResponse::success(
                watch[0].request_id,
                5,
                "watch_added",
                serde_json::json!({"path": "speed", "value": 2.0}),
            ))
            .expect("confirm speed watch");
        app.drain_responses().expect("drain speed watch");
        assert_eq!(app.inspector_watch.as_deref(), Some("speed"));
    }

    #[test]
    fn watch_replacement_backpressure_keeps_truthful_retryable_state() {
        let (client, server) = stasis_runner::live::live_session(1);
        client
            .submit(LiveRequest::new(99, LiveCommand::Pause))
            .expect("fill request queue");
        let mut app = LiveTui::new(client, std::env::temp_dir());
        app.inspector_watch = Some("score".to_string());

        app.replace_inspector_watch(Some("speed".to_string()));
        assert_eq!(app.inspector_watch.as_deref(), Some("score"));
        assert!(app.status.contains("unavailable"));

        server.drain(1);
        app.replace_inspector_watch(Some("speed".to_string()));
        assert_eq!(app.inspector_watch.as_deref(), Some("score"));
        let unwatch = server.drain(1);
        assert!(matches!(
            &unwatch[0].command,
            LiveCommand::Unwatch { path: Some(path) } if path == "score"
        ));
        server
            .respond(LiveResponse::success(
                unwatch[0].request_id,
                1,
                "watch_removed",
                serde_json::json!({"watches": []}),
            ))
            .expect("confirm unwatch");
        app.drain_responses().expect("drain unwatch");
        assert_eq!(app.inspector_watch, None);
        let watch = server.drain(1);
        assert!(matches!(
            &watch[0].command,
            LiveCommand::Watch { path } if path == "speed"
        ));
    }

    #[test]
    fn rejected_watch_does_not_create_a_ghost_or_suppress_retry() {
        let (client, server) = stasis_runner::live::live_session(8);
        let mut app = LiveTui::new(client, std::env::temp_dir());
        app.replace_inspector_watch(Some("score".to_string()));
        let first = server.drain(1);
        assert_eq!(app.inspector_watch, None);

        server
            .respond(LiveResponse::failure(
                first[0].request_id,
                1,
                "live watch limit reached",
            ))
            .expect("reject watch");
        app.drain_responses().expect("drain rejected watch");
        assert_eq!(app.inspector_watch, None);

        app.replace_inspector_watch(Some("score".to_string()));
        let retry = server.drain(1);
        assert!(matches!(
            &retry[0].command,
            LiveCommand::Watch { path } if path == "score"
        ));
    }
}
