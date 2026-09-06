use eframe::egui::{self, Color32, RichText};
use stasis_ai::task_session::{
    ActionState, Key, KeyChord, Modifiers, ShortcutMapper, TaskSession, TaskSessionCommand,
    ThreadEntryKind,
};
use stasis_runner::live::{LiveCommand, LiveRequest, LiveSessionClient};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusArea {
    Tasks,
    Reply,
    Game,
    Palette,
}
const EMPTY_TASK: &str = "Create a task";

#[derive(Debug, Clone, PartialEq, Eq)]
enum EditorIntent {
    SendReply(String, String),
    Apply(String, String),
    Test(String),
    Screenshot(String),
    GenerateImage(String),
    ImportImage(String, String),
    Cancel(String),
    Reconnect(String),
}

struct DesktopEditor {
    state: EditorState,
    client: LiveSessionClient,
    project_root: PathBuf,
    next_request: u64,
    shutdown: Arc<AtomicBool>,
}

impl DesktopEditor {
    fn new(client: LiveSessionClient, project_root: PathBuf, shutdown: Arc<AtomicBool>) -> Self {
        Self {
            state: EditorState::default(),
            client,
            project_root,
            next_request: 1,
            shutdown,
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
                EditorIntent::Reconnect(task) => {
                    let request = LiveRequest::new(self.next_request, LiveCommand::Status);
                    self.next_request = self.next_request.saturating_add(1);
                    if let Err(error) = self.client.submit(request) {
                        self.state.notice = Some(error);
                        pending.push(EditorIntent::Reconnect(task));
                    }
                }
                intent => pending.push(intent),
            }
        }
        self.state.intents = pending;
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
            let label = format!("{objective}\n{lifecycle:?} | {connection:?} | {elapsed}ms | ${:.4} | retry {retries}",
                cost as f64 / 1_000_000.0);
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
        }
    }
}

impl EditorState {
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
                self.session
                    .append_reply(&text)
                    .map_err(|e| e.to_string())?;
                self.intents.push(EditorIntent::SendReply(task, text));
                self.reply.clear();
                Ok(())
            }
            TaskSessionCommand::AcceptAction => {
                let id = self
                    .first_action(|s| matches!(s, ActionState::Proposed))
                    .ok_or_else(|| "No proposed action to accept.".to_string())?;
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
                self.intents.push(EditorIntent::Apply(task, action));
                Ok(())
            }
            TaskSessionCommand::RunFocusedTests => {
                let task = self.active_id()?;
                self.session
                    .begin_focused_tests()
                    .map_err(|e| e.to_string())?;
                self.intents.push(EditorIntent::Test(task));
                Ok(())
            }
            TaskSessionCommand::Retry => self.session.retry().map_err(|e| e.to_string()),
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
            TaskSessionCommand::MarkDone => self.session.mark_done().map_err(|e| e.to_string()),
            TaskSessionCommand::Cancel => {
                let task = self.active_id()?;
                self.session.cancel().map_err(|e| e.to_string())?;
                self.intents.push(EditorIntent::Cancel(task));
                Ok(())
            }
            TaskSessionCommand::Reconnect => {
                let task = self.active_id()?;
                self.session.reconnect().map_err(|e| e.to_string())?;
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
        ui.separator();
        egui::ScrollArea::vertical()
            .id_source("task-thread")
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for entry in &task.thread {
                    let speaker = if matches!(entry.kind, ThreadEntryKind::Reply) {
                        "You"
                    } else {
                        "AI"
                    };
                    ui.group(|ui| {
                        ui.label(RichText::new(speaker).strong());
                        ui.label(&entry.text);
                    });
                }
                for action in task.actions.values() {
                    ui.group(|ui| {
                        ui.label(RichText::new(format!("Action | {:?}", action.state)).strong());
                        ui.label(&action.description);
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
                ("Apply  Ctrl+Alt+Enter", TaskSessionCommand::ApplyAction),
                ("Test  Ctrl+T", TaskSessionCommand::RunFocusedTests),
                ("Done  Ctrl+Shift+D", TaskSessionCommand::MarkDone),
            ] {
                if ui.button(label).clicked() {
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
        painter.text(response.rect.center(), egui::Align2::CENTER_CENTER,
            "LIVE GAME\n\nThe interactive game runs in its native window\nand keeps independent keyboard and mouse focus.\n\nCtrl+Alt+G focuses this surface.",
            egui::FontId::proportional(18.0), color);
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
    use stasis_runner::live::live_session;

    fn task_state() -> EditorState {
        let mut state = EditorState::default();
        state.objective = "Change player speed".into();
        state.create_task().unwrap();
        state
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
    fn independent_tasks_keep_replies_scoped() {
        let mut state = task_state();
        state.reply = "First reply".into();
        state.handle(TaskSessionCommand::SendReply).unwrap();
        let first = state.active_id().unwrap();
        state.objective = "Change enemy art".into();
        state.create_task().unwrap();
        state.reply = "Second reply".into();
        state.handle(TaskSessionCommand::SendReply).unwrap();
        assert_eq!(state.session.active_task().unwrap().thread.len(), 1);
        state.session.switch_task(first).unwrap();
        assert_eq!(
            state.session.active_task().unwrap().thread[0].text,
            "First reply"
        );
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
    fn reconnect_cancel_and_completion_follow_task_state() {
        let mut state = task_state();
        state.session.disconnect().unwrap();
        state.handle(TaskSessionCommand::Reconnect).unwrap();
        assert!(matches!(
            state.session.active_task().unwrap().connection,
            ConnectionState::Connected
        ));
        state.session.begin_focused_tests().unwrap();
        state
            .session
            .finish_focused_tests(FocusedTestResult::passed("ok"))
            .unwrap();
        state.handle(TaskSessionCommand::MarkDone).unwrap();
        assert!(matches!(
            state.session.active_task().unwrap().lifecycle,
            TaskLifecycle::Completed
        ));
        state.objective = "Cancelable task".into();
        state.create_task().unwrap();
        state.handle(TaskSessionCommand::Cancel).unwrap();
        assert!(matches!(
            state.session.active_task().unwrap().lifecycle,
            TaskLifecycle::Canceled
        ));
    }

    #[test]
    fn flush_keeps_unhandled_test_intent_and_running_validation() {
        let (client, _server) = live_session(4);
        let mut editor =
            DesktopEditor::new(client, PathBuf::from("."), Arc::new(AtomicBool::new(false)));
        editor.state.objective = "Run focused tests".into();
        editor.state.create_task().unwrap();
        editor
            .state
            .handle(TaskSessionCommand::RunFocusedTests)
            .unwrap();

        editor.flush_intents();

        assert!(matches!(
            editor.state.intents.as_slice(),
            [EditorIntent::Test(task)] if task == "task-1"
        ));
        assert!(matches!(
            editor.state.session.active_task().unwrap().validation,
            ValidationStatus::Running
        ));
    }

    #[test]
    fn flush_removes_reconnect_after_status_submission() {
        let (client, server) = live_session(4);
        let mut editor =
            DesktopEditor::new(client, PathBuf::from("."), Arc::new(AtomicBool::new(false)));
        editor.state.objective = "Reconnect task".into();
        editor.state.create_task().unwrap();
        editor.state.session.disconnect().unwrap();
        editor.state.handle(TaskSessionCommand::Reconnect).unwrap();

        editor.flush_intents();

        assert!(editor.state.intents.is_empty());
        let requests = server.drain(4);
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].command, LiveCommand::Status);
    }

    #[test]
    fn flush_keeps_reconnect_when_status_submission_fails() {
        let (client, server) = live_session(1);
        client
            .submit(LiveRequest::new(99, LiveCommand::Status))
            .unwrap();
        let mut editor =
            DesktopEditor::new(client, PathBuf::from("."), Arc::new(AtomicBool::new(false)));
        editor.state.objective = "Reconnect task".into();
        editor.state.create_task().unwrap();
        editor.state.session.disconnect().unwrap();
        editor.state.handle(TaskSessionCommand::Reconnect).unwrap();

        editor.flush_intents();

        assert!(matches!(
            editor.state.intents.as_slice(),
            [EditorIntent::Reconnect(task)] if task == "task-1"
        ));
        assert_eq!(
            editor.state.notice.as_deref(),
            Some("live-session command queue is full")
        );
        assert_eq!(server.drain(1).len(), 1);
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
}
