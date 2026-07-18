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
use stasis_compiler::frontend::parser::completion_expected_type;
use stasis_compiler::frontend::workshop::{
    workshop_source_hash, WorkshopSourceItem, WorkshopSourceItemKind,
};
use stasis_runner::live::{
    CompletionContext, CompletionItem, CompletionQuery, LiveCommand, LiveEditOperation,
    LiveRequest, LiveResponse, LiveSessionClient, LiveSymbolTarget, TerminalBuffer, TerminalInput,
};
use std::collections::{BTreeMap, VecDeque};
use std::io::{self, Write};
use std::time::{Duration, Instant};

const MAX_TRANSCRIPT_LINES: usize = 500;
const MAX_UNDO_STATES: usize = 100;
const COMPLETION_LIMIT: usize = 64;
const TUI_REQUEST_START: u64 = 1u64 << 61;

pub(super) fn run(client: &LiveSessionClient) -> Result<bool, String> {
    let _guard = TerminalGuard::enter()?;
    let mut app = LiveTui::new(client.clone());
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
    prompt: &'static str,
    quit: bool,
    last_regions: [Vec<u8>; 5],
    last_size: Option<(u16, u16)>,
}

impl LiveTui {
    fn new(client: LiveSessionClient) -> Self {
        Self {
            client,
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
                self.refresh_completion(false);
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
        while let Some(response) = self.client.try_receive()? {
            self.handle_response(response);
        }
        Ok(())
    }

    fn request_default_inspection(&mut self) -> Result<(), String> {
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
        let mut app = LiveTui::new(client);
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
        let mut app = LiveTui::new(client);
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
        let mut app = LiveTui::new(client);
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
        let mut app = LiveTui::new(client);
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
        let mut app = LiveTui::new(client);
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
        let mut app = LiveTui::new(client);
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
        let mut app = LiveTui::new(client);

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
        let mut app = LiveTui::new(client);
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
        let mut app = LiveTui::new(client);
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
        let mut app = LiveTui::new(client);
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
