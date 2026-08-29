use std::collections::{BTreeMap, BTreeSet};

use crate::compiler::{FunctionMeta, SourceFile};
use crate::frontend::parser::parse_top_level_type_layout;

use super::{
    is_host_capability, root_name, split_array_type, substitute_path, EffectContractViolation,
    FunctionDataFlowSummary,
};

pub(super) fn validate_effect_contracts(
    files: &[SourceFile],
    functions: &[FunctionMeta],
    summaries: &[FunctionDataFlowSummary],
) -> Result<(), EffectContractViolation> {
    let mut capability_globals = BTreeMap::<&str, BTreeSet<String>>::new();
    let mut structs = BTreeMap::new();
    let mut layouts = Vec::new();
    for file in files {
        let Ok(layout) = parse_top_level_type_layout(&file.content) else {
            continue;
        };
        structs.extend(
            layout
                .structs
                .iter()
                .map(|structure| (structure.name.clone(), structure.fields.clone())),
        );
        layouts.push((file, layout));
    }
    let mut declared_regions = BTreeSet::new();
    for (file, layout) in layouts {
        let normalized_path = file.path.replace('\\', "/");
        let owned_capability = if normalized_path.ends_with("stdlib/internal/gfx_cmd.stasis") {
            Some("graphics")
        } else if normalized_path.ends_with("stdlib/internal/host_window_request.stasis") {
            Some("platform")
        } else {
            None
        };
        if let Some(capability) = owned_capability {
            capability_globals
                .entry(capability)
                .or_default()
                .extend(layout.globals.iter().map(|global| global.name.clone()));
        }
        for global in layout.globals {
            collect_declared_regions(
                &global.name,
                &global.type_name,
                &structs,
                &mut declared_regions,
                &mut BTreeSet::new(),
            );
        }
        for block in layout.global_blocks {
            declared_regions.insert(block.name.clone());
            for field in block.fields {
                collect_declared_regions(
                    &format!("{}.{}", block.name, field.name),
                    &field.type_name,
                    &structs,
                    &mut declared_regions,
                    &mut BTreeSet::new(),
                );
            }
        }
    }
    let summary_by_id = summaries
        .iter()
        .map(|summary| (summary.internal_function_id, summary))
        .collect::<BTreeMap<_, _>>();
    for boundary in functions {
        let Some(contract) = boundary.effect_contract.as_deref() else {
            continue;
        };
        let Some(summary) = summary_by_id.get(&boundary.storage_index).copied() else {
            continue;
        };
        for region in contract {
            if !is_host_capability(region) && !declared_regions.contains(region) {
                return Err(contract_violation(
                    files,
                    boundary,
                    format!(
                        "effect contract on '{}' names unknown global region '{}'",
                        boundary.name, region
                    ),
                ));
            }
        }
        let allowed_regions = contract
            .iter()
            .filter(|region| !is_host_capability(region))
            .map(String::as_str)
            .collect::<Vec<_>>();
        if let Some(path) =
            summary.aggregate.writes.iter().find(|path| {
                !write_is_allowed(path, &allowed_regions, contract, &capability_globals)
            })
        {
            let chain = write_call_chain(
                boundary.storage_index,
                path,
                &summary_by_id,
                &mut BTreeSet::new(),
            );
            return Err(contract_violation(
                files,
                boundary,
                format!(
                    "effect contract on '{}' rejects write '{}'; originating operation is reachable through {}",
                    boundary.name,
                    path,
                    display_call_chain(&chain, &summary_by_id)
                ),
            ));
        }
        if let Some(path) = summary.aggregate.parameter_writes.first() {
            let chain = effect_call_chain(
                boundary.storage_index,
                &summary_by_id,
                &mut BTreeSet::new(),
                &|candidate| candidate.aggregate.parameter_writes.contains(path),
                &|candidate| !candidate.direct.parameter_writes.is_empty(),
            );
            return Err(contract_violation(
                files,
                boundary,
                format!(
                    "effect contract on '{}' rejects write through parameter '{}': its global region is not proven; originating operation is reachable through {}",
                    boundary.name,
                    path,
                    display_call_chain(&chain, &summary_by_id)
                ),
            ));
        }
        if let Some(effect) =
            summary.aggregate.host_effects.iter().find(|effect| {
                effect.capability == "unknown" || !contract.contains(&effect.capability)
            })
        {
            let chain = effect_call_chain(
                boundary.storage_index,
                &summary_by_id,
                &mut BTreeSet::new(),
                &|candidate| candidate.aggregate.host_effects.contains(effect),
                &|candidate| candidate.direct.host_effects.contains(effect),
            );
            return Err(contract_violation(
                files,
                boundary,
                format!(
                    "effect contract on '{}' rejects host effect '{}' from '{}'; originating operation is reachable through {}",
                    boundary.name,
                    effect.capability,
                    effect.function,
                    display_call_chain(&chain, &summary_by_id)
                ),
            ));
        }
    }
    Ok(())
}

fn collect_declared_regions(
    path: &str,
    type_name: &str,
    structs: &BTreeMap<String, Vec<crate::frontend::parser::ParsedField>>,
    regions: &mut BTreeSet<String>,
    visiting: &mut BTreeSet<String>,
) {
    regions.insert(path.to_string());
    let element_type = split_array_type(type_name)
        .map(|(element, _)| element)
        .unwrap_or(type_name);
    if !visiting.insert(element_type.to_string()) {
        return;
    }
    if let Some(fields) = structs.get(element_type) {
        for field in fields {
            collect_declared_regions(
                &format!("{path}.{}", field.name),
                &field.type_name,
                structs,
                regions,
                visiting,
            );
        }
    }
    visiting.remove(element_type);
}

fn region_contains(region: &str, path: &str) -> bool {
    path == region
        || path
            .strip_prefix(region)
            .is_some_and(|suffix| suffix.starts_with('.') || suffix.starts_with('['))
}

fn write_is_allowed(
    path: &str,
    regions: &[&str],
    contract: &[String],
    capability_globals: &BTreeMap<&str, BTreeSet<String>>,
) -> bool {
    regions.iter().any(|region| region_contains(region, path))
        || contract.iter().any(|capability| {
            capability_globals
                .get(capability.as_str())
                .is_some_and(|globals| globals.contains(root_name(path)))
        })
}

fn contract_violation(
    files: &[SourceFile],
    boundary: &FunctionMeta,
    message: String,
) -> EffectContractViolation {
    EffectContractViolation {
        file: files[boundary.file_id as usize].path.clone(),
        source_start: boundary.signature_range.start,
        source_end: boundary.signature_range.end,
        function: boundary.name.clone(),
        message,
    }
}

fn effect_call_chain(
    current: u32,
    summaries: &BTreeMap<u32, &FunctionDataFlowSummary>,
    visiting: &mut BTreeSet<u32>,
    contains: &impl Fn(&FunctionDataFlowSummary) -> bool,
    originates: &impl Fn(&FunctionDataFlowSummary) -> bool,
) -> Vec<u32> {
    if !visiting.insert(current) {
        return vec![current];
    }
    let Some(summary) = summaries.get(&current).copied() else {
        return vec![current];
    };
    if originates(summary) {
        return vec![current];
    }
    for call_site in &summary.internal_call_sites {
        let child = call_site.target_id;
        let Some(child_summary) = summaries.get(&child).copied() else {
            continue;
        };
        if !contains(child_summary) && !originates(child_summary) {
            continue;
        }
        let mut branch = visiting.clone();
        let chain = effect_call_chain(child, summaries, &mut branch, contains, originates);
        if chain
            .last()
            .and_then(|id| summaries.get(id))
            .is_some_and(|leaf| originates(leaf))
        {
            let mut out = vec![current];
            out.extend(chain);
            return out;
        }
    }
    vec![current]
}

fn write_call_chain(
    current: u32,
    path: &str,
    summaries: &BTreeMap<u32, &FunctionDataFlowSummary>,
    visiting: &mut BTreeSet<(u32, String)>,
) -> Vec<u32> {
    if !visiting.insert((current, path.to_string())) {
        return vec![current];
    }
    let Some(summary) = summaries.get(&current).copied() else {
        return vec![current];
    };
    if summary
        .internal_direct_write_paths
        .iter()
        .any(|candidate| candidate == path)
    {
        return vec![current];
    }
    for call_site in &summary.internal_call_sites {
        let Some(child) = summaries.get(&call_site.target_id).copied() else {
            continue;
        };
        let substitutions = call_site
            .arguments
            .iter()
            .enumerate()
            .filter_map(|(index, argument)| argument.clone().map(|argument| (index, argument)))
            .collect::<BTreeMap<_, _>>();
        let child_path = child
            .internal_aggregate_write_paths
            .iter()
            .find(|candidate| {
                substitute_path(candidate, &substitutions, false).as_deref() == Some(path)
            });
        let Some(child_path) = child_path else {
            continue;
        };
        let mut branch = visiting.clone();
        let chain = write_call_chain(call_site.target_id, child_path, summaries, &mut branch);
        let reached_origin = chain.len() > 1
            || child
                .internal_direct_write_paths
                .iter()
                .any(|candidate| candidate == child_path);
        if reached_origin {
            let mut out = vec![current];
            out.extend(chain);
            return out;
        }
    }
    vec![current]
}

fn display_call_chain(
    chain: &[u32],
    summaries: &BTreeMap<u32, &FunctionDataFlowSummary>,
) -> String {
    chain
        .iter()
        .filter_map(|id| summaries.get(id).map(|summary| summary.function.as_str()))
        .collect::<Vec<_>>()
        .join(" -> ")
}
