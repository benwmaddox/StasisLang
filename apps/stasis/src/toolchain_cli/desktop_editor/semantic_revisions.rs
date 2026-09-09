use eframe::egui::{self, RichText};
use serde_json::Value;
use stasis_ai::task_session::{ActionState, TaskAction};

pub(super) struct ProposalRevision<'a> {
    pub revision: usize,
    pub thread_position: usize,
    pub description: &'a str,
    pub payload: Option<&'a Value>,
    pub state: &'a ActionState,
    pub current: bool,
}

pub(super) fn proposal_revisions(action: &TaskAction) -> Vec<ProposalRevision<'_>> {
    let snapshots = action.revisions.iter().map(|snapshot| ProposalRevision {
        revision: 0,
        thread_position: snapshot.thread_position,
        description: &snapshot.description,
        payload: snapshot.payload.as_ref(),
        state: &snapshot.state,
        current: false,
    });
    let current = ProposalRevision {
        revision: 0,
        thread_position: action.thread_position,
        description: &action.description,
        payload: action.payload.as_ref(),
        state: &action.state,
        current: true,
    };
    let mut proposals: Vec<ProposalRevision<'_>> = Vec::new();
    for mut snapshot in snapshots.chain(std::iter::once(current)) {
        // Repair/rejection changes state, not the proposal. A newly Proposed
        // revision stays distinct even when it repeats the exact payload.
        if let Some(previous) = proposals.last_mut() {
            if matches!(
                (previous.state, snapshot.state),
                (
                    ActionState::Proposed | ActionState::Accepted | ActionState::Applied,
                    ActionState::NeedsRepair { .. } | ActionState::Rejected { .. }
                ) | (
                    ActionState::Rejected { .. },
                    ActionState::NeedsRepair { .. }
                )
            ) && snapshot.thread_position == previous.thread_position
                && snapshot.description == previous.description
                && snapshot.payload == previous.payload
            {
                previous.state = snapshot.state;
                previous.current = snapshot.current;
                continue;
            }
        }
        snapshot.revision = proposals.len();
        proposals.push(snapshot);
    }
    proposals
}

pub(super) fn render_heading(ui: &mut egui::Ui, action: &str, proposal: &ProposalRevision<'_>) {
    ui.label(
        RichText::new(format!(
            "{} | Revision {}{}",
            action,
            proposal.revision + 1,
            if proposal.current {
                ""
            } else {
                " | Previous revision"
            }
        ))
        .strong(),
    );
    match proposal.state {
        ActionState::NeedsRepair { reason } | ActionState::Rejected { reason } => {
            ui.label(
                if matches!(proposal.state, ActionState::NeedsRepair { .. }) {
                    "Needs repair"
                } else {
                    "Rejected"
                },
            );
            ui.collapsing("Failure / rejection details", |ui| {
                ui.label(reason);
            });
        }
        state => {
            ui.label(format!("{state:?}"));
        }
    }
    ui.label(proposal.description);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use stasis_ai::task_session::{ActionKind, Task};

    #[test]
    fn repair_state_snapshots_share_one_proposal_but_identical_reproposals_do_not() {
        let mut task = Task::new("task", "Repair value", "Fixture").unwrap();
        let payload = json!({"edits": ["same payload"]});
        task.propose_action_with_payload("edit", ActionKind::Edit, "Value", payload.clone())
            .unwrap();
        task.accept_action("edit").unwrap();
        task.mark_action_for_repair("edit", "Tests failed").unwrap();
        let proposals = proposal_revisions(&task.actions["edit"]);
        assert_eq!(proposals.len(), 1);
        assert!(proposals[0].current);
        assert!(matches!(
            proposals[0].state,
            ActionState::NeedsRepair { .. }
        ));

        // Rejection is another state transition of the original proposal.
        task.reject_action("edit", "Retry unchanged").unwrap();
        assert_eq!(proposal_revisions(&task.actions["edit"]).len(), 1);
        task.repair_action_with_payload("edit", "Value", payload)
            .unwrap();
        let proposals = proposal_revisions(&task.actions["edit"]);
        assert_eq!(proposals.len(), 2);
        assert!(!proposals[0].current);
        assert!(proposals[1].current);
        assert_eq!(proposals[1].revision, 1);
        assert_eq!(proposals[0].payload, proposals[1].payload);

        let mut rejected_retry = task.clone();
        rejected_retry
            .reject_action("edit", "Reject the new proposal")
            .unwrap();
        assert_eq!(proposal_revisions(&rejected_retry.actions["edit"]).len(), 2);
        rejected_retry
            .mark_action_for_repair("edit", "Repair the new proposal")
            .unwrap();
        assert_eq!(proposal_revisions(&rejected_retry.actions["edit"]).len(), 2);

        task.accept_action("edit").unwrap();
        task.mark_action_for_repair("edit", "Still failing")
            .unwrap();
        let proposals = proposal_revisions(&task.actions["edit"]);
        assert_eq!(proposals.len(), 2);
        assert!(proposals[1].current);
        assert!(matches!(
            proposals[1].state,
            ActionState::NeedsRepair { .. }
        ));
    }
}
