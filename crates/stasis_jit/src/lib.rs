#![forbid(unsafe_code)]

use std::collections::{BTreeMap, VecDeque};
use stasis_runner::swap::contracts::{CodeGeneration, FnId, FunctionPatchSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CodePtr(pub u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitOutcome {
    pub new_generation: CodeGeneration,
    pub swapped_fn_ids: Vec<FnId>,
    pub retired_generations: Vec<CodeGeneration>,
}

/// Dev/runtime-facing indirection table (`FnId -> code_ptr`) with simple
/// generation retirement bookkeeping.
pub struct FunctionPointerTable {
    entries: BTreeMap<FnId, CodePtr>,
    generation: u64,
    pending_retire: VecDeque<CodeGeneration>,
    safe_retire_window: usize,
}

impl FunctionPointerTable {
    pub fn new() -> Self {
        Self::with_safe_retire_window(2)
    }

    pub fn with_safe_retire_window(safe_retire_window: usize) -> Self {
        Self {
            entries: BTreeMap::new(),
            generation: 0,
            pending_retire: VecDeque::new(),
            safe_retire_window,
        }
    }

    pub fn generation(&self) -> CodeGeneration {
        CodeGeneration(self.generation)
    }

    pub fn code_ptr(&self, fn_id: FnId) -> Option<CodePtr> {
        self.entries.get(&fn_id).copied()
    }

    pub fn commit_patch_set(&mut self, patch_set: &FunctionPatchSet) -> CommitOutcome {
        self.generation += 1;
        let new_generation = CodeGeneration(self.generation);

        let mut swapped_fn_ids = Vec::with_capacity(patch_set.functions.len());
        for patch in &patch_set.functions {
            let fn_id = patch.fn_id;
            let code_ptr = make_code_ptr(new_generation, fn_id);
            self.entries.insert(fn_id, code_ptr);
            swapped_fn_ids.push(fn_id);
        }

        if self.generation > 1 {
            self.pending_retire
                .push_back(CodeGeneration(self.generation - 1));
        }

        let mut retired_generations = Vec::new();
        while self.pending_retire.len() > self.safe_retire_window {
            if let Some(retired) = self.pending_retire.pop_front() {
                retired_generations.push(retired);
            }
        }

        CommitOutcome {
            new_generation,
            swapped_fn_ids,
            retired_generations,
        }
    }
}

impl Default for FunctionPointerTable {
    fn default() -> Self {
        Self::new()
    }
}

fn make_code_ptr(generation: CodeGeneration, fn_id: FnId) -> CodePtr {
    CodePtr((generation.0 << 32) | u64::from(fn_id.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use stasis_runner::swap::contracts::{FunctionPatch, FunctionPatchSet};

    fn patch_set(ids: &[u32]) -> FunctionPatchSet {
        FunctionPatchSet {
            functions: ids
                .iter()
                .copied()
                .map(|id| FunctionPatch { fn_id: FnId(id) })
                .collect(),
        }
    }

    #[test]
    fn initial_state_has_no_generation_and_no_entries() {
        let table = FunctionPointerTable::new();
        assert_eq!(table.generation(), CodeGeneration(0));
        assert_eq!(table.code_ptr(FnId(1)), None);
    }

    #[test]
    fn commit_updates_fn_ids_and_generation() {
        let mut table = FunctionPointerTable::new();
        let outcome = table.commit_patch_set(&patch_set(&[7, 11]));

        assert_eq!(outcome.new_generation, CodeGeneration(1));
        assert_eq!(outcome.swapped_fn_ids, vec![FnId(7), FnId(11)]);
        assert!(outcome.retired_generations.is_empty());
        assert_eq!(table.generation(), CodeGeneration(1));

        let ptr_7 = table.code_ptr(FnId(7)).expect("missing fn 7 code pointer");
        let ptr_11 = table.code_ptr(FnId(11)).expect("missing fn 11 code pointer");
        assert_eq!(ptr_7, CodePtr((1_u64 << 32) | 7));
        assert_eq!(ptr_11, CodePtr((1_u64 << 32) | 11));
    }

    #[test]
    fn repeated_commit_rewrites_code_ptr_for_same_fn_id() {
        let mut table = FunctionPointerTable::new();
        table.commit_patch_set(&patch_set(&[3]));
        let before = table.code_ptr(FnId(3)).expect("expected first code pointer");

        let outcome = table.commit_patch_set(&patch_set(&[3]));
        let after = table.code_ptr(FnId(3)).expect("expected rewritten code pointer");

        assert_eq!(outcome.new_generation, CodeGeneration(2));
        assert_ne!(before, after);
        assert_eq!(after, CodePtr((2_u64 << 32) | 3));
    }

    #[test]
    fn retires_old_generations_after_safe_window() {
        let mut table = FunctionPointerTable::with_safe_retire_window(2);

        let c1 = table.commit_patch_set(&patch_set(&[1]));
        let c2 = table.commit_patch_set(&patch_set(&[2]));
        let c3 = table.commit_patch_set(&patch_set(&[3]));
        let c4 = table.commit_patch_set(&patch_set(&[4]));

        assert!(c1.retired_generations.is_empty());
        assert!(c2.retired_generations.is_empty());
        assert!(c3.retired_generations.is_empty());
        assert_eq!(c4.retired_generations, vec![CodeGeneration(1)]);
    }
}
