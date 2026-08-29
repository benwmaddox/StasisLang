//! The correctness-critical part of a synchronous development hot swap.
//!
//! Compiler artifact construction and host lifecycle remain outside this
//! module.  A host supplies only the small publication participant needed to
//! stage, publish, and restore its host-owned resources.  The transaction then
//! places that publication inside the existing JIT state snapshot/rollback
//! boundary.

use super::jit::{JitProcess, JitStateLayout};
use super::state_migration::{
    activate_candidate_transactionally, finalize_runtime_preview, plan_state_migration,
    StateMigrationPreview,
};
use serde::{Deserialize, Serialize};
use std::fmt;

pub const DEVELOPMENT_SWAP_RECEIPT_SCHEMA_VERSION: u16 = 1;

/// Inputs that affect the shared development transition.
///
/// The candidate itself is passed to [`commit_development_swap`].  Keeping
/// construction out of this descriptor makes it explicit that compiling or
/// packaging a candidate does not publish it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevelopmentSwapDescriptor {
    pub changed_functions: Vec<String>,
    pub hook_may_mutate_state: bool,
}

impl DevelopmentSwapDescriptor {
    pub fn new(
        changed_functions: impl IntoIterator<Item = String>,
        hook_may_mutate_state: bool,
    ) -> Self {
        let mut changed_functions = changed_functions.into_iter().collect::<Vec<_>>();
        changed_functions.sort();
        changed_functions.dedup();
        Self {
            changed_functions,
            hook_may_mutate_state,
        }
    }

    pub fn for_candidate(candidate: &JitProcess, hook_may_mutate_state: bool) -> Self {
        let changed_functions = candidate
            .generation_metadata()
            .into_iter()
            .flat_map(|metadata| metadata.emitted_function_ids.iter())
            .filter_map(|function_id| {
                candidate
                    .program_snapshot()?
                    .function_by_id(*function_id)
                    .map(|function| function.name.clone())
            });
        Self::new(changed_functions, hook_may_mutate_state)
    }
}

/// The stable outcome tag shared by desktop and Workshop receipts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DevelopmentSwapStatus {
    Accepted,
    Rejected,
}

/// Host-independent evidence for one development swap attempt.
///
/// These fields intentionally describe the candidate and migration preview,
/// not a host's resource implementation.  Equivalent candidates therefore
/// produce equivalent receipts in desktop and Workshop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DevelopmentSwapReceipt {
    pub schema_version: u16,
    pub status: DevelopmentSwapStatus,
    pub changed_functions: Vec<String>,
    pub state_layout_compatible: bool,
    pub layout_changed: bool,
    pub from_layout_version: String,
    pub to_layout_version: String,
}

impl DevelopmentSwapReceipt {
    fn from_preview(preview: &StateMigrationPreview, status: DevelopmentSwapStatus) -> Self {
        Self {
            schema_version: DEVELOPMENT_SWAP_RECEIPT_SCHEMA_VERSION,
            status,
            changed_functions: preview.changed_functions.clone(),
            state_layout_compatible: preview.state_layout_compatible,
            layout_changed: preview.layout_changed,
            from_layout_version: preview.from_layout_version.clone(),
            to_layout_version: preview.to_layout_version.clone(),
        }
    }

    fn planning_failure(descriptor: &DevelopmentSwapDescriptor) -> Self {
        Self {
            schema_version: DEVELOPMENT_SWAP_RECEIPT_SCHEMA_VERSION,
            status: DevelopmentSwapStatus::Rejected,
            changed_functions: descriptor.changed_functions.clone(),
            state_layout_compatible: false,
            layout_changed: false,
            from_layout_version: String::new(),
            to_layout_version: String::new(),
        }
    }
}

/// A failed transition carries its deterministic receipt as well as the
/// human-facing error.  `hook_error` lets a host retain a typed presentation
/// error without making the compiler depend on that host's error type.
#[derive(Debug)]
pub struct DevelopmentSwapFailure<E = String> {
    pub receipt: DevelopmentSwapReceipt,
    pub error: String,
    pub hook_error: Option<E>,
}

impl<E> DevelopmentSwapFailure<E> {
    fn new(receipt: DevelopmentSwapReceipt, error: impl Into<String>) -> Self {
        Self {
            receipt,
            error: error.into(),
            hook_error: None,
        }
    }
}

/// The host-owned part of a development swap.
///
/// `stage` must not publish anything.  `publish` is called after the candidate
/// runtime has entered the state-migration transaction and before the hook is
/// run.  `restore` is called for every rejection or infrastructure failure,
/// including failures from `publish` itself.
pub trait DevelopmentSwapHost {
    type Staged;

    fn stage(
        &mut self,
        candidate: &JitProcess,
        descriptor: &DevelopmentSwapDescriptor,
    ) -> Result<Self::Staged, String>;

    fn publish(&mut self, staged: &mut Self::Staged) -> Result<(), String>;

    fn restore(&mut self, staged: Self::Staged) -> Result<(), String>;
}

enum ApplyError<E> {
    Host(String),
    Hook(E),
}

impl<E: fmt::Display> fmt::Display for ApplyError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Host(error) => formatter.write_str(error),
            Self::Hook(error) => error.fmt(formatter),
        }
    }
}

fn restore_after_failure<P, E>(
    host: &mut P,
    staged: P::Staged,
    mut failure: DevelopmentSwapFailure<E>,
) -> DevelopmentSwapFailure<E>
where
    P: DevelopmentSwapHost,
{
    if let Err(error) = host.restore(staged) {
        failure.error = format!("{}; host restoration failed: {error}", failure.error);
    }
    failure
}

/// Commit a fully compiled candidate at a synchronous safe point.
///
/// The function owns the common order: validate and finalize the migration
/// preview, stage host resources, activate and snapshot/restore the candidate
/// through [`activate_candidate_transactionally`], publish host state, run the
/// optional hook, and only then accept the candidate into `active`.
pub fn commit_development_swap<P, E, H>(
    active: &mut JitProcess,
    candidate: JitProcess,
    descriptor: DevelopmentSwapDescriptor,
    host: &mut P,
    hook: H,
) -> Result<DevelopmentSwapReceipt, DevelopmentSwapFailure<E>>
where
    P: DevelopmentSwapHost,
    E: fmt::Display,
    H: FnOnce(&JitProcess) -> Result<(), E>,
{
    let active_layout: JitStateLayout = active.state_layout();
    let candidate_layout = candidate.state_layout();
    let mut preview = match plan_state_migration(
        &active_layout,
        &candidate_layout,
        descriptor.changed_functions.clone(),
        false,
        None,
    ) {
        Ok(preview) => preview,
        Err(error) => {
            return Err(DevelopmentSwapFailure::new(
                DevelopmentSwapReceipt::planning_failure(&descriptor),
                error,
            ));
        }
    };
    finalize_runtime_preview(&candidate, &mut preview);
    let rejected_receipt =
        DevelopmentSwapReceipt::from_preview(&preview, DevelopmentSwapStatus::Rejected);
    if !preview.state_layout_compatible {
        return Err(DevelopmentSwapFailure::new(
            rejected_receipt,
            preview
                .rejection
                .clone()
                .unwrap_or_else(|| "incoming state layout is incompatible".to_string()),
        ));
    }

    let mut staged = match host.stage(&candidate, &descriptor) {
        Ok(staged) => staged,
        Err(error) => return Err(DevelopmentSwapFailure::new(rejected_receipt, error)),
    };

    let activation = activate_candidate_transactionally(
        Some(&*active),
        &candidate,
        &preview,
        descriptor.hook_may_mutate_state,
        || {
            host.publish(&mut staged).map_err(ApplyError::Host)?;
            hook(&candidate).map_err(ApplyError::Hook)
        },
        |result: &Result<(), ApplyError<E>>| result.is_ok(),
    );

    match activation {
        Ok(Ok(())) => {
            active.accept_staged_candidate(candidate);
            Ok(DevelopmentSwapReceipt::from_preview(
                &preview,
                DevelopmentSwapStatus::Accepted,
            ))
        }
        Ok(Err(ApplyError::Host(error))) => Err(restore_after_failure(
            host,
            staged,
            DevelopmentSwapFailure {
                receipt: rejected_receipt,
                error,
                hook_error: None,
            },
        )),
        Ok(Err(ApplyError::Hook(error))) => Err(restore_after_failure(
            host,
            staged,
            DevelopmentSwapFailure {
                receipt: rejected_receipt,
                error: error.to_string(),
                hook_error: Some(error),
            },
        )),
        Err(error) => Err(restore_after_failure(
            host,
            staged,
            DevelopmentSwapFailure::new(rejected_receipt, error),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::jit::JitScalarValue;
    use std::cell::Cell;
    use std::rc::Rc;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct StagedHost {
        previous: u32,
        next: u32,
    }

    struct TestHost {
        current: Rc<Cell<u32>>,
        next: u32,
        fail_stage: bool,
        fail_publish: bool,
        restores: usize,
    }

    impl DevelopmentSwapHost for TestHost {
        type Staged = StagedHost;

        fn stage(
            &mut self,
            _candidate: &JitProcess,
            _descriptor: &DevelopmentSwapDescriptor,
        ) -> Result<Self::Staged, String> {
            if self.fail_stage {
                return Err("stage failed".to_string());
            }
            Ok(StagedHost {
                previous: self.current.get(),
                next: self.next,
            })
        }

        fn publish(&mut self, staged: &mut Self::Staged) -> Result<(), String> {
            if self.fail_publish {
                self.current.set(staged.next);
                return Err("publish failed".to_string());
            }
            self.current.set(staged.next);
            Ok(())
        }

        fn restore(&mut self, staged: Self::Staged) -> Result<(), String> {
            self.current.set(staged.previous);
            self.restores += 1;
            Ok(())
        }
    }

    fn active_and_candidate(source: &str) -> (JitProcess, JitProcess) {
        let mut active = JitProcess::new();
        active.upsert_file("main.stasis", source);
        active.compile().expect("active source compiles");
        let mut candidate = active.staged_candidate();
        candidate
            .compile_staged()
            .expect("candidate source compiles");
        (active, candidate)
    }

    fn code_only_sources() -> (&'static str, &'static str) {
        (
            "function main(): i32 { return 1; } function tick(): i32 { return 1; }",
            "function main(): i32 { return 2; } function tick(): i32 { return 2; }",
        )
    }

    #[test]
    fn accepted_swap_publishes_before_hook_and_returns_common_receipt() {
        let (active_source, candidate_source) = code_only_sources();
        let (mut active, _) = active_and_candidate(active_source);
        let mut candidate = active.staged_candidate();
        candidate.upsert_file("main.stasis", candidate_source);
        candidate
            .compile_staged()
            .expect("candidate source compiles");
        let current = Rc::new(Cell::new(1));
        let hook_current = Rc::clone(&current);
        let mut host = TestHost {
            current: Rc::clone(&current),
            next: 2,
            fail_stage: false,
            fail_publish: false,
            restores: 0,
        };
        let hook_observed = std::cell::Cell::new(0u32);
        let receipt = commit_development_swap(
            &mut active,
            candidate,
            DevelopmentSwapDescriptor::new(vec!["tick".to_string(), "main".to_string()], false),
            &mut host,
            |_| {
                hook_observed.set(hook_current.get());
                Ok::<(), String>(())
            },
        )
        .expect("swap accepted");
        assert_eq!(receipt.status, DevelopmentSwapStatus::Accepted);
        assert_eq!(receipt.changed_functions, vec!["main", "tick"]);
        assert_eq!(current.get(), 2);
        assert_eq!(hook_observed.get(), 2);
        assert_eq!(host.restores, 0);
        assert_eq!(active.execute_i32_noarg_by_name("main"), Ok(2));
    }

    #[test]
    fn hook_failure_restores_host_and_runtime_state() {
        let source = "global State { score: i32; } function main(): i32 { State.score = 7; return 0; } function tick(): i32 { return State.score; }";
        let (mut active, _) = active_and_candidate(source);
        active
            .execute_i32_noarg_by_name("main")
            .expect("initialize state");
        let mut candidate = active.staged_candidate();
        candidate.upsert_file(
            "main.stasis",
            "global State { score: i32; } function main(): i32 { return 8; } function tick(): i32 { return State.score; } function on_code_swap(): void { State.score = 99; return; }",
        );
        candidate
            .compile_staged()
            .expect("candidate source compiles");
        let current = Rc::new(Cell::new(4));
        let mut host = TestHost {
            current: Rc::clone(&current),
            next: 5,
            fail_stage: false,
            fail_publish: false,
            restores: 0,
        };
        let failure = commit_development_swap(
            &mut active,
            candidate,
            DevelopmentSwapDescriptor::new(vec!["on_code_swap".to_string()], true),
            &mut host,
            |candidate| {
                candidate
                    .write_global_scalar("State.score", JitScalarValue::I32(99))
                    .expect("mutate candidate state");
                Err::<(), _>("hook rejected".to_string())
            },
        )
        .expect_err("hook rejection");
        assert_eq!(failure.receipt.status, DevelopmentSwapStatus::Rejected);
        assert_eq!(failure.hook_error.as_deref(), Some("hook rejected"));
        assert_eq!(current.get(), 4);
        assert_eq!(host.restores, 1);
        assert_eq!(
            active.read_global_scalar("State.score"),
            Ok(JitScalarValue::I32(7))
        );
    }

    #[test]
    fn publication_failure_restores_host_and_runtime() {
        let (active_source, candidate_source) = code_only_sources();
        let (mut active, _) = active_and_candidate(active_source);
        let mut candidate = active.staged_candidate();
        candidate.upsert_file("main.stasis", candidate_source);
        candidate
            .compile_staged()
            .expect("candidate source compiles");
        let current = Rc::new(Cell::new(1));
        let mut host = TestHost {
            current: Rc::clone(&current),
            next: 2,
            fail_stage: false,
            fail_publish: true,
            restores: 0,
        };
        let failure = commit_development_swap(
            &mut active,
            candidate,
            DevelopmentSwapDescriptor::new(vec!["main".to_string()], false),
            &mut host,
            |_| Ok::<(), String>(()),
        )
        .expect_err("publication failure");
        assert!(failure.error.contains("publish failed"));
        assert_eq!(current.get(), 1);
        assert_eq!(host.restores, 1);
        assert_eq!(active.execute_i32_noarg_by_name("main"), Ok(1));
    }

    #[test]
    fn stage_failure_rejects_before_runtime_or_host_publication() {
        let (active_source, candidate_source) = code_only_sources();
        let (mut active, _) = active_and_candidate(active_source);
        let mut candidate = active.staged_candidate();
        candidate.upsert_file("main.stasis", candidate_source);
        candidate
            .compile_staged()
            .expect("candidate source compiles");
        let current = Rc::new(Cell::new(1));
        let mut host = TestHost {
            current: Rc::clone(&current),
            next: 2,
            fail_stage: true,
            fail_publish: false,
            restores: 0,
        };
        let failure = commit_development_swap(
            &mut active,
            candidate,
            DevelopmentSwapDescriptor::new(vec!["main".to_string()], false),
            &mut host,
            |_| Ok::<(), String>(()),
        )
        .expect_err("stage failure");
        assert!(failure.error.contains("stage failed"));
        assert_eq!(failure.receipt.status, DevelopmentSwapStatus::Rejected);
        assert_eq!(current.get(), 1);
        assert_eq!(host.restores, 0);
        assert_eq!(active.execute_i32_noarg_by_name("main"), Ok(1));
    }

    #[test]
    fn incompatible_layout_rejects_before_host_stage() {
        let (mut active, _) = active_and_candidate(
            "global State { score: i32; } function main(): i32 { return State.score; }",
        );
        let mut candidate = active.staged_candidate();
        candidate.upsert_file(
            "main.stasis",
            "global State { score: f32; } function main(): i32 { return 0; }",
        );
        candidate
            .compile_staged()
            .expect("candidate source compiles");
        let current = Rc::new(Cell::new(1));
        let mut host = TestHost {
            current: Rc::clone(&current),
            next: 2,
            fail_stage: false,
            fail_publish: false,
            restores: 0,
        };
        let failure = commit_development_swap(
            &mut active,
            candidate,
            DevelopmentSwapDescriptor::new(Vec::<String>::new(), false),
            &mut host,
            |_| Ok::<(), String>(()),
        )
        .expect_err("incompatible layout");
        assert_eq!(failure.receipt.status, DevelopmentSwapStatus::Rejected);
        assert!(!failure.receipt.state_layout_compatible);
        assert_eq!(current.get(), 1);
        assert_eq!(host.restores, 0);
    }
}
