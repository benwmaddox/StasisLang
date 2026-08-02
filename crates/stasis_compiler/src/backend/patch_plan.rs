use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::backend::reachability::compute_reachable_function_ids;
use crate::compiler::{FunctionId, FunctionMeta, SourceFile};
use crate::identity::SymbolId;

const LIFECYCLE_ROOTS: [&str; 4] = ["main", "tick", "render", "on_code_swap"];

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FunctionKey {
    pub symbol_id: SymbolId,
    pub name: String,
}

impl FunctionKey {
    pub fn from_function(function: &FunctionMeta) -> Self {
        Self {
            symbol_id: function.symbol_id.clone(),
            name: function.name.clone(),
        }
    }

    pub fn display_name(&self) -> String {
        self.symbol_id.to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedFunction {
    pub key: FunctionKey,
    pub body_hash: u64,
    pub dependencies: BTreeSet<FunctionKey>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AcceptedProgram {
    pub functions: BTreeMap<FunctionKey, AcceptedFunction>,
    pub reachable: BTreeSet<FunctionKey>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchReason {
    ColdStart,
    BodyChanged,
    AddedOrSignatureChanged,
    BecameReachable,
    LoweredContractChanged,
    SccPeer { changed: FunctionKey },
    DirectCaller { callee: FunctionKey },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchReasonChain {
    pub function: FunctionKey,
    pub reason: PatchReason,
    pub path_from_change: Vec<FunctionKey>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchPlan {
    pub cold_start: bool,
    pub changed: Vec<FunctionKey>,
    pub re_jit_ids: Vec<FunctionId>,
    pub re_jit: Vec<FunctionKey>,
    pub reused: Vec<FunctionKey>,
    pub retained_dependencies: Vec<FunctionKey>,
    pub affected_host_entries: Vec<FunctionKey>,
    pub removed: Vec<FunctionKey>,
    pub reasons: Vec<PatchReasonChain>,
}

impl PatchPlan {
    pub fn reason_for(&self, key: &FunctionKey) -> Option<&PatchReasonChain> {
        self.reasons.iter().find(|reason| &reason.function == key)
    }
}

pub fn capture_accepted_program(
    functions: &[FunctionMeta],
    files: &[SourceFile],
    required_roots: &[String],
) -> Result<AcceptedProgram, String> {
    let graph = CurrentGraph::build(functions, files, required_roots)?;
    Ok(AcceptedProgram {
        functions: graph
            .by_key
            .iter()
            .map(|(key, function)| {
                (
                    key.clone(),
                    AcceptedFunction {
                        key: key.clone(),
                        body_hash: function.body_hash,
                        dependencies: function.dependencies.clone(),
                    },
                )
            })
            .collect(),
        reachable: graph.reachable,
    })
}

pub fn plan_patch(
    functions: &[FunctionMeta],
    files: &[SourceFile],
    required_roots: &[String],
    accepted: Option<&AcceptedProgram>,
    lowered_contract_changes: &BTreeSet<FunctionKey>,
) -> Result<PatchPlan, String> {
    let graph = CurrentGraph::build(functions, files, required_roots)?;
    let cold_start = accepted.is_none();
    let mut reasons: BTreeMap<FunctionKey, PatchReasonChain> = BTreeMap::new();
    let mut queue = VecDeque::new();
    let removed: Vec<FunctionKey> = accepted
        .map(|accepted| {
            accepted
                .functions
                .keys()
                .filter(|key| !graph.by_key.contains_key(*key))
                .cloned()
                .collect()
        })
        .unwrap_or_default();

    if let Some(accepted) = accepted {
        for key in &graph.reachable {
            let Some(current) = graph.by_key.get(key) else {
                return Err(format!(
                    "reachable function '{}' is missing",
                    key.display_name()
                ));
            };
            let seed_reason = match accepted.functions.get(key) {
                None => Some(PatchReason::AddedOrSignatureChanged),
                Some(_) if !accepted.reachable.contains(key) => Some(PatchReason::BecameReachable),
                Some(previous) if previous.body_hash != current.body_hash => {
                    Some(PatchReason::BodyChanged)
                }
                Some(_) if lowered_contract_changes.contains(key) => {
                    Some(PatchReason::LoweredContractChanged)
                }
                Some(_) => None,
            };
            if let Some(reason) = seed_reason {
                insert_seed(&mut reasons, &mut queue, key.clone(), reason);
            }
        }
        let removed_reachable: BTreeSet<FunctionKey> = removed
            .iter()
            .filter(|key| accepted.reachable.contains(*key))
            .cloned()
            .collect();
        for caller in &graph.reachable {
            let Some(removed_callee) = accepted
                .functions
                .get(caller)
                .and_then(|function| {
                    function
                        .dependencies
                        .iter()
                        .find(|dependency| removed_reachable.contains(dependency))
                })
                .cloned()
            else {
                continue;
            };
            if reasons.contains_key(caller) {
                continue;
            }
            insert_seed(
                &mut reasons,
                &mut queue,
                caller.clone(),
                PatchReason::DirectCaller {
                    callee: removed_callee.clone(),
                },
            );
            reasons
                .get_mut(caller)
                .expect("removed-callee seed was inserted")
                .path_from_change
                .insert(0, removed_callee);
        }
    } else {
        for key in &graph.reachable {
            insert_seed(
                &mut reasons,
                &mut queue,
                key.clone(),
                PatchReason::ColdStart,
            );
        }
    }

    let changed: Vec<FunctionKey> = reasons.keys().cloned().collect();
    if !cold_start {
        expand_affected_closure(&graph, &mut reasons, &mut queue);
    }

    let re_jit: Vec<FunctionKey> = reasons.keys().cloned().collect();
    let re_jit_set: BTreeSet<FunctionKey> = re_jit.iter().cloned().collect();
    let reused: Vec<FunctionKey> = graph.reachable.difference(&re_jit_set).cloned().collect();
    let retained_dependencies: BTreeSet<FunctionKey> = re_jit
        .iter()
        .flat_map(|key| {
            graph
                .by_key
                .get(key)
                .into_iter()
                .flat_map(|function| function.dependencies.iter())
        })
        .filter(|dependency| graph.reachable.contains(*dependency))
        .filter(|dependency| !re_jit_set.contains(*dependency))
        .cloned()
        .collect();
    let affected_host_entries = re_jit
        .iter()
        .filter(|key| graph.host_entries.contains(*key))
        .cloned()
        .collect();
    let re_jit_ids = re_jit
        .iter()
        .filter_map(|key| graph.by_key.get(key).map(|function| function.id))
        .collect();
    Ok(PatchPlan {
        cold_start,
        changed,
        re_jit_ids,
        re_jit,
        reused,
        retained_dependencies: retained_dependencies.into_iter().collect(),
        affected_host_entries,
        removed,
        reasons: reasons.into_values().collect(),
    })
}

fn insert_seed(
    reasons: &mut BTreeMap<FunctionKey, PatchReasonChain>,
    queue: &mut VecDeque<FunctionKey>,
    key: FunctionKey,
    reason: PatchReason,
) {
    if reasons.contains_key(&key) {
        return;
    }
    reasons.insert(
        key.clone(),
        PatchReasonChain {
            function: key.clone(),
            reason,
            path_from_change: vec![key.clone()],
        },
    );
    queue.push_back(key);
}

fn expand_affected_closure(
    graph: &CurrentGraph,
    reasons: &mut BTreeMap<FunctionKey, PatchReasonChain>,
    queue: &mut VecDeque<FunctionKey>,
) {
    let mut expanded_components = BTreeSet::new();
    while let Some(callee) = queue.pop_front() {
        if let Some(component_index) = graph.scc_index_by_key.get(&callee).copied() {
            if expanded_components.insert(component_index) {
                if let Some(component) = graph.sccs.get(component_index) {
                    let base_path = reasons
                        .get(&callee)
                        .map(|reason| reason.path_from_change.clone())
                        .unwrap_or_else(|| vec![callee.clone()]);
                    for peer in component {
                        if reasons.contains_key(peer) {
                            continue;
                        }
                        let mut path = base_path.clone();
                        path.push(peer.clone());
                        reasons.insert(
                            peer.clone(),
                            PatchReasonChain {
                                function: peer.clone(),
                                reason: PatchReason::SccPeer {
                                    changed: callee.clone(),
                                },
                                path_from_change: path,
                            },
                        );
                        queue.push_back(peer.clone());
                    }
                }
            }
        }
        let Some(callers) = graph.reverse.get(&callee) else {
            continue;
        };
        for caller in callers {
            if !graph.reachable.contains(caller) || reasons.contains_key(caller) {
                continue;
            }
            let mut path = reasons
                .get(&callee)
                .map(|reason| reason.path_from_change.clone())
                .unwrap_or_else(|| vec![callee.clone()]);
            path.push(caller.clone());
            reasons.insert(
                caller.clone(),
                PatchReasonChain {
                    function: caller.clone(),
                    reason: PatchReason::DirectCaller {
                        callee: callee.clone(),
                    },
                    path_from_change: path,
                },
            );
            queue.push_back(caller.clone());
        }
    }
}

#[derive(Debug, Clone)]
struct CurrentFunction {
    id: FunctionId,
    body_hash: u64,
    dependencies: BTreeSet<FunctionKey>,
}

#[derive(Debug, Clone)]
struct CurrentGraph {
    by_key: BTreeMap<FunctionKey, CurrentFunction>,
    reachable: BTreeSet<FunctionKey>,
    reverse: BTreeMap<FunctionKey, BTreeSet<FunctionKey>>,
    host_entries: BTreeSet<FunctionKey>,
    scc_index_by_key: BTreeMap<FunctionKey, usize>,
    sccs: Vec<BTreeSet<FunctionKey>>,
}

impl CurrentGraph {
    fn build(
        functions: &[FunctionMeta],
        files: &[SourceFile],
        required_roots: &[String],
    ) -> Result<Self, String> {
        let mut key_by_id = BTreeMap::new();
        for function in functions {
            files.get(function.file_id as usize).ok_or_else(|| {
                format!(
                    "function '{}' references missing source file",
                    function.name
                )
            })?;
            key_by_id.insert(function.id, FunctionKey::from_function(function));
        }
        let reachable_ids = compute_reachable_function_ids(functions, required_roots);
        let reachable: BTreeSet<FunctionKey> = reachable_ids
            .iter()
            .filter_map(|id| key_by_id.get(id).cloned())
            .collect();
        let required_names: BTreeSet<&str> = LIFECYCLE_ROOTS
            .iter()
            .copied()
            .chain(required_roots.iter().map(String::as_str))
            .collect();
        let host_entries = functions
            .iter()
            .filter(|function| required_names.contains(function.name.as_str()))
            .filter_map(|function| key_by_id.get(&function.id).cloned())
            .collect();
        let mut by_key = BTreeMap::new();
        let mut reverse: BTreeMap<FunctionKey, BTreeSet<FunctionKey>> = BTreeMap::new();
        for function in functions {
            let key = key_by_id
                .get(&function.id)
                .cloned()
                .ok_or_else(|| format!("function '{}' has no stable key", function.name))?;
            let dependencies: BTreeSet<FunctionKey> = function
                .dependencies
                .iter()
                .filter_map(|dependency| key_by_id.get(dependency).cloned())
                .collect();
            for dependency in &dependencies {
                reverse
                    .entry(dependency.clone())
                    .or_default()
                    .insert(key.clone());
            }
            by_key.insert(
                key,
                CurrentFunction {
                    id: function.id,
                    body_hash: function.body_hash,
                    dependencies,
                },
            );
        }
        let (scc_index_by_key, sccs) = strongly_connected_components(&by_key, &reachable);
        Ok(Self {
            by_key,
            reachable,
            reverse,
            host_entries,
            scc_index_by_key,
            sccs,
        })
    }
}

fn strongly_connected_components(
    functions: &BTreeMap<FunctionKey, CurrentFunction>,
    reachable: &BTreeSet<FunctionKey>,
) -> (BTreeMap<FunctionKey, usize>, Vec<BTreeSet<FunctionKey>>) {
    let mut visited = BTreeSet::new();
    let mut finish_order = Vec::with_capacity(reachable.len());
    for start in reachable {
        if visited.contains(start) {
            continue;
        }
        let mut stack = vec![(start.clone(), false)];
        while let Some((key, expanded)) = stack.pop() {
            if expanded {
                finish_order.push(key);
                continue;
            }
            if !visited.insert(key.clone()) {
                continue;
            }
            stack.push((key.clone(), true));
            if let Some(function) = functions.get(&key) {
                for dependency in function.dependencies.iter().rev() {
                    if reachable.contains(dependency) && !visited.contains(dependency) {
                        stack.push((dependency.clone(), false));
                    }
                }
            }
        }
    }

    let mut reverse: BTreeMap<FunctionKey, BTreeSet<FunctionKey>> = BTreeMap::new();
    for (caller, function) in functions {
        if !reachable.contains(caller) {
            continue;
        }
        for callee in &function.dependencies {
            if reachable.contains(callee) {
                reverse
                    .entry(callee.clone())
                    .or_default()
                    .insert(caller.clone());
            }
        }
    }
    let mut assigned = BTreeSet::new();
    let mut components = Vec::new();
    for start in finish_order.into_iter().rev() {
        if !assigned.insert(start.clone()) {
            continue;
        }
        let mut component = BTreeSet::from([start.clone()]);
        let mut stack = vec![start];
        while let Some(key) = stack.pop() {
            if let Some(callers) = reverse.get(&key) {
                for caller in callers.iter().rev() {
                    if assigned.insert(caller.clone()) {
                        component.insert(caller.clone());
                        stack.push(caller.clone());
                    }
                }
            }
        }
        components.push(component);
    }
    let mut index_by_key = BTreeMap::new();
    for (index, component) in components.iter().enumerate() {
        for key in component {
            index_by_key.insert(key.clone(), index);
        }
    }
    (index_by_key, components)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::Compiler;
    use std::time::Instant;

    fn indexed(files: &[(&str, &str)]) -> Compiler {
        let mut compiler = Compiler::new();
        for (path, source) in files {
            compiler.upsert_file(*path, *source);
        }
        compiler.index_pass().expect("fixture should index");
        compiler
    }

    fn test_key(name: impl Into<String>, discriminator: u64) -> FunctionKey {
        let name = name.into();
        let path = crate::identity::CanonicalSourcePath::project_relative("scale.stasis")
            .expect("canonical fixture path");
        FunctionKey {
            symbol_id: SymbolId::function(&path, &name, &format!("test-{discriminator:016x}")),
            name,
        }
    }

    fn key_by_name(plan: &[FunctionKey]) -> Vec<String> {
        let mut names: Vec<String> = plan.iter().map(|key| key.name.clone()).collect();
        names.sort();
        names
    }

    fn accepted(source: &str) -> AcceptedProgram {
        let compiler = indexed(&[("main.stasis", source)]);
        capture_accepted_program(compiler.functions(), compiler.files(), &[]).unwrap()
    }

    fn plan(previous: &AcceptedProgram, source: &str) -> PatchPlan {
        let compiler = indexed(&[("main.stasis", source)]);
        plan_patch(
            compiler.functions(),
            compiler.files(),
            &[],
            Some(previous),
            &BTreeSet::new(),
        )
        .unwrap()
    }

    #[test]
    fn cold_plan_emits_every_reachable_function() {
        let compiler = indexed(&[(
            "main.stasis",
            "function leaf(): i32 { return 1; } function unused(): i32 { return 9; } function main(): i32 { return leaf(); }",
        )]);
        let plan = plan_patch(
            compiler.functions(),
            compiler.files(),
            &[],
            None,
            &BTreeSet::new(),
        )
        .unwrap();
        assert!(plan.cold_start);
        assert_eq!(key_by_name(&plan.re_jit), vec!["leaf", "main"]);
    }

    #[test]
    fn leaf_edit_rebuilds_reverse_chain_only() {
        let before = "function c(): i32 { return 1; } function b(): i32 { return c(); } function a(): i32 { return b(); } function unrelated(): i32 { return 8; } function main(): i32 { return a(); }";
        let previous = accepted(before);
        let after = before.replace("return 1", "return 2");
        let plan = plan(&previous, &after);
        assert_eq!(key_by_name(&plan.changed), vec!["c"]);
        assert_eq!(key_by_name(&plan.re_jit), vec!["a", "b", "c", "main"]);
        assert!(!key_by_name(&plan.re_jit).contains(&"unrelated".to_string()));
    }

    #[test]
    fn mid_edit_reuses_unchanged_callees() {
        let before = "function u(): i32 { return 1; } function a(): i32 { return u() + 1; } function main(): i32 { return a(); }";
        let previous = accepted(before);
        let after = before.replace("u() + 1", "u() + 2");
        let plan = plan(&previous, &after);
        assert_eq!(key_by_name(&plan.re_jit), vec!["a", "main"]);
        assert_eq!(key_by_name(&plan.retained_dependencies), vec!["u"]);
    }

    #[test]
    fn shared_edit_rebuilds_diamond_and_root() {
        let before = "function shared(): i32 { return 1; } function left(): i32 { return shared(); } function right(): i32 { return shared(); } function main(): i32 { return left() + right(); }";
        let previous = accepted(before);
        let plan = plan(&previous, &before.replace("return 1", "return 2"));
        assert_eq!(
            key_by_name(&plan.re_jit),
            vec!["left", "main", "right", "shared"]
        );
    }

    #[test]
    fn shared_edit_rebuilds_all_affected_host_roots() {
        let before = "function shared(): i32 { return 1; } function tick(): i32 { return shared(); } function render(): i32 { return shared(); }";
        let previous = accepted(before);
        let plan = plan(&previous, &before.replace("return 1", "return 2"));
        assert_eq!(key_by_name(&plan.re_jit), vec!["render", "shared", "tick"]);
        assert_eq!(
            key_by_name(&plan.affected_host_entries),
            vec!["render", "tick"]
        );
    }

    #[test]
    fn host_entry_called_by_stasis_still_propagates_to_its_caller() {
        let before = "function tick(): i32 { return 1; } function main(): i32 { return tick(); }";
        let previous = accepted(before);
        let plan = plan(&previous, &before.replace("return 1", "return 2"));
        assert_eq!(key_by_name(&plan.re_jit), vec!["main", "tick"]);
    }

    #[test]
    fn mutual_recursion_rebuilds_as_one_scc() {
        let before = "function even(v: i32): i32 { if (v <= 0) { return 1; } return odd(v - 1); } function odd(v: i32): i32 { if (v <= 0) { return 0; } return even(v - 1); } function main(): i32 { return even(4); }";
        let previous = accepted(before);
        let plan = plan(&previous, &before.replace("return 0", "return 2"));
        assert_eq!(key_by_name(&plan.changed), vec!["odd"]);
        assert_eq!(key_by_name(&plan.re_jit), vec!["even", "main", "odd"]);
        assert!(matches!(
            plan.reason_for(plan.re_jit.iter().find(|key| key.name == "even").unwrap())
                .unwrap()
                .reason,
            PatchReason::SccPeer { .. }
        ));
    }

    #[test]
    fn reverse_caller_expands_its_recursion_cycle() {
        let before = "function leaf(): i32 { return 1; } function first(v: i32): i32 { if (v <= 0) { return leaf(); } return second(v - 1); } function second(v: i32): i32 { if (v <= 0) { return 0; } return first(v - 1); } function main(): i32 { return first(2); }";
        let previous = accepted(before);
        let plan = plan(&previous, &before.replace("return 1", "return 2"));
        assert_eq!(
            key_by_name(&plan.re_jit),
            vec!["first", "leaf", "main", "second"]
        );
    }

    #[test]
    fn iterative_scc_planning_handles_five_thousand_functions() {
        let keys: Vec<FunctionKey> = (0..5000)
            .map(|index| test_key(format!("fn_{index}"), index as u64))
            .collect();
        let functions: BTreeMap<FunctionKey, CurrentFunction> = keys
            .iter()
            .enumerate()
            .map(|(index, key)| {
                (
                    key.clone(),
                    CurrentFunction {
                        id: index as FunctionId,
                        body_hash: index as u64,
                        dependencies: BTreeSet::from([keys[(index + 1) % keys.len()].clone()]),
                    },
                )
            })
            .collect();
        let reachable = keys.iter().cloned().collect();
        let (index_by_key, components) = strongly_connected_components(&functions, &reachable);
        assert_eq!(index_by_key.len(), 5000);
        assert_eq!(components.len(), 1);
        assert_eq!(components[0].len(), 5000);
    }

    fn synthetic_graph(
        dependencies: Vec<Vec<usize>>,
        main_dependencies: Vec<usize>,
    ) -> (CurrentGraph, Vec<FunctionKey>) {
        let function_count = dependencies.len();
        let mut keys: Vec<FunctionKey> = (0..function_count)
            .map(|index| test_key(format!("fn_{index}"), index as u64))
            .collect();
        keys.push(test_key("main", u64::MAX));
        let main_index = function_count;
        let mut all_dependencies = dependencies;
        all_dependencies.push(main_dependencies);
        let by_key: BTreeMap<FunctionKey, CurrentFunction> = keys
            .iter()
            .enumerate()
            .map(|(index, key)| {
                (
                    key.clone(),
                    CurrentFunction {
                        id: index as FunctionId,
                        body_hash: index as u64,
                        dependencies: all_dependencies[index]
                            .iter()
                            .map(|dependency| keys[*dependency].clone())
                            .collect(),
                    },
                )
            })
            .collect();
        let mut reachable = BTreeSet::new();
        let mut pending = vec![keys[main_index].clone()];
        while let Some(key) = pending.pop() {
            if !reachable.insert(key.clone()) {
                continue;
            }
            if let Some(function) = by_key.get(&key) {
                pending.extend(function.dependencies.iter().cloned());
            }
        }
        assert_eq!(
            reachable.len(),
            keys.len(),
            "synthetic graph must be root-reachable"
        );
        let mut reverse: BTreeMap<FunctionKey, BTreeSet<FunctionKey>> = BTreeMap::new();
        for (caller, function) in &by_key {
            for callee in &function.dependencies {
                reverse
                    .entry(callee.clone())
                    .or_default()
                    .insert(caller.clone());
            }
        }
        let (scc_index_by_key, sccs) = strongly_connected_components(&by_key, &reachable);
        (
            CurrentGraph {
                by_key,
                reachable,
                reverse,
                host_entries: BTreeSet::from([keys[main_index].clone()]),
                scc_index_by_key,
                sccs,
            },
            keys,
        )
    }

    fn affected_keys(graph: &CurrentGraph, changed: &FunctionKey) -> BTreeSet<String> {
        let mut reasons = BTreeMap::new();
        let mut queue = VecDeque::new();
        insert_seed(
            &mut reasons,
            &mut queue,
            changed.clone(),
            PatchReason::BodyChanged,
        );
        expand_affected_closure(graph, &mut reasons, &mut queue);
        reasons.into_keys().map(|key| key.name).collect()
    }

    fn topology_plan_percentiles(
        graph: &CurrentGraph,
        changed: &FunctionKey,
        measured_samples: usize,
    ) -> (f64, f64) {
        let mut samples = Vec::with_capacity(measured_samples);
        for sample in 0..(5 + measured_samples) {
            let started = Instant::now();
            let _ = affected_keys(graph, changed);
            if sample >= 5 {
                samples.push(started.elapsed());
            }
        }
        samples.sort();
        let percentile = |value: usize| {
            let rank = (value * samples.len()).div_ceil(100).saturating_sub(1);
            samples[rank.min(samples.len() - 1)].as_secs_f64() * 1000.0
        };
        (percentile(50), percentile(95))
    }

    fn names(indices: impl IntoIterator<Item = usize>) -> BTreeSet<String> {
        indices
            .into_iter()
            .map(|index| format!("fn_{index}"))
            .chain(std::iter::once("main".to_string()))
            .collect()
    }

    fn report_topology_plan(
        topology: &str,
        function_count: usize,
        graph: &CurrentGraph,
        changed: &FunctionKey,
    ) {
        let samples = if function_count == 5000 { 10 } else { 30 };
        let (p50, p95) = topology_plan_percentiles(graph, changed, samples);
        eprintln!(
            "topology_closure topology={topology} functions={function_count} samples={samples} closure_ms_p50={p50:.3} closure_ms_p95={p95:.3}"
        );
    }

    #[test]
    fn scaled_topology_matrix_has_exact_selective_closures() {
        for function_count in [100, 1000, 5000] {
            let mut chain = vec![Vec::new(); function_count];
            for (index, dependencies) in chain.iter_mut().enumerate().take(function_count - 1) {
                dependencies.push(index + 1);
            }
            let (graph, keys) = synthetic_graph(chain, vec![0]);
            assert_eq!(
                affected_keys(&graph, &keys[0]),
                names([0]),
                "chain {function_count}"
            );
            report_topology_plan("chain", function_count, &graph, &keys[0]);

            let mut branching = vec![Vec::new(); function_count];
            for (index, dependencies) in branching.iter_mut().enumerate() {
                for child in [index * 2 + 1, index * 2 + 2] {
                    if child < function_count {
                        dependencies.push(child);
                    }
                }
            }
            let mut branch_indices = Vec::new();
            let mut ancestor = function_count - 1;
            loop {
                branch_indices.push(ancestor);
                if ancestor == 0 {
                    break;
                }
                ancestor = (ancestor - 1) / 2;
            }
            let (graph, keys) = synthetic_graph(branching, vec![0]);
            assert_eq!(
                affected_keys(&graph, &keys[function_count - 1]),
                names(branch_indices),
                "branching {function_count}"
            );
            report_topology_plan(
                "branching",
                function_count,
                &graph,
                &keys[function_count - 1],
            );

            let mut diamond = vec![Vec::new(); function_count];
            diamond[0] = vec![1, 2, 4];
            diamond[1] = vec![3];
            diamond[2] = vec![3];
            for (index, dependencies) in diamond
                .iter_mut()
                .enumerate()
                .take(function_count - 1)
                .skip(4)
            {
                dependencies.push(index + 1);
            }
            let (graph, keys) = synthetic_graph(diamond, vec![0]);
            assert_eq!(
                affected_keys(&graph, &keys[3]),
                names([0, 1, 2, 3]),
                "diamond {function_count}"
            );
            report_topology_plan("diamond", function_count, &graph, &keys[3]);

            let mut shared = vec![Vec::new(); function_count];
            for dependencies in shared.iter_mut().take(function_count - 1) {
                dependencies.push(function_count - 1);
            }
            let shared_callers: Vec<usize> = (0..function_count - 1).collect();
            let (graph, keys) = synthetic_graph(shared, shared_callers);
            assert_eq!(
                affected_keys(&graph, &keys[function_count - 1]),
                names(0..function_count),
                "shared {function_count}"
            );
            report_topology_plan("shared", function_count, &graph, &keys[function_count - 1]);

            let mut scc = vec![Vec::new(); function_count];
            for (index, dependencies) in scc.iter_mut().enumerate() {
                dependencies.push((index + 1) % function_count);
            }
            let (graph, keys) = synthetic_graph(scc, vec![0]);
            assert_eq!(
                affected_keys(&graph, &keys[function_count / 2]),
                names(0..function_count),
                "scc {function_count}"
            );
            report_topology_plan("scc", function_count, &graph, &keys[function_count / 2]);
        }
    }

    #[test]
    fn unreachable_edit_emits_nothing_until_reachable() {
        let before = "function hidden(): i32 { return 1; } function main(): i32 { return 0; }";
        let previous = accepted(before);
        let changed = before.replace("return 1", "return 2");
        let unchanged_reachability = plan(&previous, &changed);
        assert!(unchanged_reachability.re_jit.is_empty());

        let reachable = changed.replace("return 0", "return hidden()");
        let reachable_plan = plan(&previous, &reachable);
        assert_eq!(key_by_name(&reachable_plan.re_jit), vec!["hidden", "main"]);
    }

    #[test]
    fn signature_change_and_updated_caller_rebuild_to_root() {
        let before = "function leaf(v: i32): i32 { return v; } function mid(): i32 { return leaf(1); } function main(): i32 { return mid(); }";
        let previous = accepted(before);
        let after = "function leaf(v: i32, extra: i32): i32 { return v + extra; } function mid(): i32 { return leaf(1, 2); } function main(): i32 { return mid(); }";
        let plan = plan(&previous, after);
        assert_eq!(key_by_name(&plan.re_jit), vec!["leaf", "main", "mid"]);
        assert_eq!(plan.removed.len(), 1);
        assert_eq!(plan.removed[0].name, "leaf");
    }

    #[test]
    fn removed_reachable_callee_rebuilds_unchanged_callers() {
        let before = "function leaf(): i32 { return 1; } function main(): i32 { return leaf(); }";
        let previous = accepted(before);
        let plan = plan(&previous, "function main(): i32 { return leaf(); }");

        assert_eq!(key_by_name(&plan.re_jit), vec!["main"]);
        assert_eq!(key_by_name(&plan.removed), vec!["leaf"]);
        let main = plan
            .re_jit
            .iter()
            .find(|key| key.name == "main")
            .expect("main invalidated");
        let reason = plan.reason_for(main).expect("main reason");
        assert!(matches!(
            &reason.reason,
            PatchReason::DirectCaller { callee } if callee.name == "leaf"
        ));
        assert_eq!(
            reason
                .path_from_change
                .iter()
                .map(|key| key.name.as_str())
                .collect::<Vec<_>>(),
            vec!["leaf", "main"]
        );
    }

    #[test]
    fn renamed_function_and_updated_caller_are_planned() {
        let before = "function old(): i32 { return 1; } function main(): i32 { return old(); }";
        let previous = accepted(before);
        let after =
            "function renamed(): i32 { return 1; } function main(): i32 { return renamed(); }";
        let plan = plan(&previous, after);
        assert_eq!(key_by_name(&plan.re_jit), vec!["main", "renamed"]);
        assert_eq!(key_by_name(&plan.removed), vec!["old"]);
    }

    #[test]
    fn multi_file_plan_uses_stable_source_keys() {
        let before = indexed(&[
            ("math.stasis", "function leaf(): i32 { return 1; }"),
            ("main.stasis", "function main(): i32 { return leaf(); }"),
        ]);
        let previous = capture_accepted_program(before.functions(), before.files(), &[]).unwrap();
        let after = indexed(&[
            ("math.stasis", "function leaf(): i32 { return 2; }"),
            ("main.stasis", "function main(): i32 { return leaf(); }"),
        ]);
        let plan = plan_patch(
            after.functions(),
            after.files(),
            &[],
            Some(&previous),
            &BTreeSet::new(),
        )
        .unwrap();
        assert_eq!(key_by_name(&plan.re_jit), vec!["leaf", "main"]);
        assert!(plan
            .re_jit
            .iter()
            .any(|key| key.symbol_id.canonical().contains("|math.stasis|")));
    }

    #[test]
    fn lowered_contract_change_seeds_normal_reverse_invalidation() {
        let source = "function leaf(): i32 { return 1; } function main(): i32 { return leaf(); }";
        let compiler = indexed(&[("main.stasis", source)]);
        let previous =
            capture_accepted_program(compiler.functions(), compiler.files(), &[]).unwrap();
        let leaf = previous
            .functions
            .keys()
            .find(|key| key.name == "leaf")
            .unwrap()
            .clone();
        let plan = plan_patch(
            compiler.functions(),
            compiler.files(),
            &[],
            Some(&previous),
            &BTreeSet::from([leaf]),
        )
        .unwrap();
        assert_eq!(key_by_name(&plan.re_jit), vec!["leaf", "main"]);
    }

    #[test]
    fn reason_chains_are_deterministic() {
        let before = "function leaf(): i32 { return 1; } function mid(): i32 { return leaf(); } function main(): i32 { return mid(); }";
        let previous = accepted(before);
        let after = before.replace("return 1", "return 2");
        let first = plan(&previous, &after);
        let second = plan(&previous, &after);
        assert_eq!(first, second);
        let main = first.re_jit.iter().find(|key| key.name == "main").unwrap();
        assert_eq!(
            first
                .reason_for(main)
                .unwrap()
                .path_from_change
                .iter()
                .map(|key| key.name.as_str())
                .collect::<Vec<_>>(),
            vec!["leaf", "mid", "main"]
        );
    }

    #[test]
    fn invalid_source_fails_before_a_patch_can_be_planned() {
        let mut compiler = Compiler::new();
        compiler.upsert_file("main.stasis", "function main(: i32 { return 0; }");
        assert!(compiler.index_pass().is_err());
    }
}
