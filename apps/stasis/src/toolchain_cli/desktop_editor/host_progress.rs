use super::ProgressStage;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

const MAX_HOST_REQUESTS: usize = 8;
const MAX_HOST_EVENTS: usize = 32;

#[derive(Clone, Debug)]
pub(super) struct HostProgress {
    pub task_id: String,
    pub request_id: u64,
    pub events: Vec<(ProgressStage, u64)>,
    started: Instant,
    closed: bool,
    truncated: bool,
    pub cancel_requested: bool,
}

impl HostProgress {
    pub fn task_to_tests_ms(&self, task_started: Instant) -> Option<u64> {
        let (_, elapsed) = self
            .events
            .iter()
            .find(|(stage, _)| *stage == ProgressStage::FocusedTestsPassed)?;
        (self.events.last()?.0 == ProgressStage::Completed).then(|| {
            (self
                .started
                .saturating_duration_since(task_started)
                .as_millis() as u64)
                .saturating_add(*elapsed)
        })
    }

    pub fn phase_ms(&self, stage: ProgressStage) -> Option<u64> {
        if self.truncated {
            return None;
        }
        let mut total = None;
        for pair in self.events.windows(2) {
            if pair[0].0 == stage {
                total = Some(
                    total
                        .unwrap_or(0u64)
                        .saturating_add(pair[1].1.saturating_sub(pair[0].1)),
                );
            }
        }
        total
    }
}

#[derive(Default)]
pub(super) struct HostProgressState {
    pub snapshots: BTreeMap<String, HostProgress>,
    admitted: BTreeSet<(String, u64)>,
    next_id: u64,
}

impl HostProgressState {
    pub fn admit(&mut self, task_id: &str) -> Result<u64, String> {
        if (!self.snapshots.contains_key(task_id)
            && self.snapshots.len() >= stasis_ai::task_session::MAX_TASKS)
            || self.admitted.len() >= MAX_HOST_REQUESTS
            || self.admitted.iter().any(|(task, _)| task == task_id)
        {
            return Err("desktop host executor is at capacity or task is busy".into());
        }
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or("host request IDs exhausted")?;
        let request_id = self.next_id;
        self.admitted.insert((task_id.to_string(), request_id));
        self.snapshots.insert(
            task_id.to_string(),
            HostProgress {
                task_id: task_id.to_string(),
                request_id,
                events: vec![(ProgressStage::Queued, 0)],
                started: Instant::now(),
                closed: false,
                truncated: false,
                cancel_requested: false,
            },
        );
        Ok(request_id)
    }

    pub fn report(&mut self, task: &str, id: u64, stage: ProgressStage) -> bool {
        let Some(record) = self.snapshots.get(task) else {
            return false;
        };
        let elapsed = record.started.elapsed().as_millis().min(u64::MAX as u128) as u64;
        self.report_at(task, id, stage, elapsed)
    }

    pub(super) fn report_at(
        &mut self,
        task: &str,
        id: u64,
        stage: ProgressStage,
        elapsed: u64,
    ) -> bool {
        let Some(record) = self.snapshots.get_mut(task) else {
            return false;
        };
        if record.request_id != id || record.closed {
            return false;
        }
        let terminal = matches!(
            stage,
            ProgressStage::Completed | ProgressStage::Canceled | ProgressStage::Failed
        );
        let last = record.events.last().unwrap();
        if last.0 == stage || elapsed < last.1 {
            return false;
        }
        if record.events.len() >= MAX_HOST_EVENTS - 1 && !terminal {
            record.truncated = true;
            return false;
        }
        record.events.push((stage, elapsed));
        record.closed = terminal;
        true
    }

    pub fn cancel(&mut self, task: &str) {
        if let Some(record) = self.snapshots.get(task) {
            let id = record.request_id;
            if !record.closed {
                self.snapshots.get_mut(task).unwrap().cancel_requested = true;
                self.report(task, id, ProgressStage::CancelRequested);
            }
        }
    }

    pub fn release(&mut self, task: &str, id: u64) {
        self.admitted.remove(&(task.to_string(), id));
    }

    pub fn discard(&mut self, task: &str, id: u64) {
        self.release(task, id);
        if self
            .snapshots
            .get(task)
            .is_some_and(|record| record.request_id == id)
        {
            self.snapshots.remove(task);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_progress_order_isolation_retry_and_timing() {
        let mut state = HostProgressState::default();
        let a = state.admit("a").unwrap();
        let b = state.admit("b").unwrap();
        assert!(state.report_at("a", a, ProgressStage::ApplyingAtomically, 10));
        assert!(!state.report_at("b", a, ProgressStage::Compiling, 15));
        assert!(state.report_at("a", a, ProgressStage::Compiling, 30));
        assert!(!state.report_at("a", a, ProgressStage::RunningFocusedTests, 29));
        assert!(state.report_at("a", a, ProgressStage::RunningFocusedTests, 80));
        assert!(state.report_at("a", a, ProgressStage::FocusedTestsPassed, 100));
        assert!(state.report_at("a", a, ProgressStage::Completed, 110));
        let record = &state.snapshots["a"];
        assert_eq!(record.phase_ms(ProgressStage::ApplyingAtomically), Some(20));
        assert_eq!(record.phase_ms(ProgressStage::Compiling), Some(50));
        assert_eq!(
            record.phase_ms(ProgressStage::RunningFocusedTests),
            Some(20)
        );
        assert_eq!(
            state.snapshots["b"].events,
            vec![(ProgressStage::Queued, 0)]
        );
        state.release("a", a);
        let retry = state.admit("a").unwrap();
        assert_ne!(retry, a);
        assert!(!state.report_at("a", a, ProgressStage::Failed, 200));
        assert!(state.report_at("b", b, ProgressStage::Failed, 5));
    }

    #[test]
    fn cancellation_request_does_not_hide_in_flight_atomic_outcome() {
        let mut state = HostProgressState::default();
        let id = state.admit("a").unwrap();
        state.report("a", id, ProgressStage::ApplyingAtomically);
        state.cancel("a");
        assert!(state.snapshots["a"].cancel_requested);
        assert!(!state.snapshots["a"].closed);
        assert!(state.report("a", id, ProgressStage::Completed));
        assert_eq!(
            state.snapshots["a"].events.last().unwrap().0,
            ProgressStage::Completed
        );
        assert_eq!(state.snapshots["a"].task_to_tests_ms(Instant::now()), None);
    }

    #[test]
    fn host_progress_cancel_and_bound_preserve_terminal_and_admission() {
        let mut state = HostProgressState::default();
        let a = state.admit("a").unwrap();
        for i in 1..100 {
            state.report_at(
                "a",
                a,
                if i % 2 == 0 {
                    ProgressStage::Compiling
                } else {
                    ProgressStage::RunningFocusedTests
                },
                i,
            );
        }
        assert!(state.report_at("a", a, ProgressStage::Canceled, 100));
        assert_eq!(state.snapshots["a"].events.len(), MAX_HOST_EVENTS);
        assert!(!state.report_at("a", a, ProgressStage::Completed, 101));
        assert!(state.admit("a").is_err()); // cancellation does not free running work
        for i in 1..MAX_HOST_REQUESTS {
            state.admit(&format!("task-{i}")).unwrap();
        }
        assert!(state.admit("overflow").is_err());
        state.release("a", a);
        assert!(state.admit("a").is_ok());
    }
}
