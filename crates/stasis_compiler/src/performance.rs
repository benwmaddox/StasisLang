use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::backend::state_layout::{StateLayout, StateMemoryEntry, StateMemoryReport};
use crate::compiler::SourceFile;
use crate::data_flow::{FunctionBoundedIteration, FunctionDataFlowSummary, FunctionHostCallCost};
use crate::frontend::parser::{parse_top_level_functions, ParsedFunctionAnnotationArgumentKind};

pub const PERFORMANCE_COST_SCHEMA_VERSION: u32 = 1;
const MOBILE_RUNTIME_SHELL_ESTIMATE_BYTES: u64 = 512 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PerformanceCostReport {
    pub schema_version: u32,
    pub tick_budget_us: Option<u64>,
    pub functions: Vec<FunctionCostReport>,
    pub layout_choices: Vec<CollectionLayoutChoice>,
    pub mobile: MobileCostEstimate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionCostReport {
    pub function: String,
    pub structural_bound_complete: bool,
    pub worst_nested_iteration_product: Option<u64>,
    pub fields_scanned: Vec<FieldScanCost>,
    pub conservative_max_bytes_scanned: u64,
    pub pools_iterated: Vec<PoolIterationCost>,
    pub host_calls: Vec<FunctionHostCallCost>,
    pub bounded_iterations: Vec<FunctionBoundedIteration>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldScanCost {
    pub path: String,
    pub element_bytes: u64,
    pub conservative_max_visits: u64,
    pub conservative_max_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PoolIterationCost {
    pub path: String,
    pub capacity: u64,
    pub bytes_per_element: u64,
    pub max_iteration_product: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollectionLayoutChoice {
    pub path: String,
    pub active_layout: String,
    pub active_field_groups: Vec<Vec<String>>,
    pub aos_field_group: Vec<String>,
    pub soa_bytes: u64,
    pub aos_stride_bytes: u64,
    pub aos_padding_bytes_per_element: u64,
    pub aos_bytes: u64,
    pub recommendation: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobileCostEstimate {
    pub aot_object_code_bytes: u64,
    pub literal_data_bytes: u64,
    pub state_capacity_bytes: u64,
    pub command_buffer_bytes: u64,
    pub package_estimate_bytes: u64,
    pub peak_state_recommendation_bytes: u64,
    pub package_estimate_basis: String,
}

pub fn tick_budget_us(files: &[SourceFile]) -> Result<Option<u64>, String> {
    let mut budget = None;
    for file in files {
        for function in parse_top_level_functions(&file.content)? {
            let annotations = function
                .annotations
                .iter()
                .filter(|annotation| annotation.name == "tick_budget_us")
                .collect::<Vec<_>>();
            if annotations.is_empty() {
                continue;
            }
            if annotations.len() != 1 {
                return Err("duplicate @tick_budget_us annotation".to_string());
            }
            if function.name != "tick" {
                return Err(format!(
                    "@tick_budget_us may only annotate tick(), not '{}'",
                    function.name
                ));
            }
            let annotation = annotations[0];
            if !annotation.has_parentheses
                || annotation.arguments.len() != 1
                || annotation.arguments[0].kind != ParsedFunctionAnnotationArgumentKind::Integer
            {
                return Err(
                    "@tick_budget_us expects exactly one unsigned integer argument".to_string(),
                );
            }
            let value = annotation.arguments[0]
                .text
                .parse::<u64>()
                .map_err(|error| format!("invalid @tick_budget_us value: {error}"))?;
            if value == 0 {
                return Err("@tick_budget_us must be greater than zero".to_string());
            }
            if budget.replace(value).is_some() {
                return Err("multiple @tick_budget_us annotations were found".to_string());
            }
        }
    }
    Ok(budget)
}

pub fn build_performance_cost_report(
    files: &[SourceFile],
    summaries: &[FunctionDataFlowSummary],
    layout: &StateLayout,
    memory: &StateMemoryReport,
    aot_object_code_bytes: u64,
    literal_data_bytes: u64,
) -> Result<PerformanceCostReport, String> {
    let tick_budget_us = tick_budget_us(files)?;
    let functions = summaries
        .iter()
        .map(|summary| function_cost(summary, layout, memory))
        .collect::<Result<Vec<_>, _>>()?;
    let layout_choices = collection_layout_choices(layout, memory)?;
    let command_buffer_bytes = memory
        .command_buffers
        .iter()
        .try_fold(0u64, |total, buffer| {
            total.checked_add(buffer.capacity_bytes)
        })
        .ok_or_else(|| "command-buffer byte estimate overflow".to_string())?;
    let package_estimate_bytes = aot_object_code_bytes
        .checked_add(literal_data_bytes)
        .and_then(|total| total.checked_add(memory.projected_capacity_bytes))
        .and_then(|total| total.checked_add(MOBILE_RUNTIME_SHELL_ESTIMATE_BYTES))
        .ok_or_else(|| "mobile package estimate overflow".to_string())?;
    let peak_state_recommendation_bytes = memory
        .projected_capacity_bytes
        .checked_add(memory.projected_capacity_bytes / 4)
        .and_then(|value| value.checked_next_power_of_two())
        .ok_or_else(|| "peak state recommendation overflow".to_string())?;
    Ok(PerformanceCostReport {
        schema_version: PERFORMANCE_COST_SCHEMA_VERSION,
        tick_budget_us,
        functions,
        layout_choices,
        mobile: MobileCostEstimate {
            aot_object_code_bytes,
            literal_data_bytes,
            state_capacity_bytes: memory.projected_capacity_bytes,
            command_buffer_bytes,
            package_estimate_bytes,
            peak_state_recommendation_bytes,
            package_estimate_basis: "arm64 AOT object bytes + literal bytes + projected state + 512 KiB SDL runtime-shell allowance".to_string(),
        },
    })
}

fn function_cost(
    summary: &FunctionDataFlowSummary,
    layout: &StateLayout,
    memory: &StateMemoryReport,
) -> Result<FunctionCostReport, String> {
    let iterations = &summary.direct.bounded_iterations;
    let worst_nested_iteration_product = iterations
        .iter()
        .filter_map(|iteration| iteration.max_iteration_product)
        .max();
    let mut field_visits = BTreeMap::<String, (u64, u64)>::new();
    for iteration in iterations {
        let Some(visits) = iteration.max_iteration_product else {
            continue;
        };
        for path in &iteration.scanned_paths {
            if let Some(entry) = memory_entry_for_scan(memory, path) {
                let item = field_visits
                    .entry(path.clone())
                    .or_insert((entry.element_bytes, 0));
                item.1 = item
                    .1
                    .checked_add(visits)
                    .ok_or_else(|| format!("field scan visit overflow for '{path}'"))?;
            }
        }
    }
    let fields_scanned = field_visits
        .into_iter()
        .map(|(path, (element_bytes, conservative_max_visits))| {
            let conservative_max_bytes = element_bytes
                .checked_mul(conservative_max_visits)
                .ok_or_else(|| format!("field scan byte overflow for '{path}'"))?;
            Ok(FieldScanCost {
                path,
                element_bytes,
                conservative_max_visits,
                conservative_max_bytes,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let conservative_max_bytes_scanned = fields_scanned
        .iter()
        .try_fold(0u64, |total, field| {
            total.checked_add(field.conservative_max_bytes)
        })
        .ok_or_else(|| format!("function '{}' field scan byte overflow", summary.function))?;
    let mut pools_iterated = Vec::new();
    for iteration in iterations {
        if iteration.kind != "foreach" {
            continue;
        }
        let Some(collection) = layout
            .collections
            .iter()
            .find(|collection| collection.path == iteration.bound)
        else {
            continue;
        };
        let capacity = u64::try_from(collection.capacity)
            .map_err(|_| format!("collection '{}' has negative capacity", collection.path))?;
        let bytes_per_element = memory
            .entries
            .iter()
            .filter(|entry| entry.path == collection.path && entry.kind == "collection_field")
            .try_fold(0u64, |total, entry| total.checked_add(entry.element_bytes))
            .ok_or_else(|| format!("pool byte cost overflow for '{}'", collection.path))?;
        pools_iterated.push(PoolIterationCost {
            path: iteration.bound.clone(),
            capacity,
            bytes_per_element,
            max_iteration_product: iteration.max_iteration_product,
        });
    }
    let direct_host_calls = summary
        .direct
        .host_call_costs
        .iter()
        .map(|cost| cost.function.as_str())
        .collect::<BTreeSet<_>>();
    let mut host_calls = summary.aggregate.host_call_costs.clone();
    for cost in &mut host_calls {
        cost.scope = if direct_host_calls.contains(cost.function.as_str()) {
            "direct_and_transitive".to_string()
        } else {
            "transitive".to_string()
        };
    }
    host_calls.sort_by(|left, right| left.function.cmp(&right.function));
    Ok(FunctionCostReport {
        function: summary.function.clone(),
        structural_bound_complete: iterations
            .iter()
            .all(|iteration| iteration.max_iteration_product.is_some()),
        worst_nested_iteration_product,
        fields_scanned,
        conservative_max_bytes_scanned,
        pools_iterated,
        host_calls,
        bounded_iterations: iterations.clone(),
    })
}

fn memory_entry_for_scan<'a>(
    memory: &'a StateMemoryReport,
    scan_path: &str,
) -> Option<&'a StateMemoryEntry> {
    memory.entries.iter().find(|entry| {
        let path = if entry.kind == "collection_field" && entry.field.is_empty() {
            format!("{}[*]", entry.path)
        } else if entry.field.is_empty() {
            entry.path.clone()
        } else {
            format!("{}[*].{}", entry.path, entry.field)
        };
        path == scan_path
    })
}

fn collection_layout_choices(
    layout: &StateLayout,
    memory: &StateMemoryReport,
) -> Result<Vec<CollectionLayoutChoice>, String> {
    layout
        .collections
        .iter()
        .map(|collection| {
            let capacity = u64::try_from(collection.capacity)
                .map_err(|_| format!("collection '{}' has negative capacity", collection.path))?;
            let capacity = memory
                .capacity_changes
                .iter()
                .find(|change| change.path == collection.path)
                .map_or(capacity, |change| change.new_capacity);
            let entries = collection
                .fields
                .iter()
                .filter_map(|field| {
                    memory.entries.iter().find(|entry| {
                        entry.path == collection.path && entry.field == field.field
                    })
                })
                .collect::<Vec<_>>();
            let soa_stride = entries.iter().map(|entry| entry.element_bytes).sum::<u64>();
            let mut offset = 0u64;
            let mut max_alignment = 1u64;
            for entry in &entries {
                let alignment = entry.alignment_bytes.max(1);
                max_alignment = max_alignment.max(alignment);
                offset = align_up(offset, alignment)?;
                offset = offset
                    .checked_add(entry.element_bytes)
                    .ok_or_else(|| "AoS stride overflow".to_string())?;
            }
            let aos_stride = align_up(offset, max_alignment)?;
            let aos_padding = aos_stride.saturating_sub(soa_stride);
            let soa_bytes = soa_stride
                .checked_mul(capacity)
                .ok_or_else(|| "SoA byte estimate overflow".to_string())?;
            let aos_bytes = aos_stride
                .checked_mul(capacity)
                .ok_or_else(|| "AoS byte estimate overflow".to_string())?;
            let (recommendation, reason) = if aos_padding == 0 && entries.len() <= 2 {
                (
                    "aos_candidate",
                    "all fields fit without padding; profile whole-record access before changing the active SoA lowering",
                )
            } else {
                (
                    "soa",
                    "active SoA avoids AoS padding and supports field-selective bounded scans",
                )
            };
            Ok(CollectionLayoutChoice {
                path: collection.path.clone(),
                active_layout: "soa".to_string(),
                active_field_groups: collection
                    .fields
                    .iter()
                    .map(|field| vec![field.field.clone()])
                    .collect(),
                aos_field_group: collection
                    .fields
                    .iter()
                    .map(|field| field.field.clone())
                    .collect(),
                soa_bytes,
                aos_stride_bytes: aos_stride,
                aos_padding_bytes_per_element: aos_padding,
                aos_bytes,
                recommendation: recommendation.to_string(),
                reason: reason.to_string(),
            })
        })
        .collect()
}

fn align_up(value: u64, alignment: u64) -> Result<u64, String> {
    let mask = alignment.saturating_sub(1);
    value
        .checked_add(mask)
        .map(|value| value & !mask)
        .ok_or_else(|| "layout alignment overflow".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::jit::JitProcess;

    #[test]
    fn tick_budget_annotation_is_explicit_and_tick_only() {
        let files = vec![SourceFile {
            path: "game.stasis".to_string(),
            content: "function @tick_budget_us(250) tick(): i32 { return 0; }".to_string(),
            hash: 0,
            functions: Vec::new(),
        }];
        assert_eq!(tick_budget_us(&files), Ok(Some(250)));

        let invalid = vec![SourceFile {
            path: "game.stasis".to_string(),
            content: "function @tick_budget_us(250) helper(): i32 { return 0; }".to_string(),
            hash: 0,
            functions: Vec::new(),
        }];
        assert!(tick_budget_us(&invalid)
            .expect_err("non-tick annotation must fail")
            .contains("only annotate tick"));

        let mut jit = JitProcess::new();
        jit.upsert_file("game.stasis", invalid[0].content.clone());
        assert!(format!(
            "{:?}",
            jit.compile().expect_err("compiler must validate budget")
        )
        .contains("only annotate tick"));

        let parse = |source: &str| {
            tick_budget_us(&[SourceFile {
                path: "budget.stasis".to_string(),
                content: source.to_string(),
                hash: 0,
                functions: Vec::new(),
            }])
        };
        assert_eq!(
            parse("// @tick_budget_us(9)\nfunction tick(): i32 { return 0; }"),
            Ok(None)
        );
        assert_eq!(
            parse("function @tick_budget_us ( 300 ) tick(): i32 { return 0; }"),
            Ok(Some(300))
        );
        for source in [
            "function @tick_budget_us tick(): i32 { return 0; }",
            "function @tick_budget_us(1) @tick_budget_us(2) tick(): i32 { return 0; }",
            "function @tick_budget_us(0) tick(): i32 { return 0; }",
            "function @tick_budget_us(18446744073709551616) tick(): i32 { return 0; }",
            "function @tick_budget_us(1,) tick(): i32 { return 0; }",
            "function @tick_budget_us(,1) tick(): i32 { return 0; }",
            "function @tick_budget_us(1,,2) tick(): i32 { return 0; }",
        ] {
            assert!(parse(source).is_err(), "must reject {source}");
        }
    }

    #[test]
    fn report_counts_repeated_pool_scans_and_transitive_host_calls() {
        let source = r#"
global values: i32[4];
extern function print_i32(value: i32): void;

function helper(value: i32): void {
    print_i32(value);
}

function tick(): i32 {
    let total: i32 = 0;
    foreach (let value in values) {
        total += value;
        total += value;
        helper(value);
    }
    foreach (let value in values) {
        total += value;
        total += value;
        helper(value);
    }
    return total;
}
"#;
        let mut jit = JitProcess::new();
        jit.set_required_emit_roots(&["tick".to_string()]);
        jit.upsert_file("costs.stasis", source);
        jit.compile().expect("compile bounded repeated scans");
        let memory = jit
            .state_memory_report(&BTreeMap::new(), 1024 * 1024)
            .expect("state memory report");
        let report = jit
            .performance_cost_report(&memory, 0, 0)
            .expect("performance report");
        let tick = report
            .functions
            .iter()
            .find(|function| function.function == "tick")
            .expect("tick costs");
        assert_eq!(tick.pools_iterated.len(), 2);
        assert_eq!(tick.fields_scanned[0].path, "values[*]");
        assert_eq!(tick.fields_scanned[0].conservative_max_visits, 24);
        assert_eq!(
            tick.host_calls
                .iter()
                .find(|cost| cost.function == "print_i32")
                .expect("transitive host call")
                .max_invocations,
            Some(8)
        );
    }
}
