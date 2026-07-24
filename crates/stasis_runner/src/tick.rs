use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickPhase {
    BetweenTicks,
    Gameplay,
    StructuralCommit,
    Normalize,
    Validate,
    HashAndSnapshot,
    Render,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TickError {
    pub tick: u64,
    pub phase: TickPhase,
    pub message: String,
}

impl fmt::Display for TickError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "tick {} rejected during {:?}: {}",
            self.tick, self.phase, self.message
        )
    }
}

impl std::error::Error for TickError {}

pub trait TickTransaction {
    type Checkpoint;
    type Snapshot;

    fn checkpoint(&mut self) -> Result<Self::Checkpoint, String>;
    fn restore(&mut self, checkpoint: Self::Checkpoint);
    fn gameplay(&mut self) -> Result<(), String>;
    fn commit_structural(&mut self) -> Result<(), String>;
    fn normalize(&mut self) -> Result<(), String>;
    fn validate(&mut self) -> Result<(), String>;
    fn state_hash(&mut self) -> Result<u64, String>;
    fn capture(&mut self) -> Result<Self::Snapshot, String>;
    fn render(&mut self) -> Result<(), String>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TickCommit<S> {
    pub tick: u64,
    pub state_hash: u64,
    pub snapshot: S,
}

#[derive(Debug, Clone)]
pub struct TickCoordinator {
    accepted_ticks: u64,
    phase: TickPhase,
    last_state_hash: Option<u64>,
}

impl Default for TickCoordinator {
    fn default() -> Self {
        Self {
            accepted_ticks: 0,
            phase: TickPhase::BetweenTicks,
            last_state_hash: None,
        }
    }
}

impl TickCoordinator {
    pub fn accepted_ticks(&self) -> u64 {
        self.accepted_ticks
    }

    pub fn phase(&self) -> TickPhase {
        self.phase
    }

    pub fn last_state_hash(&self) -> Option<u64> {
        self.last_state_hash
    }

    pub fn require_between_ticks(&self) -> Result<(), String> {
        if self.phase == TickPhase::BetweenTicks {
            Ok(())
        } else {
            Err(format!(
                "between-tick operation rejected during {:?}",
                self.phase
            ))
        }
    }

    pub fn run_tick<Transaction>(
        &mut self,
        transaction: &mut Transaction,
    ) -> Result<TickCommit<Transaction::Snapshot>, TickError>
    where
        Transaction: TickTransaction,
    {
        let Some(tick) = self.accepted_ticks.checked_add(1) else {
            return Err(TickError {
                tick: self.accepted_ticks,
                phase: TickPhase::BetweenTicks,
                message: "accepted tick counter overflow".to_string(),
            });
        };
        let checkpoint = transaction.checkpoint().map_err(|message| TickError {
            tick,
            phase: TickPhase::BetweenTicks,
            message,
        })?;

        self.phase = TickPhase::Gameplay;
        if let Err(error) = transaction.gameplay() {
            return self.reject(transaction, checkpoint, tick, error);
        }

        self.phase = TickPhase::StructuralCommit;
        if let Err(error) = transaction.commit_structural() {
            return self.reject(transaction, checkpoint, tick, error);
        }

        self.phase = TickPhase::Normalize;
        if let Err(error) = transaction.normalize() {
            return self.reject(transaction, checkpoint, tick, error);
        }

        self.phase = TickPhase::Validate;
        if let Err(error) = transaction.validate() {
            return self.reject(transaction, checkpoint, tick, error);
        }

        self.phase = TickPhase::HashAndSnapshot;
        let state_hash = match transaction.state_hash() {
            Ok(state_hash) => state_hash,
            Err(error) => return self.reject(transaction, checkpoint, tick, error),
        };
        let snapshot = match transaction.capture() {
            Ok(snapshot) => snapshot,
            Err(error) => return self.reject(transaction, checkpoint, tick, error),
        };

        self.accepted_ticks = tick;
        self.last_state_hash = Some(state_hash);
        self.phase = TickPhase::Render;
        if let Err(message) = transaction.render() {
            self.phase = TickPhase::BetweenTicks;
            return Err(TickError {
                tick,
                phase: TickPhase::Render,
                message,
            });
        }
        self.phase = TickPhase::BetweenTicks;

        Ok(TickCommit {
            tick,
            state_hash,
            snapshot,
        })
    }

    fn reject<Transaction>(
        &mut self,
        transaction: &mut Transaction,
        checkpoint: Transaction::Checkpoint,
        tick: u64,
        message: String,
    ) -> Result<TickCommit<Transaction::Snapshot>, TickError>
    where
        Transaction: TickTransaction,
    {
        let phase = self.phase;
        transaction.restore(checkpoint);
        self.phase = TickPhase::BetweenTicks;
        Err(TickError {
            tick,
            phase,
            message,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpawnRequest(usize);

type PoolCommitStep<State> = Box<dyn FnMut(&mut State) -> Result<(), String>>;

pub struct DeclaredPoolCommits<State> {
    pools: Vec<(String, PoolCommitStep<State>)>,
}

impl<State> Default for DeclaredPoolCommits<State> {
    fn default() -> Self {
        Self { pools: Vec::new() }
    }
}

impl<State> DeclaredPoolCommits<State> {
    pub fn declare(
        &mut self,
        name: impl Into<String>,
        commit: impl FnMut(&mut State) -> Result<(), String> + 'static,
    ) {
        self.pools.push((name.into(), Box::new(commit)));
    }

    pub fn commit_all(&mut self, state: &mut State) -> Result<(), String> {
        for (declaration_index, (name, commit)) in self.pools.iter_mut().enumerate() {
            commit(state).map_err(|error| {
                format!("pool '{name}' at declaration index {declaration_index} rejected: {error}")
            })?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolCommit<const CAPACITY: usize> {
    old_to_new: [Option<usize>; CAPACITY],
    spawned_indices: [Option<usize>; CAPACITY],
}

impl<const CAPACITY: usize> PoolCommit<CAPACITY> {
    pub fn repaired_index(&self, original_index: usize) -> Option<usize> {
        self.old_to_new.get(original_index).copied().flatten()
    }

    pub fn repair_optional(&self, index: &mut Option<usize>) {
        *index = index.and_then(|value| self.repaired_index(value));
    }

    pub fn spawned_index(&self, request: SpawnRequest) -> Option<usize> {
        self.spawned_indices.get(request.0).copied().flatten()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundedPool<T, const CAPACITY: usize> {
    items: Vec<T>,
    removals: Vec<usize>,
    additions: Vec<T>,
    scratch: Vec<T>,
}

impl<T, const CAPACITY: usize> Default for BoundedPool<T, CAPACITY> {
    fn default() -> Self {
        Self {
            items: Vec::with_capacity(CAPACITY),
            removals: Vec::with_capacity(CAPACITY),
            additions: Vec::with_capacity(CAPACITY),
            scratch: Vec::with_capacity(CAPACITY),
        }
    }
}

impl<T, const CAPACITY: usize> BoundedPool<T, CAPACITY> {
    pub fn from_items(items: Vec<T>) -> Result<Self, String> {
        if items.len() > CAPACITY {
            return Err(format!(
                "pool initializer has {} items; capacity is {CAPACITY}",
                items.len()
            ));
        }
        let mut pool = Self::default();
        pool.items.extend(items);
        Ok(pool)
    }

    pub fn items(&self) -> &[T] {
        &self.items
    }

    pub fn items_mut(&mut self) -> &mut [T] {
        &mut self.items
    }

    pub fn queue_remove(&mut self, original_index: usize) -> Result<(), String> {
        if self.removals.len() >= CAPACITY {
            return Err(format!("removal queue capacity {CAPACITY} exceeded"));
        }
        self.removals.push(original_index);
        Ok(())
    }

    pub fn queue_destroy(&mut self, original_index: usize) -> Result<(), String> {
        self.queue_remove(original_index)
    }

    pub fn queue_add(&mut self, value: T) -> Result<SpawnRequest, String> {
        if self.additions.len() >= CAPACITY {
            return Err(format!("addition queue capacity {CAPACITY} exceeded"));
        }
        let request = SpawnRequest(self.additions.len());
        self.additions.push(value);
        Ok(request)
    }

    pub fn queue_spawn(&mut self, value: T) -> Result<SpawnRequest, String> {
        self.queue_add(value)
    }

    pub fn has_pending_requests(&self) -> bool {
        !self.removals.is_empty() || !self.additions.is_empty()
    }
}

impl<T: Clone, const CAPACITY: usize> BoundedPool<T, CAPACITY> {
    pub fn commit(&mut self) -> Result<PoolCommit<CAPACITY>, String> {
        let mut removed = [false; CAPACITY];
        for &index in &self.removals {
            if index >= self.items.len() {
                return Err(format!(
                    "removal index {index} is outside original pool length {}",
                    self.items.len()
                ));
            }
            let slot = &mut removed[index];
            if *slot {
                return Err(format!(
                    "removal index {index} was requested more than once"
                ));
            }
            *slot = true;
        }

        let survivor_count = self.items.len().saturating_sub(self.removals.len());
        let final_count = survivor_count
            .checked_add(self.additions.len())
            .ok_or_else(|| "pool size overflow".to_string())?;
        if final_count > CAPACITY {
            return Err(format!(
                "pool commit needs {final_count} slots; capacity is {CAPACITY}"
            ));
        }

        self.scratch.clear();
        let mut old_to_new = [None; CAPACITY];
        for (old_index, item) in self.items.iter().enumerate() {
            if !removed[old_index] {
                old_to_new[old_index] = Some(self.scratch.len());
                self.scratch.push(item.clone());
            }
        }

        let mut spawned_indices = [None; CAPACITY];
        for (request_index, item) in self.additions.iter().enumerate() {
            spawned_indices[request_index] = Some(self.scratch.len());
            self.scratch.push(item.clone());
        }

        std::mem::swap(&mut self.items, &mut self.scratch);
        self.scratch.clear();
        self.removals.clear();
        self.additions.clear();
        Ok(PoolCommit {
            old_to_new,
            spawned_indices,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundedEventQueue<T, const CAPACITY: usize> {
    pending: Vec<T>,
    committed: Vec<T>,
}

impl<T, const CAPACITY: usize> Default for BoundedEventQueue<T, CAPACITY> {
    fn default() -> Self {
        Self {
            pending: Vec::with_capacity(CAPACITY),
            committed: Vec::with_capacity(CAPACITY),
        }
    }
}

impl<T, const CAPACITY: usize> BoundedEventQueue<T, CAPACITY> {
    pub fn emit(&mut self, event: T) -> Result<(), String> {
        if self.pending.len() >= CAPACITY {
            return Err(format!("event queue capacity {CAPACITY} exceeded"));
        }
        self.pending.push(event);
        Ok(())
    }

    pub fn commit(&mut self) -> Result<(), String> {
        if !self.committed.is_empty() {
            return Err(
                "committed events must be consumed before the next tick commit".to_string(),
            );
        }
        self.committed.append(&mut self.pending);
        Ok(())
    }

    pub fn committed(&self) -> &[T] {
        &self.committed
    }

    pub fn drain_committed(&mut self) -> std::vec::Drain<'_, T> {
        self.committed.drain(..)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    #[test]
    fn pool_commit_uses_original_indices_stable_compaction_and_request_order() {
        let mut pool = BoundedPool::<i32, 6>::from_items(vec![10, 20, 30, 40]).unwrap();
        pool.queue_remove(3).unwrap();
        pool.queue_remove(1).unwrap();
        let first = pool.queue_add(50).unwrap();
        let second = pool.queue_spawn(60).unwrap();

        let commit = pool.commit().unwrap();

        assert_eq!(pool.items(), &[10, 30, 50, 60]);
        assert_eq!(commit.repaired_index(0), Some(0));
        assert_eq!(commit.repaired_index(1), None);
        assert_eq!(commit.repaired_index(2), Some(1));
        assert_eq!(commit.repaired_index(3), None);
        assert_eq!(commit.spawned_index(first), Some(2));
        assert_eq!(commit.spawned_index(second), Some(3));
    }

    #[test]
    fn rejected_pool_commit_preserves_items_and_pending_requests() {
        let mut pool = BoundedPool::<i32, 3>::from_items(vec![10, 20, 30]).unwrap();
        pool.queue_remove(1).unwrap();
        pool.queue_remove(1).unwrap();
        pool.queue_add(40).unwrap();

        let error = pool.commit().unwrap_err();

        assert!(error.contains("more than once"));
        assert_eq!(pool.items(), &[10, 20, 30]);
        assert!(pool.has_pending_requests());
    }

    #[test]
    fn removal_inside_capacity_but_outside_original_length_is_rejected() {
        let mut pool = BoundedPool::<i32, 4>::from_items(vec![10, 20]).unwrap();
        pool.queue_remove(3).unwrap();

        let error = pool.commit().unwrap_err();

        assert!(error.contains("outside original pool length 2"));
        assert_eq!(pool.items(), &[10, 20]);
    }

    #[test]
    fn events_preserve_request_order_and_cannot_overwrite_unconsumed_delivery() {
        let mut events = BoundedEventQueue::<&str, 2>::default();
        events.emit("destroyed").unwrap();
        events.emit("spawned").unwrap();
        assert!(events.emit("overflow").is_err());
        events.commit().unwrap();
        assert_eq!(events.committed(), &["destroyed", "spawned"]);
        assert!(events.commit().is_err());
        assert_eq!(
            events.drain_committed().collect::<Vec<_>>(),
            vec!["destroyed", "spawned"]
        );
    }

    #[test]
    fn pool_set_commits_in_declaration_order_and_names_rejection() {
        let mut commits = DeclaredPoolCommits::<Vec<&str>>::default();
        commits.declare("enemies", |order| {
            order.push("enemies");
            Ok(())
        });
        commits.declare("effects", |order| {
            order.push("effects");
            Err("capacity 4 exceeded".to_string())
        });
        let mut order = Vec::new();

        let error = commits.commit_all(&mut order).unwrap_err();

        assert_eq!(order, ["enemies", "effects"]);
        assert!(error.contains("pool 'effects' at declaration index 1"));
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct State {
        value: i32,
        pool: BoundedPool<i32, 4>,
    }

    struct TestTransaction<'a> {
        state: &'a mut State,
        stages: &'a RefCell<Vec<&'static str>>,
        reject_validation: bool,
        rendered: &'a Cell<bool>,
    }

    impl TickTransaction for TestTransaction<'_> {
        type Checkpoint = State;
        type Snapshot = State;

        fn checkpoint(&mut self) -> Result<Self::Checkpoint, String> {
            Ok(self.state.clone())
        }

        fn restore(&mut self, checkpoint: Self::Checkpoint) {
            *self.state = checkpoint;
        }

        fn gameplay(&mut self) -> Result<(), String> {
            self.stages.borrow_mut().push("gameplay");
            self.state.value += 1;
            self.state.pool.queue_destroy(0)?;
            self.state.pool.queue_spawn(30)?;
            Ok(())
        }

        fn commit_structural(&mut self) -> Result<(), String> {
            self.stages.borrow_mut().push("structural");
            self.state.pool.commit()?;
            Ok(())
        }

        fn normalize(&mut self) -> Result<(), String> {
            self.stages.borrow_mut().push("normalize");
            self.state.value *= 2;
            Ok(())
        }

        fn validate(&mut self) -> Result<(), String> {
            self.stages.borrow_mut().push("validate");
            if self.reject_validation {
                return Err("broken invariant".to_string());
            }
            (self.state.value == 4)
                .then_some(())
                .ok_or_else(|| "value invariant failed".to_string())
        }

        fn state_hash(&mut self) -> Result<u64, String> {
            self.stages.borrow_mut().push("hash");
            Ok(self.state.value as u64 * 100 + self.state.pool.items().len() as u64)
        }

        fn capture(&mut self) -> Result<Self::Snapshot, String> {
            self.stages.borrow_mut().push("snapshot");
            Ok(self.state.clone())
        }

        fn render(&mut self) -> Result<(), String> {
            self.stages.borrow_mut().push("render");
            self.rendered.set(true);
            Ok(())
        }
    }

    #[test]
    fn normalized_tick_runs_in_order_and_renders_only_accepted_state() {
        let mut coordinator = TickCoordinator::default();
        let mut state = State {
            value: 1,
            pool: BoundedPool::from_items(vec![10, 20]).unwrap(),
        };
        let stages = RefCell::new(Vec::new());
        let rendered = Cell::new(false);
        let mut transaction = TestTransaction {
            state: &mut state,
            stages: &stages,
            reject_validation: false,
            rendered: &rendered,
        };

        let commit = coordinator.run_tick(&mut transaction).unwrap();

        assert_eq!(
            *stages.borrow(),
            [
                "gameplay",
                "structural",
                "normalize",
                "validate",
                "hash",
                "snapshot",
                "render"
            ]
        );
        assert_eq!(commit.tick, 1);
        assert_eq!(commit.state_hash, 402);
        assert_eq!(commit.snapshot, state);
        assert!(rendered.get());
        assert_eq!(coordinator.phase(), TickPhase::BetweenTicks);
    }

    #[test]
    fn invariant_rejection_restores_previous_accepted_boundary() {
        let mut coordinator = TickCoordinator::default();
        let mut state = State {
            value: 7,
            pool: BoundedPool::from_items(vec![10, 20]).unwrap(),
        };
        let baseline = state.clone();
        let stages = RefCell::new(Vec::new());
        let rendered = Cell::new(false);
        let mut transaction = TestTransaction {
            state: &mut state,
            stages: &stages,
            reject_validation: true,
            rendered: &rendered,
        };

        let error = coordinator.run_tick(&mut transaction).unwrap_err();

        assert_eq!(error.phase, TickPhase::Validate);
        assert_eq!(state, baseline);
        assert!(!rendered.get());
        assert_eq!(coordinator.accepted_ticks(), 0);
        assert_eq!(coordinator.phase(), TickPhase::BetweenTicks);
    }
}
