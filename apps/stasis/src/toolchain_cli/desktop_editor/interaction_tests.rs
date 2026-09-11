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

fn key_event(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }
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

fn visible_text_starting_with(
    output: &egui::FullOutput,
    wanted: &str,
) -> Vec<(String, egui::Rect)> {
    fn collect(
        shape: &egui::epaint::Shape,
        clip: egui::Rect,
        wanted: &str,
        found: &mut Vec<(String, egui::Rect)>,
    ) {
        match shape {
            egui::epaint::Shape::Text(text) if text.galley.job.text.starts_with(wanted) => {
                found.push((
                    text.galley.job.text.clone(),
                    text.galley
                        .rect
                        .translate(text.pos.to_vec2())
                        .intersect(clip),
                ));
            }
            egui::epaint::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect(shape, clip, wanted, found);
                }
            }
            _ => {}
        }
    }
    let mut found = Vec::new();
    for shape in &output.shapes {
        collect(&shape.shape, shape.clip_rect, wanted, &mut found);
    }
    found
}

fn accesskit_nodes(output: &egui::FullOutput) -> Vec<&egui::accesskit::Node> {
    let update = output
        .platform_output
        .accesskit_update
        .as_ref()
        .expect("accessibility tree update");
    fn visit<'a>(
        id: egui::accesskit::NodeId,
        nodes: &'a [(egui::accesskit::NodeId, egui::accesskit::Node)],
        ordered: &mut Vec<&'a egui::accesskit::Node>,
    ) {
        let node = nodes
            .iter()
            .find_map(|(candidate, node)| (*candidate == id).then_some(node))
            .expect("accessibility child node");
        ordered.push(node);
        for child in node.children() {
            visit(*child, nodes, ordered);
        }
    }
    let mut ordered = Vec::new();
    visit(
        update.tree.as_ref().expect("accessibility tree").root,
        &update.nodes,
        &mut ordered,
    );
    ordered
}

fn accesskit_node_named<'a>(
    nodes: &'a [&egui::accesskit::Node],
    name: &str,
) -> &'a egui::accesskit::Node {
    nodes
        .iter()
        .copied()
        .find(|node| node.name() == Some(name))
        .unwrap_or_else(|| panic!("missing accessible node named {name:?}"))
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
fn ready_proposals_auto_apply_in_chronological_order_without_accept_buttons() {
    let (mut editor, root, payload) = super::tests::review_fixture("timeline_pointer_preview");
    editor
        .state
        .session
        .active_task_mut()
        .unwrap()
        .actions
        .clear();
    editor
        .state
        .session
        .active_task_mut()
        .unwrap()
        .activity
        .clear();
    for (id, description) in [
        ("z-first", "First chronological proposal"),
        ("a-second", "Second chronological proposal"),
    ] {
        editor
            .state
            .session
            .active_task_mut()
            .unwrap()
            .propose_action_with_payload(
                id,
                stasis_ai::ActionKind::Edit,
                description,
                payload.clone(),
            )
            .unwrap();
    }
    super::tests::finish_preview(&mut editor);
    let context = egui::Context::default();
    let size = egui::vec2(1100.0, 1400.0);
    frame(&mut editor, &context, size, vec![]);
    let output = frame(&mut editor, &context, size, vec![]);
    assert!(text_rects(&output, "Accept").is_empty());
    let task = editor.state.session.active_task().unwrap();
    assert_eq!(task.actions["z-first"].state, ActionState::Accepted);
    assert_eq!(task.actions["a-second"].state, ActionState::Proposed);
    assert!(editor.busy_tasks.contains("task-1"));
    std::fs::remove_dir_all(root).unwrap();
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
        editor
            .state
            .session
            .append_result("Ready for review")
            .unwrap();
        let context = egui::Context::default();
        context.set_pixels_per_point(scale);
        let size = egui::vec2(width, height);
        frame(&mut editor, &context, size, vec![]);
        let output = frame(&mut editor, &context, size, vec![]);
        let labels: &[&str] = if width < 760.0 {
            &[
                "Reply to Stasis AI...",
                "Attach",
                "Send",
                "Success",
                "Reject",
            ]
        } else {
            &[
                "Reply to Stasis AI...",
                "Attach image",
                "Send (Ctrl+Enter)",
                "Success (Ctrl+Shift+D)",
                "Reject (Ctrl+Esc)",
            ]
        };
        for label in labels {
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
fn compact_and_wide_layouts_expose_named_accessible_controls() {
    for width in [520.0, 1100.0] {
        let mut editor = editor();
        let context = egui::Context::default();
        context.enable_accesskit();
        let size = egui::vec2(width, 900.0);
        let output = frame(&mut editor, &context, size, vec![]);
        let nodes = accesskit_nodes(&output);

        assert_eq!(
            accesskit_node_named(&nodes, "Reply to Stasis AI about Improve player movement").role(),
            egui::accesskit::Role::MultilineTextInput
        );
        if width < 760.0 {
            let node_names = nodes.iter().map(|node| node.name()).collect::<Vec<_>>();
            assert!(!node_names.contains(&Some("New task objective")));
            assert!(!node_names.contains(&Some(
                "Provider and model. Current selection: Provider pending, model pending"
            )));
            assert_eq!(
                accesskit_node_named(&nodes, "Ctrl+K").role(),
                egui::accesskit::Role::Button
            );
            let mut new_task_editor = self::editor();
            new_task_editor.state.dispatch(TaskSessionCommand::NewTask);
            let new_task_context = egui::Context::default();
            new_task_context.enable_accesskit();
            let output = frame(&mut new_task_editor, &new_task_context, size, vec![]);
            let nodes = accesskit_nodes(&output);
            assert_eq!(
                accesskit_node_named(&nodes, "New task objective").role(),
                egui::accesskit::Role::TextInput
            );
        } else {
            let task_name = "Task: Improve player movement. Status: current";
            assert_eq!(
                accesskit_node_named(&nodes, "New task objective").role(),
                egui::accesskit::Role::TextInput
            );
            assert_eq!(
                accesskit_node_named(&nodes, task_name).role(),
                egui::accesskit::Role::ToggleButton
            );
            assert_eq!(
                accesskit_node_named(
                    &nodes,
                    "Provider and model. Current selection: Provider pending, model pending"
                )
                .role(),
                egui::accesskit::Role::ComboBox
            );
        }
    }
}

#[test]
fn compact_layout_honors_new_task_focus_before_typing() {
    let mut editor = editor();
    editor.state.objective.clear();
    editor.state.focus = FocusArea::Tasks;
    editor.state.focus_pending = true;
    let context = egui::Context::default();
    let size = egui::vec2(520.0, 700.0);

    frame(&mut editor, &context, size, vec![]);
    frame(
        &mut editor,
        &context,
        size,
        vec![egui::Event::Text("Describe the next task".into())],
    );

    assert_eq!(editor.state.objective, "Describe the next task");
    assert!(!editor.state.focus_pending);
}

#[test]
fn compact_layout_hides_queued_task_chrome_behind_the_task_menu() {
    let mut editor = editor();
    let active_objective = editor
        .state
        .session
        .active_task()
        .unwrap()
        .objective
        .clone();
    for objective in [
        "Add an arena tileset",
        "Polish the pause menu",
        "Add dash ability",
    ] {
        editor.state.objective = objective.into();
        editor.state.create_task().unwrap();
    }
    let context = egui::Context::default();
    let size = egui::vec2(520.0, 700.0);

    frame(&mut editor, &context, size, vec![]);
    let output = frame(&mut editor, &context, size, vec![]);
    let active = text_rects(&output, &active_objective)
        .into_iter()
        .find(|rect| rect.max.y < 150.0)
        .expect("active compact task header");

    assert!(
        egui::Rect::from_min_size(egui::Pos2::ZERO, size).contains_rect(active),
        "active compact header remains clipped: {active:?}"
    );
    for queued in [
        "Add an arena tileset",
        "Polish the pause menu",
        "Add dash ability",
    ] {
        assert!(
            text_rects(&output, queued).is_empty(),
            "showed queued task {queued}"
        );
    }
}

#[test]
fn visuals_use_high_contrast_boundaries_and_no_transition_motion() {
    fn contrast_ratio(a: Color32, b: Color32) -> f32 {
        let luminance = |color: Color32| {
            let channel = |value: u8| {
                let value = value as f32 / 255.0;
                if value <= 0.04045 {
                    value / 12.92
                } else {
                    ((value + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * channel(color.r()) + 0.7152 * channel(color.g()) + 0.0722 * channel(color.b())
        };
        let (lighter, darker) = if luminance(a) >= luminance(b) {
            (luminance(a), luminance(b))
        } else {
            (luminance(b), luminance(a))
        };
        (lighter + 0.05) / (darker + 0.05)
    }

    let context = egui::Context::default();
    configure_visuals(&context);
    let style = context.style();

    assert_eq!(style.animation_time, 0.0);
    assert_eq!(style.visuals.widgets.inactive.bg_stroke.color, border());
    assert_eq!(
        style.visuals.widgets.noninteractive.fg_stroke.color,
        muted_text()
    );
    assert!(contrast_ratio(border(), panel_fill()) >= 3.0);
    assert!(contrast_ratio(muted_text(), panel_fill()) >= 4.5);
}

#[test]
fn disabled_send_ignores_pointer_and_focus_command_allows_typing() {
    let mut editor = editor();
    let context = egui::Context::default();
    let size = egui::vec2(1100.0, 900.0);
    frame(&mut editor, &context, size, vec![]);
    let output = frame(&mut editor, &context, size, vec![]);
    let send = text_rects(&output, "Send (Ctrl+Enter)")[0].center();
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
fn ready_task_keeps_send_success_and_reject_choices_visible() {
    let mut editor = editor();
    editor.controller = TaskController::new(|_, _| Ok(ProviderReply::new("acknowledged")));
    editor.state.session.begin_focused_tests().unwrap();
    editor
        .state
        .session
        .finish_focused_tests(stasis_ai::FocusedTestResult::passed("ready"))
        .unwrap();
    editor.validation_fingerprints.insert(
        "task-1".into(),
        ("fixture-source".into(), vec!["focused".into()]),
    );
    editor.state.reply = "Please make one last small adjustment".into();

    let context = egui::Context::default();
    let size = egui::vec2(940.0, 900.0);
    frame(&mut editor, &context, size, vec![]);
    let output = frame(&mut editor, &context, size, vec![]);

    assert_eq!(text_rects(&output, "Attach image").len(), 1);
    assert_eq!(text_rects(&output, "Send (Ctrl+Enter)").len(), 1);
    assert_eq!(text_rects(&output, "Success (Ctrl+Shift+D)").len(), 1);
    assert_eq!(text_rects(&output, "Reject (Ctrl+Esc)").len(), 1);

    let send = text_rects(&output, "Send (Ctrl+Enter)")[0].center();
    click(&mut editor, &context, size, send);
    assert!(editor
        .state
        .session
        .active_task()
        .unwrap()
        .thread
        .iter()
        .any(|message| message.text == "Please make one last small adjustment"));
    assert_eq!(
        editor.state.session.active_task().unwrap().lifecycle,
        TaskLifecycle::Active
    );
}

#[test]
fn compact_reply_shows_only_the_transcript_and_four_outcomes() {
    let mut editor = editor();
    let reply = "Implemented dash movement and verified cooldown boundaries without changing deterministic collision behavior.";
    let task = editor.state.session.active_task_mut().unwrap();
    task.append_result(reply).unwrap();
    task.begin_focused_tests().unwrap();
    task.finish_focused_tests(stasis_ai::FocusedTestResult::passed(
        "Dash distance and cooldown boundaries passed.",
    ))
    .unwrap();
    editor.validation_fingerprints.insert(
        "task-1".into(),
        ("fixture-source".into(), vec!["focused".into()]),
    );

    let context = egui::Context::default();
    let size = egui::vec2(366.0, 900.0);
    frame(&mut editor, &context, size, vec![]);
    let output = frame(&mut editor, &context, size, vec![]);

    for label in ["Attach", "Send", "Success", "Reject"] {
        assert_eq!(text_rects(&output, label).len(), 1, "missing {label}");
    }
    for hidden in [
        "New task objective",
        "Tile Editor + Game",
        "Export chat as HTML",
        "Usage  0 tokens",
        "Success (Ctrl+Shift+D)",
    ] {
        assert!(text_rects(&output, hidden).is_empty(), "showed {hidden}");
    }
    assert_eq!(text_rects(&output, "Stasis AI").len(), 1);
    assert_eq!(text_rects(&output, "Applied / tests passed").len(), 1);
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
    assert!(
        screen.contains_rect(text_rects(&output, reply)[0]),
        "compact transcript text must wrap inside the viewport"
    );
    let buttons =
        ["Attach", "Send", "Success", "Reject"].map(|label| text_rects(&output, label)[0]);
    for pair in buttons.windows(2) {
        assert!(
            pair[0].max.x <= pair[1].min.x,
            "compact outcome buttons overlap"
        );
    }
}

#[test]
fn compact_running_request_replaces_outcomes_with_stop() {
    let mut editor = editor();
    let (release, waiting) = mpsc::channel();
    let waiting = Mutex::new(waiting);
    editor.controller = TaskController::new(move |_, _| {
        waiting
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        Ok(ProviderReply::new("late"))
    });
    editor
        .controller
        .send(&mut editor.state.session, &TaskId::new("task-1"))
        .unwrap();

    let context = egui::Context::default();
    let size = egui::vec2(366.0, 900.0);
    frame(&mut editor, &context, size, vec![]);
    let output = frame(&mut editor, &context, size, vec![]);
    assert_eq!(text_rects(&output, "Stop (Esc)").len(), 1);
    for hidden in ["Attach", "Send", "Success", "Reject"] {
        assert!(text_rects(&output, hidden).is_empty(), "showed {hidden}");
    }

    frame(
        &mut editor,
        &context,
        size,
        vec![key_event(egui::Key::Escape, egui::Modifiers::NONE)],
    );
    assert_eq!(
        editor.state.session.active_task().unwrap().lifecycle,
        TaskLifecycle::Active
    );
    assert_eq!(
        editor
            .controller
            .snapshot(&TaskId::new("task-1"))
            .unwrap()
            .state,
        stasis_ai::TaskRequestState::Canceled
    );
    release.send(()).unwrap();
}

#[test]
fn wide_header_usage_never_overlaps_provider() {
    for width in [800.0, 900.0] {
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
fn compact_chrome_reserves_space_for_notices_task_creation_and_status() {
    let notice = "AI reply completed for task-1; 1 action(s) proposed";
    let objective = "Make the paddle slightly wider without changing collision timing";
    for width in [420.0, 520.0, 620.0] {
        let mut editor = editor();
        editor.project_root = PathBuf::from(
            "a-very-long-project-directory-name-that-must-not-hide-task-creation-controls",
        );
        editor.state.notice = Some(notice.into());
        editor.state.session.active_task_mut().unwrap().objective = objective.into();
        let context = egui::Context::default();
        let size = egui::vec2(width, 949.0);

        frame(&mut editor, &context, size, vec![]);
        let output = frame(&mut editor, &context, size, vec![]);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
        let stasis = text_rects(&output, "Stasis")[0];
        let notice_rect = text_rects(&output, notice)[0];
        let title = visible_text_starting_with(&output, "Make the paddle")
            .into_iter()
            .map(|(_, rect)| rect)
            .next()
            .expect("compact task header title");

        assert!(
            notice_rect.min.y >= stasis.max.y,
            "top rows overlap at {width}"
        );
        assert!(screen.contains_rect(title));
        assert!(text_rects(&output, "New task objective").is_empty());
        assert!(text_rects(&output, "Provider: Codex / gpt-5.6-sol  v").is_empty());

        editor.state.dispatch(TaskSessionCommand::NewTask);
        frame(&mut editor, &context, size, vec![]);
        let output = frame(&mut editor, &context, size, vec![]);
        let objective_input = text_rects(&output, "New task objective")[0];
        let create = text_rects(&output, "+ Task")[0];
        assert!(screen.contains_rect(objective_input));
        assert!(screen.contains_rect(create));
        assert!(objective_input.max.x < create.min.x);
        editor.state.objective = "Create from the compact header".into();
        click(&mut editor, &context, size, create.center());
        assert_eq!(
            editor.state.session.task("task-2").unwrap().objective,
            "Create from the compact header"
        );
        assert_eq!(
            editor.state.session.task("task-2").unwrap().lifecycle,
            TaskLifecycle::Queued
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
        .send(&mut editor.state.session, &TaskId::new("task-1"))
        .unwrap();
    assert!(editor.ui_busy(editor.state.session.active_task().unwrap()));
    let context = egui::Context::default();
    let size = egui::vec2(1100.0, 900.0);
    frame(&mut editor, &context, size, vec![]);
    let output = frame(&mut editor, &context, size, vec![]);
    assert_eq!(text_rects(&output, "Reject (Ctrl+Esc)").len(), 1);
    assert_eq!(text_rects(&output, "Send (Ctrl+Enter)").len(), 1);
    editor.state.objective = "Independent task".into();
    editor.state.create_task().unwrap();
    assert!(editor.ui_busy(editor.state.session.active_task().unwrap()));
    assert!(!editor.ui_busy(editor.state.session.task("task-2").unwrap()));
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
        text_rects(&output, "Keep task (Esc)")[0].center(),
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
    assert_eq!(text_rects(&output, "Reject task (Enter)").len(), 1);
    frame(
        &mut editor,
        &context,
        size,
        vec![key_event(egui::Key::Enter, egui::Modifiers::NONE)],
    );
    editor.flush_intents();
    assert_eq!(
        editor.state.session.task("task-1").unwrap().lifecycle,
        TaskLifecycle::Canceled
    );
    assert_eq!(
        editor.state.session.active_task().unwrap().lifecycle,
        TaskLifecycle::Queued
    );
}

#[test]
fn disconnected_active_task_keeps_simple_outcomes_visible() {
    let mut editor = editor();
    editor.state.session.disconnect().unwrap();
    let context = egui::Context::default();
    let size = egui::vec2(680.0, 900.0);
    frame(&mut editor, &context, size, vec![]);
    let output = frame(&mut editor, &context, size, vec![]);
    assert!(text_rects(&output, "Reconnect").is_empty());
    assert_eq!(text_rects(&output, "Send").len(), 1);
    assert!(text_rects(&output, "Success").is_empty());
    assert_eq!(text_rects(&output, "Reject").len(), 1);

    frame(
        &mut editor,
        &context,
        size,
        vec![key_event(egui::Key::Escape, egui::Modifiers::CTRL)],
    );
    assert_eq!(editor.state.cancel_confirmation.as_deref(), Some("task-1"));
}

#[test]
fn queue_gate_moves_and_starts_only_the_next_task() {
    let mut editor = editor();
    for objective in ["Second task", "Third task"] {
        editor.state.objective = objective.into();
        editor.state.create_task().unwrap();
    }
    editor
        .state
        .session
        .task_mut("task-1")
        .unwrap()
        .cancel()
        .unwrap();
    editor
        .state
        .session
        .select_queue_gate_after(&TaskId::new("task-1"));

    let context = egui::Context::default();
    let size = egui::vec2(1100.0, 900.0);
    let output = frame(&mut editor, &context, size, vec![]);
    assert_eq!(text_rects(&output, "Start task (Enter)").len(), 1);
    assert_eq!(text_rects(&output, "Move to back (B)").len(), 1);
    assert_eq!(text_rects(&output, "Reject... (Del)").len(), 1);
    frame(
        &mut editor,
        &context,
        size,
        vec![key_event(egui::Key::B, egui::Modifiers::NONE)],
    );
    assert_eq!(editor.state.active_id().unwrap(), "task-3");

    frame(
        &mut editor,
        &context,
        size,
        vec![key_event(egui::Key::Enter, egui::Modifiers::NONE)],
    );
    assert_eq!(
        editor.state.session.task("task-3").unwrap().lifecycle,
        TaskLifecycle::Active
    );
    let started = editor.state.session.task("task-3").unwrap();
    assert_eq!(started.thread.len(), 1);
    assert_eq!(started.thread[0].text, "Third task");
}

#[test]
fn compact_queue_gate_shows_only_the_next_decision() {
    let mut editor = editor();
    for objective in ["Add an arena tileset", "Polish the pause menu"] {
        editor.state.objective = objective.into();
        editor.state.create_task().unwrap();
    }
    editor
        .state
        .session
        .task_mut("task-1")
        .unwrap()
        .cancel()
        .unwrap();
    editor
        .state
        .session
        .select_queue_gate_after(&TaskId::new("task-1"));

    let context = egui::Context::default();
    let size = egui::vec2(366.0, 900.0);
    frame(&mut editor, &context, size, vec![]);
    let output = frame(&mut editor, &context, size, vec![]);
    assert_eq!(text_rects(&output, "Next task").len(), 1);
    assert_eq!(text_rects(&output, "Start (Enter)").len(), 1);
    assert_eq!(text_rects(&output, "Back (B)").len(), 1);
    assert_eq!(text_rects(&output, "Reject (Del)").len(), 1);
    assert_eq!(
        text_rects(&output, "After this: Polish the pause menu").len(),
        1
    );
    for hidden in ["New task objective", "Reply to Stasis AI...", "Provider"] {
        assert!(text_rects(&output, hidden).is_empty(), "showed {hidden}");
    }
}

#[test]
fn queue_reject_shortcut_still_requires_confirmation() {
    let mut editor = editor();
    editor.state.objective = "Queued task".into();
    editor.state.create_task().unwrap();
    editor
        .state
        .session
        .task_mut("task-1")
        .unwrap()
        .cancel()
        .unwrap();
    editor
        .state
        .session
        .select_queue_gate_after(&TaskId::new("task-1"));

    let context = egui::Context::default();
    frame(
        &mut editor,
        &context,
        egui::vec2(680.0, 900.0),
        vec![key_event(egui::Key::Delete, egui::Modifiers::NONE)],
    );
    assert_eq!(editor.state.cancel_confirmation.as_deref(), Some("task-2"));
    assert_eq!(
        editor.state.session.task("task-2").unwrap().lifecycle,
        TaskLifecycle::Queued
    );
}

#[test]
fn queue_rollback_shortcut_requires_confirmation_and_keeps_task_queued() {
    let mut editor = editor();
    editor.state.objective = "Queued task".into();
    editor.state.create_task().unwrap();
    let completed = editor.state.session.task_mut("task-1").unwrap();
    completed.cancel().unwrap();
    completed.lifecycle = TaskLifecycle::Completed;
    editor.completion_commits.insert(
        "task-1".into(),
        stasis_ai::session_store::TaskCompletionCommit {
            commit: "0123456789012345678901234567890123456789".into(),
            paths: Vec::new(),
            reverted_by: None,
        },
    );
    editor
        .state
        .session
        .select_queue_gate_after(&TaskId::new("task-1"));

    let context = egui::Context::default();
    frame(
        &mut editor,
        &context,
        egui::vec2(680.0, 900.0),
        vec![key_event(egui::Key::R, egui::Modifiers::NONE)],
    );
    assert_eq!(
        editor.rollback_confirmation,
        Some(("task-2".into(), "task-1".into()))
    );
    assert_eq!(
        editor.state.session.task("task-2").unwrap().lifecycle,
        TaskLifecycle::Queued
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
