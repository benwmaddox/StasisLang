use serde_json::json;
use stasis_ai::{
    ActionKind, ActionState, ProviderActionProposal, ProviderReply, TaskController,
    TaskControllerEvent, TaskId, TaskSession,
};
use std::time::{Duration, Instant};

fn proposal(id: &str, repair: bool) -> ProviderActionProposal {
    ProviderActionProposal {
        id: id.into(),
        kind: ActionKind::Edit,
        description: format!("edit {id}"),
        payload: json!({"schema_version": 1, "edits": [{"new_source": id}]}),
        repair,
    }
}

fn session() -> TaskSession {
    let mut session = TaskSession::new();
    session
        .new_task("origin", "fix behavior", "project")
        .unwrap();
    session.append_reply("implement the fix").unwrap();
    session
}

fn drain(controller: &TaskController, session: &mut TaskSession) -> Vec<TaskControllerEvent> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let events = controller.poll(session);
        if !events.is_empty() {
            return events;
        }
        assert!(Instant::now() < deadline, "provider did not complete");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn proposal_is_drained_only_to_origin_and_requires_explicit_acceptance() {
    let controller = TaskController::new(|_, _| {
        let mut reply = ProviderReply::new("Review the proposed fix.");
        reply.proposals.push(proposal("fix", false));
        Ok(reply)
    });
    let mut session = session();
    controller.send(&session, &TaskId::new("origin")).unwrap();
    session
        .new_task("other", "other objective", "project")
        .unwrap();
    assert!(matches!(
        drain(&controller, &mut session)[0],
        TaskControllerEvent::Completed { .. }
    ));
    assert!(session.task("other").unwrap().actions.is_empty());
    let task = session.task_mut("origin").unwrap();
    assert_eq!(
        task.actions["fix"].payload,
        Some(proposal("fix", false).payload)
    );
    assert!(task.apply_action("fix").is_err());
    task.accept_action("fix").unwrap();
    task.apply_action("fix").unwrap();
    assert!(task.mark_done().is_err());
}

#[test]
fn rejected_work_can_be_repaired_without_regenerating_applied_work() {
    let controller = TaskController::new(|_, _| {
        let mut reply = ProviderReply::new("Repair only the rejected change.");
        reply.proposals.push(proposal("bad", true));
        Ok(reply)
    });
    let mut session = session();
    let task = session.task_mut("origin").unwrap();
    for id in ["good", "bad"] {
        task.propose_action_with_payload(id, ActionKind::Edit, id, json!({"original": id}))
            .unwrap();
    }
    task.accept_action("good").unwrap();
    task.apply_action("good").unwrap();
    task.reject_action("bad", "wrong target").unwrap();
    let accepted = task.actions["good"].clone();
    controller.send(&session, &TaskId::new("origin")).unwrap();
    assert!(matches!(
        drain(&controller, &mut session)[0],
        TaskControllerEvent::Completed { .. }
    ));
    let task = session.task("origin").unwrap();
    assert_eq!(task.actions["good"], accepted);
    assert_eq!(task.actions["bad"].state, ActionState::Proposed);
    assert_eq!(
        task.actions["bad"].revisions[0].payload,
        Some(json!({"original": "bad"}))
    );
}

#[test]
fn provider_cannot_replace_accepted_work_even_in_a_mixed_reply() {
    let controller = TaskController::new(|_, _| {
        let mut reply = ProviderReply::new("Replace accepted work.");
        reply.proposals = vec![proposal("new", false), proposal("accepted", true)];
        Ok(reply)
    });
    let mut session = session();
    let task = session.task_mut("origin").unwrap();
    task.propose_action_with_payload("accepted", ActionKind::Edit, "keep", json!({"keep": true}))
        .unwrap();
    task.accept_action("accepted").unwrap();
    let before = task.clone();
    controller.send(&session, &TaskId::new("origin")).unwrap();
    assert!(matches!(
        drain(&controller, &mut session)[0],
        TaskControllerEvent::Failed { .. }
    ));
    assert_eq!(session.task("origin").unwrap(), &before);
}

#[test]
fn cancellation_discards_a_late_proposal() {
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let release_rx = std::sync::Mutex::new(release_rx);
    let controller = TaskController::new(move |_, _| {
        started_tx.send(()).unwrap();
        release_rx
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        let mut reply = ProviderReply::new("late proposal");
        reply.proposals.push(proposal("late", false));
        Ok(reply)
    });
    let mut session = session();
    controller.send(&session, &TaskId::new("origin")).unwrap();
    started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    controller
        .cancel(&mut session, &TaskId::new("origin"))
        .unwrap();
    release_tx.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if controller
            .poll(&mut session)
            .iter()
            .any(|event| matches!(event, TaskControllerEvent::Stale { .. }))
        {
            break;
        }
        assert!(Instant::now() < deadline, "late result was not drained");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(session.task("origin").unwrap().actions.is_empty());
    assert_eq!(session.task("origin").unwrap().thread.len(), 1);
}

#[test]
fn provider_context_omits_payload_and_revision_history() {
    let controller = TaskController::new(|request, _| {
        let context = serde_json::to_string(&request.actions).unwrap();
        assert!(context.len() < 1024);
        assert!(!context.contains("large_payload"));
        assert!(!context.contains("revisions"));
        assert_eq!(request.actions[0].state, "needs_repair");
        Ok(ProviderReply::new("Repair the rejected action."))
    });
    let mut session = session();
    let task = session.task_mut("origin").unwrap();
    task.propose_action_with_payload(
        "edit",
        ActionKind::Edit,
        "change",
        json!({"large_payload": "x".repeat(200_000)}),
    )
    .unwrap();
    task.accept_action("edit").unwrap();
    task.mark_action_for_repair("edit", "fix conflict").unwrap();
    controller.send(&session, &TaskId::new("origin")).unwrap();
    assert!(matches!(
        drain(&controller, &mut session)[0],
        TaskControllerEvent::Completed { .. }
    ));
}
