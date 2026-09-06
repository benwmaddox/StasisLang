use super::*;

fn editor() -> DesktopEditor {
    let (client, _server) = stasis_runner::live::live_session(8);
    let mut editor =
        DesktopEditor::new(client, PathBuf::from("."), Arc::new(AtomicBool::new(false)));
    editor.state.objective = "Improve player movement".into();
    editor.state.create_task().unwrap();
    editor.state.focus_pending = false;
    editor
}

fn frame(
    editor: &mut DesktopEditor,
    context: &egui::Context,
    size: egui::Vec2,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            events,
            ..Default::default()
        },
        |context| editor.ui(context),
    )
}

fn text_rects(output: &egui::FullOutput, wanted: &str) -> Vec<egui::Rect> {
    fn collect(shape: &egui::epaint::Shape, wanted: &str, found: &mut Vec<egui::Rect>) {
        match shape {
            egui::epaint::Shape::Text(text) if text.galley.job.text == wanted => {
                found.push(text.galley.rect.translate(text.pos.to_vec2()));
            }
            egui::epaint::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect(shape, wanted, found);
                }
            }
            _ => {}
        }
    }
    let mut found = Vec::new();
    for shape in &output.shapes {
        collect(&shape.shape, wanted, &mut found);
    }
    found
}

fn click(
    editor: &mut DesktopEditor,
    context: &egui::Context,
    size: egui::Vec2,
    position: egui::Pos2,
) {
    for pressed in [true, false] {
        frame(
            editor,
            context,
            size,
            vec![
                egui::Event::PointerMoved(position),
                egui::Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }
}

#[test]
fn header_reports_repair_and_closed_states_instead_of_validation_only() {
    let mut editor = editor();
    let task = editor.state.session.active_task_mut().unwrap();
    task.propose_action("movement", "Adjust movement").unwrap();
    task.accept_action("movement").unwrap();
    task.apply_action("movement").unwrap();
    task.mark_action_for_repair("movement", "Fix cooldown")
        .unwrap();
    assert_eq!(task_header_status(task), ("needs repair", failure()));
    task.cancel().unwrap();
    assert_eq!(task_header_status(task), ("canceled", failure()));
    let mut completed = stasis_ai::Task::new("done", "Completed objective", "project").unwrap();
    completed.lifecycle = TaskLifecycle::Completed;
    assert_eq!(task_header_status(&completed), ("done", accent()));
}

#[test]
fn pointer_accepts_displayed_action_and_hover_uses_interactive_style() {
    let mut editor = editor();
    editor
        .state
        .session
        .propose_action("z-first", "First chronological proposal")
        .unwrap();
    editor
        .state
        .session
        .propose_action("a-second", "Second chronological proposal")
        .unwrap();
    let context = egui::Context::default();
    let size = egui::vec2(1100.0, 900.0);
    frame(&mut editor, &context, size, vec![]);
    let output = frame(&mut editor, &context, size, vec![]);
    let buttons = text_rects(&output, "Accept");
    assert_eq!(buttons.len(), 2);
    let position = buttons[0].center();
    let hovered = frame(
        &mut editor,
        &context,
        size,
        vec![egui::Event::PointerMoved(position)],
    );
    fn has_hover(shape: &egui::epaint::Shape, position: egui::Pos2) -> bool {
        match shape {
            egui::epaint::Shape::Rect(rect) => {
                rect.rect.contains(position) && rect.fill == Color32::from_rgb(29, 42, 56)
            }
            egui::epaint::Shape::Vec(shapes) => {
                shapes.iter().any(|shape| has_hover(shape, position))
            }
            _ => false,
        }
    }
    assert!(hovered
        .shapes
        .iter()
        .any(|shape| has_hover(&shape.shape, position)));
    click(&mut editor, &context, size, position);
    let task = editor.state.session.active_task().unwrap();
    assert_eq!(task.actions["z-first"].state, ActionState::Accepted);
    assert_eq!(task.actions["a-second"].state, ActionState::Proposed);
}

#[test]
fn composer_stays_visible_at_narrow_wide_and_high_dpi_sizes() {
    for (width, height, scale) in [
        (520.0, 600.0, 1.0),
        (680.0, 900.0, 1.0),
        (1440.0, 900.0, 1.0),
        (900.0, 700.0, 2.0),
    ] {
        let mut editor = editor();
        for index in 0..30 {
            editor
                .state
                .session
                .append_reply(format!(
                    "Message {index}: keep the composer visible as this task grows."
                ))
                .unwrap();
        }
        let context = egui::Context::default();
        context.set_pixels_per_point(scale);
        let size = egui::vec2(width, height);
        frame(&mut editor, &context, size, vec![]);
        let output = frame(&mut editor, &context, size, vec![]);
        for label in ["Reply to Stasis AI...", "Send to AI"] {
            let rects = text_rects(&output, label);
            assert_eq!(rects.len(), 1, "{width}x{height}@{scale}: {label}");
            assert!(
                egui::Rect::from_min_size(egui::Pos2::ZERO, size).contains_rect(rects[0]),
                "{width}x{height}@{scale}: {label} clipped: {:?}",
                rects[0]
            );
        }
    }
}

#[test]
fn disabled_send_ignores_pointer_and_focus_command_allows_typing() {
    let mut editor = editor();
    let context = egui::Context::default();
    let size = egui::vec2(1100.0, 900.0);
    frame(&mut editor, &context, size, vec![]);
    let output = frame(&mut editor, &context, size, vec![]);
    let send = text_rects(&output, "Send to AI")[0].center();
    click(&mut editor, &context, size, send);
    assert!(editor.controller.snapshot(&TaskId::new("task-1")).is_none());
    assert!(editor
        .state
        .session
        .active_task()
        .unwrap()
        .thread
        .is_empty());
    editor.state.handle(TaskSessionCommand::NewTask).unwrap();
    frame(&mut editor, &context, size, vec![]);
    let objective_focus = context.memory(|memory| memory.focused()).unwrap();
    editor.state.handle(TaskSessionCommand::FocusReply).unwrap();
    frame(&mut editor, &context, size, vec![]);
    assert_ne!(
        context.memory(|memory| memory.focused()).unwrap(),
        objective_focus
    );
    frame(
        &mut editor,
        &context,
        size,
        vec![egui::Event::Text("Keep the draft task-local".into())],
    );
    assert_eq!(editor.state.reply, "Keep the draft task-local");
}

#[test]
fn header_usage_never_overlaps_provider_at_compact_widths() {
    for width in [520.0, 680.0, 900.0] {
        let mut editor = editor();
        let task = editor.state.session.active_task_mut().unwrap();
        task.set_provider_state(ProviderState {
            provider: Some("installed_codex_subscription".into()),
            model: Some("gpt-5.6-sol".into()),
            ..ProviderState::default()
        })
        .unwrap();
        task.record_turn(1840, 2410, 386, 1200).unwrap();
        let context = egui::Context::default();
        let size = egui::vec2(width, 900.0);
        frame(&mut editor, &context, size, vec![]);
        let output = frame(&mut editor, &context, size, vec![]);
        let provider = text_rects(&output, "Provider: Codex / gpt-5.6-sol  v")[0];
        let usage = text_rects(&output, "Usage  2796 tokens")[0];
        assert!(
            usage.min.y > provider.max.y,
            "header rows overlap at {width}: {provider:?} / {usage:?}"
        );
    }
}

#[test]
fn provider_request_disables_new_work_only_on_its_own_task() {
    let mut editor = editor();
    let (release, waiting) = mpsc::channel();
    let waiting = Mutex::new(waiting);
    editor.controller = TaskController::new(move |_, _| {
        waiting
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        Ok(ProviderReply::new("complete"))
    });
    editor
        .controller
        .send(&editor.state.session, &TaskId::new("task-1"))
        .unwrap();
    assert!(editor.ui_busy(editor.state.session.active_task().unwrap()));
    let context = egui::Context::default();
    let size = egui::vec2(1100.0, 900.0);
    frame(&mut editor, &context, size, vec![]);
    let output = frame(&mut editor, &context, size, vec![]);
    assert_eq!(text_rects(&output, "Cancel task").len(), 1);
    assert!(text_rects(&output, "Send to AI").is_empty());
    editor.state.objective = "Independent task".into();
    editor.state.create_task().unwrap();
    assert!(!editor.ui_busy(editor.state.session.active_task().unwrap()));
    release.send(()).unwrap();
}

#[test]
fn cancel_requires_confirmation_and_keeps_originating_task_identity() {
    let mut editor = editor();
    let context = egui::Context::default();
    let size = egui::vec2(1100.0, 900.0);
    editor.state.handle(TaskSessionCommand::Cancel).unwrap();
    editor.flush_intents();
    assert_eq!(
        editor.state.session.active_task().unwrap().lifecycle,
        TaskLifecycle::Active
    );
    frame(&mut editor, &context, size, vec![]);
    let output = frame(&mut editor, &context, size, vec![]);
    click(
        &mut editor,
        &context,
        size,
        text_rects(&output, "Keep task open")[0].center(),
    );
    assert!(editor.state.cancel_confirmation.is_none());
    assert_eq!(
        editor.state.session.active_task().unwrap().lifecycle,
        TaskLifecycle::Active
    );
    editor.state.handle(TaskSessionCommand::Cancel).unwrap();
    editor.state.objective = "Another task".into();
    editor.state.create_task().unwrap();
    frame(&mut editor, &context, size, vec![]);
    let output = frame(&mut editor, &context, size, vec![]);
    click(
        &mut editor,
        &context,
        size,
        text_rects(&output, "Permanently cancel task")[0].center(),
    );
    assert_eq!(
        editor.state.session.task("task-1").unwrap().lifecycle,
        TaskLifecycle::Canceled
    );
    assert_eq!(
        editor.state.session.active_task().unwrap().lifecycle,
        TaskLifecycle::Active
    );
}

#[test]
fn unavailable_image_intents_settle_once_without_importing_assets() {
    let mut editor = editor();
    editor
        .state
        .session
        .add_generated_image(
            "image",
            "missing.png",
            stasis_ai::task_session::ImageAttribution::new("fixture", None, None).unwrap(),
        )
        .unwrap();
    editor
        .state
        .session
        .approve_generated_image("image")
        .unwrap();
    editor
        .state
        .intents
        .push(EditorIntent::GenerateImage("task-1".into()));
    editor
        .state
        .intents
        .push(EditorIntent::ImportImage("task-1".into(), "image".into()));
    editor.flush_intents();
    assert!(editor.state.intents.is_empty());
    assert!(editor
        .state
        .notice
        .as_deref()
        .unwrap()
        .contains("unavailable"));
    let before = editor.state.session.clone();
    editor.flush_intents();
    assert_eq!(editor.state.session, before);
    assert_eq!(
        editor.state.session.active_task().unwrap().generated_images["image"].handoff,
        ImageHandoffState::Pending
    );
}

#[test]
fn disconnected_task_can_open_provider_menu() {
    let mut editor = editor();
    editor.state.session.disconnect().unwrap();
    let context = egui::Context::default();
    let size = egui::vec2(1100.0, 900.0);
    frame(&mut editor, &context, size, vec![]);
    let output = frame(&mut editor, &context, size, vec![]);
    let label = "Provider: Provider pending / model pending  v";
    click(
        &mut editor,
        &context,
        size,
        text_rects(&output, label)[0].center(),
    );
    let output = frame(&mut editor, &context, size, vec![]);
    assert_eq!(text_rects(&output, "Codex subscription").len(), 1);
}
