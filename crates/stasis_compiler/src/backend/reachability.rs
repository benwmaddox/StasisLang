use crate::backend::ReachabilityPolicy;
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
    function.name == root_name
        && match root_name {
            "tick" => function.params.is_empty(),
            "on_code_swap" => {
                function.params.is_empty()
                    && function.return_type == crate::frontend::types::TYPE_ID_VOID
            }
            _ => true,
        }
}

pub(crate) fn compute_reachable_function_ids(
    functions: &[FunctionMeta],
    required_emit_roots: &[String],
) -> BTreeSet<FunctionId> {
    compute_reachable_function_ids_with_policy(
        functions,
        required_emit_roots,
        ReachabilityPolicy::Development,
    )
}

pub(crate) fn compute_reachable_function_ids_with_policy(
    functions: &[FunctionMeta],
    required_emit_roots: &[String],
    policy: ReachabilityPolicy,
) -> BTreeSet<FunctionId> {
    let mut roots: Vec<FunctionId> = Vec::new();
    roots.extend(
        functions
            .iter()
            .filter(|function| function.host_export.is_some())
            .map(|function| function.id),
    );
    for root_name in DEFAULT_ROOTS {
        if policy == ReachabilityPolicy::Release && root_name == "on_code_swap" {
            continue;
        }
        roots.extend(
            functions
                .iter()
                .filter(|function| {
                    function.requires_contract.is_none() && matches_root(function, root_name)
                })
                .map(|function| function.id),
        );
    }
    for root_name in required_emit_roots {
        if policy == ReachabilityPolicy::Release && root_name == "on_code_swap" {
            continue;
        }
        roots.extend(
            functions
                .iter()
                .filter(|function| {
                    function.requires_contract.is_none() && matches_root(function, root_name)
                })
                .map(|function| function.id),
        );
    }
    if roots.is_empty() && policy == ReachabilityPolicy::Development {
        return functions
            .iter()
            .filter(|function| function.requires_contract.is_none())
            .map(|function| function.id)
            .collect();
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
            if functions
                .iter()
                .find(|candidate| candidate.id == *dependency)
                .is_none_or(|candidate| candidate.requires_contract.is_none())
            {
                stack.push(*dependency);
            }
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

#[cfg(test)]
mod release_tests {
    use crate::backend::{aot::AotProcess, wasm::WasmProcess, ReachabilityPolicy};
    const SOURCE: &str =
        include_str!("../../../../tests/stasis/seams/release_swap_roots.stasis.fixture");

    #[test]
    fn release_aot_prunes_reload_closure_and_separates_cached_policy() {
        let mut process = AotProcess::new();
        process.upsert_file("swap.stasis", SOURCE);
        process.compile().expect("development compile");
        let development_revision = process.program_snapshot().unwrap().source_revision();
        for policy in [ReachabilityPolicy::Release, ReachabilityPolicy::Development] {
            process.set_reachability_policy(policy);
            process.compile().expect("compile changed policy");
            let snapshot = process.program_snapshot().unwrap();
            assert_eq!(snapshot.reachability_policy(), policy);
            let development = policy == ReachabilityPolicy::Development;
            assert_eq!(
                snapshot.source_revision() == development_revision,
                development
            );
            for function in snapshot.functions() {
                let retained = process
                    .artifacts()
                    .iter()
                    .any(|artifact| artifact.function_id == function.id);
                if ["on_code_swap", "reload_only"].contains(&function.name.as_str()) {
                    assert_eq!(retained, development, "{}", function.name);
                } else {
                    assert!(retained, "shared/startup function {} lost", function.name);
                }
            }
            let paths = snapshot
                .asset_references()
                .iter()
                .filter_map(|asset| asset.logical_path.as_deref())
                .collect::<Vec<_>>();
            assert!(paths.contains(&"assets/shared.svg"));
            assert_eq!(paths.contains(&"assets/reload.svg"), development);
            assert_eq!(
                process
                    .string_literals()
                    .values()
                    .any(|value| value == "assets/reload.svg"),
                development
            );
        }
    }

    #[test]
    fn release_preserves_explicit_same_name_calls_by_function_identity() {
        let source = "function on_code_swap(value: i32): i32 { return value + 1; } function on_code_swap(): void { return; } function main(): i32 { return on_code_swap(6); }";
        let mut process = AotProcess::new();
        process.set_reachability_policy(ReachabilityPolicy::Release);
        process.upsert_file("explicit.stasis", source);
        process.compile().expect("release ordinary overload call");
        let snapshot = process.program_snapshot().unwrap();
        let reachable = snapshot.reachable_function_ids();
        for function in snapshot
            .functions()
            .iter()
            .filter(|function| function.name == "on_code_swap")
        {
            assert_eq!(
                reachable.contains(&function.id),
                !function.params.is_empty()
            );
        }
        let mut wasm = WasmProcess::new();
        wasm.set_reachability_policy(ReachabilityPolicy::Release);
        wasm.upsert_file("explicit.stasis", source);
        wasm.compile().expect("Wasm ordinary overload call");
        assert!(!wasm
            .module_bytes()
            .windows(b"on_code_swap".len())
            .any(|bytes| bytes == b"on_code_swap"));
    }

    #[test]
    fn release_without_host_entry_does_not_retain_reload_only_program() {
        let mut process = AotProcess::new();
        process.set_reachability_policy(ReachabilityPolicy::Release);
        process.upsert_file(
            "only_swap.stasis",
            "function on_code_swap(): void { return; }",
        );
        process.compile().expect("compile no release root");
        assert!(process.artifacts().is_empty());
    }
}
