use eframe::egui::{self, Color32, RichText};
use serde_json::{json, Value};
use stasis_ai::task_session::{
    ActionState, FallbackState, Key, KeyChord, Modifiers, ProviderState, RoutingState,
    ShortcutMapper, TaskId, TaskSession, TaskSessionCommand, ThreadEntryKind,
};
use stasis_ai::{
    run_agent_with_profile, AgentEvent, AgentProfile, ProviderConfig, ProviderReply,
    ProviderRequest, ProviderUsage, TaskController, TaskControllerEvent, ToolCall, ToolExecutor,
    ToolObservation,
};
use stasis_runner::live::{LiveCommand, LiveRequest, LiveSessionClient};
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

struct ReplyOnlyTools;

impl ToolExecutor for ReplyOnlyTools {
    fn execute(&mut self, calls: &[ToolCall], _canceled: &AtomicBool) -> Vec<ToolObservation> {
        calls
            .iter()
            .map(|call| ToolObservation::error(&call.tool, "desktop chat is reply-only"))
            .collect()
    }
}

fn bounded_provider_label(value: Option<&str>, fallback: &str) -> String {
    value
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(fallback)
        .chars()
        .take(96)
        .collect()
}

fn usage_u64(usage: &Value, pointers: &[&str]) -> u64 {
    pointers
        .iter()
        .find_map(|pointer| usage.pointer(pointer).and_then(Value::as_u64))
        .unwrap_or(0)
}

fn configured_provider_state(config: &ProviderConfig) -> ProviderState {
    let (route, fallback) = match config {
        ProviderConfig::Codex => ("direct".to_string(), FallbackState::Unconfigured),
        ProviderConfig::OpenRouter(openrouter) => {
            let route = if !openrouter.routing.only.is_empty() {
                format!("only:{}", openrouter.routing.only.join(","))
            } else if !openrouter.routing.order.is_empty() {
                format!("order:{}", openrouter.routing.order.join(","))
            } else {
                format!("openrouter:{:?}", openrouter.routing.sort).to_ascii_lowercase()
            };
            let fallback = if openrouter.routing.allow_fallbacks {
                FallbackState::Ready {
                    provider: "openrouter".to_string(),
                    model: Some(bounded_provider_label(
                        Some(&openrouter.model),
                        "configured",
                    )),
                    route: Some(bounded_provider_label(Some(&route), "openrouter")),
                }
            } else {
                FallbackState::Unconfigured
            };
            (route, fallback)
        }
    };
    ProviderState {
        provider: Some(config.provider_name().to_string()),
        model: Some(bounded_provider_label(Some(&config.model()), "configured")),
        routing: RoutingState::Assigned {
            route: bounded_provider_label(Some(&route), "direct"),
        },
        fallback,
    }
}

fn provider_reply_state(config: &ProviderConfig, usage: Option<&Value>) -> ProviderState {
    let provider = bounded_provider_label(
        usage
            .and_then(|value| value.get("resolved_provider"))
            .and_then(Value::as_str),
        config.provider_name(),
    );
    let model = bounded_provider_label(
        usage
            .and_then(|value| value.get("resolved_model"))
            .and_then(Value::as_str),
        &config.model(),
    );
    let route = match usage.and_then(|value| value.get("route")) {
        Some(Value::String(route)) => bounded_provider_label(Some(route), "direct"),
        Some(Value::Object(_)) => bounded_provider_label(
            Some(&format!("{}:{provider}", config.provider_name())),
            "direct",
        ),
        _ => {
            let RoutingState::Assigned { route } = configured_provider_state(config).routing else {
                unreachable!("configured provider route is always assigned")
            };
            route
        }
    };
    let fallback = if usage
        .and_then(|value| value.get("fallback"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        FallbackState::Active {
            provider: provider.clone(),
            model: Some(model.clone()),
            route: Some(route.clone()),
        }
    } else {
        configured_provider_state(config).fallback
    };
    ProviderState {
        provider: Some(provider),
        model: Some(model),
        routing: RoutingState::Assigned { route },
        fallback,
    }
}

fn provider_reply_usage(usage: Option<&Value>) -> ProviderUsage {
    let input_tokens = usage
        .map(|value| {
            usage_u64(
                value,
                &["/tokens/input_tokens", "/tokens/prompt", "/input_tokens"],
            )
        })
        .unwrap_or(0);
    let output_tokens = usage
        .map(|value| {
            usage_u64(
                value,
                &[
                    "/tokens/output_tokens",
                    "/tokens/completion",
                    "/output_tokens",
                ],
            )
        })
        .unwrap_or(0);
    let estimated_cost_micros = usage
        .and_then(|value| value.get("cost"))
        .and_then(Value::as_f64)
        .filter(|cost| cost.is_finite() && *cost > 0.0)
        .map(|cost| (cost * 1_000_000.0).round() as u64)
        .unwrap_or(0);
    ProviderUsage {
        input_tokens,
        output_tokens,
        estimated_cost_micros,
    }
}

fn run_reply_provider(
    request: ProviderRequest,
    canceled: Arc<AtomicBool>,
) -> Result<ProviderReply, String> {
    let config = ProviderConfig::from_env()?;
    let mut provider = config.clone().build()?;
    let prompt = request
        .context
        .last()
        .map(|entry| entry.text.trim())
        .filter(|text| !text.is_empty())
        .unwrap_or(request.objective.as_str())
        .to_string();
    let initial_context = json!({
        "task_id": request.task_id,
        "objective": request.objective,
        "project_summary": request.project_summary,
        "relevant_files": request.relevant_files,
        "relevant_symbols": request.relevant_symbols,
        "relevant_tests": request.relevant_tests,
        "thread": request.context,
    });
    let profile = AgentProfile {
        role: "Stasis desktop task assistant".to_string(),
        instruction: "Answer the user's task-scoped message. Do not edit files or claim that actions were executed. Keep the response concise and self-contained.".to_string(),
        max_turns: 1,
        ..AgentProfile::default()
    };
    let mut usage = None;
    let text = run_agent_with_profile(
        &mut provider,
        &mut ReplyOnlyTools,
        &profile,
        &prompt,
        initial_context,
        Vec::new(),
        &canceled,
        |event| {
            if let AgentEvent::ProviderUsage(value) = event {
                usage = Some(value);
            }
        },
    )?;
    let mut reply = ProviderReply::new(text);
    reply.provider = provider_reply_state(&config, usage.as_ref());
    reply.usage = provider_reply_usage(usage.as_ref());
    Ok(reply)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum EditorIntent {
    SendReply(String, String),
    Apply(String, String),
    Test(String),
    Screenshot(String),
    GenerateImage(String),
    ImportImage(String, String),
    Cancel(String),
    Retry(String),
    Reconnect(String),
}

struct DesktopEditor {
    state: EditorState,
    controller: TaskController,
    client: LiveSessionClient,
    project_root: PathBuf,
    shutdown: Arc<AtomicBool>,
}

impl DesktopEditor {
    fn new(client: LiveSessionClient, project_root: PathBuf, shutdown: Arc<AtomicBool>) -> Self {
        Self {
            state: EditorState::default(),
            controller: TaskController::new(run_reply_provider),
            client,
            project_root,
            shutdown,
        }
    }

    fn process_shortcuts(&mut self, context: &egui::Context) {
        let commands = context.input(|input| {
            input
                .events
                .iter()
                .filter_map(EditorState::chord)
                .filter_map(|chord| self.state.shortcuts.command_for(chord))
                .collect::<Vec<_>>()
        });
        for command in commands {
            self.state.dispatch(command);
        }
    }

    fn flush_intents(&mut self) {
        let intents = std::mem::take(&mut self.state.intents);
        let mut pending = Vec::with_capacity(intents.len());
        for intent in intents {
            match intent {
                EditorIntent::SendReply(task, text) => {
                    let task = TaskId::new(task);
                    let mut candidate = self.state.session.clone();
                    let accepted = candidate
                        .task_mut(&task)
                        .and_then(|task| task.append_reply(&text))
                        .map_err(|error| error.to_string())
                        .and_then(|()| {
                            if let Ok(config) = ProviderConfig::from_env() {
                                candidate
                                    .task_mut(&task)
                                    .and_then(|task| {
                                        task.set_provider_state(configured_provider_state(&config))
                                    })
                                    .map_err(|error| error.to_string())?;
                            }
                            Ok(())
                        })
                        .and_then(|()| {
                            self.controller
                                .send(&candidate, &task)
                                .map_err(|error| error.to_string())
                        });
                    match accepted {
                        Ok(_) => self.state.session = candidate,
                        Err(error) => {
                            if self.state.session.active_task_id() == Some(&task)
                                && self.state.reply.is_empty()
                            {
                                self.state.reply = text;
                            }
                            self.state.notice = Some(error);
                        }
                    }
                }
                EditorIntent::Retry(task) => {
                    let task = TaskId::new(task);
                    if let Err(error) = self.controller.retry(&mut self.state.session, &task) {
                        self.state.notice = Some(error.to_string());
                    }
                }
                EditorIntent::Cancel(task) => {
                    let task = TaskId::new(task);
                    if let Err(error) = self.controller.cancel(&mut self.state.session, &task) {
                        self.state.notice = Some(error.to_string());
                    }
                }
                EditorIntent::Reconnect(task) => {
                    let task = TaskId::new(task);
                    if let Err(error) = self.controller.reconnect(&mut self.state.session, &task) {
                        self.state.notice = Some(error.to_string());
                    }
                }
                intent => pending.push(intent),
            }
        }
        self.state.intents = pending;
    }

    fn poll_controller(&mut self) {
        for event in self.controller.poll(&mut self.state.session) {
            self.state.notice = match event {
                TaskControllerEvent::Completed { task_id, .. } => {
                    Some(format!("AI reply completed for {task_id}"))
                }
                TaskControllerEvent::Failed {
                    task_id, message, ..
                } => Some(format!("AI reply failed for {task_id}: {message}")),
                TaskControllerEvent::Canceled { task_id, .. } => {
                    Some(format!("AI reply canceled for {task_id}"))
                }
                TaskControllerEvent::Stale { task_id, .. } => {
                    Some(format!("Ignored an obsolete AI reply for {task_id}"))
                }
            };
        }
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
            let request = self.controller.snapshot(&TaskId::new(&id));
            let request_status = request
                .as_ref()
                .map(|snapshot| format!(" | {:?}", snapshot.state))
                .unwrap_or_default();
            let retries = request
                .as_ref()
                .map_or(retries, |snapshot| snapshot.retry_count);
            let label = format!(
                "{objective}\n{lifecycle:?} | {connection:?}{request_status} | {elapsed}ms | ${:.4} | retry {retries}",
                cost as f64 / 1_000_000.0
            );
            if ui
                .selectable_label(active.as_deref() == Some(&id), label)
                .clicked()
            {
                self.state.notice = self
                    .state
                    .session
                    .switch_task(&id)
                    .err()
                    .map(|e| e.to_string());
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
        let objective = self.objective.trim();
        if objective.is_empty() {
            self.focus = FocusArea::Tasks;
            return Err("Enter a task objective first.".into());
        }
        let id = format!("task-{}", self.next_task);
        self.session
            .new_task(
                id.as_str(),
                objective,
                "Stasis project; fresh task-scoped context",
            )
            .map_err(|e| e.to_string())?;
        self.next_task = self.next_task.saturating_add(1);
        self.objective.clear();
        self.reply.clear();
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
        self.session
            .switch_task(&ids[next])
            .map_err(|e| e.to_string())
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
                self.session.switch_task(id).map_err(|e| e.to_string())
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
            TaskSessionCommand::Retry => {
                let task = self.active_id()?;
                self.intents.push(EditorIntent::Retry(task));
                Ok(())
            }
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
                self.intents.push(EditorIntent::Cancel(task));
                Ok(())
            }
            TaskSessionCommand::Reconnect => {
                let task = self.active_id()?;
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
        ui.label(format!(
            "{}ms | {} input / {} output tokens | ${:.4} | {} retries",
            task.metrics.elapsed_ms,
            task.metrics.input_tokens,
            task.metrics.output_tokens,
            task.metrics.estimated_cost_micros as f64 / 1_000_000.0,
            task.metrics.retry_count,
        ));
        if let Some(request) = self.controller.snapshot(&task.id) {
            ui.label(format!(
                "Request {} | {:?} | {}ms | retry {}{}",
                request.request_id.get(),
                request.state,
                request.elapsed_ms,
                request.retry_count,
                request
                    .error
                    .as_deref()
                    .map(|error| format!(" | {error}"))
                    .unwrap_or_default(),
            ));
        }
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
                ("Retry  Ctrl+R", TaskSessionCommand::Retry),
                ("Cancel  Ctrl+Esc", TaskSessionCommand::Cancel),
                ("Reconnect  Ctrl+Shift+R", TaskSessionCommand::Reconnect),
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
        let mut chosen = None;
        egui::Window::new("Command palette  Ctrl+K")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_TOP, [0.0, 72.0])
            .show(context, |ui| {
                ui.text_edit_singleline(&mut self.state.palette_query)
                    .request_focus();
                let query = self.state.palette_query.to_ascii_lowercase();
                for (label, command) in commands {
                    if (query.is_empty() || label.to_ascii_lowercase().contains(&query))
                        && ui.button(label).clicked()
                    {
                        chosen = Some(command);
                    }
                }
            });
        if let Some(command) = chosen {
            self.state.palette_open = false;
            self.state.palette_query.clear();
            self.state.dispatch(command);
        }
    }
}

impl eframe::App for DesktopEditor {
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        if self.shutdown.load(Ordering::Acquire) {
            context.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        context.request_repaint_after(Duration::from_millis(100));
        self.poll_controller();
        self.process_shortcuts(context);
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
        self.palette(context);
        self.flush_intents();
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
    use std::sync::Barrier;

    fn task_state() -> EditorState {
        let mut state = EditorState::default();
        state.objective = "Change player speed".into();
        state.create_task().unwrap();
        state
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
    fn independent_tasks_keep_queued_replies_scoped() {
        let mut state = task_state();
        state.reply = "First reply".into();
        state.handle(TaskSessionCommand::SendReply).unwrap();
        state.objective = "Change enemy art".into();
        state.create_task().unwrap();
        state.reply = "Second reply".into();
        state.handle(TaskSessionCommand::SendReply).unwrap();
        assert!(matches!(
            state.intents.as_slice(),
            [EditorIntent::SendReply(first, first_text), EditorIntent::SendReply(second, second_text)]
                if first == "task-1" && first_text == "First reply"
                    && second == "task-2" && second_text == "Second reply"
        ));
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
    fn reconnect_and_cancel_commands_target_the_active_task() {
        let mut state = task_state();
        state.session.disconnect().unwrap();
        state.handle(TaskSessionCommand::Reconnect).unwrap();
        assert!(matches!(
            state.intents.last(),
            Some(EditorIntent::Reconnect(task)) if task == "task-1"
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
            state.intents.last(),
            Some(EditorIntent::Cancel(task)) if task == "task-2"
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
    fn reconnect_without_a_request_preserves_disconnected_state() {
        let (client, server) = live_session(4);
        let mut editor =
            DesktopEditor::new(client, PathBuf::from("."), Arc::new(AtomicBool::new(false)));
        editor.state.objective = "Reconnect task".into();
        editor.state.create_task().unwrap();
        editor.state.session.disconnect().unwrap();
        editor.state.handle(TaskSessionCommand::Reconnect).unwrap();

        editor.flush_intents();

        assert!(editor.state.intents.is_empty());
        assert!(matches!(
            editor.state.session.active_task().unwrap().connection,
            ConnectionState::Disconnected
        ));
        assert!(editor
            .state
            .notice
            .as_deref()
            .unwrap()
            .contains("no AI request"));
        assert!(server.drain(4).is_empty());
    }

    #[test]
    fn provider_usage_reads_both_supported_transport_shapes() {
        let openrouter = serde_json::json!({
            "tokens": {"prompt": 12, "completion": 7},
            "cost": 0.00125
        });
        assert_eq!(
            provider_reply_usage(Some(&openrouter)),
            ProviderUsage {
                input_tokens: 12,
                output_tokens: 7,
                estimated_cost_micros: 1_250,
            }
        );
        let codex = serde_json::json!({
            "tokens": {"input_tokens": 20, "output_tokens": 5}
        });
        assert_eq!(provider_reply_usage(Some(&codex)).input_tokens, 20);
        assert_eq!(provider_reply_usage(Some(&codex)).output_tokens, 5);
        let codex_state = provider_reply_state(
            &ProviderConfig::Codex,
            Some(&serde_json::json!({
                "resolved_provider": "installed_codex_subscription",
                "resolved_model": "test-model",
                "route": "direct"
            })),
        );
        assert!(matches!(
            codex_state.routing,
            RoutingState::Assigned { route } if route == "direct"
        ));
    }

    #[test]
    fn openrouter_response_displays_the_resolved_route_without_raw_route_json() {
        let config = ProviderConfig::OpenRouter(stasis_ai::OpenRouterConfig {
            api_key: "test-only".into(),
            base_url: "https://example.invalid".into(),
            model: "example/model".into(),
            routing: stasis_ai::RoutingConfig::default(),
            timeout: Duration::from_secs(1),
        });
        let usage = serde_json::json!({
            "resolved_provider": "cerebras",
            "resolved_model": "example/model",
            "route": {"sort": "throughput", "allow_fallbacks": true},
            "fallback": true
        });

        let state = provider_reply_state(&config, Some(&usage));

        assert_eq!(state.provider.as_deref(), Some("cerebras"));
        assert!(matches!(
            state.routing,
            RoutingState::Assigned { route } if route == "openrouter:cerebras"
        ));
        assert!(matches!(state.fallback, FallbackState::Active { .. }));
    }

    #[test]
    fn flush_does_not_send_reconnect_to_the_live_game_queue() {
        let (client, server) = live_session(1);
        let mut editor =
            DesktopEditor::new(client, PathBuf::from("."), Arc::new(AtomicBool::new(false)));
        editor.state.objective = "Reconnect task".into();
        editor.state.create_task().unwrap();
        editor.state.session.disconnect().unwrap();
        editor.state.handle(TaskSessionCommand::Reconnect).unwrap();

        editor.flush_intents();

        assert!(editor.state.intents.is_empty());
        assert!(server.drain(1).is_empty());
    }

    #[test]
    fn busy_task_restores_the_unsent_reply_draft() {
        let (client, _server) = live_session(1);
        let barrier = Arc::new(Barrier::new(2));
        let worker_barrier = Arc::clone(&barrier);
        let mut editor =
            DesktopEditor::new(client, PathBuf::from("."), Arc::new(AtomicBool::new(false)));
        editor.controller = TaskController::new(move |_, _| {
            worker_barrier.wait();
            Ok(ProviderReply::new("finished"))
        });
        editor.state.objective = "Busy task".into();
        editor.state.create_task().unwrap();
        editor.state.reply = "first".into();
        editor.state.handle(TaskSessionCommand::SendReply).unwrap();
        editor.flush_intents();
        editor.state.reply = "keep this draft".into();
        editor.state.handle(TaskSessionCommand::SendReply).unwrap();

        editor.flush_intents();

        assert_eq!(editor.state.reply, "keep this draft");
        assert_eq!(editor.state.session.active_task().unwrap().thread.len(), 1);
        assert!(editor
            .state
            .notice
            .as_deref()
            .is_some_and(|notice| notice.contains("already has an AI request")));
        barrier.wait();
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
