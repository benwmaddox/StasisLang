use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::backend::reachability::compute_reachable_function_ids;
use crate::compiler::{FunctionId, FunctionMeta, SourceFile};

const LIFECYCLE_ROOTS: [&str; 4] = ["main", "tick", "render", "on_code_swap"];

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FunctionKey {
    pub source_path: String,
    pub name: String,
    pub signature_hash: u64,
}

impl FunctionKey {
    pub fn display_name(&self) -> String {
        format!(
            "{}::{}#{:016x}",
            self.source_path, self.name, self.signature_hash
        )
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
                Some(previous) if !accepted.reachable.contains(key) => {
                    Some(PatchReason::BecameReachable)
                }
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
    let removed = accepted
        .map(|accepted| {
            accepted
                .functions
                .keys()
                .filter(|key| !graph.by_key.contains_key(*key))
                .cloned()
                .collect()
        })
        .unwrap_or_default();

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
    while let Some(callee) = queue.pop_front() {
        if let Some(component) = graph.scc_by_key.get(&callee) {
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
        if graph.host_entries.contains(&callee) {
            continue;
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
    scc_by_key: BTreeMap<FunctionKey, BTreeSet<FunctionKey>>,
}

impl CurrentGraph {
    fn build(
        functions: &[FunctionMeta],
        files: &[SourceFile],
        required_roots: &[String],
    ) -> Result<Self, String> {
        let mut key_by_id = BTreeMap::new();
        for function in functions {
            let file = files.get(function.file_id as usize).ok_or_else(|| {
                format!(
                    "function '{}' references missing source file",
                    function.name
                )
            })?;
            key_by_id.insert(
                function.id,
                FunctionKey {
                    source_path: file.path.clone(),
                    name: function.name.clone(),
                    signature_hash: function.signature_hash,
                },
            );
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
        let scc_by_key = strongly_connected_components(&by_key, &reachable);
        Ok(Self {
            by_key,
            reachable,
            reverse,
            host_entries,
            scc_by_key,
        })
    }
}

fn strongly_connected_components(
    functions: &BTreeMap<FunctionKey, CurrentFunction>,
    reachable: &BTreeSet<FunctionKey>,
) -> BTreeMap<FunctionKey, BTreeSet<FunctionKey>> {
    struct TarjanState {
        next_index: usize,
        indices: BTreeMap<FunctionKey, usize>,
        lowlinks: BTreeMap<FunctionKey, usize>,
        stack: Vec<FunctionKey>,
        on_stack: BTreeSet<FunctionKey>,
        components: Vec<BTreeSet<FunctionKey>>,
    }

    fn visit(
        key: &FunctionKey,
        functions: &BTreeMap<FunctionKey, CurrentFunction>,
        reachable: &BTreeSet<FunctionKey>,
        state: &mut TarjanState,
    ) {
        let index = state.next_index;
        state.next_index += 1;
        state.indices.insert(key.clone(), index);
        state.lowlinks.insert(key.clone(), index);
        state.stack.push(key.clone());
        state.on_stack.insert(key.clone());

        if let Some(function) = functions.get(key) {
            for dependency in &function.dependencies {
                if !reachable.contains(dependency) {
                    continue;
                }
                if !state.indices.contains_key(dependency) {
                    visit(dependency, functions, reachable, state);
                    let dependency_low = state.lowlinks[dependency];
                    let key_low = state.lowlinks[key];
                    state
                        .lowlinks
                        .insert(key.clone(), key_low.min(dependency_low));
                } else if state.on_stack.contains(dependency) {
                    let dependency_index = state.indices[dependency];
                    let key_low = state.lowlinks[key];
                    state
                        .lowlinks
                        .insert(key.clone(), key_low.min(dependency_index));
                }
            }
        }

        if state.lowlinks[key] == state.indices[key] {
            let mut component = BTreeSet::new();
            while let Some(member) = state.stack.pop() {
                state.on_stack.remove(&member);
                component.insert(member.clone());
                if member == *key {
                    break;
                }
            }
            state.components.push(component);
        }
    }

    let mut state = TarjanState {
        next_index: 0,
        indices: BTreeMap::new(),
        lowlinks: BTreeMap::new(),
        stack: Vec::new(),
        on_stack: BTreeSet::new(),
        components: Vec::new(),
    };
    for key in reachable {
        if !state.indices.contains_key(key) {
            visit(key, functions, reachable, &mut state);
        }
    }
    let mut by_key = BTreeMap::new();
    for component in state.components {
        for key in &component {
            by_key.insert(key.clone(), component.clone());
        }
    }
    by_key
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::Compiler;

    fn indexed(files: &[(&str, &str)]) -> Compiler {
        let mut compiler = Compiler::new();
        for (path, source) in files {
            compiler.upsert_file(*path, *source);
        }
        compiler.index_pass().expect("fixture should index");
        compiler
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
            .any(|key| key.source_path == "math.stasis"));
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
