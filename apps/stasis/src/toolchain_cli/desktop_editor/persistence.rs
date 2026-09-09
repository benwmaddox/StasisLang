use super::*;
use stasis_ai::session_store::{ExecutionReceipt, LoadOutcome, SessionSnapshot, WindowPreferences};

pub(super) fn snapshot(editor: &DesktopEditor) -> SessionSnapshot {
    let mut preview_fingerprints = editor
        .persisted_snapshot
        .as_ref()
        .map(|saved| saved.preview_fingerprints.clone())
        .unwrap_or_default();
    for (key, record) in &editor.state.semantic_previews {
        let identity = preview_identity(key);
        match &record.result {
            Some(Ok(preview)) => {
                preview_fingerprints
                    .insert(identity, (preview.source_fingerprint.clone(), record.stale));
            }
            Some(_) => {
                if let Some((_, stale)) = preview_fingerprints.get_mut(&identity) {
                    *stale = true;
                }
            }
            None => {}
        }
    }
    SessionSnapshot {
        session: editor.state.session.clone(),
        task_order: editor
            .state
            .session
            .tasks()
            .map(|task| task.id.to_string())
            .collect(),
        drafts: editor.state.drafts.clone(),
        objective: editor.state.objective.clone(),
        reply: editor.state.reply.clone(),
        next_task_number: editor.state.next_task,
        next_capture_number: editor.next_capture,
        execution_receipts: editor
            .execution_receipts
            .iter()
            .map(|((task_id, action_id), receipt)| ExecutionReceipt {
                task_id: task_id.clone(),
                action_id: action_id.clone(),
                receipt: receipt.clone(),
            })
            .collect(),
        validation_receipts: editor.validation_receipts.clone(),
        validation_fingerprints: editor.validation_fingerprints.clone(),
        preview_fingerprints,
        in_flight: editor
            .state
            .session
            .tasks()
            .filter(|task| {
                editor
                    .controller
                    .snapshot(&task.id)
                    .is_some_and(|request| request.state == stasis_ai::TaskRequestState::Running)
            })
            .map(|task| task.id.to_string())
            .collect(),
        uncertain_calls: editor.uncertain_calls.clone(),
        expanded: editor.expanded.clone(),
        window_preferences: editor.window_preferences.clone(),
        media_hashes: editor.media_hashes.clone(),
        unavailable_media: editor.unavailable_media.clone(),
    }
}

pub(super) fn restore(editor: &mut DesktopEditor, loaded: LoadOutcome) {
    let Some(mut saved) = loaded.snapshot.as_ref().cloned() else {
        if !loaded.diagnostics.is_empty() {
            editor.state.notice = Some(diagnostic_notice(&loaded));
        }
        return;
    };
    let mut stale = Vec::new();
    for task in saved.session.tasks.values_mut() {
        if task.validation.is_running()
            || (task.validation.is_passing()
                && !saved.validation_fingerprints.contains_key(task.id.as_str()))
        {
            task.validation = ValidationStatus::NotRun;
            stale.push(task.id.to_string());
            let _ = task.append_host_result(
                "Saved validation is unverified after restart; run focused tests again.",
            );
        }
    }
    for (task_id, (expected, paths)) in &saved.validation_fingerprints {
        let current = super::super::desktop_source_fingerprint(&editor.project_root, paths);
        if current.as_ref() != Ok(expected) {
            if let Ok(task) = saved.session.task_mut(task_id) {
                task.validation = ValidationStatus::NotRun;
                let _ = task.append_host_result(
                    "Saved validation is stale because project sources changed after the last session.",
                );
            }
            stale.push(task_id.clone());
        }
    }
    for task_id in &stale {
        saved.validation_fingerprints.remove(task_id);
        saved.validation_receipts.remove(task_id);
    }
    editor.state.session = saved.session.clone();
    editor.state.drafts = saved.drafts.clone();
    editor.state.objective = saved.objective.clone();
    editor.state.reply = saved.reply.clone();
    editor.state.next_task = saved.next_task_number.max(1);
    editor.next_capture = saved.next_capture_number.max(1);
    editor.execution_receipts = saved
        .execution_receipts
        .iter()
        .map(|value| {
            (
                (value.task_id.clone(), value.action_id.clone()),
                value.receipt.clone(),
            )
        })
        .collect();
    editor.validation_receipts = saved.validation_receipts.clone();
    editor.validation_fingerprints = saved.validation_fingerprints.clone();
    editor.uncertain_calls = saved.uncertain_calls.clone();
    editor.expanded = saved.expanded.clone();
    editor.window_preferences = saved.window_preferences.clone();
    editor.window_preferences = editor
        .window_preferences
        .map(|value| bounded_window(value.size));
    editor.media_hashes = saved.media_hashes.clone();
    editor.unavailable_media = saved.unavailable_media.clone();
    // Previews include source-derived plans and are deliberately rebuilt after restart.
    editor.state.semantic_previews.clear();
    let current_source = super::super::desktop_source_fingerprint(&editor.project_root, &[]);
    for task in saved.session.tasks() {
        for action in task.actions.values() {
            for proposal in proposal_revisions(action) {
                let Some(payload) = proposal.payload else {
                    continue;
                };
                let key = SemanticPreviewKey::new(
                    task.id.as_str(),
                    action.id.as_str(),
                    proposal.revision,
                    payload,
                );
                if let Some((expected, was_stale)) =
                    saved.preview_fingerprints.get(&preview_identity(&key))
                {
                    if !was_stale && current_source.as_ref() == Ok(expected) {
                        editor.recovered_previews.insert(key, expected.clone());
                    } else {
                        editor.state.semantic_previews.insert(key, SemanticPreviewRecord {
                            result: Some(Err("Saved preview is stale because project sources changed; request a revised proposal.".into())),
                            stale: true,
                        });
                    }
                }
            }
        }
    }
    let mut notices = Vec::new();
    if !loaded.diagnostics.is_empty() {
        notices.push(diagnostic_notice(&loaded));
    }
    if !stale.is_empty() {
        notices.push(format!("Validation became stale for {}.", stale.join(", ")));
    }
    if !editor.uncertain_calls.is_empty() {
        notices.push(format!(
            "AI request outcome is uncertain for {}; review the task before retrying.",
            editor
                .uncertain_calls
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !notices.is_empty() {
        editor.state.notice = Some(notices.join(" "));
    }
    editor.persisted_snapshot = Some(saved);
}

fn diagnostic_notice(loaded: &LoadOutcome) -> String {
    let messages = loaded
        .diagnostics
        .iter()
        .map(|diagnostic| format!("{diagnostic:?}"))
        .collect::<Vec<_>>()
        .join("; ");
    format!("Recovered editor state with diagnostics: {messages}")
}

fn preview_identity(key: &SemanticPreviewKey) -> String {
    serde_json::to_string(&(&key.task, &key.action, key.revision, &key.payload_hash))
        .expect("string tuple")
}

pub(super) fn bounded_window(size: [f32; 2]) -> WindowPreferences {
    WindowPreferences {
        size: [size[0].clamp(520.0, 3840.0), size[1].clamp(600.0, 2160.0)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stasis_runner::live::live_session;

    fn reopen(root: &std::path::Path) -> DesktopEditor {
        let (client, _server) = live_session(1);
        DesktopEditor::new(client, root.to_path_buf(), Arc::new(AtomicBool::new(false)))
            .with_persistence()
    }

    #[test]
    fn autosave_coalesces_edits_and_does_not_wait_for_busy_writer() {
        let (mut editor, root, _) = super::super::tests::review_fixture("autosave_coalescing");
        editor.store = Some(SessionStore::open(&root).unwrap());
        editor.persist_if_changed();
        let now = Instant::now();
        editor.next_autosave = now + Duration::from_millis(500);
        for index in 0..100 {
            editor.state.reply = format!("draft {index}");
            editor.poll_autosave(now);
            assert!(editor.autosave.is_none());
        }
        assert_ne!(
            editor.persisted_snapshot.as_ref().unwrap().reply,
            "draft 99"
        );
        editor.poll_autosave(now + Duration::from_millis(500));
        assert!(editor.autosave.is_some());
        editor.finish_autosave();
        assert_eq!(
            editor
                .store
                .as_ref()
                .unwrap()
                .load()
                .unwrap()
                .snapshot
                .unwrap()
                .reply,
            "draft 99"
        );

        let (release, wait) = mpsc::channel();
        let saved = snapshot(&editor);
        editor.autosave = Some(thread::spawn(move || {
            wait.recv_timeout(Duration::from_secs(2)).unwrap();
            Ok(saved)
        }));
        editor.state.reply = "draft written during save".into();
        editor.poll_autosave(now + Duration::from_secs(1));
        assert!(editor.autosave.is_some());
        release.send(()).unwrap();
        editor.finish_autosave();
        editor.poll_autosave(now + Duration::from_secs(2));
        editor.finish_autosave();
        assert_eq!(
            editor
                .store
                .as_ref()
                .unwrap()
                .load()
                .unwrap()
                .snapshot
                .unwrap()
                .reply,
            "draft written during save"
        );
        editor.poll_autosave(now + Duration::from_secs(3));
        assert!(editor.autosave.is_none());
        // Erasure drains the writer before removing state, so it cannot resurrect history.
        editor.state.reply = "erase pending draft".into();
        editor.poll_autosave(now + Duration::from_secs(4));
        assert!(editor.autosave.is_some());
        editor.erase_history().unwrap();
        assert!(editor
            .store
            .as_ref()
            .unwrap()
            .load()
            .unwrap()
            .snapshot
            .is_none());
        drop(editor);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn owned_attachment_survives_restart_and_history_erasure() {
        let (editor, root, _) = super::super::tests::review_fixture("owned_media_restart");
        let mut editor = editor.with_persistence();
        let task_id = editor.state.session.active_task_id().unwrap().clone();
        let mut encoded = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgba8(2, 2)
            .write_to(&mut encoded, image::ImageFormat::Png)
            .unwrap();
        editor
            .attach_encoded_image(
                &task_id,
                "owned".into(),
                "reference.png".into(),
                AttachmentOrigin::Clipboard,
                encoded.get_ref(),
            )
            .unwrap();
        let source = editor.state.session.task(&task_id).unwrap().screenshots["owned"]
            .source
            .clone();
        editor.persist_if_changed();
        drop(editor);
        assert!(std::path::Path::new(&source).is_file());
        let mut recovered = reopen(&root);
        assert!(!recovered.unavailable_media.contains(&source));
        assert_eq!(
            recovered.state.session.task(&task_id).unwrap().screenshots["owned"].source,
            source
        );
        recovered.erase_history().unwrap();
        drop(recovered);
        assert!(std::path::Path::new(&source).is_file());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn restart_preserves_revision_drafts_and_rebuilds_previews() {
        let (mut editor, root, payload) = super::super::tests::review_fixture("session_restart");
        editor.store = Some(SessionStore::open(&root).unwrap());
        editor.state.reply = "Keep this draft".into();
        editor
            .state
            .session
            .active_task_mut()
            .unwrap()
            .append_reply("Review the proposed value change.")
            .unwrap();
        editor
            .expanded
            .insert("task-1/value/1/src/main.stasis".into());
        editor.window_preferences = Some(bounded_window([1100.0, 800.0]));
        super::super::tests::finish_preview(&mut editor);
        let task = editor.state.session.active_task_mut().unwrap();
        task.accept_action("value").unwrap();
        task.repair_action_with_payload("value", "Revised value proposal", payload.clone())
            .unwrap();
        task.set_provider_state(configured_provider_state(&ProviderConfig::Codex))
            .unwrap();
        let active = task.id.clone();
        editor
            .state
            .session
            .new_task("task-2", "Another task", "Sample")
            .unwrap();
        editor.state.session.switch_task(&active).unwrap();
        editor.state.drafts.insert(
            "task-2".into(),
            ("Another objective".into(), "Another reply".into()),
        );
        super::super::tests::finish_preview(&mut editor);
        editor.persist_if_changed();
        let before = snapshot(&editor);
        drop(editor);

        let mut recovered = reopen(&root);
        assert_eq!(recovered.state.session, before.session);
        assert_eq!(snapshot(&recovered).task_order, before.task_order);
        assert_eq!(recovered.state.drafts, before.drafts);
        assert_eq!(
            recovered.state.session.active_task().unwrap().actions["value"]
                .revisions
                .len(),
            1
        );
        assert_eq!(recovered.state.reply, "Keep this draft");
        assert_eq!(recovered.expanded, before.expanded);
        assert_eq!(recovered.window_preferences, before.window_preferences);
        assert!(recovered.state.semantic_previews.is_empty());
        assert_eq!(
            recovered.state.session.active_task().unwrap().actions["value"]
                .payload
                .as_ref(),
            Some(&payload)
        );
        super::super::tests::finish_preview(&mut recovered);
        assert!(!recovered.state.semantic_previews.is_empty());
        assert!(recovered
            .state
            .semantic_previews
            .values()
            .all(|record| matches!(record.result, Some(Ok(_)))));
        drop(recovered);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn restart_invalidates_changed_source_and_interrupted_validation() {
        let (mut editor, root, _) = super::super::tests::review_fixture("session_stale");
        editor.store = Some(SessionStore::open(&root).unwrap());
        let task_id = editor.state.session.active_task_id().unwrap().to_string();
        let fingerprint = super::super::super::desktop_source_fingerprint(&root, &[]).unwrap();
        editor
            .validation_fingerprints
            .insert(task_id.clone(), (fingerprint, vec![]));
        editor
            .validation_receipts
            .insert(task_id.clone(), json!({"passed": true}));
        editor.state.session.active_task_mut().unwrap().validation = ValidationStatus::Passed {
            summary: "passed".into(),
        };
        editor.persist_if_changed();
        drop(editor);
        std::fs::write(
            root.join("src/main.stasis"),
            "function main(): i32 { return 4; }\n",
        )
        .unwrap();
        let mut recovered = reopen(&root);
        assert_eq!(
            recovered.state.session.active_task().unwrap().validation,
            ValidationStatus::NotRun
        );
        assert!(recovered.validation_receipts.is_empty());
        assert!(recovered.validation_fingerprints.is_empty());
        assert!(recovered.state.notice.as_deref().unwrap().contains("stale"));
        recovered
            .state
            .session
            .active_task_mut()
            .unwrap()
            .validation = ValidationStatus::Running;
        recovered.persist_if_changed();
        drop(recovered);
        let recovered = reopen(&root);
        assert_eq!(
            recovered.state.session.active_task().unwrap().validation,
            ValidationStatus::NotRun
        );
        drop(recovered);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn accepted_preview_recovers_only_against_the_saved_source_fingerprint() {
        let (mut editor, root, _) = super::super::tests::review_fixture("session_accepted");
        editor.store = Some(SessionStore::open(&root).unwrap());
        super::super::tests::finish_preview(&mut editor);
        editor
            .state
            .session
            .active_task_mut()
            .unwrap()
            .accept_action("value")
            .unwrap();
        editor.persist_if_changed();
        drop(editor);
        let mut recovered = reopen(&root);
        // Saving during asynchronous recovery must not lose the accepted fingerprint.
        recovered.state.reply = "Continue after restart".into();
        recovered.persist_if_changed();
        drop(recovered);
        let mut recovered = reopen(&root);
        super::super::tests::finish_preview(&mut recovered);
        assert!(recovered.state.reviewed_preview("task-1", "value").is_ok());
        drop(recovered);
        let source = root.join("src/main.stasis");
        let old = std::fs::read_to_string(&source).unwrap();
        std::fs::write(&source, format!("{old}\n// changed after acceptance\n")).unwrap();
        let mut recovered = reopen(&root);
        super::super::tests::finish_preview(&mut recovered);
        assert!(recovered.state.reviewed_preview("task-1", "value").is_err());
        assert!(recovered
            .state
            .semantic_previews
            .values()
            .all(|record| record.stale));
        assert!(recovered.semantic_job.is_none());
        recovered.persist_if_changed();
        drop(recovered);
        std::fs::write(&source, old).unwrap();
        let recovered = reopen(&root);
        assert!(recovered
            .state
            .semantic_previews
            .values()
            .all(|record| record.stale));
        drop(recovered);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn restart_never_replays_provider_intent_and_erase_removes_history() {
        let (mut editor, root, _) = super::super::tests::review_fixture("session_uncertain");
        editor.store = Some(SessionStore::open(&root).unwrap());
        let task = editor.state.session.active_task_id().unwrap().clone();
        let candidate = editor.state.session.clone();
        editor.persist_provider_intent(&task, &candidate).unwrap();
        drop(editor);
        let mut recovered = reopen(&root);
        assert!(recovered.uncertain_calls.contains(task.as_str()));
        assert!(recovered.controller.snapshot(&task).is_none());
        assert!(recovered.state.intents.is_empty());
        recovered.erase_history().unwrap();
        assert_eq!(recovered.state.session.task_count(), 0);
        assert!(recovered.uncertain_calls.is_empty());
        assert!(root.join("src/main.stasis").exists());
        drop(recovered);
        let recovered = reopen(&root);
        assert_eq!(recovered.state.session.task_count(), 0);
        drop(recovered);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn exit_persists_apply_that_finishes_during_host_shutdown() {
        let (mut editor, root, _) =
            super::super::tests::review_fixture("session_exit_during_apply");
        editor.store = Some(SessionStore::open(&root).unwrap());
        super::super::tests::finish_preview(&mut editor);
        editor
            .state
            .session
            .active_task_mut()
            .unwrap()
            .accept_action("value")
            .unwrap();
        let preview = editor
            .state
            .reviewed_preview("task-1", "value")
            .unwrap()
            .clone();
        editor.busy_tasks.insert("task-1".into());

        let (request_tx, _request_rx) = mpsc::sync_channel(8);
        let (result_tx, result_rx) = mpsc::channel();
        let shutdown = Arc::new(AtomicBool::new(false));
        let worker_shutdown = Arc::clone(&shutdown);
        let worker_root = root.clone();
        let worker = thread::spawn(move || {
            while !worker_shutdown.load(Ordering::Acquire) {
                thread::yield_now();
            }
            let result =
                super::super::super::desktop_apply_semantic_preview(&worker_root, &preview)
                    .and_then(|(summary, receipt)| {
                        receipt
                            .get("source_fingerprint")
                            .and_then(Value::as_str)
                            .map(|fingerprint| (summary, receipt.clone(), fingerprint.to_string()))
                            .ok_or_else(|| {
                                "semantic edit receipt omitted its source fingerprint".to_string()
                            })
                    });
            result_tx
                .send(HostResult {
                    request_id: 1,
                    task_id: "task-1".into(),
                    operation: HostOperation::Apply {
                        action_id: "value".into(),
                        preview,
                    },
                    result,
                })
                .unwrap();
        });
        editor.host = HostExecutor {
            requests: Some(request_tx),
            results: result_rx,
            progress: Arc::new(Mutex::new(HostProgressState::default())),
            canceled: Arc::new(Mutex::new(BTreeSet::new())),
            shutdown,
            worker: Some(worker),
        };

        eframe::App::on_exit(&mut editor, None);
        drop(editor);

        let recovered = reopen(&root);
        let task = recovered.state.session.task("task-1").unwrap();
        assert!(matches!(task.actions["value"].state, ActionState::Applied));
        assert!(task.validation.is_passing());
        assert!(recovered
            .execution_receipts
            .contains_key(&("task-1".into(), "value".into())));
        assert!(recovered.validation_fingerprints.contains_key("task-1"));
        drop(recovered);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn corrupt_history_is_not_overwritten_by_autosave() {
        let (mut editor, root, _) = super::super::tests::review_fixture("session_corrupt");
        editor.store = Some(SessionStore::open(&root).unwrap());
        editor.persist_if_changed();
        drop(editor);
        let path = root.join(".stasis/editor/session.json");
        std::fs::write(&path, b"{truncated").unwrap();
        let mut recovered = reopen(&root);
        assert!(recovered.recovery_error);
        recovered.state.reply = "New draft".into();
        recovered.persist_if_changed();
        assert_eq!(std::fs::read(&path).unwrap(), b"{truncated");
        recovered.erase_history().unwrap();
        assert!(!recovered.recovery_error);
        recovered.state.reply = "Replacement draft".into();
        recovered.persist_if_changed();
        drop(recovered);
        let recovered = reopen(&root);
        assert_eq!(recovered.state.reply, "Replacement draft");
        drop(recovered);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn disconnected_task_can_resume_after_restart_without_replaying() {
        let (mut editor, root, _) = super::super::tests::review_fixture("session_reconnect");
        editor.store = Some(SessionStore::open(&root).unwrap());
        editor
            .state
            .session
            .active_task_mut()
            .unwrap()
            .disconnect()
            .unwrap();
        let task = editor.state.session.active_task_id().unwrap().clone();
        editor.uncertain_calls.insert(task.to_string());
        editor.persist_if_changed();
        drop(editor);
        let mut recovered = reopen(&root);
        recovered
            .state
            .intents
            .push(EditorIntent::Reconnect(task.to_string()));
        recovered.flush_intents();
        assert_eq!(
            recovered.state.session.active_task().unwrap().connection,
            ConnectionState::Connected
        );
        assert!(recovered.controller.snapshot(&task).is_none());
        assert!(recovered.uncertain_calls.contains(task.as_str()));
        recovered
            .state
            .intents
            .push(EditorIntent::Retry(task.to_string()));
        recovered.flush_intents();
        assert!(recovered.uncertain_calls.contains(task.as_str()));
        assert!(recovered.controller.snapshot(&task).is_none());
        drop(recovered);
        std::fs::remove_dir_all(root).unwrap();
    }
}
