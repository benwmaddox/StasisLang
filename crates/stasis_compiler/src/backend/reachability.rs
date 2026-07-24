use crate::compiler::{FunctionId, FunctionMeta};
use std::collections::BTreeSet;

const DEFAULT_ROOTS: [&str; 7] = [
    "tick",
    "commit_tick",
    "normalize_tick",
    "validate_tick",
    "main",
    "render",
    "on_code_swap",
];

pub(crate) fn compute_reachable_function_ids(
    functions: &[FunctionMeta],
    required_emit_roots: &[String],
) -> BTreeSet<FunctionId> {
    let mut roots: Vec<FunctionId> = Vec::new();
    for root_name in DEFAULT_ROOTS {
        roots.extend(
            functions
                .iter()
                .filter(|function| function.name == root_name)
                .map(|function| function.id),
        );
    }
    for root_name in required_emit_roots {
        roots.extend(
            functions
                .iter()
                .filter(|function| function.name == *root_name)
                .map(|function| function.id),
        );
    }
    if roots.is_empty() {
        return functions.iter().map(|function| function.id).collect();
    }

    let mut reachable: BTreeSet<FunctionId> = BTreeSet::new();
    let mut stack = roots;
    while let Some(function_id) = stack.pop() {
        if !reachable.insert(function_id) {
            continue;
        }
        let Some(function) = functions.get(function_id as usize) else {
            continue;
        };
        for dependency in &function.dependencies {
            stack.push(*dependency);
        }
    }

    // If any overload of a name is reachable, treat the entire name-family as reachable so call
    // resolution can remain deterministic.
    let reachable_names: BTreeSet<String> = functions
        .iter()
        .filter(|function| reachable.contains(&function.id))
        .map(|function| function.name.clone())
        .collect();
    for function in functions {
        if reachable_names.contains(&function.name) {
            reachable.insert(function.id);
        }
    }

    reachable
}
