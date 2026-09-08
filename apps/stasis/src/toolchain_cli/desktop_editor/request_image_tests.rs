use super::*;

#[test]
fn text_only_send_preserves_completed_images_and_allows_done() {
    let (client, _server) = stasis_runner::live::live_session(4);
    let mut editor =
        DesktopEditor::new(client, PathBuf::from("."), Arc::new(AtomicBool::new(false)));
    editor.state.objective = "Inspect pixels".into();
    editor.state.create_task().unwrap();
    let task_id = editor.state.session.active_task_id().unwrap().clone();
    let task = editor.state.session.task_mut(&task_id).unwrap();
    task.set_vision_capability(true).unwrap();
    task.attach_screenshot("done", "done.png").unwrap();
    task.complete_screenshot_analysis("done").unwrap();
    let completed = task.screenshots[&stasis_ai::ScreenshotId::new("done")].clone();
    let (sent, received) = mpsc::channel();
    editor.controller = TaskController::new(move |request, _| {
        sent.send(request.screenshots).unwrap();
        Ok(ProviderReply::new("text reply"))
    });
    editor.state.intents.push(EditorIntent::SendReply(
        task_id.to_string(),
        "Thanks".into(),
    ));
    editor.flush_intents();
    assert!(received
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .is_empty());
    let deadline = Instant::now() + Duration::from_secs(2);
    while editor.controller.poll(&mut editor.state.session).is_empty() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(1));
    }
    let task = editor.state.session.task_mut(&task_id).unwrap();
    assert_eq!(
        task.screenshots[&stasis_ai::ScreenshotId::new("done")],
        completed
    );
    task.validation = ValidationStatus::Passed {
        summary: "validated".into(),
    };
    task.mark_done().unwrap();
}

#[test]
fn admission_resets_only_images_selected_for_this_request() {
    let mut state = EditorState::default();
    state.objective = "Inspect again".into();
    state.create_task().unwrap();
    let task_id = state.session.active_task_id().unwrap().clone();
    let task = state.session.task_mut(&task_id).unwrap();
    task.set_vision_capability(true).unwrap();
    for id in ["selected", "retained"] {
        task.attach_screenshot(id, format!("{id}.png")).unwrap();
        task.complete_screenshot_analysis(id).unwrap();
    }
    task.select_screenshot_for_request("selected").unwrap();
    let controller = TaskController::new(|_, _| Ok(ProviderReply::new("done")));
    controller.send(&mut state.session, &task_id).unwrap();
    let task = state.session.task(&task_id).unwrap();
    assert_eq!(
        task.screenshots[&stasis_ai::ScreenshotId::new("selected")].upload,
        UploadState::Pending
    );
    assert_eq!(
        task.screenshots[&stasis_ai::ScreenshotId::new("selected")].analysis,
        ScreenshotAnalysisState::Pending
    );
    assert_eq!(
        task.screenshots[&stasis_ai::ScreenshotId::new("retained")].upload,
        UploadState::Uploaded
    );
    assert_eq!(
        task.screenshots[&stasis_ai::ScreenshotId::new("retained")].analysis,
        ScreenshotAnalysisState::Completed
    );
}

#[test]
fn dispatch_checks_task_request_consent_and_exact_content() {
    let mut state = EditorState::default();
    state.objective = "Inspect selected pixels".into();
    state.create_task().unwrap();
    let task_id = state.session.active_task_id().unwrap().clone();
    let mut store = SessionAttachmentStore::new();
    let image = store
        .insert_rgba(
            &task_id,
            "selected".into(),
            "clipboard.png".into(),
            AttachmentOrigin::Clipboard,
            1,
            1,
            &[17, 34, 51, 255],
        )
        .unwrap();
    let task = state.session.task_mut(&task_id).unwrap();
    task.set_vision_capability(true).unwrap();
    task.attach_screenshot_with_sha256("selected", image.path.display().to_string(), &image.sha256)
        .unwrap();
    task.select_screenshot_for_request("selected").unwrap();
    let (tx, rx) = mpsc::channel();
    let controller = TaskController::new(move |request, _| {
        tx.send(request).unwrap();
        Ok(ProviderReply::new("inspected"))
    });
    controller.send(&mut state.session, &task_id).unwrap();
    let request = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let config = ProviderConfig::OpenRouter(stasis_ai::OpenRouterConfig {
        api_key: "local-test-only".into(),
        base_url: "http://127.0.0.1:1".into(),
        model: "test/vision".into(),
        routing: stasis_ai::RoutingConfig::default(),
        timeout: Duration::from_secs(1),
    });
    assert_eq!(
        verified_provider_screenshot_paths(&config, &request).unwrap(),
        vec![image.path.clone()]
    );
    let mut changed = request.clone();
    changed.screenshots[0].provenance.task_id = TaskId::new("other");
    assert!(verified_provider_screenshot_paths(&config, &changed)
        .unwrap_err()
        .contains("provider task"));
    changed = request.clone();
    changed.screenshots[0].request_id = None;
    assert!(verified_provider_screenshot_paths(&config, &changed)
        .unwrap_err()
        .contains("provider request"));
    changed = request.clone();
    changed.screenshots[0].consent_to_send = false;
    assert!(verified_provider_screenshot_paths(&config, &changed)
        .unwrap_err()
        .contains("consent"));
    std::fs::write(&image.path, b"changed pixels").unwrap();
    assert!(verified_provider_screenshot_paths(&config, &request)
        .unwrap_err()
        .contains("changed after"));
}
