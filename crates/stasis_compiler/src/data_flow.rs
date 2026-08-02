use std::collections::{BTreeMap, BTreeSet};
use std::hash::{DefaultHasher, Hash, Hasher};

use crate::backend::emit::{
    collect_supported_call_signatures, eval_const_i64, resolve_call_signature, AssignOp,
    AssignTarget, CallSignatureMap, ComparisonOp, ResolvedExternCallSignature, SimpleCondition,
    SimpleExpr, SimpleStmt,
};
use crate::compiler::{FunctionMeta, SourceFile};
use crate::frontend::parser::{parse_top_level_extern_functions, parse_top_level_type_layout};
use crate::frontend::types::{
    TypeCategory, TypeId, TypeTable, TYPE_ID_BOOL, TYPE_ID_F32, TYPE_ID_F64, TYPE_ID_I32,
};

pub const FUNCTION_DATA_FLOW_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FunctionDataFlowEffects {
    pub reads: Vec<String>,
    pub writes: Vec<String>,
    pub parameter_reads: Vec<String>,
    pub parameter_writes: Vec<String>,
    pub calls: Vec<String>,
    pub host_calls: Vec<String>,
    #[serde(default)]
    pub host_call_costs: Vec<FunctionHostCallCost>,
    pub bounded_iterations: Vec<FunctionBoundedIteration>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct FunctionHostCallCost {
    pub function: String,
    pub max_invocations: Option<u64>,
    pub scope: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct FunctionBoundedIteration {
    pub function: String,
    pub kind: String,
    pub bound: String,
    pub max_iterations: Option<u64>,
    #[serde(default)]
    pub nesting_depth: u32,
    #[serde(default)]
    pub max_iteration_product: Option<u64>,
    #[serde(default)]
    pub source_order: u32,
    pub reads: Vec<String>,
    #[serde(default)]
    pub scanned_paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FunctionDataFlowSummary {
    pub schema_version: u32,
    pub function: String,
    pub file: String,
    pub source_start: u32,
    pub source_end: u32,
    pub signature_hash: String,
    pub direct: FunctionDataFlowEffects,
    pub aggregate: FunctionDataFlowEffects,
    #[serde(skip)]
    pub(crate) internal_direct_fingerprint: u64,
    #[serde(skip)]
    pub(crate) internal_syntax_fingerprint: u64,
    #[serde(skip)]
    internal_function_id: u32,
    #[serde(skip)]
    internal_signature_hash: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct EffectSets {
    reads: BTreeSet<String>,
    writes: BTreeSet<String>,
    parameter_reads: BTreeSet<String>,
    parameter_writes: BTreeSet<String>,
    calls: BTreeSet<String>,
    host_calls: BTreeSet<String>,
    host_call_costs: BTreeMap<String, Option<u64>>,
    bounded_iterations: BTreeSet<FunctionBoundedIteration>,
    call_sites: Vec<CallSite>,
    iteration_scans: BTreeMap<u32, Vec<String>>,
    iteration_products: BTreeMap<u32, Option<u64>>,
    active_iterations: Vec<u32>,
    next_iteration_id: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CallSite {
    target_id: u32,
    arguments: Vec<Option<String>>,
    max_invocations: Option<u64>,
    outer_nesting_depth: u32,
}

impl EffectSets {
    fn insert_read(&mut self, path: String) {
        self.record_scanned_path(&path);
        if path.starts_with('$') {
            self.parameter_reads.insert(path);
        } else {
            self.reads.insert(path);
        }
    }

    fn insert_write(&mut self, path: String) {
        self.record_scanned_path(&path);
        if path.starts_with('$') {
            self.parameter_writes.insert(path);
        } else {
            self.writes.insert(path);
        }
    }

    fn to_effects(&self, parameter_names: &[String]) -> FunctionDataFlowEffects {
        FunctionDataFlowEffects {
            reads: self.reads.iter().cloned().collect(),
            writes: self.writes.iter().cloned().collect(),
            parameter_reads: public_parameter_paths(&self.parameter_reads, parameter_names),
            parameter_writes: public_parameter_paths(&self.parameter_writes, parameter_names),
            calls: self.calls.iter().cloned().collect(),
            host_calls: self.host_calls.iter().cloned().collect(),
            host_call_costs: self
                .host_call_costs
                .iter()
                .map(|(function, max_invocations)| FunctionHostCallCost {
                    function: function.clone(),
                    max_invocations: *max_invocations,
                    scope: "direct".to_string(),
                })
                .collect(),
            bounded_iterations: self
                .bounded_iterations
                .iter()
                .map(|iteration| public_iteration(iteration, parameter_names))
                .collect(),
        }
    }

    fn record_scanned_path(&mut self, path: &str) {
        if !path.contains("[*]") || path.ends_with(".length") {
            return;
        }
        if let Some(iteration_id) = self.active_iterations.last().copied() {
            self.iteration_scans
                .entry(iteration_id)
                .or_default()
                .push(path.to_string());
        }
    }

    fn record_host_call(&mut self, function: &str) {
        let multiplier = self
            .active_iterations
            .last()
            .and_then(|id| self.iteration_products.get(id).copied())
            .unwrap_or(Some(1));
        merge_host_call_cost(
            self.host_call_costs
                .entry(function.to_string())
                .or_insert(Some(0)),
            multiplier,
        );
    }

    fn record_call_site(&mut self, target_id: u32, arguments: Vec<Option<String>>) {
        let max_invocations = self
            .active_iterations
            .last()
            .and_then(|id| self.iteration_products.get(id).copied())
            .unwrap_or(Some(1));
        let outer_nesting_depth = u32::try_from(self.active_iterations.len()).unwrap_or(u32::MAX);
        if let Some(existing) = self.call_sites.iter_mut().find(|call_site| {
            call_site.target_id == target_id
                && call_site.arguments == arguments
                && call_site.outer_nesting_depth == outer_nesting_depth
        }) {
            merge_host_call_cost(&mut existing.max_invocations, max_invocations);
            return;
        }
        self.call_sites.push(CallSite {
            target_id,
            arguments,
            max_invocations,
            outer_nesting_depth,
        });
    }
}

fn merge_host_call_costs(
    target: &mut BTreeMap<String, Option<u64>>,
    source: &BTreeMap<String, Option<u64>>,
    multiplier: Option<u64>,
) {
    for (function, count) in source {
        let scaled = multiplier
            .zip(*count)
            .and_then(|(multiplier, count)| multiplier.checked_mul(count));
        merge_host_call_cost(target.entry(function.clone()).or_insert(Some(0)), scaled);
    }
}

fn merge_host_call_cost(target: &mut Option<u64>, value: Option<u64>) {
    *target = target
        .zip(value)
        .and_then(|(target, value)| target.checked_add(value));
}

struct AnalysisContext<'a> {
    globals: BTreeSet<String>,
    constants: BTreeMap<String, i64>,
    view_parameters_by_function: BTreeMap<u32, BTreeSet<usize>>,
    fixed_parameter_capacities: BTreeMap<u32, BTreeMap<usize, u64>>,
    extern_functions: BTreeSet<String>,
    collection_capacities: BTreeMap<String, u64>,
    path_types: BTreeMap<String, TypeId>,
    field_types: BTreeMap<TypeId, BTreeMap<String, TypeId>>,
    call_signatures: CallSignatureMap,
    fingerprint: u64,
    types: &'a TypeTable,
}

pub(crate) fn build_function_data_flow_summaries(
    files: &[SourceFile],
    functions: &[FunctionMeta],
    statements_by_id: &[Vec<SimpleStmt>],
    included_function_ids: &BTreeSet<u32>,
    changed_function_ids: &BTreeSet<u32>,
    types: &TypeTable,
    previous: &[FunctionDataFlowSummary],
    previous_context_fingerprint: u64,
) -> Result<(Option<Vec<FunctionDataFlowSummary>>, u64), String> {
    let metadata_free = files.iter().all(|file| {
        !["struct", "global", "const", "extern"]
            .iter()
            .any(|keyword| file.content.contains(keyword))
    });
    let mut previous_by_id = vec![None; functions.len()];
    let previous_ids_are_valid = previous.iter().all(|summary| {
        let Some(slot) = previous_by_id.get_mut(summary.internal_function_id as usize) else {
            return false;
        };
        if slot.is_some() {
            return false;
        }
        *slot = Some(summary);
        true
    });
    if metadata_free
        && previous_ids_are_valid
        && previous.len() == included_function_ids.len()
        && functions
            .iter()
            .filter(|function| included_function_ids.contains(&function.id))
            .all(|function| {
                let file = &files[function.file_id as usize];
                previous_by_id[function.storage_index as usize].is_some_and(|summary| {
                    summary.internal_signature_hash == function.signature_hash
                        && summary.file == file.path
                        && summary.function == function.name
                        && summary.source_start == function.source_range.start
                        && summary.source_end == function.source_range.end
                        && (!changed_function_ids.contains(&function.id)
                            || summary.internal_syntax_fingerprint
                                == effect_syntax_fingerprint(
                                    &statements_by_id[function.storage_index as usize],
                                ))
                })
            })
    {
        return Ok((None, previous_context_fingerprint));
    }
    let previous_by_key: BTreeMap<_, _> = previous
        .iter()
        .map(|summary| {
            (
                (
                    summary.file.as_str(),
                    summary.function.as_str(),
                    summary.signature_hash.as_str(),
                ),
                summary,
            )
        })
        .collect();
    let context = build_context(files, functions, types)?;
    let reuse_candidate = previous.len() == included_function_ids.len()
        && previous_context_fingerprint == context.fingerprint;
    let mut direct_by_id = Vec::with_capacity(functions.len());
    for function in functions {
        if !included_function_ids.contains(&function.id) {
            direct_by_id.push(EffectSets::default());
            continue;
        }
        if reuse_candidate && !changed_function_ids.contains(&function.id) {
            direct_by_id.push(EffectSets::default());
        } else {
            direct_by_id.push(analyze_function_effects(
                function,
                statements_by_id,
                &context,
            )?);
        }
    }
    let can_reuse_aggregates = reuse_candidate
        && changed_function_ids
            .iter()
            .filter(|function_id| included_function_ids.contains(function_id))
            .all(|function| {
                let Some(function) = functions.iter().find(|candidate| candidate.id == *function)
                else {
                    return false;
                };
                let file = &files[function.file_id as usize];
                let signature_hash = format!("{:016x}", function.signature_hash);
                previous_by_key
                    .get(&(
                        file.path.as_str(),
                        function.name.as_str(),
                        signature_hash.as_str(),
                    ))
                    .is_some_and(|summary| {
                        summary.direct
                            == direct_by_id[function.storage_index as usize]
                                .to_effects(&function.param_names)
                            && summary.internal_direct_fingerprint
                                == effect_fingerprint(
                                    &direct_by_id[function.storage_index as usize],
                                )
                    })
            });
    if !can_reuse_aggregates && reuse_candidate {
        for function in functions {
            if included_function_ids.contains(&function.id)
                && !changed_function_ids.contains(&function.id)
            {
                direct_by_id[function.storage_index as usize] =
                    analyze_function_effects(function, statements_by_id, &context)?;
            }
        }
    }
    if can_reuse_aggregates
        && functions
            .iter()
            .filter(|function| included_function_ids.contains(&function.id))
            .all(|function| {
                let file = &files[function.file_id as usize];
                let signature_hash = format!("{:016x}", function.signature_hash);
                previous_by_key
                    .get(&(
                        file.path.as_str(),
                        function.name.as_str(),
                        signature_hash.as_str(),
                    ))
                    .is_some_and(|summary| {
                        summary.source_start == function.source_range.start
                            && summary.source_end == function.source_range.end
                    })
            })
    {
        return Ok((None, context.fingerprint));
    }
    let aggregate_by_id = if can_reuse_aggregates {
        None
    } else {
        Some(build_aggregate_effects(&direct_by_id)?)
    };
    let mut out = Vec::with_capacity(functions.len());
    for function in functions {
        if !included_function_ids.contains(&function.id) {
            continue;
        }
        let file = &files[function.file_id as usize];
        let signature_hash = format!("{:016x}", function.signature_hash);
        let aggregate = if let Some(aggregate_by_id) = &aggregate_by_id {
            aggregate_by_id[function.storage_index as usize].to_effects(&function.param_names)
        } else {
            previous_by_key[&(
                file.path.as_str(),
                function.name.as_str(),
                signature_hash.as_str(),
            )]
                .aggregate
                .clone()
        };
        let direct = if can_reuse_aggregates && !changed_function_ids.contains(&function.id) {
            previous_by_key[&(
                file.path.as_str(),
                function.name.as_str(),
                signature_hash.as_str(),
            )]
                .direct
                .clone()
        } else {
            direct_by_id[function.storage_index as usize].to_effects(&function.param_names)
        };
        let internal_direct_fingerprint =
            if can_reuse_aggregates && !changed_function_ids.contains(&function.id) {
                previous_by_key[&(
                    file.path.as_str(),
                    function.name.as_str(),
                    signature_hash.as_str(),
                )]
                    .internal_direct_fingerprint
            } else {
                effect_fingerprint(&direct_by_id[function.storage_index as usize])
            };
        let internal_syntax_fingerprint =
            effect_syntax_fingerprint(&statements_by_id[function.storage_index as usize]);
        out.push(FunctionDataFlowSummary {
            schema_version: FUNCTION_DATA_FLOW_SCHEMA_VERSION,
            function: function.name.clone(),
            file: file.path.clone(),
            source_start: function.source_range.start,
            source_end: function.source_range.end,
            signature_hash,
            direct,
            aggregate,
            internal_direct_fingerprint,
            internal_syntax_fingerprint,
            // Explicitly internal dense storage position; the public compiler identity is FnId.
            internal_function_id: function.storage_index,
            internal_signature_hash: function.signature_hash,
        });
    }
    out.sort_by(|left, right| {
        left.file
            .cmp(&right.file)
            .then(left.source_start.cmp(&right.source_start))
            .then(left.function.cmp(&right.function))
    });
    Ok((Some(out), context.fingerprint))
}

fn analyze_function_effects(
    function: &FunctionMeta,
    statements_by_id: &[Vec<SimpleStmt>],
    context: &AnalysisContext<'_>,
) -> Result<EffectSets, String> {
    let statements = statements_by_id
        .get(function.storage_index as usize)
        .ok_or_else(|| format!("function '{}' has no statement artifact", function.name))?;
    let mut effects = EffectSets::default();
    let mut locals = function.param_names.iter().cloned().collect();
    let mut local_types: BTreeMap<String, TypeId> = function
        .param_names
        .iter()
        .cloned()
        .zip(function.params.iter().copied())
        .collect();
    let view_parameters = context
        .view_parameters_by_function
        .get(&function.storage_index);
    let aliases = function
        .param_names
        .iter()
        .enumerate()
        .filter(|(index, _)| view_parameters.is_some_and(|positions| positions.contains(index)))
        .map(|(index, name)| (name.clone(), format!("${index}")))
        .collect();
    analyze_statements(
        statements,
        function.storage_index,
        &function.name,
        context,
        &mut locals,
        &mut local_types,
        &aliases,
        &mut effects,
        0,
        Some(1),
    );
    effects.bounded_iterations = effects
        .bounded_iterations
        .iter()
        .cloned()
        .map(|mut iteration| {
            iteration.scanned_paths = effects
                .iteration_scans
                .get(&iteration.source_order)
                .map_or_else(Vec::new, |paths| paths.iter().cloned().collect());
            iteration
        })
        .collect();
    Ok(effects)
}

fn effect_fingerprint(effects: &EffectSets) -> u64 {
    let mut hasher = DefaultHasher::new();
    effects.call_sites.hash(&mut hasher);
    hasher.finish()
}

fn effect_syntax_fingerprint(statements: &[SimpleStmt]) -> u64 {
    let mut hasher = DefaultHasher::new();
    hash_statement_shapes(statements, &mut hasher);
    hasher.finish()
}

fn hash_statement_shapes(statements: &[SimpleStmt], hasher: &mut DefaultHasher) {
    statements.len().hash(hasher);
    for statement in statements {
        std::mem::discriminant(statement).hash(hasher);
        match statement {
            SimpleStmt::Let {
                name,
                type_id,
                expression,
            } => {
                name.hash(hasher);
                type_id.hash(hasher);
                hash_expression_shape(expression, hasher);
            }
            SimpleStmt::Assign {
                target,
                op,
                expression,
            } => {
                format!("{target:?}|{op:?}").hash(hasher);
                hash_expression_shape(expression, hasher);
            }
            SimpleStmt::Convert {
                target,
                kind,
                source,
            } => {
                format!("{target:?}|{kind:?}").hash(hasher);
                hash_expression_shape(source, hasher);
            }
            SimpleStmt::If {
                condition,
                then_statements,
                else_statements,
            } => {
                hash_condition_shape(condition, hasher);
                hash_statement_shapes(then_statements, hasher);
                else_statements.is_some().hash(hasher);
                if let Some(else_statements) = else_statements {
                    hash_statement_shapes(else_statements, hasher);
                }
            }
            SimpleStmt::For {
                init,
                condition,
                step,
                body_statements,
            } => {
                format!("{init:?}|{condition:?}|{step:?}").hash(hasher);
                hash_statement_shapes(body_statements, hasher);
            }
            SimpleStmt::Foreach {
                item_name,
                index_name,
                collection_path,
                body_statements,
            } => {
                item_name.hash(hasher);
                index_name.hash(hasher);
                collection_path.hash(hasher);
                hash_statement_shapes(body_statements, hasher);
            }
            SimpleStmt::Expr(expression) | SimpleStmt::Return(expression) => {
                hash_expression_shape(expression, hasher)
            }
            SimpleStmt::Noop | SimpleStmt::Continue | SimpleStmt::ReturnVoid => {}
        }
    }
}

fn hash_condition_shape(condition: &SimpleCondition, hasher: &mut DefaultHasher) {
    std::mem::discriminant(condition).hash(hasher);
    match condition {
        SimpleCondition::Comparison { lhs, op, rhs } => {
            format!("{op:?}").hash(hasher);
            hash_expression_shape(lhs, hasher);
            hash_expression_shape(rhs, hasher);
        }
        SimpleCondition::Expr(expression) => hash_expression_shape(expression, hasher),
        SimpleCondition::And(lhs, rhs) | SimpleCondition::Or(lhs, rhs) => {
            hash_condition_shape(lhs, hasher);
            hash_condition_shape(rhs, hasher);
        }
        SimpleCondition::Not(inner) => hash_condition_shape(inner, hasher),
    }
}

fn hash_expression_shape(expression: &SimpleExpr, hasher: &mut DefaultHasher) {
    std::mem::discriminant(expression).hash(hasher);
    match expression {
        SimpleExpr::Int(_)
        | SimpleExpr::Float(_)
        | SimpleExpr::Bool(_)
        | SimpleExpr::StringLiteral(_) => {}
        SimpleExpr::Condition(condition) => hash_condition_shape(condition, hasher),
        SimpleExpr::Identifier(path) => path.hash(hasher),
        SimpleExpr::IndexedPath {
            collection_path,
            index,
            suffix,
        } => {
            collection_path.hash(hasher);
            hash_expression_shape(index, hasher);
            suffix.hash(hasher);
        }
        SimpleExpr::Call { target, args } => {
            target.hash(hasher);
            args.len().hash(hasher);
            for argument in args {
                hash_expression_shape(argument, hasher);
            }
        }
        SimpleExpr::Binary { lhs, op, rhs } => {
            op.hash(hasher);
            hash_expression_shape(lhs, hasher);
            hash_expression_shape(rhs, hasher);
        }
    }
}

fn build_context<'a>(
    files: &[SourceFile],
    functions: &'a [FunctionMeta],
    types: &'a TypeTable,
) -> Result<AnalysisContext<'a>, String> {
    let mut globals = BTreeSet::new();
    let mut constants = BTreeMap::new();
    let mut extern_functions = BTreeSet::new();
    let mut resolved_externs = Vec::new();
    let mut structs = BTreeMap::new();
    let mut global_types = Vec::new();
    for file in files {
        if ["struct", "global", "const"]
            .iter()
            .any(|keyword| file.content.contains(keyword))
        {
            let layout = parse_top_level_type_layout(&file.content)?;
            for constant in layout.constants {
                if let Ok(value) = constant.value_text.trim().parse::<i64>() {
                    constants.insert(constant.name, value);
                }
            }
            for structure in layout.structs {
                structs.insert(structure.name.clone(), structure.fields);
            }
            for global in layout.globals {
                globals.insert(global.name.clone());
                global_types.push((global.name, global.type_name));
            }
            for block in layout.global_blocks {
                globals.insert(block.name.clone());
                structs.insert(block.name.clone(), block.fields);
                global_types.push((block.name.clone(), block.name));
            }
        }
        if file.content.contains("extern") {
            for external in parse_top_level_extern_functions(&file.content)? {
                extern_functions.insert(external.name.clone());
                let params: Option<Vec<TypeId>> = external
                    .params
                    .iter()
                    .map(|parameter| types.resolve(&parameter.type_name))
                    .collect();
                if let (Some(params), Some(return_type)) =
                    (params, types.resolve(&external.return_type_name))
                {
                    resolved_externs.push(ResolvedExternCallSignature {
                        name: external.name,
                        symbol: external.symbol_name,
                        params,
                        return_type,
                    });
                }
            }
        }
    }
    let mut collection_capacities = BTreeMap::new();
    let mut path_types = BTreeMap::new();
    for (path, type_name) in &global_types {
        collect_collection_capacities(
            path,
            type_name,
            &structs,
            &constants,
            &mut collection_capacities,
            &mut BTreeSet::new(),
        );
        collect_path_types(
            path,
            type_name,
            &structs,
            types,
            &mut path_types,
            &mut BTreeSet::new(),
        );
    }
    let mut view_parameters_by_function = BTreeMap::new();
    let mut fixed_parameter_capacities = BTreeMap::new();
    for function in functions {
        let mut positions = BTreeSet::new();
        let mut capacities = BTreeMap::new();
        for (index, type_id) in function.params.iter().enumerate() {
            if types
                .type_info(*type_id)
                .is_some_and(|info| !matches!(info.category, TypeCategory::Builtin))
            {
                positions.insert(index);
            }
            if let Some(capacity) = types
                .fixed_collection_len(*type_id)
                .and_then(|capacity| u64::try_from(capacity).ok())
            {
                capacities.insert(index, capacity);
            }
        }
        view_parameters_by_function.insert(function.storage_index, positions);
        fixed_parameter_capacities.insert(function.storage_index, capacities);
    }
    let field_types = structs
        .iter()
        .filter_map(|(name, fields)| {
            let owner = types.resolve(name)?;
            let fields = fields
                .iter()
                .filter_map(|field| {
                    types
                        .resolve(&field.type_name)
                        .map(|ty| (field.name.clone(), ty))
                })
                .collect();
            Some((owner, fields))
        })
        .collect();
    let mut call_signatures =
        collect_supported_call_signatures(functions, &resolved_externs, types);
    for signatures in call_signatures.values_mut() {
        for signature in signatures {
            if let Some(function_id) = signature.function_id {
                signature.function_id = functions
                    .iter()
                    .find(|function| function.id == function_id)
                    .map(|function| function.storage_index);
            }
        }
    }
    let mut fingerprint_hasher = DefaultHasher::new();
    format!("{structs:?}|{global_types:?}|{constants:?}|{resolved_externs:?}")
        .hash(&mut fingerprint_hasher);
    let fingerprint = fingerprint_hasher.finish();
    Ok(AnalysisContext {
        globals,
        constants,
        view_parameters_by_function,
        fixed_parameter_capacities,
        extern_functions,
        collection_capacities,
        path_types,
        field_types,
        call_signatures,
        fingerprint,
        types,
    })
}

fn collect_path_types(
    path: &str,
    type_name: &str,
    structs: &BTreeMap<String, Vec<crate::frontend::parser::ParsedField>>,
    types: &TypeTable,
    paths: &mut BTreeMap<String, TypeId>,
    visiting: &mut BTreeSet<String>,
) {
    if let Some(type_id) = types.resolve(type_name) {
        paths.insert(path.to_string(), type_id);
    }
    if let Some((element_type, _)) = split_array_type(type_name) {
        collect_path_types(
            &format!("{path}[*]"),
            element_type,
            structs,
            types,
            paths,
            visiting,
        );
        return;
    }
    if !visiting.insert(type_name.to_string()) {
        return;
    }
    if let Some(fields) = structs.get(type_name) {
        for field in fields {
            collect_path_types(
                &format!("{path}.{}", field.name),
                &field.type_name,
                structs,
                types,
                paths,
                visiting,
            );
        }
    }
    visiting.remove(type_name);
}

fn collect_collection_capacities(
    path: &str,
    type_name: &str,
    structs: &BTreeMap<String, Vec<crate::frontend::parser::ParsedField>>,
    constants: &BTreeMap<String, i64>,
    capacities: &mut BTreeMap<String, u64>,
    visiting: &mut BTreeSet<String>,
) {
    if let Some((element_type, capacity_text)) = split_array_type(type_name) {
        if let Some(capacity) = parse_capacity(capacity_text, constants) {
            capacities.insert(path.to_string(), capacity);
        }
        collect_collection_capacities(
            &format!("{path}[*]"),
            element_type,
            structs,
            constants,
            capacities,
            visiting,
        );
        return;
    }
    if !visiting.insert(type_name.to_string()) {
        return;
    }
    if let Some(fields) = structs.get(type_name) {
        for field in fields {
            collect_collection_capacities(
                &format!("{path}.{}", field.name),
                &field.type_name,
                structs,
                constants,
                capacities,
                visiting,
            );
        }
    }
    visiting.remove(type_name);
}

fn split_array_type(type_name: &str) -> Option<(&str, &str)> {
    let trimmed = type_name.trim();
    let open = trimmed.rfind('[')?;
    let capacity = trimmed.get(open + 1..trimmed.len().checked_sub(1)?)?;
    trimmed
        .ends_with(']')
        .then_some((trimmed[..open].trim(), capacity.trim()))
}

fn parse_capacity(text: &str, constants: &BTreeMap<String, i64>) -> Option<u64> {
    text.parse::<u64>().ok().or_else(|| {
        constants
            .get(text)
            .and_then(|value| u64::try_from(*value).ok())
    })
}

fn analyze_statements(
    statements: &[SimpleStmt],
    function_id: u32,
    function: &str,
    context: &AnalysisContext<'_>,
    locals: &mut BTreeSet<String>,
    local_types: &mut BTreeMap<String, TypeId>,
    aliases: &BTreeMap<String, String>,
    effects: &mut EffectSets,
    nesting_depth: u32,
    parent_iteration_product: Option<u64>,
) {
    for statement in statements {
        match statement {
            SimpleStmt::Noop | SimpleStmt::Continue | SimpleStmt::ReturnVoid => {}
            SimpleStmt::Let {
                name,
                type_id,
                expression,
            } => {
                let inferred_type =
                    type_id.or_else(|| expression_type(expression, context, local_types, aliases));
                analyze_expression(expression, context, locals, local_types, aliases, effects);
                locals.insert(name.clone());
                if let Some(type_id) = inferred_type {
                    local_types.insert(name.clone(), type_id);
                }
            }
            SimpleStmt::Assign {
                target,
                op,
                expression,
            } => {
                analyze_expression(expression, context, locals, local_types, aliases, effects);
                analyze_assignment_target(
                    target,
                    *op != AssignOp::Set,
                    context,
                    locals,
                    local_types,
                    aliases,
                    effects,
                );
            }
            SimpleStmt::Convert { target, source, .. } => {
                analyze_expression(source, context, locals, local_types, aliases, effects);
                analyze_assignment_target(
                    target,
                    false,
                    context,
                    locals,
                    local_types,
                    aliases,
                    effects,
                );
            }
            SimpleStmt::If {
                condition,
                then_statements,
                else_statements,
            } => {
                analyze_condition(condition, context, locals, local_types, aliases, effects);
                analyze_nested_statements(
                    then_statements,
                    function_id,
                    function,
                    context,
                    locals,
                    local_types,
                    aliases,
                    effects,
                    nesting_depth,
                    parent_iteration_product,
                );
                if let Some(else_statements) = else_statements {
                    analyze_nested_statements(
                        else_statements,
                        function_id,
                        function,
                        context,
                        locals,
                        local_types,
                        aliases,
                        effects,
                        nesting_depth,
                        parent_iteration_product,
                    );
                }
            }
            SimpleStmt::For {
                init,
                condition,
                step,
                body_statements,
            } => {
                let mut loop_locals = locals.clone();
                let mut loop_local_types = local_types.clone();
                analyze_statements(
                    std::slice::from_ref(init.as_ref()),
                    function_id,
                    function,
                    context,
                    &mut loop_locals,
                    &mut loop_local_types,
                    aliases,
                    effects,
                    nesting_depth,
                    parent_iteration_product,
                );
                let mut bound_reads = EffectSets::default();
                analyze_condition(
                    condition,
                    context,
                    &loop_locals,
                    &loop_local_types,
                    aliases,
                    &mut bound_reads,
                );
                let max_iterations = static_for_max_iterations(
                    init,
                    condition,
                    step,
                    body_statements,
                    &context.constants,
                );
                let max_iteration_product = parent_iteration_product
                    .zip(max_iterations)
                    .and_then(|(parent, current)| parent.checked_mul(current));
                let iteration_id = effects.next_iteration_id;
                effects.next_iteration_id = effects.next_iteration_id.saturating_add(1);
                effects
                    .iteration_products
                    .insert(iteration_id, max_iteration_product);
                effects.bounded_iterations.insert(FunctionBoundedIteration {
                    function: function.to_string(),
                    kind: "for".to_string(),
                    bound: display_condition(condition),
                    max_iterations,
                    nesting_depth,
                    max_iteration_product,
                    source_order: iteration_id,
                    reads: bound_reads.reads.into_iter().collect(),
                    scanned_paths: Vec::new(),
                });
                effects.active_iterations.push(iteration_id);
                analyze_condition(
                    condition,
                    context,
                    &loop_locals,
                    &loop_local_types,
                    aliases,
                    effects,
                );
                analyze_nested_statements(
                    body_statements,
                    function_id,
                    function,
                    context,
                    &loop_locals,
                    &loop_local_types,
                    aliases,
                    effects,
                    nesting_depth.saturating_add(1),
                    max_iteration_product,
                );
                analyze_statements(
                    std::slice::from_ref(step.as_ref()),
                    function_id,
                    function,
                    context,
                    &mut loop_locals,
                    &mut loop_local_types,
                    aliases,
                    effects,
                    nesting_depth.saturating_add(1),
                    max_iteration_product,
                );
                effects.active_iterations.pop();
            }
            SimpleStmt::Foreach {
                item_name,
                index_name,
                collection_path,
                body_statements,
            } => {
                let state_collection =
                    normalize_state_path(collection_path, context, locals, aliases);
                let normalized = state_collection
                    .clone()
                    .unwrap_or_else(|| collection_path.clone());
                let bound_path = format!("{normalized}.length");
                if context.globals.contains(root_name(&normalized)) {
                    effects.insert_read(bound_path.clone());
                } else if normalized.starts_with('$') {
                    effects.insert_read(bound_path.clone());
                }
                let max_iterations = context
                    .collection_capacities
                    .get(&normalized)
                    .copied()
                    .or_else(|| {
                        symbolic_parameter_index(&normalized).and_then(|index| {
                            context
                                .fixed_parameter_capacities
                                .get(&function_id)?
                                .get(&index)
                                .copied()
                        })
                    });
                let max_iteration_product = parent_iteration_product
                    .zip(max_iterations)
                    .and_then(|(parent, current)| parent.checked_mul(current));
                let iteration_id = effects.next_iteration_id;
                effects.next_iteration_id = effects.next_iteration_id.saturating_add(1);
                effects
                    .iteration_products
                    .insert(iteration_id, max_iteration_product);
                effects.bounded_iterations.insert(FunctionBoundedIteration {
                    function: function.to_string(),
                    kind: "foreach".to_string(),
                    bound: normalized.clone(),
                    max_iterations,
                    nesting_depth,
                    max_iteration_product,
                    source_order: iteration_id,
                    reads: (context.globals.contains(root_name(&normalized))
                        || normalized.starts_with('$'))
                    .then_some(vec![bound_path])
                    .unwrap_or_default(),
                    scanned_paths: Vec::new(),
                });
                let mut loop_locals = locals.clone();
                let mut loop_local_types = local_types.clone();
                loop_locals.insert(item_name.clone());
                if let Some(index_name) = index_name {
                    loop_locals.insert(index_name.clone());
                }
                let mut loop_aliases = aliases.clone();
                if state_collection.is_some() {
                    loop_aliases.insert(item_name.clone(), format!("{normalized}[*]"));
                }
                if let Some(collection_type) =
                    path_type(collection_path, context, local_types, aliases)
                {
                    if let Some(element_type) =
                        context.types.indexed_element_type_id(collection_type)
                    {
                        loop_local_types.insert(item_name.clone(), element_type);
                    }
                }
                effects.active_iterations.push(iteration_id);
                analyze_nested_statements(
                    body_statements,
                    function_id,
                    function,
                    context,
                    &loop_locals,
                    &loop_local_types,
                    &loop_aliases,
                    effects,
                    nesting_depth.saturating_add(1),
                    max_iteration_product,
                );
                effects.active_iterations.pop();
            }
            SimpleStmt::Expr(expression) | SimpleStmt::Return(expression) => {
                analyze_expression(expression, context, locals, local_types, aliases, effects);
            }
        }
    }
}

fn analyze_nested_statements(
    statements: &[SimpleStmt],
    function_id: u32,
    function: &str,
    context: &AnalysisContext<'_>,
    locals: &BTreeSet<String>,
    local_types: &BTreeMap<String, TypeId>,
    aliases: &BTreeMap<String, String>,
    effects: &mut EffectSets,
    nesting_depth: u32,
    parent_iteration_product: Option<u64>,
) {
    analyze_statements(
        statements,
        function_id,
        function,
        context,
        &mut locals.clone(),
        &mut local_types.clone(),
        aliases,
        effects,
        nesting_depth,
        parent_iteration_product,
    );
}

fn analyze_assignment_target(
    target: &AssignTarget,
    reads_existing: bool,
    context: &AnalysisContext<'_>,
    locals: &BTreeSet<String>,
    local_types: &BTreeMap<String, TypeId>,
    aliases: &BTreeMap<String, String>,
    effects: &mut EffectSets,
) {
    let path = match target {
        AssignTarget::Local(path) | AssignTarget::GlobalPath(path) => {
            normalize_state_path(path, context, locals, aliases)
        }
        AssignTarget::IndexedPath {
            collection_path,
            index,
            suffix,
        } => {
            analyze_expression(index, context, locals, local_types, aliases, effects);
            normalize_state_path(collection_path, context, locals, aliases)
                .map(|path| indexed_state_path(&path, suffix))
        }
    };
    if let Some(path) = path {
        if reads_existing {
            effects.insert_read(path.clone());
        }
        effects.insert_write(path);
    }
}

fn analyze_expression(
    expression: &SimpleExpr,
    context: &AnalysisContext<'_>,
    locals: &BTreeSet<String>,
    local_types: &BTreeMap<String, TypeId>,
    aliases: &BTreeMap<String, String>,
    effects: &mut EffectSets,
) {
    match expression {
        SimpleExpr::Int(_)
        | SimpleExpr::Float(_)
        | SimpleExpr::Bool(_)
        | SimpleExpr::StringLiteral(_) => {}
        SimpleExpr::Condition(condition) => {
            analyze_condition(condition, context, locals, local_types, aliases, effects)
        }
        SimpleExpr::Identifier(path) => {
            if let Some(path) = normalize_state_path(path, context, locals, aliases) {
                effects.insert_read(path);
            }
        }
        SimpleExpr::IndexedPath {
            collection_path,
            index,
            suffix,
        } => {
            analyze_expression(index, context, locals, local_types, aliases, effects);
            if let Some(path) = normalize_state_path(collection_path, context, locals, aliases) {
                effects.insert_read(indexed_state_path(&path, suffix));
            }
        }
        SimpleExpr::Call { target, args } => {
            let target_id = resolve_internal_call(target, args, context, local_types, aliases);
            if let Some(target_id) = target_id {
                effects.calls.insert(target.clone());
                effects.record_call_site(
                    target_id,
                    args.iter()
                        .map(|argument| expression_effect_path(argument, context, locals, aliases))
                        .collect(),
                );
            } else if context.extern_functions.contains(target) {
                effects.host_calls.insert(target.clone());
                effects.record_host_call(target);
            }
            for (index, argument) in args.iter().enumerate() {
                let is_view = target_id
                    .and_then(|target_id| context.view_parameters_by_function.get(&target_id))
                    .is_some_and(|positions| positions.contains(&index));
                if is_view {
                    analyze_view_argument(argument, context, locals, local_types, aliases, effects);
                } else {
                    analyze_expression(argument, context, locals, local_types, aliases, effects);
                }
            }
        }
        SimpleExpr::Binary { lhs, rhs, .. } => {
            analyze_expression(lhs, context, locals, local_types, aliases, effects);
            analyze_expression(rhs, context, locals, local_types, aliases, effects);
        }
    }
}

fn analyze_view_argument(
    expression: &SimpleExpr,
    context: &AnalysisContext<'_>,
    locals: &BTreeSet<String>,
    local_types: &BTreeMap<String, TypeId>,
    aliases: &BTreeMap<String, String>,
    effects: &mut EffectSets,
) {
    match expression {
        SimpleExpr::Identifier(_) => {}
        SimpleExpr::IndexedPath { index, .. } => {
            analyze_expression(index, context, locals, local_types, aliases, effects)
        }
        _ => analyze_expression(expression, context, locals, local_types, aliases, effects),
    }
}

fn analyze_condition(
    condition: &SimpleCondition,
    context: &AnalysisContext<'_>,
    locals: &BTreeSet<String>,
    local_types: &BTreeMap<String, TypeId>,
    aliases: &BTreeMap<String, String>,
    effects: &mut EffectSets,
) {
    match condition {
        SimpleCondition::Comparison { lhs, rhs, .. } => {
            analyze_expression(lhs, context, locals, local_types, aliases, effects);
            analyze_expression(rhs, context, locals, local_types, aliases, effects);
        }
        SimpleCondition::Expr(expression) => {
            analyze_expression(expression, context, locals, local_types, aliases, effects)
        }
        SimpleCondition::And(lhs, rhs) | SimpleCondition::Or(lhs, rhs) => {
            analyze_condition(lhs, context, locals, local_types, aliases, effects);
            analyze_condition(rhs, context, locals, local_types, aliases, effects);
        }
        SimpleCondition::Not(inner) => {
            analyze_condition(inner, context, locals, local_types, aliases, effects)
        }
    }
}

fn resolve_internal_call(
    target: &str,
    args: &[SimpleExpr],
    context: &AnalysisContext<'_>,
    local_types: &BTreeMap<String, TypeId>,
    aliases: &BTreeMap<String, String>,
) -> Option<u32> {
    let argument_types: Vec<TypeId> = args
        .iter()
        .map(|argument| expression_type(argument, context, local_types, aliases))
        .collect::<Option<_>>()?;
    resolve_call_signature(
        target,
        &argument_types,
        &context.call_signatures,
        context.types,
        &context.field_types,
    )
    .ok()?
    .function_id
}

fn expression_type(
    expression: &SimpleExpr,
    context: &AnalysisContext<'_>,
    local_types: &BTreeMap<String, TypeId>,
    aliases: &BTreeMap<String, String>,
) -> Option<TypeId> {
    match expression {
        SimpleExpr::Int(_) => Some(TYPE_ID_I32),
        SimpleExpr::Float(_) => Some(TYPE_ID_F32),
        SimpleExpr::Bool(_) | SimpleExpr::Condition(_) => Some(TYPE_ID_BOOL),
        SimpleExpr::StringLiteral(_) => context.types.string_literal_type_id(),
        SimpleExpr::Identifier(path) => path_type(path, context, local_types, aliases),
        SimpleExpr::IndexedPath {
            collection_path,
            suffix,
            ..
        } => {
            let collection = path_type(collection_path, context, local_types, aliases)?;
            let element = context.types.indexed_element_type_id(collection)?;
            field_suffix_type(element, suffix, &context.field_types)
        }
        SimpleExpr::Call { target, args } => match target.as_str() {
            "i32_to_f32" | "sin_fast" | "cos_fast" => Some(TYPE_ID_F32),
            "f32_to_i32" | "fixed32_from_i32" | "fixed32_to_i32" | "fixed32_mul"
            | "fixed32_div" | "fixed32_from_ratio" => Some(TYPE_ID_I32),
            _ => {
                let argument_types: Vec<TypeId> = args
                    .iter()
                    .map(|argument| expression_type(argument, context, local_types, aliases))
                    .collect::<Option<_>>()?;
                resolve_call_signature(
                    target,
                    &argument_types,
                    &context.call_signatures,
                    context.types,
                    &context.field_types,
                )
                .ok()
                .map(|signature| signature.return_type)
            }
        },
        SimpleExpr::Binary { lhs, rhs, .. } => {
            let lhs = expression_type(lhs, context, local_types, aliases)?;
            let rhs = expression_type(rhs, context, local_types, aliases)?;
            if lhs == TYPE_ID_F64 || rhs == TYPE_ID_F64 {
                Some(TYPE_ID_F64)
            } else if lhs == TYPE_ID_F32 || rhs == TYPE_ID_F32 {
                Some(TYPE_ID_F32)
            } else {
                Some(lhs)
            }
        }
    }
}

fn path_type(
    path: &str,
    context: &AnalysisContext<'_>,
    local_types: &BTreeMap<String, TypeId>,
    aliases: &BTreeMap<String, String>,
) -> Option<TypeId> {
    if let Some(type_id) = context.path_types.get(path) {
        return Some(*type_id);
    }
    let root = root_name(path);
    let root_type = local_types.get(root).copied().or_else(|| {
        aliases
            .get(root)
            .and_then(|alias| context.path_types.get(alias).copied())
    })?;
    let suffix = path.strip_prefix(root)?.trim_start_matches('.');
    field_suffix_type(root_type, suffix, &context.field_types)
}

fn field_suffix_type(
    mut type_id: TypeId,
    suffix: &str,
    field_types: &BTreeMap<TypeId, BTreeMap<String, TypeId>>,
) -> Option<TypeId> {
    if suffix.is_empty() {
        return Some(type_id);
    }
    for field in suffix.trim_start_matches('.').split('.') {
        type_id = *field_types.get(&type_id)?.get(field)?;
    }
    Some(type_id)
}

fn normalize_state_path(
    path: &str,
    context: &AnalysisContext<'_>,
    locals: &BTreeSet<String>,
    aliases: &BTreeMap<String, String>,
) -> Option<String> {
    let root = root_name(path);
    if let Some(collection_path) = aliases.get(root) {
        return Some(format!("{collection_path}{}", &path[root.len()..]));
    }
    if locals.contains(root) || context.constants.contains_key(root) {
        return None;
    }
    context.globals.contains(root).then(|| path.to_string())
}

fn root_name(path: &str) -> &str {
    path.split(['.', '[']).next().unwrap_or(path)
}

fn indexed_state_path(collection_path: &str, suffix: &str) -> String {
    if suffix.is_empty() {
        format!("{collection_path}[*]")
    } else {
        format!("{collection_path}[*].{}", suffix.trim_start_matches('.'))
    }
}

fn expression_effect_path(
    expression: &SimpleExpr,
    context: &AnalysisContext,
    locals: &BTreeSet<String>,
    aliases: &BTreeMap<String, String>,
) -> Option<String> {
    match expression {
        SimpleExpr::Identifier(path) => normalize_state_path(path, context, locals, aliases),
        SimpleExpr::IndexedPath {
            collection_path,
            suffix,
            ..
        } => normalize_state_path(collection_path, context, locals, aliases)
            .map(|path| indexed_state_path(&path, suffix)),
        _ => None,
    }
}

fn build_aggregate_effects(direct_by_id: &[EffectSets]) -> Result<Vec<EffectSets>, String> {
    let components = strongly_connected_components(direct_by_id);
    let mut component_by_function = vec![0usize; direct_by_id.len()];
    for (component_id, component) in components.iter().enumerate() {
        for function_id in component {
            component_by_function[*function_id] = component_id;
        }
    }
    let mut component_edges = vec![BTreeSet::new(); components.len()];
    for (function_id, direct) in direct_by_id.iter().enumerate() {
        let source = component_by_function[function_id];
        for call_site in &direct.call_sites {
            let target = call_site.target_id as usize;
            if target < direct_by_id.len() {
                let target = component_by_function[target];
                if source != target {
                    component_edges[source].insert(target);
                }
            }
        }
    }
    let mut component_order = Vec::with_capacity(components.len());
    let mut state = vec![0u8; components.len()];
    for root in 0..components.len() {
        if state[root] != 0 {
            continue;
        }
        let mut stack = vec![(root, false)];
        while let Some((component_id, expanded)) = stack.pop() {
            if expanded {
                state[component_id] = 2;
                component_order.push(component_id);
                continue;
            }
            if state[component_id] != 0 {
                continue;
            }
            state[component_id] = 1;
            stack.push((component_id, true));
            for target in component_edges[component_id].iter().rev() {
                if state[*target] == 0 {
                    stack.push((*target, false));
                }
            }
        }
    }

    let mut aggregate_by_id = vec![EffectSets::default(); direct_by_id.len()];
    for component_id in component_order {
        let component = &components[component_id];
        let cyclic = component.len() > 1
            || direct_by_id[component[0]]
                .call_sites
                .iter()
                .any(|call_site| call_site.target_id as usize == component[0]);
        let mut converged = false;
        for _ in 0..component.len().saturating_add(2) {
            let mut next = Vec::with_capacity(component.len());
            for function_id in component {
                let direct = &direct_by_id[*function_id];
                let mut aggregate = EffectSets::default();
                merge_substituted_effects(
                    direct,
                    &BTreeMap::new(),
                    true,
                    Some(1),
                    0,
                    &mut aggregate,
                );
                for call_site in &direct.call_sites {
                    let Some(child) = aggregate_by_id.get(call_site.target_id as usize) else {
                        continue;
                    };
                    let child_substitutions = call_site
                        .arguments
                        .iter()
                        .enumerate()
                        .filter_map(|(index, path)| path.clone().map(|path| (index, path)))
                        .collect();
                    let recursive_edge =
                        cyclic && component.contains(&(call_site.target_id as usize));
                    let multiplier = if recursive_edge {
                        None
                    } else {
                        call_site.max_invocations
                    };
                    merge_substituted_effects(
                        child,
                        &child_substitutions,
                        false,
                        multiplier,
                        if recursive_edge {
                            0
                        } else {
                            call_site.outer_nesting_depth
                        },
                        &mut aggregate,
                    );
                }
                next.push(aggregate);
            }
            if component
                .iter()
                .zip(&next)
                .all(|(function_id, aggregate)| aggregate_by_id[*function_id] == *aggregate)
            {
                converged = true;
                break;
            }
            for (function_id, aggregate) in component.iter().zip(next) {
                aggregate_by_id[*function_id] = aggregate;
            }
            if !cyclic {
                converged = true;
                break;
            }
        }
        if !converged {
            return Err(format!(
                "function data-flow cycle did not converge for function ids {:?}",
                component
            ));
        }
    }
    Ok(aggregate_by_id)
}

fn strongly_connected_components(direct_by_id: &[EffectSets]) -> Vec<Vec<usize>> {
    let mut visited = vec![false; direct_by_id.len()];
    let mut finish = Vec::with_capacity(direct_by_id.len());
    for root in 0..direct_by_id.len() {
        if visited[root] {
            continue;
        }
        visited[root] = true;
        let mut stack = vec![(root, 0usize)];
        while let Some((function_id, edge_index)) = stack.pop() {
            if let Some(call_site) = direct_by_id[function_id].call_sites.get(edge_index) {
                stack.push((function_id, edge_index + 1));
                let child = call_site.target_id as usize;
                if child < direct_by_id.len() && !visited[child] {
                    visited[child] = true;
                    stack.push((child, 0));
                }
            } else {
                finish.push(function_id);
            }
        }
    }
    let mut reverse = vec![Vec::new(); direct_by_id.len()];
    for (function_id, direct) in direct_by_id.iter().enumerate() {
        for call_site in &direct.call_sites {
            let child = call_site.target_id as usize;
            if child < reverse.len() {
                reverse[child].push(function_id);
            }
        }
    }
    visited.fill(false);
    let mut components = Vec::new();
    for root in finish.into_iter().rev() {
        if visited[root] {
            continue;
        }
        visited[root] = true;
        let mut component = Vec::new();
        let mut stack = vec![root];
        while let Some(function_id) = stack.pop() {
            component.push(function_id);
            for parent in &reverse[function_id] {
                if !visited[*parent] {
                    visited[*parent] = true;
                    stack.push(*parent);
                }
            }
        }
        component.sort_unstable();
        components.push(component);
    }
    components
}

fn merge_substituted_effects(
    direct: &EffectSets,
    substitutions: &BTreeMap<usize, String>,
    retain_unmapped_parameters: bool,
    host_call_multiplier: Option<u64>,
    outer_nesting_depth: u32,
    aggregate: &mut EffectSets,
) {
    aggregate.reads.extend(direct.reads.iter().cloned());
    aggregate.writes.extend(direct.writes.iter().cloned());
    aggregate.calls.extend(direct.calls.iter().cloned());
    aggregate
        .host_calls
        .extend(direct.host_calls.iter().cloned());
    merge_host_call_costs(
        &mut aggregate.host_call_costs,
        &direct.host_call_costs,
        host_call_multiplier,
    );
    for path in &direct.parameter_reads {
        if let Some(path) = substitute_path(path, substitutions, retain_unmapped_parameters) {
            aggregate.insert_read(path);
        }
    }
    for path in &direct.parameter_writes {
        if let Some(path) = substitute_path(path, substitutions, retain_unmapped_parameters) {
            aggregate.insert_write(path);
        }
    }
    for iteration in &direct.bounded_iterations {
        let mut iteration = iteration.clone();
        iteration.max_iteration_product = host_call_multiplier
            .zip(iteration.max_iteration_product)
            .and_then(|(multiplier, product)| multiplier.checked_mul(product));
        iteration.nesting_depth = iteration.nesting_depth.saturating_add(outer_nesting_depth);
        if let Some(bound) =
            substitute_path(&iteration.bound, substitutions, retain_unmapped_parameters)
        {
            iteration.bound = bound;
        }
        iteration.reads = iteration
            .reads
            .iter()
            .filter_map(|path| substitute_path(path, substitutions, retain_unmapped_parameters))
            .collect();
        iteration.scanned_paths = iteration
            .scanned_paths
            .iter()
            .filter_map(|path| substitute_path(path, substitutions, retain_unmapped_parameters))
            .collect();
        aggregate.bounded_iterations.insert(iteration);
    }
}

fn substitute_path(
    path: &str,
    substitutions: &BTreeMap<usize, String>,
    retain_unmapped: bool,
) -> Option<String> {
    let Some(symbolic) = path.strip_prefix('$') else {
        return Some(path.to_string());
    };
    let digit_count = symbolic.bytes().take_while(u8::is_ascii_digit).count();
    let index = symbolic[..digit_count].parse::<usize>().ok()?;
    let suffix = &symbolic[digit_count..];
    substitutions
        .get(&index)
        .map(|base| format!("{base}{suffix}"))
        .or_else(|| retain_unmapped.then(|| path.to_string()))
}

fn public_parameter_paths(paths: &BTreeSet<String>, parameter_names: &[String]) -> Vec<String> {
    paths
        .iter()
        .filter_map(|path| public_parameter_path(path, parameter_names))
        .collect()
}

fn public_iteration(
    iteration: &FunctionBoundedIteration,
    parameter_names: &[String],
) -> FunctionBoundedIteration {
    FunctionBoundedIteration {
        function: iteration.function.clone(),
        kind: iteration.kind.clone(),
        bound: public_parameter_path(&iteration.bound, parameter_names)
            .unwrap_or_else(|| iteration.bound.clone()),
        max_iterations: iteration.max_iterations,
        nesting_depth: iteration.nesting_depth,
        max_iteration_product: iteration.max_iteration_product,
        source_order: iteration.source_order,
        reads: iteration
            .reads
            .iter()
            .map(|path| {
                public_parameter_path(path, parameter_names).unwrap_or_else(|| path.clone())
            })
            .collect(),
        scanned_paths: iteration
            .scanned_paths
            .iter()
            .map(|path| {
                public_parameter_path(path, parameter_names).unwrap_or_else(|| path.clone())
            })
            .collect(),
    }
}

fn public_parameter_path(path: &str, parameter_names: &[String]) -> Option<String> {
    let symbolic = path.strip_prefix('$')?;
    let digit_count = symbolic.bytes().take_while(u8::is_ascii_digit).count();
    let index = symbolic[..digit_count].parse::<usize>().ok()?;
    parameter_names
        .get(index)
        .map(|name| format!("{name}{}", &symbolic[digit_count..]))
}

fn symbolic_parameter_index(path: &str) -> Option<usize> {
    let symbolic = path.strip_prefix('$')?;
    let digit_count = symbolic.bytes().take_while(u8::is_ascii_digit).count();
    symbolic[..digit_count].parse().ok()
}

fn static_for_max_iterations(
    init: &SimpleStmt,
    condition: &SimpleCondition,
    step: &SimpleStmt,
    body_statements: &[SimpleStmt],
    constants: &BTreeMap<String, i64>,
) -> Option<u64> {
    let (variable, start) = match init {
        SimpleStmt::Let {
            name, expression, ..
        } => (name.as_str(), eval_integer(expression, constants)?),
        SimpleStmt::Assign {
            target: AssignTarget::Local(name),
            op: AssignOp::Set,
            expression,
        } => (name.as_str(), eval_integer(expression, constants)?),
        _ => return None,
    };
    if statements_write_local(body_statements, variable) {
        return None;
    }
    let (op, end) = match condition {
        SimpleCondition::Comparison {
            lhs: SimpleExpr::Identifier(name),
            op,
            rhs,
        } if name == variable => (*op, eval_integer(rhs, constants)?),
        _ => return None,
    };
    let increment = match step {
        SimpleStmt::Assign {
            target: AssignTarget::Local(name),
            op: AssignOp::Add,
            expression,
        } if name == variable => eval_integer(expression, constants)?,
        SimpleStmt::Assign {
            target: AssignTarget::Local(name),
            op: AssignOp::Set,
            expression: SimpleExpr::Binary { lhs, op: '+', rhs },
        } if name == variable
            && matches!(lhs.as_ref(), SimpleExpr::Identifier(value) if value == variable) =>
        {
            eval_integer(rhs, constants)?
        }
        _ => return None,
    };
    if increment <= 0 {
        return None;
    }
    let distance = match op {
        ComparisonOp::Lt => end.saturating_sub(start),
        ComparisonOp::Le => end.saturating_sub(start).saturating_add(1),
        _ => return None,
    };
    if distance <= 0 {
        return Some(0);
    }
    let distance = u64::try_from(distance).ok()?;
    let increment = u64::try_from(increment).ok()?;
    Some((distance + increment - 1) / increment)
}

fn statements_write_local(statements: &[SimpleStmt], name: &str) -> bool {
    statements.iter().any(|statement| match statement {
        SimpleStmt::Assign { target, .. } | SimpleStmt::Convert { target, .. } => {
            matches!(target, AssignTarget::Local(path) if path == name)
        }
        SimpleStmt::If {
            then_statements,
            else_statements,
            ..
        } => {
            statements_write_local(then_statements, name)
                || else_statements
                    .as_deref()
                    .is_some_and(|statements| statements_write_local(statements, name))
        }
        SimpleStmt::For {
            init,
            step,
            body_statements,
            ..
        } => {
            statements_write_local(std::slice::from_ref(init.as_ref()), name)
                || statements_write_local(std::slice::from_ref(step.as_ref()), name)
                || statements_write_local(body_statements, name)
        }
        SimpleStmt::Foreach {
            body_statements, ..
        } => statements_write_local(body_statements, name),
        _ => false,
    })
}

fn eval_integer(expression: &SimpleExpr, constants: &BTreeMap<String, i64>) -> Option<i64> {
    match expression {
        SimpleExpr::Identifier(name) => constants.get(name).copied(),
        _ => eval_const_i64(expression),
    }
}

fn display_condition(condition: &SimpleCondition) -> String {
    match condition {
        SimpleCondition::Comparison { lhs, op, rhs } => format!(
            "{} {} {}",
            display_expression(lhs),
            match op {
                ComparisonOp::Eq => "==",
                ComparisonOp::Ne => "!=",
                ComparisonOp::Lt => "<",
                ComparisonOp::Le => "<=",
                ComparisonOp::Gt => ">",
                ComparisonOp::Ge => ">=",
            },
            display_expression(rhs)
        ),
        SimpleCondition::Expr(expression) => display_expression(expression),
        SimpleCondition::And(lhs, rhs) => {
            format!(
                "({}) && ({})",
                display_condition(lhs),
                display_condition(rhs)
            )
        }
        SimpleCondition::Or(lhs, rhs) => {
            format!(
                "({}) || ({})",
                display_condition(lhs),
                display_condition(rhs)
            )
        }
        SimpleCondition::Not(inner) => format!("!({})", display_condition(inner)),
    }
}

fn display_expression(expression: &SimpleExpr) -> String {
    match expression {
        SimpleExpr::Int(value) => value.to_string(),
        SimpleExpr::Float(value) => value.to_string(),
        SimpleExpr::Bool(value) => value.to_string(),
        SimpleExpr::StringLiteral(_) => "string".to_string(),
        SimpleExpr::Condition(condition) => display_condition(condition),
        SimpleExpr::Identifier(name) => name.clone(),
        SimpleExpr::IndexedPath {
            collection_path,
            index,
            suffix,
        } => format!(
            "{collection_path}[{}]{}",
            display_expression(index),
            if suffix.is_empty() {
                String::new()
            } else {
                format!(".{}", suffix.trim_start_matches('.'))
            }
        ),
        SimpleExpr::Call { target, .. } => format!("{target}(...)"),
        SimpleExpr::Binary { lhs, op, rhs } => format!(
            "{} {} {}",
            display_expression(lhs),
            op,
            display_expression(rhs)
        ),
    }
}

#[cfg(test)]
mod tests {
    use crate::backend::jit::JitProcess;
    use std::path::Path;

    #[test]
    fn representative_sample_compiles_to_cranelift_and_reports_runtime_state() {
        let sample = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../samples/function_data_flow/src/main.stasis");
        let source = std::fs::read_to_string(&sample).expect("read data-flow sample");
        let mut jit = JitProcess::new();
        jit.set_project_root(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .to_string_lossy(),
        )
        .expect("set repository root");
        jit.set_required_emit_roots(&["main".to_string()]);
        jit.upsert_file(sample.to_string_lossy(), source);
        jit.compile().expect("compile data-flow sample");

        assert_eq!(jit.execute_i32_noarg_by_name("main").expect("run main"), 0);
        assert_eq!(jit.read_i32_global_path("state.score"), 13);
        let tick = jit
            .function_data_flow_summaries()
            .iter()
            .find(|summary| summary.function == "tick")
            .expect("tick summary");
        assert_eq!(tick.direct.calls, vec!["sum_enemy_health"]);
        assert_eq!(tick.direct.host_calls, vec!["print_i32"]);
        assert_eq!(tick.direct.bounded_iterations[0].max_iterations, Some(3));
        assert!(tick.aggregate.writes.contains(&"state.score".to_string()));
    }

    #[test]
    fn nested_loop_products_and_deepest_field_scans_are_bounded() {
        let sample = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../samples/bounded_performance/src/main.stasis");
        let source = std::fs::read_to_string(&sample).expect("read bounded-cost sample");
        let mut jit = JitProcess::new();
        jit.set_project_root(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .to_string_lossy(),
        )
        .expect("set repository root");
        jit.set_required_emit_roots(&["main".to_string(), "tick".to_string()]);
        jit.upsert_file(sample.to_string_lossy(), source);
        jit.compile().expect("compile bounded-cost sample");
        assert_eq!(jit.execute_i32_noarg_by_name("main").expect("run main"), 0);

        let scan = jit
            .function_data_flow_summaries()
            .iter()
            .find(|summary| summary.function == "expensive_scan")
            .expect("expensive scan summary");
        let inner = scan
            .direct
            .bounded_iterations
            .iter()
            .find(|iteration| iteration.nesting_depth == 1)
            .expect("nested loop");
        assert_eq!(inner.max_iterations, Some(16));
        assert_eq!(inner.max_iteration_product, Some(512));
        assert!(inner
            .scanned_paths
            .contains(&"particles[*].score".to_string()));
        let outer = scan
            .direct
            .bounded_iterations
            .iter()
            .find(|iteration| iteration.nesting_depth == 0)
            .expect("outer loop");
        assert_eq!(outer.max_iteration_product, Some(32));
        assert!(outer.scanned_paths.is_empty());
    }

    #[test]
    fn host_call_cost_uses_lexical_nested_iteration_product() {
        let mut jit = JitProcess::new();
        jit.set_required_emit_roots(&["tick".to_string()]);
        jit.upsert_file(
            "host_cost.stasis",
            "extern function print_i32(value: i32): void;\nfunction tick(): i32 { for (let x: i32 = 0; x < 2; x += 1) { for (let y: i32 = 0; y < 3; y += 1) { print_i32(x + y); } } return 0; }\n",
        );
        jit.compile().expect("compile host-call cost fixture");
        let tick = jit
            .function_data_flow_summaries()
            .iter()
            .find(|summary| summary.function == "tick")
            .expect("tick summary");
        assert_eq!(tick.direct.host_call_costs.len(), 1);
        assert_eq!(tick.direct.host_call_costs[0].function, "print_i32");
        assert_eq!(tick.direct.host_call_costs[0].max_invocations, Some(6));
    }
}
