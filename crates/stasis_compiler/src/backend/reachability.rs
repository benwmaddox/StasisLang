use crate::compiler::{FunctionId, FunctionMeta};
use std::collections::BTreeSet;

const DEFAULT_ROOTS: [&str; 6] = [
    "tick",
    "main",
    "render",
    "on_code_swap",
    "gfx_cmd_construction_reset",
    "gfx_cmd_construction_finish",
];

pub(crate) fn matches_root(function: &FunctionMeta, root_name: &str) -> bool {
    function.name == root_name && (root_name != "tick" || function.params.is_empty())
}

pub(crate) fn compute_reachable_function_ids(
    functions: &[FunctionMeta],
    required_emit_roots: &[String],
) -> BTreeSet<FunctionId> {
    let mut roots: Vec<FunctionId> = Vec::new();
    for root_name in DEFAULT_ROOTS {
        roots.extend(
            functions
                .iter()
                .filter(|function| matches_root(function, root_name))
                .map(|function| function.id),
        );
    }
    for root_name in required_emit_roots {
        roots.extend(
            functions
                .iter()
                .filter(|function| matches_root(function, root_name))
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
        let Some(function) = functions.iter().find(|function| function.id == function_id) else {
            continue;
        };
        for dependency in &function.dependencies {
            stack.push(*dependency);
        }
    }

    reachable
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::Compiler;

    #[test]
    fn only_zero_argument_tick_is_a_default_root() {
        let mut compiler = Compiler::new();
        compiler.upsert_file(
            "tick_overloads.stasis",
            "function tick(value: i32): i32 { return value; }\n\
             function tick(): i32 { return 0; }\n\
             function main(): i32 { return 0; }\n",
        );
        compiler
            .check()
            .expect("overloaded tick fixture should compile");

        let zero_argument_tick = compiler
            .functions()
            .iter()
            .find(|function| function.name == "tick" && function.params.is_empty())
            .expect("zero-argument tick");
        let parameterized_tick = compiler
            .functions()
            .iter()
            .find(|function| function.name == "tick" && !function.params.is_empty())
            .expect("parameterized tick");
        let reachable = compute_reachable_function_ids(compiler.functions(), &[]);

        assert!(reachable.contains(&zero_argument_tick.id));
        assert!(!reachable.contains(&parameterized_tick.id));
    }
}
