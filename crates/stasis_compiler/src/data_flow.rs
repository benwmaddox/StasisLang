use std::collections::{BTreeMap, BTreeSet};
use std::hash::{DefaultHasher, Hash, Hasher};

use crate::backend::compile_analysis::{
    collect_supported_call_signatures, resolve_call_signature, CallSignatureMap,
    ResolvedExternCallSignature,
};
use crate::compiler::{FunctionMeta, SourceFile};
use crate::frontend::body_parser::parse_simple_expression;
use crate::frontend::parser::{parse_top_level_extern_functions, parse_top_level_type_layout};
use crate::frontend::types::{
    TypeCategory, TypeId, TypeTable, TypedCollectionDescriptor, TypedCollectionKind, TYPE_ID_BOOL,
    TYPE_ID_F32, TYPE_ID_F64, TYPE_ID_I32, TYPE_ID_VOID,
};
use crate::ir::hir::{
    eval_const_i64, AssignOp, AssignTarget, ComparisonOp, SimpleCondition, SimpleExpr, SimpleStmt,
};

mod effect_contracts;

pub(crate) fn validate_effect_contracts(
    files: &[SourceFile],
    functions: &[FunctionMeta],
    summaries: &[FunctionDataFlowSummary],
) -> Result<(), EffectContractViolation> {
    effect_contracts::validate_effect_contracts(files, functions, summaries)
}

pub const FUNCTION_DATA_FLOW_SCHEMA_VERSION: u32 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EffectContractViolation {
    pub file: String,
    pub source_start: u32,
    pub source_end: u32,
    pub function: String,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FunctionDataFlowEffects {
    pub reads: Vec<String>,
    pub writes: Vec<String>,
    pub parameter_reads: Vec<String>,
    pub parameter_writes: Vec<String>,
    pub calls: Vec<String>,
    pub host_calls: Vec<String>,
    #[serde(default)]
    pub host_effects: Vec<FunctionHostEffect>,
    #[serde(default)]
    pub host_call_costs: Vec<FunctionHostCallCost>,
    pub bounded_iterations: Vec<FunctionBoundedIteration>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct FunctionHostEffect {
    pub function: String,
    pub capability: String,
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
    pub(crate) internal_function_id: u32,
    #[serde(skip)]
    internal_signature_hash: u64,
    #[serde(skip)]
    pub(crate) parameter_storage_kinds: Vec<ParameterStorageKind>,
    #[serde(skip)]
    internal_call_sites: Vec<CallSite>,
    #[serde(skip)]
    internal_direct_write_paths: Vec<String>,
    #[serde(skip)]
    internal_aggregate_write_paths: Vec<String>,
}

impl FunctionDataFlowSummary {
    pub(crate) fn resolved_callee_storage_indices(&self) -> impl Iterator<Item = u32> + '_ {
        self.internal_call_sites
            .iter()
            .map(|call_site| call_site.target_id)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub(crate) enum ParameterStorageKind {
    #[default]
    Dynamic,
    Aos,
    Soa,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompilerLocalType {
    pub file: String,
    pub function: String,
    pub name: String,
    pub type_name: String,
    pub inferred: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct EffectSets {
    reads: BTreeSet<String>,
    writes: BTreeSet<String>,
    parameter_reads: BTreeSet<String>,
    parameter_writes: BTreeSet<String>,
    calls: BTreeSet<String>,
    host_calls: BTreeSet<String>,
    host_effects: BTreeSet<FunctionHostEffect>,
    host_call_costs: BTreeMap<String, Option<u64>>,
    bounded_iterations: BTreeSet<FunctionBoundedIteration>,
    call_sites: Vec<CallSite>,
    iteration_scans: BTreeMap<u32, Vec<String>>,
    iteration_products: BTreeMap<u32, Option<u64>>,
    active_iterations: Vec<u32>,
    next_iteration_id: u32,
    local_types: Vec<(String, TypeId, bool)>,
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
            host_effects: self.host_effects.iter().cloned().collect(),
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
        if !path.contains("[*]") || path.ends_with(".length") || path.ends_with(".max_length") {
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
    extern_effects: BTreeMap<String, Option<Vec<String>>>,
    internal_function_targets: BTreeMap<String, Vec<u32>>,
    collection_capacities: BTreeMap<String, u64>,
    path_types: BTreeMap<String, TypeId>,
    field_types: BTreeMap<TypeId, BTreeMap<String, TypeId>>,
    typed_collection_descriptors: BTreeMap<String, TypedCollectionDescriptor>,
    call_signatures: CallSignatureMap,
    fingerprint: u64,
    types: &'a TypeTable,
}

pub(crate) fn validate_program_semantics(
    files: &[SourceFile],
    functions: &[FunctionMeta],
    statements_by_id: &[Vec<SimpleStmt>],
    types: &TypeTable,
) -> Result<(), (u32, String)> {
    // Resolve storage types before checking properties, including globals whose
    // types never appear in a function signature.
    let mut semantic_types = types.clone();
    let constants = crate::backend::compile_analysis::collect_top_level_constant_values(
        files,
        &mut semantic_types,
    )
    .map_err(|message| (0, message))?;
    let paths = crate::backend::compile_analysis::collect_global_path_types(
        files,
        &mut semantic_types,
        &constants,
    )
    .map_err(|message| (0, message))?;
    let fields = crate::backend::compile_analysis::collect_named_struct_field_types(
        files,
        &mut semantic_types,
        &constants,
    )
    .map_err(|message| (0, message))?;
    let mut context =
        build_context(files, functions, &semantic_types).map_err(|message| (0, message))?;
    context.path_types.extend(paths);
    for (owner, fields) in fields {
        context.field_types.entry(owner).or_default().extend(fields);
    }
    for function in functions {
        if semantic_types.is_typed_collection_type(function.return_type) {
            return Err((
                function.storage_index,
                format!(
                    "function '{}' cannot return a typed collection value",
                    function.name
                ),
            ));
        }
        if let Some((index, _)) = function
            .params
            .iter()
            .enumerate()
            .find(|(_, type_id)| semantic_types.is_typed_collection_type(**type_id))
        {
            return Err((
                function.storage_index,
                format!(
                    "function '{}' cannot accept typed collection parameter {}",
                    function.name, index
                ),
            ));
        }
        let statements = statements_by_id
            .get(function.storage_index as usize)
            .ok_or_else(|| {
                (
                    function.storage_index,
                    format!("function '{}' has no statement artifact", function.name),
                )
            })?;
        let mut local_types = function
            .param_names
            .iter()
            .cloned()
            .zip(function.params.iter().copied())
            .collect();
        validate_statements(
            statements,
            function.return_type,
            &context,
            &mut local_types,
            0,
        )
        .map_err(|message| (function.storage_index, message))?;
    }
    validate_requires_contracts(files, functions, statements_by_id, &context)
        .map_err(|(storage_index, message)| (storage_index, message))?;
    Ok(())
}

fn validate_statements(
    statements: &[SimpleStmt],
    return_type: TypeId,
    context: &AnalysisContext<'_>,
    local_types: &mut BTreeMap<String, TypeId>,
    loop_depth: usize,
) -> Result<(), String> {
    for statement in statements {
        match statement {
            SimpleStmt::Noop => {}
            SimpleStmt::Let {
                name,
                type_id,
                expression,
            } => {
                if local_types.contains_key(name) {
                    return Err(format!("let binding '{name}' shadows existing variable"));
                }
                validate_expression_access(expression, context, local_types)?;
                if type_id.is_some_and(|type_id| context.types.is_typed_collection_type(type_id)) {
                    return Err(format!(
                        "let binding '{name}' cannot store a typed collection value"
                    ));
                }
                let expression_type = semantic_expression_type_with_expected(
                    expression,
                    *type_id,
                    context,
                    local_types,
                );
                if expression_type
                    .is_some_and(|type_id| context.types.is_typed_collection_type(type_id))
                {
                    return Err(format!(
                        "let binding '{name}' cannot store a typed collection value"
                    ));
                }
                if let (Some(expected), Some(found)) = (type_id, expression_type) {
                    if !assignment_types_compatible(*expected, found, context.types) {
                        return Err(type_mismatch(
                            &format!("let binding '{name}'"),
                            *expected,
                            found,
                            context.types,
                        ));
                    }
                }
                if let Some(binding_type) = (*type_id).or(expression_type) {
                    local_types.insert(name.clone(), binding_type);
                }
            }
            SimpleStmt::Assign {
                target, expression, ..
            } => {
                validate_expression_access(expression, context, local_types)?;
                validate_assignment_target_access(target, context, local_types)?;
                if let Some(target_type) =
                    semantic_assignment_target_type(target, context, local_types)
                {
                    if context.types.is_typed_collection_type(target_type) {
                        return Err(
                            "typed collection values cannot be assignment targets".to_string()
                        );
                    }
                    if let Some(expression_type) = semantic_expression_type_with_expected(
                        expression,
                        Some(target_type),
                        context,
                        local_types,
                    ) {
                        if context.types.is_typed_collection_type(expression_type) {
                            return Err(
                                "typed collection values cannot be used in assignments".to_string()
                            );
                        }
                        if !assignment_types_compatible(target_type, expression_type, context.types)
                        {
                            return Err(type_mismatch(
                                "assignment",
                                target_type,
                                expression_type,
                                context.types,
                            ));
                        }
                    }
                }
            }
            SimpleStmt::Convert {
                target,
                kind,
                source,
            } => {
                validate_expression_access(source, context, local_types)?;
                validate_assignment_target_access(target, context, local_types)?;
                let (required_source, allowed_targets, name) = match kind {
                    crate::ir::hir::ConversionKind::FromI32 => {
                        (TYPE_ID_I32, [TYPE_ID_F32, TYPE_ID_F64], "from_i32")
                    }
                    crate::ir::hir::ConversionKind::FromF32 => {
                        (TYPE_ID_F32, [TYPE_ID_I32, TYPE_ID_F64], "from_f32")
                    }
                    crate::ir::hir::ConversionKind::FromF64 => {
                        (TYPE_ID_F64, [TYPE_ID_I32, TYPE_ID_F32], "from_f64")
                    }
                };
                if let (Some(target_type), Some(source_type)) = (
                    semantic_assignment_target_type(target, context, local_types),
                    semantic_expression_type_with_expected(
                        source,
                        Some(required_source),
                        context,
                        local_types,
                    ),
                ) {
                    if source_type != required_source {
                        return Err(format!(
                            "{name} source expression must be {}",
                            type_name(required_source, context.types)
                        ));
                    }
                    if !allowed_targets.contains(&target_type) {
                        return Err(format!(
                            "{name} target must be {} or {}",
                            type_name(allowed_targets[0], context.types),
                            type_name(allowed_targets[1], context.types)
                        ));
                    }
                }
            }
            SimpleStmt::If {
                condition,
                then_statements,
                else_statements,
            } => {
                validate_condition_access(condition, context, local_types)?;
                validate_condition(condition, context, local_types)?;
                let mut then_locals = local_types.clone();
                validate_statements(
                    then_statements,
                    return_type,
                    context,
                    &mut then_locals,
                    loop_depth,
                )?;
                if let Some(else_statements) = else_statements {
                    let mut else_locals = local_types.clone();
                    validate_statements(
                        else_statements,
                        return_type,
                        context,
                        &mut else_locals,
                        loop_depth,
                    )?;
                }
            }
            SimpleStmt::For {
                init,
                condition,
                step,
                body_statements,
            } => {
                let mut loop_locals = local_types.clone();
                validate_statements(
                    std::slice::from_ref(init.as_ref()),
                    return_type,
                    context,
                    &mut loop_locals,
                    loop_depth + 1,
                )?;
                validate_condition_access(condition, context, &loop_locals)?;
                validate_condition(condition, context, &loop_locals)?;
                let mut body_locals = loop_locals.clone();
                validate_statements(
                    body_statements,
                    return_type,
                    context,
                    &mut body_locals,
                    loop_depth + 1,
                )?;
                validate_statements(
                    std::slice::from_ref(step.as_ref()),
                    return_type,
                    context,
                    &mut loop_locals,
                    loop_depth + 1,
                )?;
            }
            SimpleStmt::Foreach {
                item_name,
                index_name,
                collection_path,
                body_statements,
            } => {
                if is_typed_collection_path_or_descendant(collection_path, context, local_types) {
                    return Err(
                        "typed collection paths may only be used as the first argument of a compiler-owned typed collection operation"
                            .to_string(),
                    );
                }
                let mut loop_locals = local_types.clone();
                if loop_locals.contains_key(item_name) {
                    return Err(format!(
                        "foreach item binding '{item_name}' shadows existing variable"
                    ));
                }
                if let Some(element_type) =
                    path_type(collection_path, context, local_types, &BTreeMap::new())
                        .and_then(|collection| context.types.indexed_element_type_id(collection))
                {
                    loop_locals.insert(item_name.clone(), element_type);
                }
                if let Some(index_name) = index_name {
                    if loop_locals.contains_key(index_name) || index_name == item_name {
                        return Err(format!(
                            "foreach index binding '{index_name}' shadows existing variable"
                        ));
                    }
                    loop_locals.insert(index_name.clone(), TYPE_ID_I32);
                }
                validate_statements(
                    body_statements,
                    return_type,
                    context,
                    &mut loop_locals,
                    loop_depth + 1,
                )?;
            }
            SimpleStmt::Expr(expression) => {
                validate_expression_access(expression, context, local_types)?
            }
            SimpleStmt::Continue if loop_depth == 0 => {
                return Err("continue statement is only valid inside loops".to_string());
            }
            SimpleStmt::Continue => {}
            SimpleStmt::Return(expression) => {
                validate_expression_access(expression, context, local_types)?;
                if let Some(expression_type) = semantic_expression_type_with_expected(
                    expression,
                    Some(return_type),
                    context,
                    local_types,
                ) {
                    if context.types.is_typed_collection_type(expression_type) {
                        return Err(
                            "typed collection values cannot be returned from expressions"
                                .to_string(),
                        );
                    }
                    if !assignment_types_compatible(return_type, expression_type, context.types) {
                        return Err(type_mismatch(
                            "return expression",
                            return_type,
                            expression_type,
                            context.types,
                        ));
                    }
                }
            }
            SimpleStmt::ReturnVoid if return_type != TYPE_ID_VOID => {
                return Err(format!(
                    "return statement expected {} expression",
                    type_name(return_type, context.types)
                ));
            }
            SimpleStmt::ReturnVoid => {}
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
struct CollectionGuardProof {
    action: TypedCollectionOperation,
    receiver: SimpleExpr,
    key_args: Vec<SimpleExpr>,
}

#[derive(Debug, Clone, Default)]
struct CollectionGuardState {
    proofs: Vec<CollectionGuardProof>,
}

#[derive(Debug, Clone)]
struct RequiredFunctionContract {
    proof: CollectionGuardProof,
    param_names: Vec<String>,
}

impl CollectionGuardState {
    fn add(&mut self, proof: CollectionGuardProof) {
        if !self
            .proofs
            .iter()
            .any(|existing| collection_guard_proofs_match(existing, &proof))
        {
            self.proofs.push(proof);
        }
    }

    fn consume(&mut self, proof: &CollectionGuardProof) -> bool {
        let Some(index) = self
            .proofs
            .iter()
            .position(|existing| collection_guard_proofs_match(existing, proof))
        else {
            return false;
        };
        self.proofs.remove(index);
        true
    }

    fn invalidate(&mut self) {
        self.proofs.clear();
    }
}

fn collection_guard_proofs_match(
    available: &CollectionGuardProof,
    required: &CollectionGuardProof,
) -> bool {
    available.receiver == required.receiver
        && available.key_args == required.key_args
        && (available.action == required.action
            || matches!(
                (available.action, required.action),
                (
                    TypedCollectionOperation::GridGet,
                    TypedCollectionOperation::GridSet
                ) | (
                    TypedCollectionOperation::GridSet,
                    TypedCollectionOperation::GridGet
                ) | (
                    TypedCollectionOperation::BitsetTest,
                    TypedCollectionOperation::BitsetSet
                ) | (
                    TypedCollectionOperation::BitsetSet,
                    TypedCollectionOperation::BitsetTest
                ) | (
                    TypedCollectionOperation::QueuePeek,
                    TypedCollectionOperation::QueuePhysicalIndex
                ) | (
                    TypedCollectionOperation::QueuePhysicalIndex,
                    TypedCollectionOperation::QueuePeek
                ) | (
                    TypedCollectionOperation::RingBufferPeek,
                    TypedCollectionOperation::RingBufferPhysicalIndex
                ) | (
                    TypedCollectionOperation::RingBufferPhysicalIndex,
                    TypedCollectionOperation::RingBufferPeek
                ) | (
                    TypedCollectionOperation::PriorityQueuePeek,
                    TypedCollectionOperation::PriorityQueuePeekPriority
                ) | (
                    TypedCollectionOperation::PriorityQueuePeekPriority,
                    TypedCollectionOperation::PriorityQueuePeek
                )
            ))
}

fn validate_requires_contracts(
    files: &[SourceFile],
    functions: &[FunctionMeta],
    statements_by_id: &[Vec<SimpleStmt>],
    context: &AnalysisContext<'_>,
) -> Result<(), (u32, String)> {
    let mut required_proofs: BTreeMap<u32, RequiredFunctionContract> = BTreeMap::new();
    for function in functions {
        let Some(requirements) = &function.requires_contract else {
            continue;
        };
        let proof = parse_requires_proof(function, requirements, context)
            .map_err(|message| (function.storage_index, message))?;
        required_proofs.insert(
            function.storage_index,
            RequiredFunctionContract {
                proof,
                param_names: function.param_names.clone(),
            },
        );
    }

    let (callers, call_edges) = collect_internal_call_graph(functions, statements_by_id, context);
    for function in functions {
        let Some(contract) = required_proofs.get(&function.storage_index) else {
            continue;
        };
        validate_required_function_shape(
            files,
            function,
            statements_by_id,
            context,
            &contract.proof,
            callers.get(function.storage_index as usize),
            &call_edges,
        )
        .map_err(|message| (function.storage_index, message))?;
    }

    for function in functions {
        let statements = statements_by_id
            .get(function.storage_index as usize)
            .ok_or_else(|| {
                (
                    function.storage_index,
                    format!("function '{}' has no statement artifact", function.name),
                )
            })?;
        let mut local_types = function
            .param_names
            .iter()
            .cloned()
            .zip(function.params.iter().copied())
            .collect();
        let mut state = CollectionGuardState::default();
        // A required helper's single action is the expansion target.  Its
        // caller owns the proof; checking the action here would incorrectly
        // require a second guard inside the helper body.
        if required_proofs.contains_key(&function.storage_index) {
            continue;
        }
        validate_guarded_statements(
            statements,
            context,
            &required_proofs,
            &mut local_types,
            &mut state,
        )
        .map_err(|message| (function.storage_index, message))?;
    }
    Ok(())
}

fn parse_requires_proof(
    function: &FunctionMeta,
    requirements: &[String],
    context: &AnalysisContext<'_>,
) -> Result<CollectionGuardProof, String> {
    if requirements.len() != 1 {
        return Err(format!(
            "function '{}' @requires must contain exactly one direct collection can_* expression",
            function.name
        ));
    }
    let source = requirements[0].trim();
    let expression = parse_simple_expression(source).map_err(|error| {
        format!(
            "function '{}' has malformed @requires expression '{}': {error}",
            function.name, requirements[0]
        )
    })?;
    let SimpleExpr::Call { target, args } = &expression else {
        return Err(format!(
            "function '{}' @requires must be a direct receiver can_* call, for example @requires(events.can_push())",
            function.name
        ));
    };
    let local_types: BTreeMap<_, _> = function
        .param_names
        .iter()
        .cloned()
        .zip(function.params.iter().copied())
        .collect();
    let operation = typed_collection_operation(target, args, context, &local_types)
        .map_err(|error| format!("function '{}' @requires is invalid: {error}", function.name))?
        .ok_or_else(|| {
            format!(
                "function '{}' @requires must be a direct receiver can_* call, got '{}'",
                function.name, source
            )
        })?;
    if !is_can_operation(operation) {
        return Err(format!(
            "function '{}' @requires must name can_push, can_insert, can_put, can_get, can_remove, can_pop, can_peek, can_add, or can_access; '{}' is not a preflight",
            function.name, target
        ));
    }
    guard_proof_for_can(operation, args).ok_or_else(|| {
        format!(
            "function '{}' @requires has an invalid receiver or key/index argument list",
            function.name
        )
    })
}

fn validate_required_function_shape(
    files: &[SourceFile],
    function: &FunctionMeta,
    statements_by_id: &[Vec<SimpleStmt>],
    context: &AnalysisContext<'_>,
    required_proof: &CollectionGuardProof,
    callers: Option<&BTreeSet<u32>>,
    call_edges: &[BTreeSet<u32>],
) -> Result<(), String> {
    if function_is_extern(function, files) {
        return Err(format!(
            "function '{}' with @requires must be internal; extern/exported functions cannot be expanded",
            function.name
        ));
    }
    if function_is_runtime_root(function) || callers.is_none_or(BTreeSet::is_empty) {
        return Err(format!(
            "function '{}' with @requires must be an internal non-root helper with a caller",
            function.name
        ));
    }
    if context
        .internal_function_targets
        .get(&function.name)
        .is_none_or(|targets| targets.len() != 1)
    {
        return Err(format!(
            "function '{}' with @requires must be uniquely callable (overloaded helpers are not eligible for mandatory expansion)",
            function.name
        ));
    }
    if function_reaches_itself(function.storage_index, call_edges) {
        return Err(format!(
            "function '{}' with @requires cannot be recursive",
            function.name
        ));
    }

    let statements = statements_by_id
        .get(function.storage_index as usize)
        .ok_or_else(|| {
            format!(
                "function '{}' with @requires has no statement artifact",
                function.name
            )
        })?;
    let action_expression = match statements.as_slice() {
        [SimpleStmt::Expr(expression)] | [SimpleStmt::Return(expression)] => expression,
        _ => {
            return Err(format!(
                "function '{}' with @requires must have exactly one direct collection action statement (Expr or Return)",
                function.name
            ));
        }
    };
    let SimpleExpr::Call { target, args } = action_expression else {
        return Err(format!(
            "function '{}' with @requires must have exactly one direct collection action call",
            function.name
        ));
    };
    if args
        .iter()
        .any(|argument| !safe_requires_expansion_argument(argument))
    {
        return Err(format!(
            "function '{}' with @requires must use only side-effect-free action arguments; nested calls could invalidate the caller-owned proof",
            function.name
        ));
    }
    let local_types: BTreeMap<_, _> = function
        .param_names
        .iter()
        .cloned()
        .zip(function.params.iter().copied())
        .collect();
    let operation = typed_collection_operation(target, args, context, &local_types)
        .map_err(|error| {
            format!(
                "function '{}' @requires action is invalid: {error}",
                function.name
            )
        })?
        .ok_or_else(|| {
            format!(
                "function '{}' with @requires must contain a compiler-owned collection action",
                function.name
            )
        })?;
    let Some(action_proof) = guard_proof_for_action(operation, args) else {
        return Err(format!(
            "function '{}' @requires action '{}' is not a guarded collection action with a matching can_* preflight",
            function.name, target
        ));
    };
    if !collection_guard_proofs_match(&action_proof, required_proof) {
        return Err(format!(
            "function '{}' @requires preflight does not exactly match its action receiver and key/index arguments",
            function.name
        ));
    }
    Ok(())
}

fn function_is_extern(function: &FunctionMeta, files: &[SourceFile]) -> bool {
    files
        .get(function.file_id as usize)
        .and_then(|file| {
            file.content
                .get(function.signature_range.start as usize..function.signature_range.end as usize)
        })
        .is_some_and(|signature| {
            signature.contains("@extern") || signature.trim_start().starts_with("extern ")
        })
}

fn function_is_runtime_root(function: &FunctionMeta) -> bool {
    matches!(
        function.name.as_str(),
        "main"
            | "render"
            | "on_code_swap"
            | "gfx_cmd_construction_reset"
            | "gfx_cmd_construction_finish"
    ) || (function.name == "tick" && function.params.is_empty())
}

fn function_reaches_itself(function_id: u32, call_edges: &[BTreeSet<u32>]) -> bool {
    fn visit(
        current: u32,
        target: u32,
        edges: &[BTreeSet<u32>],
        visited: &mut BTreeSet<u32>,
    ) -> bool {
        if !visited.insert(current) {
            return false;
        }
        edges
            .get(current as usize)
            .into_iter()
            .flatten()
            .any(|callee| *callee == target || visit(*callee, target, edges, visited))
    }
    visit(function_id, function_id, call_edges, &mut BTreeSet::new())
}

fn collect_internal_call_graph(
    functions: &[FunctionMeta],
    statements_by_id: &[Vec<SimpleStmt>],
    context: &AnalysisContext<'_>,
) -> (Vec<BTreeSet<u32>>, Vec<BTreeSet<u32>>) {
    let mut edges = vec![BTreeSet::new(); functions.len()];
    let mut callers = vec![BTreeSet::new(); functions.len()];
    for function in functions {
        let Some(statements) = statements_by_id.get(function.storage_index as usize) else {
            continue;
        };
        let mut local_types = function
            .param_names
            .iter()
            .cloned()
            .zip(function.params.iter().copied())
            .collect();
        collect_internal_calls_in_statements(
            statements,
            function.storage_index,
            context,
            &mut local_types,
            &BTreeMap::new(),
            &mut edges,
            &mut callers,
        );
    }
    (callers, edges)
}

fn collect_internal_calls_in_statements(
    statements: &[SimpleStmt],
    caller: u32,
    context: &AnalysisContext<'_>,
    local_types: &mut BTreeMap<String, TypeId>,
    aliases: &BTreeMap<String, String>,
    edges: &mut [BTreeSet<u32>],
    callers: &mut [BTreeSet<u32>],
) {
    for statement in statements {
        match statement {
            SimpleStmt::Noop | SimpleStmt::Continue | SimpleStmt::ReturnVoid => {}
            SimpleStmt::Let {
                name,
                type_id,
                expression,
            } => {
                collect_internal_calls_in_expression(
                    expression,
                    caller,
                    context,
                    local_types,
                    aliases,
                    edges,
                    callers,
                );
                let inferred =
                    type_id.or_else(|| expression_type(expression, context, local_types, aliases));
                if let Some(type_id) = inferred {
                    local_types.insert(name.clone(), type_id);
                }
            }
            SimpleStmt::Assign {
                target, expression, ..
            } => {
                collect_internal_calls_in_expression(
                    expression,
                    caller,
                    context,
                    local_types,
                    aliases,
                    edges,
                    callers,
                );
                if let AssignTarget::IndexedPath { index, .. } = target {
                    collect_internal_calls_in_expression(
                        index,
                        caller,
                        context,
                        local_types,
                        aliases,
                        edges,
                        callers,
                    );
                }
            }
            SimpleStmt::Convert { target, source, .. } => {
                collect_internal_calls_in_expression(
                    source,
                    caller,
                    context,
                    local_types,
                    aliases,
                    edges,
                    callers,
                );
                if let AssignTarget::IndexedPath { index, .. } = target {
                    collect_internal_calls_in_expression(
                        index,
                        caller,
                        context,
                        local_types,
                        aliases,
                        edges,
                        callers,
                    );
                }
            }
            SimpleStmt::If {
                condition,
                then_statements,
                else_statements,
            } => {
                collect_internal_calls_in_condition(
                    condition,
                    caller,
                    context,
                    local_types,
                    aliases,
                    edges,
                    callers,
                );
                let mut then_types = local_types.clone();
                collect_internal_calls_in_statements(
                    then_statements,
                    caller,
                    context,
                    &mut then_types,
                    aliases,
                    edges,
                    callers,
                );
                if let Some(else_statements) = else_statements {
                    let mut else_types = local_types.clone();
                    collect_internal_calls_in_statements(
                        else_statements,
                        caller,
                        context,
                        &mut else_types,
                        aliases,
                        edges,
                        callers,
                    );
                }
            }
            SimpleStmt::For {
                init,
                condition,
                step,
                body_statements,
            } => {
                let mut loop_types = local_types.clone();
                collect_internal_calls_in_statements(
                    std::slice::from_ref(init.as_ref()),
                    caller,
                    context,
                    &mut loop_types,
                    aliases,
                    edges,
                    callers,
                );
                collect_internal_calls_in_condition(
                    condition,
                    caller,
                    context,
                    &loop_types,
                    aliases,
                    edges,
                    callers,
                );
                let mut body_types = loop_types.clone();
                collect_internal_calls_in_statements(
                    body_statements,
                    caller,
                    context,
                    &mut body_types,
                    aliases,
                    edges,
                    callers,
                );
                collect_internal_calls_in_statements(
                    std::slice::from_ref(step.as_ref()),
                    caller,
                    context,
                    &mut loop_types,
                    aliases,
                    edges,
                    callers,
                );
            }
            SimpleStmt::Foreach {
                item_name,
                index_name,
                collection_path,
                body_statements,
            } => {
                let mut body_types = local_types.clone();
                if let Some(element_type) =
                    path_type(collection_path, context, local_types, aliases)
                        .and_then(|collection| context.types.indexed_element_type_id(collection))
                {
                    body_types.insert(item_name.clone(), element_type);
                }
                if let Some(index_name) = index_name {
                    body_types.insert(index_name.clone(), TYPE_ID_I32);
                }
                collect_internal_calls_in_statements(
                    body_statements,
                    caller,
                    context,
                    &mut body_types,
                    aliases,
                    edges,
                    callers,
                );
            }
            SimpleStmt::Expr(expression) | SimpleStmt::Return(expression) => {
                collect_internal_calls_in_expression(
                    expression,
                    caller,
                    context,
                    local_types,
                    aliases,
                    edges,
                    callers,
                );
            }
        }
    }
}

fn collect_internal_calls_in_condition(
    condition: &SimpleCondition,
    caller: u32,
    context: &AnalysisContext<'_>,
    local_types: &BTreeMap<String, TypeId>,
    aliases: &BTreeMap<String, String>,
    edges: &mut [BTreeSet<u32>],
    callers: &mut [BTreeSet<u32>],
) {
    match condition {
        SimpleCondition::Comparison { lhs, rhs, .. } => {
            collect_internal_calls_in_expression(
                lhs,
                caller,
                context,
                local_types,
                aliases,
                edges,
                callers,
            );
            collect_internal_calls_in_expression(
                rhs,
                caller,
                context,
                local_types,
                aliases,
                edges,
                callers,
            );
        }
        SimpleCondition::Expr(expression) => collect_internal_calls_in_expression(
            expression,
            caller,
            context,
            local_types,
            aliases,
            edges,
            callers,
        ),
        SimpleCondition::And(lhs, rhs) | SimpleCondition::Or(lhs, rhs) => {
            collect_internal_calls_in_condition(
                lhs,
                caller,
                context,
                local_types,
                aliases,
                edges,
                callers,
            );
            collect_internal_calls_in_condition(
                rhs,
                caller,
                context,
                local_types,
                aliases,
                edges,
                callers,
            );
        }
        SimpleCondition::Not(inner) => collect_internal_calls_in_condition(
            inner,
            caller,
            context,
            local_types,
            aliases,
            edges,
            callers,
        ),
    }
}

fn collect_internal_calls_in_expression(
    expression: &SimpleExpr,
    caller: u32,
    context: &AnalysisContext<'_>,
    local_types: &BTreeMap<String, TypeId>,
    aliases: &BTreeMap<String, String>,
    edges: &mut [BTreeSet<u32>],
    callers: &mut [BTreeSet<u32>],
) {
    match expression {
        SimpleExpr::Condition(condition) => collect_internal_calls_in_condition(
            condition,
            caller,
            context,
            local_types,
            aliases,
            edges,
            callers,
        ),
        SimpleExpr::IndexedPath { index, .. } => collect_internal_calls_in_expression(
            index,
            caller,
            context,
            local_types,
            aliases,
            edges,
            callers,
        ),
        SimpleExpr::Call { target, args } => {
            if typed_collection_operation_for_call(target, args, context, local_types).is_none() {
                if let Some(target_id) =
                    resolve_internal_call(target, args, context, local_types, aliases)
                {
                    if let Some(callees) = edges.get_mut(caller as usize) {
                        callees.insert(target_id);
                    }
                    if let Some(target_callers) = callers.get_mut(target_id as usize) {
                        target_callers.insert(caller);
                    }
                }
            }
            for argument in args {
                collect_internal_calls_in_expression(
                    argument,
                    caller,
                    context,
                    local_types,
                    aliases,
                    edges,
                    callers,
                );
            }
        }
        SimpleExpr::Binary { lhs, rhs, .. } => {
            collect_internal_calls_in_expression(
                lhs,
                caller,
                context,
                local_types,
                aliases,
                edges,
                callers,
            );
            collect_internal_calls_in_expression(
                rhs,
                caller,
                context,
                local_types,
                aliases,
                edges,
                callers,
            );
        }
        SimpleExpr::DefaultValue(_)
        | SimpleExpr::Int(_)
        | SimpleExpr::Float(_)
        | SimpleExpr::Bool(_)
        | SimpleExpr::StringLiteral(_)
        | SimpleExpr::Identifier(_) => {}
    }
}

fn is_can_operation(operation: TypedCollectionOperation) -> bool {
    matches!(
        operation,
        TypedCollectionOperation::PoolCanPush
            | TypedCollectionOperation::PoolCanRemove
            | TypedCollectionOperation::StablePoolCanInsert
            | TypedCollectionOperation::StablePoolCanRemove
            | TypedCollectionOperation::MapCanPut
            | TypedCollectionOperation::MapCanGet
            | TypedCollectionOperation::MapCanRemove
            | TypedCollectionOperation::SetCanAdd
            | TypedCollectionOperation::SetCanRemove
            | TypedCollectionOperation::QueueCanPush
            | TypedCollectionOperation::QueueCanPop
            | TypedCollectionOperation::QueueCanPeek
            | TypedCollectionOperation::RingBufferCanPush
            | TypedCollectionOperation::RingBufferCanPop
            | TypedCollectionOperation::RingBufferCanPeek
            | TypedCollectionOperation::PriorityQueueCanPush
            | TypedCollectionOperation::PriorityQueueCanPop
            | TypedCollectionOperation::PriorityQueueCanPeek
            | TypedCollectionOperation::GridCanAccess
            | TypedCollectionOperation::BitsetCanAccess
    )
}

fn is_overwrite_oldest_operation(operation: TypedCollectionOperation) -> bool {
    matches!(
        operation,
        TypedCollectionOperation::QueueOverwriteOldest
            | TypedCollectionOperation::RingBufferOverwriteOldest
    )
}

fn can_operation_for_action(
    operation: TypedCollectionOperation,
) -> Option<TypedCollectionOperation> {
    Some(match operation {
        TypedCollectionOperation::PoolPush => TypedCollectionOperation::PoolCanPush,
        TypedCollectionOperation::PoolRemove => TypedCollectionOperation::PoolCanRemove,
        TypedCollectionOperation::StablePoolInsert => TypedCollectionOperation::StablePoolCanInsert,
        TypedCollectionOperation::StablePoolRemove => TypedCollectionOperation::StablePoolCanRemove,
        TypedCollectionOperation::MapPut => TypedCollectionOperation::MapCanPut,
        TypedCollectionOperation::MapGet => TypedCollectionOperation::MapCanGet,
        TypedCollectionOperation::MapRemove => TypedCollectionOperation::MapCanRemove,
        TypedCollectionOperation::SetAdd => TypedCollectionOperation::SetCanAdd,
        TypedCollectionOperation::SetRemove => TypedCollectionOperation::SetCanRemove,
        TypedCollectionOperation::QueuePush => TypedCollectionOperation::QueueCanPush,
        TypedCollectionOperation::QueuePop => TypedCollectionOperation::QueueCanPop,
        TypedCollectionOperation::QueuePeek => TypedCollectionOperation::QueueCanPeek,
        TypedCollectionOperation::QueuePhysicalIndex => TypedCollectionOperation::QueueCanPeek,
        TypedCollectionOperation::RingBufferPush => TypedCollectionOperation::RingBufferCanPush,
        TypedCollectionOperation::RingBufferPop => TypedCollectionOperation::RingBufferCanPop,
        TypedCollectionOperation::RingBufferPeek => TypedCollectionOperation::RingBufferCanPeek,
        TypedCollectionOperation::RingBufferPhysicalIndex => {
            TypedCollectionOperation::RingBufferCanPeek
        }
        TypedCollectionOperation::PriorityQueuePush => {
            TypedCollectionOperation::PriorityQueueCanPush
        }
        TypedCollectionOperation::PriorityQueuePop => TypedCollectionOperation::PriorityQueueCanPop,
        TypedCollectionOperation::PriorityQueuePeek => {
            TypedCollectionOperation::PriorityQueueCanPeek
        }
        TypedCollectionOperation::PriorityQueuePeekPriority => {
            TypedCollectionOperation::PriorityQueueCanPeek
        }
        TypedCollectionOperation::GridGet | TypedCollectionOperation::GridSet => {
            TypedCollectionOperation::GridCanAccess
        }
        TypedCollectionOperation::BitsetTest | TypedCollectionOperation::BitsetSet => {
            TypedCollectionOperation::BitsetCanAccess
        }
        _ => return None,
    })
}

fn guard_key_argument_indices(operation: TypedCollectionOperation) -> &'static [usize] {
    match operation {
        TypedCollectionOperation::PoolRemove
        | TypedCollectionOperation::StablePoolRemove
        | TypedCollectionOperation::MapPut
        | TypedCollectionOperation::MapGet
        | TypedCollectionOperation::MapRemove
        | TypedCollectionOperation::SetAdd
        | TypedCollectionOperation::SetRemove
        | TypedCollectionOperation::QueuePeek
        | TypedCollectionOperation::QueuePhysicalIndex
        | TypedCollectionOperation::RingBufferPeek
        | TypedCollectionOperation::RingBufferPhysicalIndex
        | TypedCollectionOperation::BitsetTest
        | TypedCollectionOperation::BitsetSet => &[1],
        TypedCollectionOperation::PoolPush
        | TypedCollectionOperation::StablePoolInsert
        | TypedCollectionOperation::QueuePush
        | TypedCollectionOperation::QueuePop
        | TypedCollectionOperation::RingBufferPush
        | TypedCollectionOperation::RingBufferPop
        | TypedCollectionOperation::PriorityQueuePush
        | TypedCollectionOperation::PriorityQueuePop
        | TypedCollectionOperation::PriorityQueuePeek
        | TypedCollectionOperation::PriorityQueuePeekPriority => &[],
        TypedCollectionOperation::GridGet | TypedCollectionOperation::GridSet => &[1, 2],
        _ => &[],
    }
}

fn guard_proof_for_can(
    operation: TypedCollectionOperation,
    args: &[SimpleExpr],
) -> Option<CollectionGuardProof> {
    if !is_can_operation(operation) || args.is_empty() {
        return None;
    }
    let action = match operation {
        TypedCollectionOperation::PoolCanPush => TypedCollectionOperation::PoolPush,
        TypedCollectionOperation::PoolCanRemove => TypedCollectionOperation::PoolRemove,
        TypedCollectionOperation::StablePoolCanInsert => TypedCollectionOperation::StablePoolInsert,
        TypedCollectionOperation::StablePoolCanRemove => TypedCollectionOperation::StablePoolRemove,
        TypedCollectionOperation::MapCanPut => TypedCollectionOperation::MapPut,
        TypedCollectionOperation::MapCanGet => TypedCollectionOperation::MapGet,
        TypedCollectionOperation::MapCanRemove => TypedCollectionOperation::MapRemove,
        TypedCollectionOperation::SetCanAdd => TypedCollectionOperation::SetAdd,
        TypedCollectionOperation::SetCanRemove => TypedCollectionOperation::SetRemove,
        TypedCollectionOperation::QueueCanPush => TypedCollectionOperation::QueuePush,
        TypedCollectionOperation::QueueCanPop => TypedCollectionOperation::QueuePop,
        TypedCollectionOperation::QueueCanPeek => TypedCollectionOperation::QueuePeek,
        TypedCollectionOperation::RingBufferCanPush => TypedCollectionOperation::RingBufferPush,
        TypedCollectionOperation::RingBufferCanPop => TypedCollectionOperation::RingBufferPop,
        TypedCollectionOperation::RingBufferCanPeek => TypedCollectionOperation::RingBufferPeek,
        TypedCollectionOperation::PriorityQueueCanPush => {
            TypedCollectionOperation::PriorityQueuePush
        }
        TypedCollectionOperation::PriorityQueueCanPop => TypedCollectionOperation::PriorityQueuePop,
        TypedCollectionOperation::PriorityQueueCanPeek => {
            TypedCollectionOperation::PriorityQueuePeek
        }
        TypedCollectionOperation::GridCanAccess => TypedCollectionOperation::GridGet,
        TypedCollectionOperation::BitsetCanAccess => TypedCollectionOperation::BitsetTest,
        _ => return None,
    };
    let key_args = args.iter().skip(1).cloned().collect();
    Some(CollectionGuardProof {
        action,
        receiver: args[0].clone(),
        key_args,
    })
}

fn guard_proof_for_action(
    operation: TypedCollectionOperation,
    args: &[SimpleExpr],
) -> Option<CollectionGuardProof> {
    let can_operation = can_operation_for_action(operation)?;
    let receiver = args.first()?.clone();
    let key_args = guard_key_argument_indices(operation)
        .iter()
        .map(|index| args.get(*index).cloned())
        .collect::<Option<Vec<_>>>()?;
    Some(CollectionGuardProof {
        action: operation,
        receiver,
        key_args,
    })
    .filter(|_| is_can_operation(can_operation))
}

fn validate_guarded_statements(
    statements: &[SimpleStmt],
    context: &AnalysisContext<'_>,
    required_proofs: &BTreeMap<u32, RequiredFunctionContract>,
    local_types: &mut BTreeMap<String, TypeId>,
    state: &mut CollectionGuardState,
) -> Result<(), String> {
    for statement in statements {
        match statement {
            SimpleStmt::Noop | SimpleStmt::Continue | SimpleStmt::ReturnVoid => {}
            SimpleStmt::Let {
                name,
                type_id,
                expression,
            } => {
                validate_guarded_expression(
                    expression,
                    context,
                    required_proofs,
                    local_types,
                    state,
                )?;
                let inferred = type_id.or_else(|| {
                    expression_type(expression, context, local_types, &BTreeMap::new())
                });
                if let Some(type_id) = inferred {
                    local_types.insert(name.clone(), type_id);
                }
            }
            SimpleStmt::Assign {
                target, expression, ..
            } => {
                validate_guarded_expression(
                    expression,
                    context,
                    required_proofs,
                    local_types,
                    state,
                )?;
                if let AssignTarget::IndexedPath { index, .. } = target {
                    validate_guarded_expression(
                        index,
                        context,
                        required_proofs,
                        local_types,
                        state,
                    )?;
                }
                state.invalidate();
            }
            SimpleStmt::Convert { target, source, .. } => {
                validate_guarded_expression(source, context, required_proofs, local_types, state)?;
                if let AssignTarget::IndexedPath { index, .. } = target {
                    validate_guarded_expression(
                        index,
                        context,
                        required_proofs,
                        local_types,
                        state,
                    )?;
                }
                state.invalidate();
            }
            SimpleStmt::If {
                condition,
                then_statements,
                else_statements,
            } => {
                let direct_proof = direct_positive_guard(condition, context, local_types)?;
                validate_guarded_condition(
                    condition,
                    context,
                    required_proofs,
                    local_types,
                    state,
                )?;
                let mut then_state = state.clone();
                if let Some(proof) = direct_proof {
                    then_state.add(proof);
                }
                let mut then_types = local_types.clone();
                validate_guarded_statements(
                    then_statements,
                    context,
                    required_proofs,
                    &mut then_types,
                    &mut then_state,
                )?;
                if let Some(else_statements) = else_statements {
                    let mut else_state = state.clone();
                    let mut else_types = local_types.clone();
                    validate_guarded_statements(
                        else_statements,
                        context,
                        required_proofs,
                        &mut else_types,
                        &mut else_state,
                    )?;
                }
                state.invalidate();
            }
            SimpleStmt::For {
                init,
                condition,
                step,
                body_statements,
            } => {
                let mut loop_types = local_types.clone();
                let mut loop_state = CollectionGuardState::default();
                validate_guarded_statements(
                    std::slice::from_ref(init.as_ref()),
                    context,
                    required_proofs,
                    &mut loop_types,
                    &mut loop_state,
                )?;
                validate_guarded_condition(
                    condition,
                    context,
                    required_proofs,
                    &mut loop_types,
                    &mut loop_state,
                )?;
                let mut body_types = loop_types.clone();
                let mut body_state = loop_state.clone();
                validate_guarded_statements(
                    body_statements,
                    context,
                    required_proofs,
                    &mut body_types,
                    &mut body_state,
                )?;
                validate_guarded_statements(
                    std::slice::from_ref(step.as_ref()),
                    context,
                    required_proofs,
                    &mut loop_types,
                    &mut loop_state,
                )?;
                state.invalidate();
            }
            SimpleStmt::Foreach {
                item_name,
                index_name,
                collection_path,
                body_statements,
            } => {
                let mut body_types = local_types.clone();
                if let Some(element_type) =
                    path_type(collection_path, context, local_types, &BTreeMap::new())
                        .and_then(|collection| context.types.indexed_element_type_id(collection))
                {
                    body_types.insert(item_name.clone(), element_type);
                }
                if let Some(index_name) = index_name {
                    body_types.insert(index_name.clone(), TYPE_ID_I32);
                }
                let mut body_state = CollectionGuardState::default();
                validate_guarded_statements(
                    body_statements,
                    context,
                    required_proofs,
                    &mut body_types,
                    &mut body_state,
                )?;
                state.invalidate();
            }
            SimpleStmt::Expr(expression) | SimpleStmt::Return(expression) => {
                validate_guarded_expression(
                    expression,
                    context,
                    required_proofs,
                    local_types,
                    state,
                )?;
            }
        }
    }
    Ok(())
}

fn direct_positive_guard(
    condition: &SimpleCondition,
    context: &AnalysisContext<'_>,
    local_types: &BTreeMap<String, TypeId>,
) -> Result<Option<CollectionGuardProof>, String> {
    let SimpleCondition::Expr(SimpleExpr::Call { target, args }) = condition else {
        return Ok(None);
    };
    let Some(operation) = typed_collection_operation_for_call(target, args, context, local_types)
    else {
        return Ok(None);
    };
    if !is_can_operation(operation) {
        return Ok(None);
    }
    let operation = typed_collection_operation(target, args, context, local_types)?
        .ok_or_else(|| format!("invalid collection guard '{}(...)'", target))?;
    Ok(guard_proof_for_can(operation, args))
}

fn validate_guarded_condition(
    condition: &SimpleCondition,
    context: &AnalysisContext<'_>,
    required_proofs: &BTreeMap<u32, RequiredFunctionContract>,
    local_types: &BTreeMap<String, TypeId>,
    state: &mut CollectionGuardState,
) -> Result<(), String> {
    match condition {
        SimpleCondition::Comparison { lhs, rhs, .. } => {
            validate_guarded_expression(lhs, context, required_proofs, local_types, state)?;
            validate_guarded_expression(rhs, context, required_proofs, local_types, state)?;
        }
        SimpleCondition::Expr(expression) => {
            validate_guarded_expression(expression, context, required_proofs, local_types, state)?
        }
        SimpleCondition::And(lhs, rhs) | SimpleCondition::Or(lhs, rhs) => {
            validate_guarded_condition(lhs, context, required_proofs, local_types, state)?;
            validate_guarded_condition(rhs, context, required_proofs, local_types, state)?;
        }
        SimpleCondition::Not(inner) => {
            validate_guarded_condition(inner, context, required_proofs, local_types, state)?;
        }
    }
    Ok(())
}

fn validate_guarded_expression(
    expression: &SimpleExpr,
    context: &AnalysisContext<'_>,
    required_proofs: &BTreeMap<u32, RequiredFunctionContract>,
    local_types: &BTreeMap<String, TypeId>,
    state: &mut CollectionGuardState,
) -> Result<(), String> {
    match expression {
        SimpleExpr::Condition(condition) => {
            validate_guarded_condition(condition, context, required_proofs, local_types, state)
        }
        SimpleExpr::IndexedPath { index, .. } => {
            validate_guarded_expression(index, context, required_proofs, local_types, state)
        }
        SimpleExpr::Call { target, args } => {
            if let Some(operation) =
                typed_collection_operation_for_call(target, args, context, local_types)
            {
                // The receiver is always an exact persistent path for a
                // compiler-owned operation.  Only ordinary argument
                // expressions can contain nested calls.
                for argument in args.iter().skip(1) {
                    validate_guarded_expression(
                        argument,
                        context,
                        required_proofs,
                        local_types,
                        state,
                    )?;
                }
                if is_can_operation(operation) {
                    return Ok(());
                }
                if let Some(proof) = guard_proof_for_action(operation, args) {
                    if !state.consume(&proof) {
                        return Err(format!(
                            "collection action '{}' requires a matching direct if (receiver.can_*()) guard; guards are exact, cannot be cached or negated, and are consumed after one action",
                            target
                        ));
                    }
                    state.invalidate();
                    return Ok(());
                }
                // overwrite_oldest is intentionally unconditional.  All
                // Other unguarded collection operations simply invalidate
                // a proof conservatively because they may change state.
                let _ = is_overwrite_oldest_operation(operation);
                state.invalidate();
                return Ok(());
            }

            for argument in args {
                validate_guarded_expression(
                    argument,
                    context,
                    required_proofs,
                    local_types,
                    state,
                )?;
            }
            let target_id =
                resolve_internal_call(target, args, context, local_types, &BTreeMap::new());
            if let Some(target_id) = target_id {
                if let Some(required_contract) = required_proofs.get(&target_id) {
                    if args
                        .iter()
                        .any(|argument| !safe_requires_expansion_argument(argument))
                    {
                        return Err(format!(
                            "call to @requires function '{}' has an unsafe argument for mandatory compile-time expansion; use a simple value expression or compute it before the guarded call",
                            target
                        ));
                    }
                    let required_proof = instantiate_required_proof(
                        &required_contract.proof,
                        &required_contract.param_names,
                        args,
                    );
                    if !state.consume(&required_proof) {
                        return Err(format!(
                            "call to @requires function '{}' requires its exact direct if (receiver.can_*()) guard at the call site",
                            target
                        ));
                    }
                }
            }
            // Ordinary calls may mutate collection state.  There is no
            // runtime witness parameter, so stale proofs cannot be carried
            // across them.
            state.invalidate();
            Ok(())
        }
        SimpleExpr::Binary { lhs, rhs, .. } => {
            validate_guarded_expression(lhs, context, required_proofs, local_types, state)?;
            validate_guarded_expression(rhs, context, required_proofs, local_types, state)
        }
        SimpleExpr::DefaultValue(_)
        | SimpleExpr::Int(_)
        | SimpleExpr::Float(_)
        | SimpleExpr::Bool(_)
        | SimpleExpr::StringLiteral(_)
        | SimpleExpr::Identifier(_) => Ok(()),
    }
}

fn safe_requires_expansion_argument(expression: &SimpleExpr) -> bool {
    match expression {
        SimpleExpr::Call { .. } => false,
        SimpleExpr::Condition(condition) => safe_requires_expansion_condition(condition),
        SimpleExpr::IndexedPath { index, .. } => safe_requires_expansion_argument(index),
        SimpleExpr::Binary { lhs, rhs, .. } => {
            safe_requires_expansion_argument(lhs) && safe_requires_expansion_argument(rhs)
        }
        SimpleExpr::DefaultValue(_)
        | SimpleExpr::Int(_)
        | SimpleExpr::Float(_)
        | SimpleExpr::Bool(_)
        | SimpleExpr::StringLiteral(_)
        | SimpleExpr::Identifier(_) => true,
    }
}

fn instantiate_required_proof(
    proof: &CollectionGuardProof,
    param_names: &[String],
    arguments: &[SimpleExpr],
) -> CollectionGuardProof {
    let substitutions = param_names
        .iter()
        .cloned()
        .zip(arguments.iter().cloned())
        .collect::<BTreeMap<_, _>>();
    CollectionGuardProof {
        action: proof.action,
        receiver: substitute_required_expression(&proof.receiver, &substitutions),
        key_args: proof
            .key_args
            .iter()
            .map(|argument| substitute_required_expression(argument, &substitutions))
            .collect(),
    }
}

fn substitute_required_expression(
    expression: &SimpleExpr,
    substitutions: &BTreeMap<String, SimpleExpr>,
) -> SimpleExpr {
    match expression {
        SimpleExpr::Identifier(path) => {
            if let Some(argument) = substitutions.get(path) {
                return argument.clone();
            }
            if let Some((root, suffix)) = path.split_once('.') {
                if let Some(SimpleExpr::Identifier(argument_root)) = substitutions.get(root) {
                    return SimpleExpr::Identifier(format!("{argument_root}.{suffix}"));
                }
            }
            expression.clone()
        }
        SimpleExpr::IndexedPath {
            collection_path,
            index,
            suffix,
        } => SimpleExpr::IndexedPath {
            collection_path: substitutions
                .get(collection_path)
                .and_then(|argument| match argument {
                    SimpleExpr::Identifier(path) => Some(path.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| collection_path.clone()),
            index: Box::new(substitute_required_expression(index, substitutions)),
            suffix: suffix.clone(),
        },
        SimpleExpr::Call { target, args } => SimpleExpr::Call {
            target: target.clone(),
            args: args
                .iter()
                .map(|argument| substitute_required_expression(argument, substitutions))
                .collect(),
        },
        SimpleExpr::Binary { lhs, op, rhs } => SimpleExpr::Binary {
            lhs: Box::new(substitute_required_expression(lhs, substitutions)),
            op: *op,
            rhs: Box::new(substitute_required_expression(rhs, substitutions)),
        },
        SimpleExpr::Condition(condition) => SimpleExpr::Condition(Box::new(
            substitute_required_condition(condition, substitutions),
        )),
        SimpleExpr::DefaultValue(_)
        | SimpleExpr::Int(_)
        | SimpleExpr::Float(_)
        | SimpleExpr::Bool(_)
        | SimpleExpr::StringLiteral(_) => expression.clone(),
    }
}

fn substitute_required_condition(
    condition: &SimpleCondition,
    substitutions: &BTreeMap<String, SimpleExpr>,
) -> SimpleCondition {
    match condition {
        SimpleCondition::Comparison { lhs, op, rhs } => SimpleCondition::Comparison {
            lhs: substitute_required_expression(lhs, substitutions),
            op: *op,
            rhs: substitute_required_expression(rhs, substitutions),
        },
        SimpleCondition::Expr(expression) => {
            SimpleCondition::Expr(substitute_required_expression(expression, substitutions))
        }
        SimpleCondition::And(lhs, rhs) => SimpleCondition::And(
            Box::new(substitute_required_condition(lhs, substitutions)),
            Box::new(substitute_required_condition(rhs, substitutions)),
        ),
        SimpleCondition::Or(lhs, rhs) => SimpleCondition::Or(
            Box::new(substitute_required_condition(lhs, substitutions)),
            Box::new(substitute_required_condition(rhs, substitutions)),
        ),
        SimpleCondition::Not(inner) => SimpleCondition::Not(Box::new(
            substitute_required_condition(inner, substitutions),
        )),
    }
}

fn safe_requires_expansion_condition(condition: &SimpleCondition) -> bool {
    match condition {
        SimpleCondition::Comparison { lhs, rhs, .. } => {
            safe_requires_expansion_argument(lhs) && safe_requires_expansion_argument(rhs)
        }
        SimpleCondition::Expr(expression) => safe_requires_expansion_argument(expression),
        SimpleCondition::And(lhs, rhs) | SimpleCondition::Or(lhs, rhs) => {
            safe_requires_expansion_condition(lhs) && safe_requires_expansion_condition(rhs)
        }
        SimpleCondition::Not(inner) => safe_requires_expansion_condition(inner),
    }
}

fn validate_condition(
    condition: &SimpleCondition,
    context: &AnalysisContext<'_>,
    local_types: &BTreeMap<String, TypeId>,
) -> Result<(), String> {
    match condition {
        SimpleCondition::Comparison { .. } => {}
        SimpleCondition::Expr(expression) => {
            if let Some(found) = semantic_expression_type(expression, context, local_types) {
                if found == TYPE_ID_BOOL {
                    return Ok(());
                }
                return Err(format!(
                    "condition expression must be bool; found {}",
                    type_name(found, context.types)
                ));
            }
        }
        SimpleCondition::And(lhs, rhs) | SimpleCondition::Or(lhs, rhs) => {
            validate_condition(lhs, context, local_types)?;
            validate_condition(rhs, context, local_types)?;
        }
        SimpleCondition::Not(inner) => validate_condition(inner, context, local_types)?,
    }
    Ok(())
}

fn semantic_expression_type(
    expression: &SimpleExpr,
    context: &AnalysisContext<'_>,
    local_types: &BTreeMap<String, TypeId>,
) -> Option<TypeId> {
    if let SimpleExpr::Identifier(name) = expression {
        if context.constants.contains_key(name) {
            return Some(TYPE_ID_I32);
        }
    }
    expression_type(expression, context, local_types, &BTreeMap::new())
}

fn semantic_expression_type_with_expected(
    expression: &SimpleExpr,
    expected: Option<TypeId>,
    context: &AnalysisContext<'_>,
    local_types: &BTreeMap<String, TypeId>,
) -> Option<TypeId> {
    let inferred = semantic_expression_type(expression, context, local_types);
    if expected == Some(TYPE_ID_F64) && inferred == Some(TYPE_ID_F32) {
        return Some(TYPE_ID_F64);
    }
    inferred
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TypedCollectionOperation {
    PoolPush,
    PoolRemove,
    PoolCanPush,
    PoolCanRemove,
    PoolCount,
    PoolCapacity,
    PoolClear,
    StablePoolInsert,
    StablePoolRemove,
    StablePoolCanInsert,
    StablePoolCanRemove,
    StablePoolCount,
    StablePoolCapacity,
    StablePoolClear,
    MapPut,
    MapGet,
    MapContains,
    MapRemove,
    MapCanPut,
    MapCanGet,
    MapCanRemove,
    SetAdd,
    SetContains,
    SetRemove,
    SetCanAdd,
    SetCanRemove,
    QueuePush,
    QueuePop,
    QueuePeek,
    QueuePhysicalIndex,
    QueueCanPush,
    QueueCanPop,
    QueueCanPeek,
    QueueCount,
    QueueCapacity,
    QueueClear,
    RingBufferPush,
    RingBufferPop,
    RingBufferPeek,
    RingBufferPhysicalIndex,
    RingBufferCanPush,
    RingBufferCanPop,
    RingBufferCanPeek,
    RingBufferCount,
    RingBufferCapacity,
    RingBufferClear,
    QueueOverwriteOldest,
    RingBufferOverwriteOldest,
    PriorityQueuePush,
    PriorityQueuePop,
    PriorityQueuePeek,
    PriorityQueuePeekPriority,
    PriorityQueueCount,
    PriorityQueueCapacity,
    PriorityQueueClear,
    PriorityQueueCanPush,
    PriorityQueueCanPop,
    PriorityQueueCanPeek,
    GridGet,
    GridSet,
    GridCanAccess,
    GridCapacity,
    GridClear,
    BitsetTest,
    BitsetSet,
    BitsetCanAccess,
    BitsetCapacity,
    BitsetClear,
}

impl TypedCollectionOperation {
    /// Resolve a receiver-format method only after its receiver's exact
    /// persistent descriptor has been found.  The names are intentionally
    /// shared with ordinary user functions; callers must supply the receiver
    /// kind before a compiler-owned operation can be claimed.
    fn from_receiver_target(target: &str, collection_kind: TypedCollectionKind) -> Option<Self> {
        use TypedCollectionKind::*;
        Some(match (target, collection_kind) {
            ("push", Pool) => Self::PoolPush,
            ("remove", Pool) => Self::PoolRemove,
            ("can_push", Pool) => Self::PoolCanPush,
            ("can_remove", Pool) => Self::PoolCanRemove,
            ("count", Pool) => Self::PoolCount,
            ("capacity", Pool) => Self::PoolCapacity,
            ("clear", Pool) => Self::PoolClear,

            ("insert", StablePool) => Self::StablePoolInsert,
            ("remove", StablePool) => Self::StablePoolRemove,
            ("can_insert", StablePool) => Self::StablePoolCanInsert,
            ("can_remove", StablePool) => Self::StablePoolCanRemove,
            ("count", StablePool) => Self::StablePoolCount,
            ("capacity", StablePool) => Self::StablePoolCapacity,
            ("clear", StablePool) => Self::StablePoolClear,

            ("put", Map) => Self::MapPut,
            ("get", Map) => Self::MapGet,
            ("contains", Map) => Self::MapContains,
            ("remove", Map) => Self::MapRemove,
            ("can_put", Map) => Self::MapCanPut,
            ("can_get", Map) => Self::MapCanGet,
            ("can_remove", Map) => Self::MapCanRemove,

            ("add", Set) => Self::SetAdd,
            ("contains", Set) => Self::SetContains,
            ("remove", Set) => Self::SetRemove,
            ("can_add", Set) => Self::SetCanAdd,
            ("can_remove", Set) => Self::SetCanRemove,

            ("push", Queue) => Self::QueuePush,
            ("pop", Queue) => Self::QueuePop,
            ("peek", Queue) => Self::QueuePeek,
            ("physical_index", Queue) => Self::QueuePhysicalIndex,
            ("can_push", Queue) => Self::QueueCanPush,
            ("can_pop", Queue) => Self::QueueCanPop,
            ("can_peek", Queue) => Self::QueueCanPeek,
            ("count", Queue) => Self::QueueCount,
            ("capacity", Queue) => Self::QueueCapacity,
            ("clear", Queue) => Self::QueueClear,
            ("overwrite_oldest", Queue) => Self::QueueOverwriteOldest,

            ("push", RingBuffer) => Self::RingBufferPush,
            ("pop", RingBuffer) => Self::RingBufferPop,
            ("peek", RingBuffer) => Self::RingBufferPeek,
            ("physical_index", RingBuffer) => Self::RingBufferPhysicalIndex,
            ("can_push", RingBuffer) => Self::RingBufferCanPush,
            ("can_pop", RingBuffer) => Self::RingBufferCanPop,
            ("can_peek", RingBuffer) => Self::RingBufferCanPeek,
            ("count", RingBuffer) => Self::RingBufferCount,
            ("capacity", RingBuffer) => Self::RingBufferCapacity,
            ("clear", RingBuffer) => Self::RingBufferClear,
            ("overwrite_oldest", RingBuffer) => Self::RingBufferOverwriteOldest,

            ("push", PriorityQueue) => Self::PriorityQueuePush,
            ("pop", PriorityQueue) => Self::PriorityQueuePop,
            ("peek", PriorityQueue) => Self::PriorityQueuePeek,
            ("peek_priority", PriorityQueue) => Self::PriorityQueuePeekPriority,
            ("count", PriorityQueue) => Self::PriorityQueueCount,
            ("capacity", PriorityQueue) => Self::PriorityQueueCapacity,
            ("clear", PriorityQueue) => Self::PriorityQueueClear,
            ("can_push", PriorityQueue) => Self::PriorityQueueCanPush,
            ("can_pop", PriorityQueue) => Self::PriorityQueueCanPop,
            ("can_peek", PriorityQueue) => Self::PriorityQueueCanPeek,

            ("get", Grid) => Self::GridGet,
            ("set", Grid) => Self::GridSet,
            ("can_access", Grid) => Self::GridCanAccess,
            ("capacity", Grid) => Self::GridCapacity,
            ("clear", Grid) => Self::GridClear,

            ("test", Bitset) => Self::BitsetTest,
            ("set", Bitset) => Self::BitsetSet,
            ("can_access", Bitset) => Self::BitsetCanAccess,
            ("capacity", Bitset) => Self::BitsetCapacity,
            ("clear", Bitset) => Self::BitsetClear,
            _ => return None,
        })
    }

    fn return_type(self) -> TypeId {
        match self {
            Self::PoolPush
            | Self::StablePoolInsert
            | Self::PoolCount
            | Self::PoolCapacity
            | Self::StablePoolCount
            | Self::StablePoolCapacity
            | Self::QueuePeek
            | Self::QueuePhysicalIndex
            | Self::QueueCount
            | Self::QueueCapacity
            | Self::RingBufferPeek
            | Self::RingBufferPhysicalIndex
            | Self::RingBufferCount
            | Self::RingBufferCapacity
            | Self::PriorityQueuePeek
            | Self::PriorityQueuePeekPriority
            | Self::PriorityQueueCount
            | Self::PriorityQueueCapacity => TYPE_ID_I32,
            Self::MapGet | Self::GridGet | Self::GridCapacity | Self::BitsetCapacity => TYPE_ID_I32,
            Self::PoolCanPush
            | Self::PoolCanRemove
            | Self::StablePoolCanInsert
            | Self::StablePoolCanRemove
            | Self::MapCanPut
            | Self::MapCanGet
            | Self::MapCanRemove
            | Self::SetCanAdd
            | Self::SetCanRemove
            | Self::QueueCanPush
            | Self::QueueCanPop
            | Self::QueueCanPeek
            | Self::RingBufferCanPush
            | Self::RingBufferCanPop
            | Self::RingBufferCanPeek
            | Self::MapContains
            | Self::SetContains
            | Self::PriorityQueueCanPush
            | Self::PriorityQueueCanPop
            | Self::PriorityQueueCanPeek
            | Self::GridCanAccess
            | Self::BitsetTest
            | Self::BitsetCanAccess => TYPE_ID_BOOL,
            Self::PoolClear
            | Self::StablePoolClear
            | Self::QueueClear
            | Self::RingBufferClear
            | Self::PriorityQueueClear
            | Self::QueueOverwriteOldest
            | Self::RingBufferOverwriteOldest
            | Self::GridSet
            | Self::GridClear
            | Self::BitsetSet
            | Self::BitsetClear
            | Self::PoolRemove
            | Self::StablePoolRemove
            | Self::MapPut
            | Self::MapRemove
            | Self::SetAdd
            | Self::SetRemove
            | Self::QueuePush
            | Self::QueuePop
            | Self::RingBufferPush
            | Self::RingBufferPop
            | Self::PriorityQueuePush
            | Self::PriorityQueuePop => TYPE_ID_VOID,
        }
    }

    fn collection_kind(self) -> TypedCollectionKind {
        match self {
            Self::PoolPush
            | Self::PoolRemove
            | Self::PoolCount
            | Self::PoolCapacity
            | Self::PoolClear
            | Self::PoolCanPush
            | Self::PoolCanRemove => TypedCollectionKind::Pool,
            Self::StablePoolInsert
            | Self::StablePoolRemove
            | Self::StablePoolCount
            | Self::StablePoolCapacity
            | Self::StablePoolClear
            | Self::StablePoolCanInsert
            | Self::StablePoolCanRemove => TypedCollectionKind::StablePool,
            Self::MapPut
            | Self::MapGet
            | Self::MapContains
            | Self::MapRemove
            | Self::MapCanPut
            | Self::MapCanGet
            | Self::MapCanRemove => TypedCollectionKind::Map,
            Self::SetAdd
            | Self::SetContains
            | Self::SetRemove
            | Self::SetCanAdd
            | Self::SetCanRemove => TypedCollectionKind::Set,
            Self::QueuePush
            | Self::QueuePop
            | Self::QueuePeek
            | Self::QueuePhysicalIndex
            | Self::QueueCount
            | Self::QueueCapacity
            | Self::QueueClear
            | Self::QueueCanPush
            | Self::QueueCanPop
            | Self::QueueCanPeek
            | Self::QueueOverwriteOldest => TypedCollectionKind::Queue,
            Self::RingBufferPush
            | Self::RingBufferPop
            | Self::RingBufferPeek
            | Self::RingBufferPhysicalIndex
            | Self::RingBufferCount
            | Self::RingBufferCapacity
            | Self::RingBufferClear
            | Self::RingBufferCanPush
            | Self::RingBufferCanPop
            | Self::RingBufferCanPeek
            | Self::RingBufferOverwriteOldest => TypedCollectionKind::RingBuffer,
            Self::PriorityQueuePush
            | Self::PriorityQueuePop
            | Self::PriorityQueuePeek
            | Self::PriorityQueuePeekPriority
            | Self::PriorityQueueCount
            | Self::PriorityQueueCapacity
            | Self::PriorityQueueClear
            | Self::PriorityQueueCanPush
            | Self::PriorityQueueCanPop
            | Self::PriorityQueueCanPeek => TypedCollectionKind::PriorityQueue,
            Self::GridGet
            | Self::GridSet
            | Self::GridCanAccess
            | Self::GridCapacity
            | Self::GridClear => TypedCollectionKind::Grid,
            Self::BitsetTest
            | Self::BitsetSet
            | Self::BitsetCanAccess
            | Self::BitsetCapacity
            | Self::BitsetClear => TypedCollectionKind::Bitset,
        }
    }

    fn expected_arity(self) -> usize {
        match self {
            Self::PoolPush
            | Self::PoolRemove
            | Self::StablePoolInsert
            | Self::StablePoolRemove
            | Self::QueuePush
            | Self::QueuePeek
            | Self::QueuePhysicalIndex
            | Self::RingBufferPush
            | Self::RingBufferPeek
            | Self::RingBufferPhysicalIndex
            | Self::MapGet
            | Self::MapContains
            | Self::MapRemove
            | Self::SetAdd
            | Self::SetContains
            | Self::SetRemove
            | Self::PoolCanRemove
            | Self::StablePoolCanRemove
            | Self::QueueCanPeek
            | Self::RingBufferCanPeek
            | Self::MapCanPut
            | Self::MapCanGet
            | Self::MapCanRemove
            | Self::SetCanAdd
            | Self::SetCanRemove
            | Self::QueueOverwriteOldest
            | Self::RingBufferOverwriteOldest => 2,
            Self::MapPut | Self::PriorityQueuePush => 3,
            Self::PoolCount
            | Self::PoolCapacity
            | Self::PoolClear
            | Self::StablePoolCount
            | Self::StablePoolCapacity
            | Self::StablePoolClear
            | Self::QueuePop
            | Self::QueueCount
            | Self::QueueCapacity
            | Self::QueueClear
            | Self::RingBufferPop
            | Self::RingBufferCount
            | Self::RingBufferCapacity
            | Self::RingBufferClear
            | Self::PriorityQueuePop
            | Self::PriorityQueueCount
            | Self::PriorityQueueCapacity
            | Self::PriorityQueueClear
            | Self::PriorityQueueCanPush
            | Self::PriorityQueueCanPop
            | Self::PriorityQueueCanPeek
            | Self::GridCapacity
            | Self::GridClear
            | Self::BitsetCapacity
            | Self::BitsetClear => 1,
            Self::PoolCanPush
            | Self::StablePoolCanInsert
            | Self::QueueCanPush
            | Self::QueueCanPop
            | Self::RingBufferCanPush
            | Self::RingBufferCanPop => 1,
            Self::GridGet | Self::GridCanAccess => 3,
            Self::GridSet => 4,
            Self::BitsetTest | Self::BitsetCanAccess => 2,
            Self::BitsetSet => 3,
            Self::PriorityQueuePeek | Self::PriorityQueuePeekPriority => 1,
        }
    }

    fn payload_argument(self) -> Option<(usize, &'static str)> {
        match self {
            Self::PoolPush => Some((1, "value")),
            Self::PoolRemove => Some((1, "index")),
            Self::StablePoolInsert => Some((1, "value")),
            Self::StablePoolRemove => Some((1, "index")),
            Self::QueuePush => Some((1, "value")),
            Self::QueuePeek => Some((1, "logical_index")),
            Self::QueuePhysicalIndex => Some((1, "logical_index")),
            Self::RingBufferPush => Some((1, "value")),
            Self::RingBufferPeek => Some((1, "logical_index")),
            Self::RingBufferPhysicalIndex => Some((1, "logical_index")),
            Self::QueueOverwriteOldest | Self::RingBufferOverwriteOldest => Some((1, "value")),
            Self::PriorityQueuePush => Some((1, "priority")),
            Self::PoolCount
            | Self::PoolCapacity
            | Self::PoolClear
            | Self::StablePoolCount
            | Self::StablePoolCapacity
            | Self::StablePoolClear
            | Self::MapPut
            | Self::MapGet
            | Self::MapContains
            | Self::MapRemove
            | Self::SetAdd
            | Self::SetContains
            | Self::SetRemove
            | Self::QueuePop
            | Self::QueueCount
            | Self::QueueCapacity
            | Self::QueueClear
            | Self::RingBufferPop
            | Self::RingBufferCount
            | Self::RingBufferCapacity
            | Self::RingBufferClear => None,
            Self::PoolCanPush
            | Self::StablePoolCanInsert
            | Self::QueueCanPush
            | Self::QueueCanPop
            | Self::RingBufferCanPush
            | Self::RingBufferCanPop
            | Self::MapCanPut
            | Self::MapCanGet
            | Self::MapCanRemove
            | Self::SetCanAdd
            | Self::SetCanRemove
            | Self::PoolCanRemove
            | Self::StablePoolCanRemove
            | Self::QueueCanPeek
            | Self::RingBufferCanPeek => None,
            Self::PriorityQueuePop
            | Self::PriorityQueuePeek
            | Self::PriorityQueuePeekPriority
            | Self::PriorityQueueCount
            | Self::PriorityQueueCapacity
            | Self::PriorityQueueClear
            | Self::PriorityQueueCanPush
            | Self::PriorityQueueCanPop
            | Self::PriorityQueueCanPeek
            | Self::GridGet
            | Self::GridSet
            | Self::GridCanAccess
            | Self::GridCapacity
            | Self::GridClear
            | Self::BitsetTest
            | Self::BitsetSet
            | Self::BitsetCanAccess
            | Self::BitsetCapacity
            | Self::BitsetClear => None,
        }
    }

    fn argument_specs(self) -> Vec<(usize, &'static str)> {
        match self {
            Self::MapPut => vec![(1, "key"), (2, "value")],
            Self::PriorityQueuePush => vec![(1, "priority"), (2, "value")],
            Self::MapGet
            | Self::MapContains
            | Self::MapRemove
            | Self::MapCanPut
            | Self::MapCanGet
            | Self::MapCanRemove
            | Self::SetAdd
            | Self::SetContains
            | Self::SetRemove
            | Self::SetCanAdd
            | Self::SetCanRemove => vec![(1, "key")],
            Self::GridGet | Self::GridCanAccess => vec![(1, "x"), (2, "y")],
            Self::GridSet => vec![(1, "x"), (2, "y"), (3, "value")],
            Self::BitsetTest | Self::BitsetCanAccess => vec![(1, "index")],
            Self::BitsetSet => vec![(1, "index"), (2, "value")],
            _ => self.payload_argument().into_iter().collect(),
        }
    }

    fn argument_type_specs(self) -> Vec<(usize, &'static str, TypeId)> {
        self.argument_specs()
            .into_iter()
            .map(|(index, name)| {
                let type_id = if self == Self::BitsetSet && name == "value" {
                    TYPE_ID_BOOL
                } else {
                    TYPE_ID_I32
                };
                (index, name, type_id)
            })
            .collect()
    }
}

fn is_typed_collection_path_or_descendant(
    path: &str,
    context: &AnalysisContext<'_>,
    local_types: &BTreeMap<String, TypeId>,
) -> bool {
    if let Some(local_type) = local_types.get(root_name(path)) {
        return context.types.is_typed_collection_type(*local_type);
    }
    context.typed_collection_descriptors.keys().any(|root| {
        path == root
            || path
                .strip_prefix(root)
                .is_some_and(|suffix| suffix.starts_with('.') || suffix.starts_with('['))
    })
}

fn exact_typed_collection_path<'a>(
    expression: &SimpleExpr,
    context: &'a AnalysisContext<'_>,
    local_types: &BTreeMap<String, TypeId>,
) -> Option<(&'a str, &'a TypedCollectionDescriptor)> {
    let SimpleExpr::Identifier(path) = expression else {
        return None;
    };
    if local_types.contains_key(root_name(path)) {
        return None;
    }
    context
        .typed_collection_descriptors
        .get_key_value(path)
        .map(|(path, descriptor)| (path.as_str(), descriptor))
}

fn typed_collection_descriptor_supports_operation(
    operation: TypedCollectionOperation,
    descriptor: &TypedCollectionDescriptor,
) -> bool {
    if descriptor.kind != operation.collection_kind() {
        return false;
    }
    match operation.collection_kind() {
        TypedCollectionKind::Map => {
            descriptor.key_type == Some(TYPE_ID_I32) && descriptor.value_type == Some(TYPE_ID_I32)
        }
        TypedCollectionKind::Set => descriptor.key_type == Some(TYPE_ID_I32),
        TypedCollectionKind::PriorityQueue => descriptor.element_type == Some(TYPE_ID_I32),
        TypedCollectionKind::Grid => {
            descriptor.element_type == Some(TYPE_ID_I32)
                && descriptor.width.is_some()
                && descriptor.height.is_some()
        }
        TypedCollectionKind::Bitset => true,
        _ => descriptor.element_type == Some(TYPE_ID_I32),
    }
}

fn typed_collection_operation_for_call(
    target: &str,
    args: &[SimpleExpr],
    context: &AnalysisContext<'_>,
    local_types: &BTreeMap<String, TypeId>,
) -> Option<TypedCollectionOperation> {
    let (_, descriptor) = exact_typed_collection_path(args.first()?, context, local_types)?;
    TypedCollectionOperation::from_receiver_target(target, descriptor.kind)
}

#[cfg(test)]
fn typed_pool_operation(
    target: &str,
    args: &[SimpleExpr],
    context: &AnalysisContext<'_>,
    local_types: &BTreeMap<String, TypeId>,
) -> Result<Option<TypedCollectionOperation>, String> {
    typed_collection_operation(target, args, context, local_types)
}

fn typed_collection_operation(
    target: &str,
    args: &[SimpleExpr],
    context: &AnalysisContext<'_>,
    local_types: &BTreeMap<String, TypeId>,
) -> Result<Option<TypedCollectionOperation>, String> {
    let Some(operation) = typed_collection_operation_for_call(target, args, context, local_types)
    else {
        return Ok(None);
    };

    let expected_arity = operation.expected_arity();
    if args.len() != expected_arity {
        return Err(format!(
            "{target} expects {expected_arity} arguments, got {}",
            args.len()
        ));
    }

    let Some((path, descriptor)) = exact_typed_collection_path(&args[0], context, local_types)
    else {
        return Err(format!(
            "{target} first argument must be an exact persistent typed collection path"
        ));
    };
    if descriptor.kind != operation.collection_kind() {
        return Err(format!(
            "{target} requires persistent {} path '{path}', found {}",
            operation.collection_kind().as_str(),
            descriptor.kind_name(),
        ));
    }
    if !typed_collection_descriptor_supports_operation(operation, descriptor) {
        match operation.collection_kind() {
            TypedCollectionKind::Map => {
                if descriptor.key_type != Some(TYPE_ID_I32) {
                    return Err(format!("{target} requires map path '{path}' with i32 key"));
                }
                if descriptor.value_type != Some(TYPE_ID_I32) {
                    return Err(format!(
                        "{target} requires map path '{path}' with i32 value"
                    ));
                }
                return Err(format!(
                    "{target} requires map path '{path}' with i32 key/value"
                ));
            }
            TypedCollectionKind::Set => {
                if descriptor.key_type != Some(TYPE_ID_I32) {
                    return Err(format!("{target} requires set path '{path}' with i32 key"));
                }
                return Err(format!("{target} requires set path '{path}' with i32 key"));
            }
            TypedCollectionKind::PriorityQueue => {
                if descriptor.element_type != Some(TYPE_ID_I32) {
                    return Err(format!(
                        "{target} requires priority_queue path '{path}' with i32 payload"
                    ));
                }
                return Err(format!(
                    "{target} requires priority_queue path '{path}' with i32 payload"
                ));
            }
            TypedCollectionKind::Grid => {
                return Err(format!(
                    "{target} requires grid path '{path}' with i32 payload"
                ));
            }
            TypedCollectionKind::Bitset => {
                return Err(format!("{target} requires bitset path '{path}'"));
            }
            _ => {
                return Err(format!(
                    "{target} requires {} path '{path}' with i32 payload",
                    operation.collection_kind().as_str(),
                ));
            }
        }
    }

    for (argument_index, argument_name, expected_type) in operation.argument_type_specs() {
        let value_type = semantic_expression_type(&args[argument_index], context, local_types)
            .ok_or_else(|| {
                format!(
                    "{target} {argument_name} argument must have type {}; its type could not be inferred",
                    type_name(expected_type, context.types)
                )
            })?;
        if value_type != expected_type {
            return Err(type_mismatch(
                &format!("{target} {argument_name} argument"),
                expected_type,
                value_type,
                context.types,
            ));
        }
    }

    if matches!(
        operation,
        TypedCollectionOperation::QueueOverwriteOldest
            | TypedCollectionOperation::RingBufferOverwriteOldest
    ) && descriptor.capacity == 0
    {
        return Err(format!(
            "{target} cannot be called on zero-capacity {} path '{path}'",
            operation.collection_kind().as_str()
        ));
    }
    Ok(Some(operation))
}

fn semantic_assignment_target_type(
    target: &AssignTarget,
    context: &AnalysisContext<'_>,
    local_types: &BTreeMap<String, TypeId>,
) -> Option<TypeId> {
    match target {
        AssignTarget::Local(path) | AssignTarget::GlobalPath(path) => {
            path_type(path, context, local_types, &BTreeMap::new())
        }
        AssignTarget::IndexedPath {
            collection_path,
            suffix,
            ..
        } => {
            let collection = path_type(collection_path, context, local_types, &BTreeMap::new())?;
            let element = context.types.indexed_element_type_id(collection)?;
            field_suffix_type(element, suffix, &context.field_types)
        }
    }
}

fn reject_array_length(
    type_id: Option<TypeId>,
    path: &str,
    types: &TypeTable,
) -> Result<(), String> {
    if type_id
        .and_then(|id| types.type_info(id))
        .is_some_and(|info| {
            matches!(
                info.category,
                TypeCategory::ArrayFixed | TypeCategory::ArrayView
            )
        })
    {
        let collection = path.strip_suffix(".length").unwrap_or(path);
        return Err(format!("array property '{path}' is unavailable; use '{collection}.max_length' for declared capacity"));
    }
    Ok(())
}

fn validate_property_access(
    path: &str,
    context: &AnalysisContext<'_>,
    local_types: &BTreeMap<String, TypeId>,
) -> Result<(), String> {
    if let Some(collection) = path.strip_suffix(".length") {
        reject_array_length(
            path_type(collection, context, local_types, &BTreeMap::new()),
            path,
            context.types,
        )?;
    }
    Ok(())
}

fn validate_indexed_property_access(
    collection: &str,
    suffix: &str,
    context: &AnalysisContext<'_>,
    local_types: &BTreeMap<String, TypeId>,
) -> Result<(), String> {
    let suffix = suffix.trim_start_matches('.');
    if suffix == "length" || suffix.ends_with(".length") {
        let parent = suffix.strip_suffix("length").unwrap().trim_end_matches('.');
        let type_id = path_type(collection, context, local_types, &BTreeMap::new())
            .and_then(|id| context.types.indexed_element_type_id(id))
            .and_then(|id| field_suffix_type(id, parent, &context.field_types));
        reject_array_length(type_id, &format!("{collection}[*].{suffix}"), context.types)?;
    }
    Ok(())
}

fn validate_assignment_target_access(
    target: &AssignTarget,
    context: &AnalysisContext<'_>,
    local_types: &BTreeMap<String, TypeId>,
) -> Result<(), String> {
    let typed_collection_target = match target {
        AssignTarget::Local(path) | AssignTarget::GlobalPath(path) => {
            is_typed_collection_path_or_descendant(path, context, local_types)
        }
        AssignTarget::IndexedPath {
            collection_path,
            suffix,
            ..
        } => {
            let indexed_path = indexed_state_path(collection_path, suffix);
            is_typed_collection_path_or_descendant(collection_path, context, local_types)
                || is_typed_collection_path_or_descendant(&indexed_path, context, local_types)
        }
    };
    if typed_collection_target {
        return Err(
            "typed collection paths may only be used as the first argument of a compiler-owned typed collection operation"
                .to_string(),
        );
    }
    match target {
        AssignTarget::Local(path) | AssignTarget::GlobalPath(path) => {
            validate_property_access(path, context, local_types)
        }
        AssignTarget::IndexedPath {
            collection_path,
            index,
            suffix,
        } => {
            validate_expression_access(index, context, local_types)?;
            validate_indexed_property_access(collection_path, suffix, context, local_types)
        }
    }
}

fn validate_condition_access(
    condition: &SimpleCondition,
    context: &AnalysisContext<'_>,
    local_types: &BTreeMap<String, TypeId>,
) -> Result<(), String> {
    match condition {
        SimpleCondition::Comparison { lhs, rhs, .. } => {
            validate_expression_access(lhs, context, local_types)?;
            validate_expression_access(rhs, context, local_types)
        }
        SimpleCondition::Expr(expression) => {
            validate_expression_access(expression, context, local_types)
        }
        SimpleCondition::And(lhs, rhs) | SimpleCondition::Or(lhs, rhs) => {
            validate_condition_access(lhs, context, local_types)?;
            validate_condition_access(rhs, context, local_types)
        }
        SimpleCondition::Not(inner) => validate_condition_access(inner, context, local_types),
    }
}

fn validate_expression_access(
    expression: &SimpleExpr,
    context: &AnalysisContext<'_>,
    local_types: &BTreeMap<String, TypeId>,
) -> Result<(), String> {
    match expression {
        SimpleExpr::Condition(condition) => {
            validate_condition_access(condition, context, local_types)
        }
        SimpleExpr::IndexedPath {
            collection_path,
            index,
            suffix,
        } => {
            validate_expression_access(index, context, local_types)?;
            if is_typed_collection_path_or_descendant(collection_path, context, local_types)
                || is_typed_collection_path_or_descendant(
                    &indexed_state_path(collection_path, suffix),
                    context,
                    local_types,
                )
            {
                return Err(
                        "typed collection paths may only be used as the first argument of a compiler-owned typed collection operation"
                        .to_string(),
                );
            }
            validate_indexed_property_access(collection_path, suffix, context, local_types)
        }
        SimpleExpr::Identifier(path) => {
            if is_typed_collection_path_or_descendant(path, context, local_types)
                || semantic_expression_type(expression, context, local_types)
                    .is_some_and(|type_id| context.types.is_typed_collection_type(type_id))
            {
                return Err(
                        "typed collection paths may only be used as the first argument of a compiler-owned typed collection operation"
                        .to_string(),
                );
            }
            validate_property_access(path, context, local_types)
        }
        SimpleExpr::Call { target, args } => {
            if typed_collection_operation_for_call(target, args, context, local_types).is_some() {
                for (index, argument) in args.iter().enumerate() {
                    if index == 0 && matches!(argument, SimpleExpr::Identifier(_)) {
                        continue;
                    }
                    validate_expression_access(argument, context, local_types)?;
                }
                typed_collection_operation(target, args, context, local_types)?;
                return Ok(());
            }
            for argument in args {
                validate_expression_access(argument, context, local_types)?;
            }
            let bare_target = target
                .rsplit_once('.')
                .map_or(target.as_str(), |(_, name)| name);
            let known = context.call_signatures.contains_key(target)
                || context.call_signatures.contains_key(bare_target)
                || context.extern_functions.contains(target)
                || context.extern_functions.contains(bare_target)
                || context.internal_function_targets.contains_key(target)
                || context.internal_function_targets.contains_key(bare_target)
                || builtin_host_effect(target).is_some()
                || is_pure_intrinsic(target);
            if known {
                Ok(())
            } else {
                let diagnostic = format!("cannot resolve call '{target}'");
                if matches!(bare_target, "begin_frame" | "end_frame") {
                    Err(format!(
                        "{diagnostic}; the host owns the entire frame lifecycle: remove this call; the host resets construction, invokes the render callback, then validates and publishes or aborts the frame; keep only public graphics drawing API calls in the render callback"
                    ))
                } else {
                    Err(diagnostic)
                }
            }
        }
        SimpleExpr::Binary { lhs, rhs, .. } => {
            validate_expression_access(lhs, context, local_types)?;
            validate_expression_access(rhs, context, local_types)
        }
        SimpleExpr::DefaultValue(_)
        | SimpleExpr::Int(_)
        | SimpleExpr::Float(_)
        | SimpleExpr::Bool(_)
        | SimpleExpr::StringLiteral(_) => Ok(()),
    }
}

fn assignment_types_compatible(
    target_type: TypeId,
    expression_type: TypeId,
    types: &TypeTable,
) -> bool {
    types.assignment_types_are_compatible(target_type, expression_type)
}

fn type_mismatch(subject: &str, expected: TypeId, found: TypeId, types: &TypeTable) -> String {
    format!(
        "{subject} expected {} expression but found {}",
        type_name(expected, types),
        type_name(found, types)
    )
}

fn type_name(type_id: TypeId, types: &TypeTable) -> String {
    types
        .type_info(type_id)
        .map_or_else(|| type_id.to_string(), |info| info.name.clone())
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
    if can_reuse_aggregates {
        for function in functions {
            if included_function_ids.contains(&function.id) {
                direct_by_id[function.storage_index as usize] =
                    analyze_function_effects(function, statements_by_id, &context)?;
            }
        }
    }
    let aggregate_by_id = if can_reuse_aggregates {
        None
    } else {
        Some(build_aggregate_effects(&direct_by_id)?)
    };
    let parameter_storage_kinds = infer_parameter_storage_kinds(functions, &direct_by_id, &context);
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
        let internal_aggregate_write_paths = if let Some(aggregate_by_id) = &aggregate_by_id {
            aggregate_by_id[function.storage_index as usize]
                .writes
                .iter()
                .chain(
                    aggregate_by_id[function.storage_index as usize]
                        .parameter_writes
                        .iter(),
                )
                .cloned()
                .collect()
        } else {
            previous_by_key[&(
                file.path.as_str(),
                function.name.as_str(),
                signature_hash.as_str(),
            )]
                .internal_aggregate_write_paths
                .clone()
        };
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
            parameter_storage_kinds: parameter_storage_kinds
                .get(function.storage_index as usize)
                .cloned()
                .unwrap_or_default(),
            internal_call_sites: direct_by_id[function.storage_index as usize]
                .call_sites
                .clone(),
            internal_direct_write_paths: direct_by_id[function.storage_index as usize]
                .writes
                .iter()
                .chain(
                    direct_by_id[function.storage_index as usize]
                        .parameter_writes
                        .iter(),
                )
                .cloned()
                .collect(),
            internal_aggregate_write_paths,
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

fn infer_parameter_storage_kinds(
    functions: &[FunctionMeta],
    direct_by_id: &[EffectSets],
    context: &AnalysisContext<'_>,
) -> Vec<Vec<ParameterStorageKind>> {
    #[derive(Clone, Copy, Default, PartialEq, Eq)]
    enum Evidence {
        #[default]
        Unknown,
        Aos,
        Soa,
        Dynamic,
    }

    fn merge(current: Evidence, next: Evidence) -> Evidence {
        match (current, next) {
            (Evidence::Dynamic, _) | (_, Evidence::Dynamic) => Evidence::Dynamic,
            (Evidence::Unknown, value) | (value, Evidence::Unknown) => value,
            (Evidence::Aos, Evidence::Aos) => Evidence::Aos,
            (Evidence::Soa, Evidence::Soa) => Evidence::Soa,
            _ => Evidence::Dynamic,
        }
    }

    fn source_parameter_index(path: &str) -> Option<usize> {
        let symbolic = path.strip_prefix('$')?;
        let digits = symbolic.bytes().take_while(u8::is_ascii_digit).count();
        (digits > 0)
            .then(|| symbolic[..digits].parse().ok())
            .flatten()
    }

    let mut evidence = functions
        .iter()
        .map(|function| vec![Evidence::Unknown; function.params.len()])
        .collect::<Vec<_>>();
    let mut has_callers = functions
        .iter()
        .map(|function| vec![false; function.params.len()])
        .collect::<Vec<_>>();

    loop {
        let previous = evidence.clone();
        for (caller_index, effects) in direct_by_id.iter().enumerate() {
            for call_site in &effects.call_sites {
                let Some(target_views) = context
                    .view_parameters_by_function
                    .get(&call_site.target_id)
                else {
                    continue;
                };
                for &parameter_index in target_views {
                    let Some(target) = evidence
                        .get_mut(call_site.target_id as usize)
                        .and_then(|parameters| parameters.get_mut(parameter_index))
                    else {
                        continue;
                    };
                    has_callers[call_site.target_id as usize][parameter_index] = true;
                    let next = match call_site
                        .arguments
                        .get(parameter_index)
                        .and_then(Option::as_deref)
                    {
                        Some(path) if path.contains("[*]") => Evidence::Soa,
                        Some(path) if path.starts_with('$') => source_parameter_index(path)
                            .and_then(|index| previous.get(caller_index)?.get(index).copied())
                            .unwrap_or(Evidence::Unknown),
                        Some(path) if context.path_types.contains_key(path) => Evidence::Aos,
                        _ => Evidence::Dynamic,
                    };
                    *target = merge(*target, next);
                }
            }
        }
        if evidence == previous {
            break;
        }
    }

    evidence
        .into_iter()
        .enumerate()
        .map(|(function_index, parameters)| {
            parameters
                .into_iter()
                .enumerate()
                .map(|(parameter_index, kind)| {
                    if !has_callers[function_index][parameter_index] {
                        return ParameterStorageKind::Dynamic;
                    }
                    match kind {
                        Evidence::Aos => ParameterStorageKind::Aos,
                        Evidence::Soa => ParameterStorageKind::Soa,
                        Evidence::Unknown | Evidence::Dynamic => ParameterStorageKind::Dynamic,
                    }
                })
                .collect()
        })
        .collect()
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

pub(crate) fn compiler_local_types(
    files: &[SourceFile],
    functions: &[FunctionMeta],
    statements_by_id: &[Vec<SimpleStmt>],
    types: &TypeTable,
) -> Result<Vec<CompilerLocalType>, String> {
    let context = build_context(files, functions, types)?;
    let mut out = Vec::new();
    for function in functions {
        let effects = analyze_function_effects(function, statements_by_id, &context)?;
        let file = files
            .get(function.file_id as usize)
            .ok_or_else(|| format!("function '{}' has no source file", function.name))?;
        out.extend(
            effects
                .local_types
                .into_iter()
                .filter_map(|(name, type_id, inferred)| {
                    Some(CompilerLocalType {
                        file: file.path.clone(),
                        function: function.name.clone(),
                        name,
                        type_name: types.type_info(type_id)?.name.clone(),
                        inferred,
                    })
                }),
        );
    }
    Ok(out)
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
        SimpleExpr::DefaultValue(type_id) => type_id.hash(hasher),
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
    let mut descriptor_types = types.clone();
    let typed_collection_descriptors =
        crate::backend::compile_analysis::collect_typed_collection_descriptors(
            files,
            &mut descriptor_types,
        )?;
    let mut globals = BTreeSet::new();
    let mut constants = BTreeMap::new();
    let mut extern_functions = BTreeSet::new();
    let mut extern_effects = BTreeMap::new();
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
                let mut effect_annotations = external
                    .annotations
                    .iter()
                    .filter(|annotation| annotation.name == "effects");
                let effect_annotation = effect_annotations.next();
                if effect_annotations.next().is_some() {
                    return Err(format!(
                        "extern function '{}' may declare @effects only once",
                        external.name
                    ));
                }
                let capabilities = if let Some(annotation) = effect_annotation {
                    Some(
                        annotation
                            .arguments
                            .iter()
                            .map(|argument| argument.text.trim().to_string())
                            .collect::<Vec<_>>(),
                    )
                } else {
                    None
                };
                if let Some(capabilities) = &capabilities {
                    if let Some(invalid) = capabilities
                        .iter()
                        .find(|capability| !is_host_capability(capability))
                    {
                        return Err(format!(
                            "extern function '{}' declares unknown host effect capability '{}'",
                            external.name, invalid
                        ));
                    }
                }
                if let Some(previous) =
                    extern_effects.insert(external.name.clone(), capabilities.clone())
                {
                    if previous != capabilities {
                        return Err(format!(
                            "extern overloads named '{}' must declare identical @effects metadata",
                            external.name
                        ));
                    }
                }
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
                        source_path: file.path.clone(),
                        trusted_graphics_source: crate::frontend::module_graph::is_recognized_graphics_implementation_source(
                            &file.path,
                            &file.original_content,
                        ) || crate::frontend::module_graph::is_explicit_graphics_test_seam_path(&file.path),
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
    let mut internal_function_targets = BTreeMap::<String, Vec<u32>>::new();
    for function in functions {
        internal_function_targets
            .entry(function.name.clone())
            .or_default()
            .push(function.storage_index);
        if !function.module_alias.is_empty() {
            internal_function_targets
                .entry(format!("{}.{}", function.module_alias, function.name))
                .or_default()
                .push(function.storage_index);
        }
    }
    let mut fingerprint_hasher = DefaultHasher::new();
    format!("{structs:?}|{global_types:?}|{constants:?}|{resolved_externs:?}|{extern_effects:?}|{internal_function_targets:?}|{typed_collection_descriptors:?}")
        .hash(&mut fingerprint_hasher);
    let fingerprint = fingerprint_hasher.finish();
    Ok(AnalysisContext {
        globals,
        constants,
        view_parameters_by_function,
        fixed_parameter_capacities,
        extern_functions,
        extern_effects,
        internal_function_targets,
        collection_capacities,
        path_types,
        field_types,
        typed_collection_descriptors,
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
                let inferred = type_id.is_none();
                let inferred_type =
                    type_id.or_else(|| expression_type(expression, context, local_types, aliases));
                analyze_expression(expression, context, locals, local_types, aliases, effects);
                locals.insert(name.clone());
                if let Some(type_id) = inferred_type {
                    effects.local_types.push((name.clone(), type_id, inferred));
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
                let bound_path = format!("{normalized}.max_length");
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

fn analyze_typed_collection_operation(
    target: &str,
    args: &[SimpleExpr],
    context: &AnalysisContext<'_>,
    locals: &BTreeSet<String>,
    local_types: &BTreeMap<String, TypeId>,
    aliases: &BTreeMap<String, String>,
    effects: &mut EffectSets,
) -> bool {
    let Some(operation) = typed_collection_operation_for_call(target, args, context, local_types)
    else {
        return false;
    };
    let expected_arity = operation.expected_arity();
    let valid_descriptor = if args.len() == expected_arity {
        args.first()
            .and_then(|first| exact_typed_collection_path(first, context, local_types))
            .filter(|(path, descriptor)| {
                !locals.contains(root_name(path))
                    && typed_collection_descriptor_supports_operation(operation, descriptor)
            })
    } else {
        None
    };
    if let Some((path, _)) = valid_descriptor {
        match operation {
            TypedCollectionOperation::PoolPush => {
                effects.insert_read(format!("{path}.count"));
                effects.insert_write(format!("{path}.count"));
                effects.insert_write(format!("{path}.values[*]"));
            }
            TypedCollectionOperation::PoolRemove => {
                effects.insert_read(format!("{path}.count"));
                effects.insert_read(format!("{path}.values[*]"));
                effects.insert_write(format!("{path}.count"));
                effects.insert_write(format!("{path}.values[*]"));
            }
            TypedCollectionOperation::PoolClear => {
                effects.insert_write(format!("{path}.count"));
                effects.insert_write(format!("{path}.values[*]"));
            }
            TypedCollectionOperation::PoolCount => {
                effects.insert_read(format!("{path}.count"));
            }
            TypedCollectionOperation::PoolCanPush | TypedCollectionOperation::PoolCanRemove => {
                effects.insert_read(format!("{path}.count"));
            }
            TypedCollectionOperation::PoolCapacity => {}
            TypedCollectionOperation::StablePoolInsert => {
                effects.insert_read(format!("{path}.count"));
                effects.insert_read(format!("{path}.occupied[*]"));
                effects.insert_write(format!("{path}.count"));
                effects.insert_write(format!("{path}.occupied[*]"));
                effects.insert_write(format!("{path}.values[*]"));
            }
            TypedCollectionOperation::StablePoolRemove => {
                effects.insert_read(format!("{path}.count"));
                effects.insert_read(format!("{path}.occupied[*]"));
                effects.insert_write(format!("{path}.count"));
                effects.insert_write(format!("{path}.occupied[*]"));
                effects.insert_write(format!("{path}.values[*]"));
            }
            TypedCollectionOperation::StablePoolClear => {
                effects.insert_write(format!("{path}.count"));
                effects.insert_write(format!("{path}.occupied[*]"));
                effects.insert_write(format!("{path}.values[*]"));
            }
            TypedCollectionOperation::StablePoolCount => {
                effects.insert_read(format!("{path}.count"));
            }
            TypedCollectionOperation::StablePoolCanInsert
            | TypedCollectionOperation::StablePoolCanRemove => {
                effects.insert_read(format!("{path}.count"));
                effects.insert_read(format!("{path}.occupied[*]"));
            }
            TypedCollectionOperation::StablePoolCapacity => {}
            TypedCollectionOperation::MapPut => {
                effects.insert_read(format!("{path}.count"));
                effects.insert_read(format!("{path}.occupied[*]"));
                effects.insert_read(format!("{path}.keys[*]"));
                effects.insert_write(format!("{path}.count"));
                effects.insert_write(format!("{path}.occupied[*]"));
                effects.insert_write(format!("{path}.keys[*]"));
                effects.insert_write(format!("{path}.values[*]"));
            }
            TypedCollectionOperation::MapGet => {
                effects.insert_read(format!("{path}.count"));
                effects.insert_read(format!("{path}.occupied[*]"));
                effects.insert_read(format!("{path}.keys[*]"));
                effects.insert_read(format!("{path}.values[*]"));
            }
            TypedCollectionOperation::MapCanGet => {
                effects.insert_read(format!("{path}.count"));
                effects.insert_read(format!("{path}.occupied[*]"));
                effects.insert_read(format!("{path}.keys[*]"));
            }
            TypedCollectionOperation::MapContains => {
                effects.insert_read(format!("{path}.count"));
                effects.insert_read(format!("{path}.occupied[*]"));
                effects.insert_read(format!("{path}.keys[*]"));
            }
            TypedCollectionOperation::MapRemove => {
                effects.insert_read(format!("{path}.count"));
                effects.insert_read(format!("{path}.occupied[*]"));
                effects.insert_read(format!("{path}.keys[*]"));
                effects.insert_write(format!("{path}.count"));
                effects.insert_write(format!("{path}.occupied[*]"));
                effects.insert_write(format!("{path}.keys[*]"));
                effects.insert_write(format!("{path}.values[*]"));
            }
            TypedCollectionOperation::MapCanPut | TypedCollectionOperation::MapCanRemove => {
                effects.insert_read(format!("{path}.count"));
                effects.insert_read(format!("{path}.occupied[*]"));
                effects.insert_read(format!("{path}.keys[*]"));
            }
            TypedCollectionOperation::SetAdd => {
                effects.insert_read(format!("{path}.count"));
                effects.insert_read(format!("{path}.occupied[*]"));
                effects.insert_read(format!("{path}.keys[*]"));
                effects.insert_write(format!("{path}.count"));
                effects.insert_write(format!("{path}.occupied[*]"));
                effects.insert_write(format!("{path}.keys[*]"));
            }
            TypedCollectionOperation::SetContains => {
                effects.insert_read(format!("{path}.count"));
                effects.insert_read(format!("{path}.occupied[*]"));
                effects.insert_read(format!("{path}.keys[*]"));
            }
            TypedCollectionOperation::SetRemove => {
                effects.insert_read(format!("{path}.count"));
                effects.insert_read(format!("{path}.occupied[*]"));
                effects.insert_read(format!("{path}.keys[*]"));
                effects.insert_write(format!("{path}.count"));
                effects.insert_write(format!("{path}.occupied[*]"));
                effects.insert_write(format!("{path}.keys[*]"));
            }
            TypedCollectionOperation::SetCanAdd | TypedCollectionOperation::SetCanRemove => {
                effects.insert_read(format!("{path}.count"));
                effects.insert_read(format!("{path}.occupied[*]"));
                effects.insert_read(format!("{path}.keys[*]"));
            }
            TypedCollectionOperation::QueuePush => {
                effects.insert_read(format!("{path}.count"));
                effects.insert_read(format!("{path}.head"));
                effects.insert_write(format!("{path}.count"));
                effects.insert_write(format!("{path}.head"));
                effects.insert_write(format!("{path}.values[*]"));
            }
            TypedCollectionOperation::QueueCanPush
            | TypedCollectionOperation::QueueCanPop
            | TypedCollectionOperation::QueueCanPeek => {
                effects.insert_read(format!("{path}.count"));
                effects.insert_read(format!("{path}.head"));
            }
            TypedCollectionOperation::QueuePop => {
                effects.insert_read(format!("{path}.count"));
                effects.insert_read(format!("{path}.head"));
                effects.insert_read(format!("{path}.values[*]"));
                effects.insert_write(format!("{path}.count"));
                effects.insert_write(format!("{path}.head"));
                effects.insert_write(format!("{path}.values[*]"));
            }
            TypedCollectionOperation::QueuePeek => {
                effects.insert_read(format!("{path}.count"));
                effects.insert_read(format!("{path}.head"));
                effects.insert_read(format!("{path}.values[*]"));
            }
            TypedCollectionOperation::QueuePhysicalIndex => {
                effects.insert_read(format!("{path}.count"));
                effects.insert_read(format!("{path}.head"));
            }
            TypedCollectionOperation::QueueCount => {
                effects.insert_read(format!("{path}.count"));
            }
            TypedCollectionOperation::QueueCapacity => {}
            TypedCollectionOperation::QueueClear => {
                effects.insert_write(format!("{path}.count"));
                effects.insert_write(format!("{path}.head"));
                effects.insert_write(format!("{path}.values[*]"));
            }
            TypedCollectionOperation::QueueOverwriteOldest => {
                effects.insert_read(format!("{path}.count"));
                effects.insert_read(format!("{path}.head"));
                effects.insert_write(format!("{path}.count"));
                effects.insert_write(format!("{path}.head"));
                effects.insert_write(format!("{path}.values[*]"));
            }
            TypedCollectionOperation::RingBufferPush => {
                effects.insert_read(format!("{path}.count"));
                effects.insert_read(format!("{path}.head"));
                effects.insert_write(format!("{path}.count"));
                effects.insert_write(format!("{path}.head"));
                effects.insert_write(format!("{path}.values[*]"));
            }
            TypedCollectionOperation::RingBufferPop => {
                effects.insert_read(format!("{path}.count"));
                effects.insert_read(format!("{path}.head"));
                effects.insert_read(format!("{path}.values[*]"));
                effects.insert_write(format!("{path}.count"));
                effects.insert_write(format!("{path}.head"));
                effects.insert_write(format!("{path}.values[*]"));
            }
            TypedCollectionOperation::RingBufferPeek => {
                effects.insert_read(format!("{path}.count"));
                effects.insert_read(format!("{path}.head"));
                effects.insert_read(format!("{path}.values[*]"));
            }
            TypedCollectionOperation::RingBufferPhysicalIndex => {
                effects.insert_read(format!("{path}.count"));
                effects.insert_read(format!("{path}.head"));
            }
            TypedCollectionOperation::RingBufferCount => {
                effects.insert_read(format!("{path}.count"));
            }
            TypedCollectionOperation::RingBufferCapacity => {}
            TypedCollectionOperation::RingBufferClear => {
                effects.insert_write(format!("{path}.count"));
                effects.insert_write(format!("{path}.head"));
                effects.insert_write(format!("{path}.values[*]"));
            }
            TypedCollectionOperation::RingBufferCanPush
            | TypedCollectionOperation::RingBufferCanPop
            | TypedCollectionOperation::RingBufferCanPeek => {
                effects.insert_read(format!("{path}.count"));
                effects.insert_read(format!("{path}.head"));
            }
            TypedCollectionOperation::RingBufferOverwriteOldest => {
                effects.insert_read(format!("{path}.count"));
                effects.insert_read(format!("{path}.head"));
                effects.insert_write(format!("{path}.count"));
                effects.insert_write(format!("{path}.head"));
                effects.insert_write(format!("{path}.values[*]"));
            }
            TypedCollectionOperation::PriorityQueuePush => {
                effects.insert_read(format!("{path}.count"));
                effects.insert_read(format!("{path}.next_order"));
                effects.insert_read(format!("{path}.priority[*]"));
                effects.insert_read(format!("{path}.order[*]"));
                effects.insert_write(format!("{path}.count"));
                effects.insert_write(format!("{path}.next_order"));
                effects.insert_write(format!("{path}.priority[*]"));
                effects.insert_write(format!("{path}.order[*]"));
                effects.insert_write(format!("{path}.values[*]"));
            }
            TypedCollectionOperation::PriorityQueuePop => {
                effects.insert_read(format!("{path}.count"));
                effects.insert_read(format!("{path}.priority[*]"));
                effects.insert_read(format!("{path}.order[*]"));
                effects.insert_read(format!("{path}.values[*]"));
                effects.insert_write(format!("{path}.count"));
                effects.insert_write(format!("{path}.priority[*]"));
                effects.insert_write(format!("{path}.order[*]"));
                effects.insert_write(format!("{path}.values[*]"));
            }
            TypedCollectionOperation::PriorityQueuePeek => {
                effects.insert_read(format!("{path}.count"));
                effects.insert_read(format!("{path}.priority[*]"));
                effects.insert_read(format!("{path}.order[*]"));
                effects.insert_read(format!("{path}.values[*]"));
            }
            TypedCollectionOperation::PriorityQueuePeekPriority => {
                effects.insert_read(format!("{path}.count"));
                effects.insert_read(format!("{path}.priority[*]"));
                effects.insert_read(format!("{path}.order[*]"));
            }
            TypedCollectionOperation::PriorityQueueCount => {
                effects.insert_read(format!("{path}.count"));
            }
            TypedCollectionOperation::PriorityQueueCapacity => {}
            TypedCollectionOperation::PriorityQueueClear => {
                effects.insert_write(format!("{path}.count"));
                effects.insert_write(format!("{path}.next_order"));
                effects.insert_write(format!("{path}.priority[*]"));
                effects.insert_write(format!("{path}.order[*]"));
                effects.insert_write(format!("{path}.values[*]"));
            }
            TypedCollectionOperation::PriorityQueueCanPush => {
                effects.insert_read(format!("{path}.count"));
                effects.insert_read(format!("{path}.next_order"));
            }
            TypedCollectionOperation::PriorityQueueCanPop
            | TypedCollectionOperation::PriorityQueueCanPeek => {
                effects.insert_read(format!("{path}.count"));
            }
            TypedCollectionOperation::GridGet => {
                effects.insert_read(format!("{path}.values[*]"));
            }
            TypedCollectionOperation::GridSet => {
                effects.insert_write(format!("{path}.values[*]"));
            }
            TypedCollectionOperation::GridCanAccess | TypedCollectionOperation::GridCapacity => {}
            TypedCollectionOperation::GridClear => {
                effects.insert_write(format!("{path}.values[*]"));
            }
            TypedCollectionOperation::BitsetTest => {
                effects.insert_read(format!("{path}.words[*]"));
            }
            TypedCollectionOperation::BitsetSet => {
                effects.insert_read(format!("{path}.words[*]"));
                effects.insert_write(format!("{path}.words[*]"));
            }
            TypedCollectionOperation::BitsetCanAccess
            | TypedCollectionOperation::BitsetCapacity => {}
            TypedCollectionOperation::BitsetClear => {
                effects.insert_write(format!("{path}.words[*]"));
            }
        }
        for argument in args.iter().skip(1) {
            analyze_expression(argument, context, locals, local_types, aliases, effects);
        }
    } else {
        for argument in args {
            analyze_expression(argument, context, locals, local_types, aliases, effects);
        }
    }
    true
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
        SimpleExpr::DefaultValue(_)
        | SimpleExpr::Int(_)
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
            if analyze_typed_collection_operation(
                target,
                args,
                context,
                locals,
                local_types,
                aliases,
                effects,
            ) {
                return;
            }
            let mut target_id = resolve_internal_call(target, args, context, local_types, aliases);
            if let Some(target_id) = target_id {
                effects.calls.insert(
                    target
                        .rsplit_once('.')
                        .map_or_else(|| target.clone(), |(_, name)| name.to_string()),
                );
                effects.record_call_site(
                    target_id,
                    args.iter()
                        .map(|argument| expression_effect_path(argument, context, locals, aliases))
                        .collect(),
                );
            } else {
                let mut handled = false;
                let mut host_call = false;
                if let Some(capability) = builtin_host_effect(target) {
                    handled = true;
                    host_call = true;
                    effects.host_calls.insert(target.clone());
                    effects.host_effects.insert(FunctionHostEffect {
                        function: target.clone(),
                        capability: capability.to_string(),
                    });
                }
                if context.extern_functions.contains(target) {
                    handled = true;
                    host_call = true;
                    effects.host_calls.insert(target.clone());
                    let capabilities = context.extern_effects.get(target).and_then(Option::as_ref);
                    if capabilities.is_none() {
                        effects.host_effects.insert(FunctionHostEffect {
                            function: target.clone(),
                            capability: "unknown".to_string(),
                        });
                    } else if let Some(capabilities) = capabilities {
                        effects
                            .host_effects
                            .extend(capabilities.iter().map(|capability| FunctionHostEffect {
                                function: target.clone(),
                                capability: capability.clone(),
                            }));
                    }
                }
                if host_call {
                    effects.record_host_call(target);
                }
                if let Some([sole_target_id]) = context
                    .internal_function_targets
                    .get(target)
                    .map(Vec::as_slice)
                {
                    handled = true;
                    target_id = Some(*sole_target_id);
                    effects.calls.insert(target.clone());
                    effects.record_call_site(
                        *sole_target_id,
                        args.iter()
                            .map(|argument| {
                                expression_effect_path(argument, context, locals, aliases)
                            })
                            .collect(),
                    );
                }
                if !handled && !is_pure_intrinsic(target) {
                    effects.host_calls.insert(target.clone());
                    effects.host_effects.insert(FunctionHostEffect {
                        function: target.clone(),
                        capability: "unknown".to_string(),
                    });
                    effects.record_host_call(target);
                }
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

pub(crate) fn is_host_capability(value: &str) -> bool {
    matches!(
        value,
        "graphics"
            | "audio"
            | "storage"
            | "network"
            | "nondeterministic"
            | "platform"
            | "memory"
            | "code_swap"
    )
}

fn builtin_host_effect(target: &str) -> Option<&'static str> {
    matches!(
        target,
        "print_i32" | "print_int" | "print_char" | "print_string"
    )
    .then_some("platform")
}

fn is_pure_intrinsic(target: &str) -> bool {
    matches!(
        target,
        "fixed32_from_i32"
            | "fixed32_to_i32"
            | "fixed32_mul"
            | "fixed32_div"
            | "fixed32_from_ratio"
            | "i32_to_f32"
            | "f32_to_i32"
            | "sin_fast"
            | "cos_fast"
    )
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
        SimpleExpr::DefaultValue(type_id) => Some(*type_id),
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
            // Indexed wildcard paths are compiler-global metadata.  A local
            // or parameter with the same root must be resolved first, or an
            // unrelated global array can override its element type.
            let root = root_name(collection_path);
            let has_lexical_root = local_types.contains_key(root) || aliases.contains_key(root);
            if !has_lexical_root {
                if let Some(type_id) = context
                    .path_types
                    .get(&indexed_state_path(collection_path, suffix))
                {
                    return Some(*type_id);
                }
            }
            let collection = path_type(collection_path, context, local_types, aliases)?;
            let element = context.types.indexed_element_type_id(collection)?;
            field_suffix_type(element, suffix, &context.field_types)
        }
        SimpleExpr::Call { target, args } => {
            if let Some(operation) =
                typed_collection_operation_for_call(target, args, context, local_types)
            {
                return Some(operation.return_type());
            }
            match target.as_str() {
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
            }
        }
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
    let root = root_name(path);
    let suffix = path.strip_prefix(root)?.trim_start_matches('.');

    // A local or parameter shadows a global with the same path root.  Resolve
    // the complete local path before falling back to the compiler's global
    // path table; otherwise a workspace file can make a local f32 look like
    // an unrelated global i32 during semantic validation.
    if let Some(root_type) = local_types.get(root).copied() {
        return field_suffix_type(root_type, suffix, &context.field_types);
    }
    if let Some(alias) = aliases.get(root) {
        let root_type = context.path_types.get(alias).copied()?;
        return field_suffix_type(root_type, suffix, &context.field_types);
    }
    context.path_types.get(path).copied()
}

fn field_suffix_type(
    mut type_id: TypeId,
    suffix: &str,
    field_types: &BTreeMap<TypeId, BTreeMap<String, TypeId>>,
) -> Option<TypeId> {
    if suffix.is_empty() {
        return Some(type_id);
    }
    if let Some(field_type) = field_types
        .get(&type_id)
        .and_then(|fields| fields.get(suffix.trim_start_matches('.')))
    {
        return Some(*field_type);
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
    aggregate
        .host_effects
        .extend(direct.host_effects.iter().cloned());
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
        SimpleExpr::DefaultValue(type_id) => format!("default({})", type_id),
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
    use super::*;
    use crate::backend::jit::JitProcess;
    use crate::compiler::{Compiler, FunctionMeta, SourceFile};
    use crate::frontend::types::TypeTable;
    use crate::identity::{CanonicalSourcePath, SymbolId};
    use crate::ir::hir::{AssignTarget, SimpleStmt};
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::Path;

    fn typed_pool_fixture(extra: &str) -> (TypeTable, Vec<SourceFile>) {
        let source = format!(
            "global actors: pool<i32, 2>;\nglobal queue: queue<i32, 2>;\nglobal floats: pool<f32, 2>;\n{extra}"
        );
        let mut types = TypeTable::new();
        for type_name in ["pool<i32, 2>", "queue<i32, 2>", "pool<f32, 2>"] {
            types
                .resolve_or_intern(type_name)
                .expect("typed collection fixture type");
        }
        let file = SourceFile {
            path: "pool_semantics.stasis".to_string(),
            content: source.clone(),
            original_content: source,
            hash: 0,
            functions: Vec::new(),
        };
        (types, vec![file])
    }

    fn pool_context<'a>(types: &'a TypeTable, files: &[SourceFile]) -> AnalysisContext<'a> {
        build_context(files, &[], types).expect("typed collection analysis context")
    }

    fn typed_stable_pool_fixture(extra: &str) -> (TypeTable, Vec<SourceFile>) {
        let source = format!(
            "global actors: stable_pool<i32, 2>;\nglobal empty_actors: stable_pool<i32, 0>;\nglobal floats: stable_pool<f32, 2>;\nglobal queue: queue<i32, 2>;\n{extra}"
        );
        let mut types = TypeTable::new();
        for type_name in [
            "stable_pool<i32, 2>",
            "stable_pool<i32, 0>",
            "stable_pool<f32, 2>",
            "queue<i32, 2>",
        ] {
            types
                .resolve_or_intern(type_name)
                .expect("typed stable pool fixture type");
        }
        let file = SourceFile {
            path: "stable_pool_semantics.stasis".to_string(),
            content: source.clone(),
            original_content: source,
            hash: 0,
            functions: Vec::new(),
        };
        (types, vec![file])
    }

    fn stable_pool_context<'a>(types: &'a TypeTable, files: &[SourceFile]) -> AnalysisContext<'a> {
        build_context(files, &[], types).expect("typed stable pool analysis context")
    }

    fn typed_queue_fixture(extra: &str) -> (TypeTable, Vec<SourceFile>) {
        let source = format!(
            "global events: queue<i32, 2>;\nglobal empty_events: queue<i32, 0>;\nglobal actors: pool<i32, 2>;\n{extra}"
        );
        let mut types = TypeTable::new();
        for type_name in ["queue<i32, 2>", "queue<i32, 0>", "pool<i32, 2>"] {
            types
                .resolve_or_intern(type_name)
                .expect("typed queue fixture type");
        }
        let file = SourceFile {
            path: "queue_semantics.stasis".to_string(),
            content: source.clone(),
            original_content: source,
            hash: 0,
            functions: Vec::new(),
        };
        (types, vec![file])
    }

    fn queue_context<'a>(types: &'a TypeTable, files: &[SourceFile]) -> AnalysisContext<'a> {
        build_context(files, &[], types).expect("typed queue analysis context")
    }

    fn typed_priority_queue_fixture(extra: &str) -> (TypeTable, Vec<SourceFile>) {
        let source = format!(
            "global events: priority_queue<i32, 2>;\nglobal empty_events: priority_queue<i32, 0>;\nglobal float_events: priority_queue<f32, 2>;\nglobal queue: queue<i32, 2>;\n{extra}"
        );
        let mut types = TypeTable::new();
        for type_name in [
            "priority_queue<i32, 2>",
            "priority_queue<i32, 0>",
            "priority_queue<f32, 2>",
            "queue<i32, 2>",
        ] {
            types
                .resolve_or_intern(type_name)
                .expect("typed priority queue fixture type");
        }
        let file = SourceFile {
            path: "priority_queue_semantics.stasis".to_string(),
            content: source.clone(),
            original_content: source,
            hash: 0,
            functions: Vec::new(),
        };
        (types, vec![file])
    }

    fn priority_queue_context<'a>(
        types: &'a TypeTable,
        files: &[SourceFile],
    ) -> AnalysisContext<'a> {
        build_context(files, &[], types).expect("typed priority queue analysis context")
    }

    fn typed_ring_buffer_fixture(extra: &str) -> (TypeTable, Vec<SourceFile>) {
        let source = format!(
            "global history: ring_buffer<i32, 2>;\nglobal empty_history: ring_buffer<i32, 0>;\nglobal actors: pool<i32, 2>;\nglobal events: queue<i32, 2>;\n{extra}"
        );
        let mut types = TypeTable::new();
        for type_name in [
            "ring_buffer<i32, 2>",
            "ring_buffer<i32, 0>",
            "pool<i32, 2>",
            "queue<i32, 2>",
        ] {
            types
                .resolve_or_intern(type_name)
                .expect("typed ring buffer fixture type");
        }
        let file = SourceFile {
            path: "ring_buffer_semantics.stasis".to_string(),
            content: source.clone(),
            original_content: source,
            hash: 0,
            functions: Vec::new(),
        };
        (types, vec![file])
    }

    fn ring_buffer_context<'a>(types: &'a TypeTable, files: &[SourceFile]) -> AnalysisContext<'a> {
        build_context(files, &[], types).expect("typed ring buffer analysis context")
    }

    fn typed_map_set_fixture(extra: &str) -> (TypeTable, Vec<SourceFile>) {
        let source = format!(
            "global entries: map<i32, i32, 2>;\nglobal empty_entries: map<i32, i32, 0>;\nglobal value_floats: map<i32, f32, 2>;\nglobal wide_keys: map<u32, i32, 2>;\nglobal members: set<i32, 2>;\nglobal empty_members: set<i32, 0>;\nglobal wide_member_keys: set<u32, 2>;\nglobal actors: pool<i32, 2>;\n{extra}"
        );
        let mut types = TypeTable::new();
        for type_name in [
            "map<i32, i32, 2>",
            "map<i32, i32, 0>",
            "map<i32, f32, 2>",
            "map<u32, i32, 2>",
            "set<i32, 2>",
            "set<i32, 0>",
            "set<u32, 2>",
            "pool<i32, 2>",
        ] {
            types
                .resolve_or_intern(type_name)
                .expect("typed map/set fixture type");
        }
        let file = SourceFile {
            path: "map_set_semantics.stasis".to_string(),
            content: source.clone(),
            original_content: source,
            hash: 0,
            functions: Vec::new(),
        };
        (types, vec![file])
    }

    fn map_set_context<'a>(types: &'a TypeTable, files: &[SourceFile]) -> AnalysisContext<'a> {
        build_context(files, &[], types).expect("typed map/set analysis context")
    }

    fn typed_grid_bitset_fixture(extra: &str) -> (TypeTable, Vec<SourceFile>) {
        let source = format!(
            "global cells: grid<i32, 2, 3>;\nglobal float_cells: grid<f32, 2, 3>;\nglobal bits: bitset<33>;\nglobal queue: queue<i32, 2>;\n{extra}"
        );
        let mut types = TypeTable::new();
        for type_name in [
            "grid<i32, 2, 3>",
            "grid<f32, 2, 3>",
            "bitset<33>",
            "queue<i32, 2>",
        ] {
            types
                .resolve_or_intern(type_name)
                .expect("typed grid/bitset fixture type");
        }
        let file = SourceFile {
            path: "grid_bitset_semantics.stasis".to_string(),
            content: source.clone(),
            original_content: source,
            hash: 0,
            functions: Vec::new(),
        };
        (types, vec![file])
    }

    fn grid_bitset_context<'a>(types: &'a TypeTable, files: &[SourceFile]) -> AnalysisContext<'a> {
        build_context(files, &[], types).expect("typed grid/bitset analysis context")
    }

    fn pool_call(target: &str, args: Vec<SimpleExpr>) -> SimpleExpr {
        SimpleExpr::Call {
            target: target.to_string(),
            args,
        }
    }

    fn test_function(
        name: &str,
        param_names: Vec<String>,
        params: Vec<TypeId>,
        return_type: TypeId,
    ) -> FunctionMeta {
        let path = CanonicalSourcePath::project_relative("pool_semantics.stasis")
            .expect("canonical fixture path");
        FunctionMeta {
            id: 0,
            symbol_id: SymbolId::function(&path, name, "test"),
            storage_index: 0,
            name: name.to_string(),
            module_alias: String::new(),
            name_hash: 0,
            file_id: 0,
            source_range: 0..0,
            signature_range: 0..0,
            signature_hash: 0,
            body_hash: 0,
            param_names,
            params,
            return_type,
            inline: false,
            effect_contract: None,
            requires_contract: None,
            dependencies: Vec::new(),
            dependents: Vec::new(),
            call_sites: Vec::new(),
            dirty: false,
        }
    }

    fn compiler_rejects(source: &str, expected: &str) {
        let mut compiler = Compiler::new();
        compiler.upsert_file("pool_semantics.stasis", source);
        let error = compiler
            .check()
            .expect_err("pool semantic fixture must be rejected");
        let message = format!("{error:?}");
        assert!(
            message.contains(expected),
            "expected diagnostic containing {expected:?}, got {message}"
        );
    }

    #[test]
    fn typed_map_and_set_operations_have_fixed_return_types_and_explicit_effects() {
        let (types, files) = typed_map_set_fixture("");
        let context = map_set_context(&types, &files);
        let local_types = BTreeMap::new();
        let locals = BTreeSet::new();
        let aliases = BTreeMap::new();
        let operations = [
            (
                "put",
                vec![
                    SimpleExpr::Identifier("entries".to_string()),
                    SimpleExpr::Int(1),
                    SimpleExpr::Int(7),
                ],
                TYPE_ID_VOID,
                vec!["entries.count", "entries.keys[*]", "entries.occupied[*]"],
                vec![
                    "entries.count",
                    "entries.keys[*]",
                    "entries.occupied[*]",
                    "entries.values[*]",
                ],
            ),
            (
                "can_get",
                vec![
                    SimpleExpr::Identifier("entries".to_string()),
                    SimpleExpr::Int(1),
                ],
                TYPE_ID_BOOL,
                vec!["entries.count", "entries.keys[*]", "entries.occupied[*]"],
                Vec::<&str>::new(),
            ),
            (
                "get",
                vec![
                    SimpleExpr::Identifier("entries".to_string()),
                    SimpleExpr::Int(1),
                ],
                TYPE_ID_I32,
                vec![
                    "entries.count",
                    "entries.keys[*]",
                    "entries.occupied[*]",
                    "entries.values[*]",
                ],
                Vec::<&str>::new(),
            ),
            (
                "contains",
                vec![
                    SimpleExpr::Identifier("entries".to_string()),
                    SimpleExpr::Int(1),
                ],
                TYPE_ID_BOOL,
                vec!["entries.count", "entries.keys[*]", "entries.occupied[*]"],
                Vec::<&str>::new(),
            ),
            (
                "remove",
                vec![
                    SimpleExpr::Identifier("entries".to_string()),
                    SimpleExpr::Int(1),
                ],
                TYPE_ID_VOID,
                vec!["entries.count", "entries.keys[*]", "entries.occupied[*]"],
                vec![
                    "entries.count",
                    "entries.keys[*]",
                    "entries.occupied[*]",
                    "entries.values[*]",
                ],
            ),
            (
                "add",
                vec![
                    SimpleExpr::Identifier("members".to_string()),
                    SimpleExpr::Int(1),
                ],
                TYPE_ID_VOID,
                vec!["members.count", "members.keys[*]", "members.occupied[*]"],
                vec!["members.count", "members.keys[*]", "members.occupied[*]"],
            ),
            (
                "contains",
                vec![
                    SimpleExpr::Identifier("members".to_string()),
                    SimpleExpr::Int(1),
                ],
                TYPE_ID_BOOL,
                vec!["members.count", "members.keys[*]", "members.occupied[*]"],
                Vec::<&str>::new(),
            ),
            (
                "remove",
                vec![
                    SimpleExpr::Identifier("members".to_string()),
                    SimpleExpr::Int(1),
                ],
                TYPE_ID_VOID,
                vec!["members.count", "members.keys[*]", "members.occupied[*]"],
                vec!["members.count", "members.keys[*]", "members.occupied[*]"],
            ),
            (
                "can_put",
                vec![
                    SimpleExpr::Identifier("entries".to_string()),
                    SimpleExpr::Int(1),
                ],
                TYPE_ID_BOOL,
                vec!["entries.count", "entries.keys[*]", "entries.occupied[*]"],
                Vec::<&str>::new(),
            ),
            (
                "can_remove",
                vec![
                    SimpleExpr::Identifier("entries".to_string()),
                    SimpleExpr::Int(1),
                ],
                TYPE_ID_BOOL,
                vec!["entries.count", "entries.keys[*]", "entries.occupied[*]"],
                Vec::<&str>::new(),
            ),
            (
                "can_add",
                vec![
                    SimpleExpr::Identifier("members".to_string()),
                    SimpleExpr::Int(1),
                ],
                TYPE_ID_BOOL,
                vec!["members.count", "members.keys[*]", "members.occupied[*]"],
                Vec::<&str>::new(),
            ),
            (
                "can_remove",
                vec![
                    SimpleExpr::Identifier("members".to_string()),
                    SimpleExpr::Int(1),
                ],
                TYPE_ID_BOOL,
                vec!["members.count", "members.keys[*]", "members.occupied[*]"],
                Vec::<&str>::new(),
            ),
        ];

        for (target, args, expected_type, expected_reads, expected_writes) in operations {
            let expression = pool_call(target, args);
            assert_eq!(
                expression_type(&expression, &context, &local_types, &aliases),
                Some(expected_type),
                "{target} return type"
            );
            validate_expression_access(&expression, &context, &local_types)
                .expect("valid typed map/set operation");
            let mut effects = EffectSets::default();
            analyze_expression(
                &expression,
                &context,
                &locals,
                &local_types,
                &aliases,
                &mut effects,
            );
            assert_eq!(
                effects.reads.iter().map(String::as_str).collect::<Vec<_>>(),
                expected_reads,
                "{target} reads"
            );
            assert_eq!(
                effects
                    .writes
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
                expected_writes,
                "{target} writes"
            );
            assert!(effects.calls.is_empty(), "{target} became an internal call");
            assert!(effects.host_calls.is_empty(), "{target} leaked a host call");
            assert!(
                effects.host_effects.is_empty(),
                "{target} leaked a host capability"
            );
        }

        for (target, path, arity) in [
            ("put", "empty_entries", 3),
            ("can_get", "empty_entries", 2),
            ("get", "empty_entries", 2),
            ("contains", "empty_entries", 2),
            ("remove", "empty_entries", 2),
            ("can_put", "empty_entries", 2),
            ("can_remove", "empty_entries", 2),
            ("add", "empty_members", 2),
            ("contains", "empty_members", 2),
            ("remove", "empty_members", 2),
            ("can_add", "empty_members", 2),
            ("can_remove", "empty_members", 2),
        ] {
            let mut args = vec![SimpleExpr::Identifier(path.to_string())];
            while args.len() < arity {
                args.push(SimpleExpr::Int(1));
            }
            typed_collection_operation(target, &args, &context, &local_types)
                .expect("zero-capacity map/set operation must be valid")
                .expect("map/set operation should be compiler-owned");
        }
    }

    #[test]
    fn typed_map_and_set_operations_require_exact_i32_persistent_paths() {
        let (types, files) = typed_map_set_fixture("");
        let context = map_set_context(&types, &files);
        let mut local_types = BTreeMap::new();
        local_types.insert(
            "entries".to_string(),
            types.resolve("map<i32, i32, 2>").expect("map type id"),
        );

        let local_error = validate_expression_access(
            &pool_call(
                "get",
                vec![
                    SimpleExpr::Identifier("entries".to_string()),
                    SimpleExpr::Int(1),
                ],
            ),
            &context,
            &local_types,
        )
        .expect_err("local map must not be a persistent path");
        assert!(local_error.contains("only be used as the first argument"));

        local_types.clear();
        let descendant_error = validate_expression_access(
            &pool_call(
                "get",
                vec![
                    SimpleExpr::Identifier("entries.keys".to_string()),
                    SimpleExpr::Int(1),
                ],
            ),
            &context,
            &local_types,
        )
        .expect_err("map storage lanes must not be receiver paths");
        assert!(descendant_error.contains("only be used as the first argument"));

        for (target, path, expected) in [
            ("get", "wide_keys", "with i32 key"),
            ("get", "value_floats", "with i32 value"),
            ("contains", "wide_member_keys", "with i32 key"),
            ("can_put", "wide_keys", "with i32 key"),
            ("can_remove", "value_floats", "with i32 value"),
            ("can_add", "wide_member_keys", "with i32 key"),
        ] {
            let error = typed_collection_operation(
                target,
                &[SimpleExpr::Identifier(path.to_string()), SimpleExpr::Int(1)],
                &context,
                &local_types,
            )
            .expect_err("invalid map/set path must be rejected");
            assert!(error.contains(expected), "{target} {path}: {error}");
        }

        for (target, args, expected) in [
            (
                "put",
                vec![
                    SimpleExpr::Identifier("entries".to_string()),
                    SimpleExpr::Int(1),
                ],
                "put expects 3 arguments",
            ),
            (
                "put",
                vec![
                    SimpleExpr::Identifier("entries".to_string()),
                    SimpleExpr::Bool(true),
                    SimpleExpr::Int(1),
                ],
                "put key argument",
            ),
            (
                "put",
                vec![
                    SimpleExpr::Identifier("entries".to_string()),
                    SimpleExpr::Int(1),
                    SimpleExpr::Bool(true),
                ],
                "put value argument",
            ),
            (
                "add",
                vec![
                    SimpleExpr::Identifier("members".to_string()),
                    SimpleExpr::Bool(true),
                ],
                "add key argument",
            ),
            (
                "can_put",
                vec![SimpleExpr::Identifier("entries".to_string())],
                "can_put expects 2 arguments",
            ),
            (
                "can_put",
                vec![
                    SimpleExpr::Identifier("entries".to_string()),
                    SimpleExpr::Bool(true),
                ],
                "can_put key argument",
            ),
            (
                "can_remove",
                vec![
                    SimpleExpr::Identifier("entries".to_string()),
                    SimpleExpr::Bool(true),
                ],
                "can_remove key argument",
            ),
            (
                "can_remove",
                vec![SimpleExpr::Identifier("members".to_string())],
                "can_remove expects 2 arguments",
            ),
            (
                "can_add",
                vec![SimpleExpr::Identifier("members".to_string())],
                "can_add expects 2 arguments",
            ),
            (
                "can_add",
                vec![
                    SimpleExpr::Identifier("members".to_string()),
                    SimpleExpr::Bool(true),
                ],
                "can_add key argument",
            ),
            (
                "can_remove",
                vec![
                    SimpleExpr::Identifier("members".to_string()),
                    SimpleExpr::Bool(true),
                ],
                "can_remove key argument",
            ),
        ] {
            let error = typed_collection_operation(target, &args, &context, &local_types)
                .expect_err("invalid map/set operation must be rejected");
            assert!(error.contains(expected), "{target}: {error}");
        }

        assert!(typed_collection_operation(
            "module.map_get",
            &[
                SimpleExpr::Identifier("entries".to_string()),
                SimpleExpr::Int(1),
            ],
            &context,
            &local_types,
        )
        .expect("qualified name should be treated as an ordinary target")
        .is_none());
    }

    #[test]
    fn typed_grid_and_bitset_operations_have_exact_types_and_effects() {
        let (types, files) = typed_grid_bitset_fixture("");
        let context = grid_bitset_context(&types, &files);
        let local_types = BTreeMap::new();
        let locals = BTreeSet::new();
        let aliases = BTreeMap::new();
        let cases = [
            (
                "get",
                vec![
                    SimpleExpr::Identifier("cells".to_string()),
                    SimpleExpr::Int(1),
                    SimpleExpr::Int(2),
                ],
                TYPE_ID_I32,
                vec!["cells.values[*]"],
                Vec::<&str>::new(),
            ),
            (
                "set",
                vec![
                    SimpleExpr::Identifier("cells".to_string()),
                    SimpleExpr::Int(1),
                    SimpleExpr::Int(2),
                    SimpleExpr::Int(7),
                ],
                TYPE_ID_VOID,
                Vec::<&str>::new(),
                vec!["cells.values[*]"],
            ),
            (
                "can_access",
                vec![
                    SimpleExpr::Identifier("cells".to_string()),
                    SimpleExpr::Int(1),
                    SimpleExpr::Int(2),
                ],
                TYPE_ID_BOOL,
                Vec::<&str>::new(),
                Vec::<&str>::new(),
            ),
            (
                "test",
                vec![
                    SimpleExpr::Identifier("bits".to_string()),
                    SimpleExpr::Int(32),
                ],
                TYPE_ID_BOOL,
                vec!["bits.words[*]"],
                Vec::<&str>::new(),
            ),
            (
                "set",
                vec![
                    SimpleExpr::Identifier("bits".to_string()),
                    SimpleExpr::Int(32),
                    SimpleExpr::Bool(true),
                ],
                TYPE_ID_VOID,
                vec!["bits.words[*]"],
                vec!["bits.words[*]"],
            ),
            (
                "clear",
                vec![SimpleExpr::Identifier("bits".to_string())],
                TYPE_ID_VOID,
                Vec::<&str>::new(),
                vec!["bits.words[*]"],
            ),
        ];
        for (target, args, expected_type, expected_reads, expected_writes) in cases {
            let expression = pool_call(target, args);
            assert_eq!(
                expression_type(&expression, &context, &local_types, &aliases),
                Some(expected_type),
                "{target} return type"
            );
            validate_expression_access(&expression, &context, &local_types)
                .expect("valid typed grid/bitset operation");
            let mut effects = EffectSets::default();
            analyze_expression(
                &expression,
                &context,
                &locals,
                &local_types,
                &aliases,
                &mut effects,
            );
            assert_eq!(
                effects.reads.iter().map(String::as_str).collect::<Vec<_>>(),
                expected_reads,
                "{target} reads"
            );
            assert_eq!(
                effects
                    .writes
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
                expected_writes,
                "{target} writes"
            );
        }

        for (target, args, expected) in [
            (
                "get",
                vec![
                    SimpleExpr::Identifier("cells".to_string()),
                    SimpleExpr::Bool(true),
                    SimpleExpr::Int(0),
                ],
                "get x argument",
            ),
            (
                "set",
                vec![
                    SimpleExpr::Identifier("cells".to_string()),
                    SimpleExpr::Int(0),
                    SimpleExpr::Int(0),
                    SimpleExpr::Bool(true),
                ],
                "set value argument",
            ),
            (
                "test",
                vec![
                    SimpleExpr::Identifier("bits".to_string()),
                    SimpleExpr::Bool(true),
                ],
                "test index argument",
            ),
            (
                "set",
                vec![
                    SimpleExpr::Identifier("bits".to_string()),
                    SimpleExpr::Int(0),
                    SimpleExpr::Int(1),
                ],
                "set value argument",
            ),
        ] {
            let error = typed_collection_operation(target, &args, &context, &local_types)
                .expect_err("wrong grid/bitset argument type must fail");
            assert!(error.contains(expected), "{target}: {error}");
        }
    }

    #[test]
    fn compiler_requires_one_exact_can_access_for_one_grid_or_bitset_action() {
        let accepted = [
            "global cells: grid<i32, 2, 3>;\n@requires(cells.can_access(x, y))\nfunction write_cell(x: i32, y: i32, value: i32): void { cells.set(x, y, value); }\nfunction main(): void { if (cells.can_access(1, 2)) { write_cell(1, 2, 7); } }",
            "global bits: bitset<33>;\n@requires(bits.can_access(index))\nfunction read_bit(index: i32): bool { return bits.test(index); }\nfunction main(): bool { if (bits.can_access(32)) { return read_bit(32); } else { return false; } }",
        ];
        for source in accepted {
            let mut compiler = Compiler::new();
            compiler.upsert_file("grid_bitset_guard.stasis", source);
            compiler
                .check()
                .expect("exact can_access proof must authorize one matching action");
        }

        for source in [
            "global cells: grid<i32, 2, 3>;\nfunction main(): void { cells.set(1, 2, 7); }",
            "global cells: grid<i32, 2, 3>;\nfunction main(): void { if (cells.can_access(1, 1)) { cells.set(1, 2, 7); } }",
            "global bits: bitset<33>;\nfunction main(): bool { if (bits.can_access(31)) { return bits.test(32); } else { return false; } }",
            "global bits: bitset<33>;\nfunction main(): void { if (bits.can_access(1)) { bits.set(1, true); bits.set(1, false); } }",
        ] {
            let mut compiler = Compiler::new();
            compiler.upsert_file("grid_bitset_guard.stasis", source);
            let error = match compiler.check() {
                Ok(result) => panic!(
                    "missing, mismatched, or reused can_access proof must fail: {source}; got {result:?}"
                ),
                Err(error) => error,
            };
            assert!(
                format!("{error:?}").contains("requires a matching direct if"),
                "{error:?}"
            );
        }
    }

    #[test]
    fn typed_pool_operations_have_fixed_return_types_and_explicit_effects() {
        let (types, files) = typed_pool_fixture("");
        let context = pool_context(&types, &files);
        let local_types = BTreeMap::new();
        let locals = BTreeSet::new();
        let aliases = BTreeMap::new();
        let operations = [
            (
                "push",
                vec![
                    SimpleExpr::Identifier("actors".to_string()),
                    SimpleExpr::Int(7),
                ],
                TYPE_ID_I32,
                vec!["actors.count"],
                vec!["actors.count", "actors.values[*]"],
            ),
            (
                "remove",
                vec![
                    SimpleExpr::Identifier("actors".to_string()),
                    SimpleExpr::Int(1),
                ],
                TYPE_ID_VOID,
                vec!["actors.count", "actors.values[*]"],
                vec!["actors.count", "actors.values[*]"],
            ),
            (
                "count",
                vec![SimpleExpr::Identifier("actors".to_string())],
                TYPE_ID_I32,
                vec!["actors.count"],
                Vec::<&str>::new(),
            ),
            (
                "capacity",
                vec![SimpleExpr::Identifier("actors".to_string())],
                TYPE_ID_I32,
                Vec::<&str>::new(),
                Vec::<&str>::new(),
            ),
            (
                "clear",
                vec![SimpleExpr::Identifier("actors".to_string())],
                TYPE_ID_VOID,
                Vec::<&str>::new(),
                vec!["actors.count", "actors.values[*]"],
            ),
        ];

        for (target, args, expected_type, expected_reads, expected_writes) in operations {
            let expression = pool_call(target, args);
            assert_eq!(
                expression_type(&expression, &context, &local_types, &aliases),
                Some(expected_type),
                "{target} return type"
            );
            validate_expression_access(&expression, &context, &local_types)
                .expect("valid typed pool operation");
            let mut effects = EffectSets::default();
            analyze_expression(
                &expression,
                &context,
                &locals,
                &local_types,
                &aliases,
                &mut effects,
            );
            let reads: Vec<_> = effects.reads.iter().map(String::as_str).collect();
            let writes: Vec<_> = effects.writes.iter().map(String::as_str).collect();
            assert_eq!(reads, expected_reads, "{target} reads");
            assert_eq!(writes, expected_writes, "{target} writes");
            assert!(effects.calls.is_empty(), "{target} became an internal call");
            assert!(effects.host_calls.is_empty(), "{target} leaked a host call");
            assert!(
                effects.host_effects.is_empty(),
                "{target} leaked a host capability"
            );
        }
    }

    #[test]
    fn typed_stable_pool_operations_have_fixed_return_types_and_explicit_effects() {
        let (types, files) = typed_stable_pool_fixture("");
        let context = stable_pool_context(&types, &files);
        assert_eq!(
            context.typed_collection_descriptors["actors"].kind.as_str(),
            "stable_pool"
        );
        assert_eq!(context.typed_collection_descriptors["actors"].capacity, 2);
        assert_eq!(
            context.typed_collection_descriptors["empty_actors"].capacity,
            0
        );
        let local_types = BTreeMap::new();
        let locals = BTreeSet::new();
        let aliases = BTreeMap::new();
        let operations = [
            (
                "insert",
                vec![
                    SimpleExpr::Identifier("actors".to_string()),
                    SimpleExpr::Int(7),
                ],
                TYPE_ID_I32,
                vec!["actors.count", "actors.occupied[*]"],
                vec!["actors.count", "actors.occupied[*]", "actors.values[*]"],
            ),
            (
                "remove",
                vec![
                    SimpleExpr::Identifier("actors".to_string()),
                    SimpleExpr::Int(1),
                ],
                TYPE_ID_VOID,
                vec!["actors.count", "actors.occupied[*]"],
                vec!["actors.count", "actors.occupied[*]", "actors.values[*]"],
            ),
            (
                "count",
                vec![SimpleExpr::Identifier("actors".to_string())],
                TYPE_ID_I32,
                vec!["actors.count"],
                Vec::<&str>::new(),
            ),
            (
                "capacity",
                vec![SimpleExpr::Identifier("actors".to_string())],
                TYPE_ID_I32,
                Vec::<&str>::new(),
                Vec::<&str>::new(),
            ),
            (
                "clear",
                vec![SimpleExpr::Identifier("actors".to_string())],
                TYPE_ID_VOID,
                Vec::<&str>::new(),
                vec!["actors.count", "actors.occupied[*]", "actors.values[*]"],
            ),
        ];

        for (target, args, expected_type, expected_reads, expected_writes) in operations {
            let expression = pool_call(target, args);
            assert_eq!(
                expression_type(&expression, &context, &local_types, &aliases),
                Some(expected_type),
                "{target} return type"
            );
            validate_expression_access(&expression, &context, &local_types)
                .expect("valid typed stable pool operation");
            let mut effects = EffectSets::default();
            analyze_expression(
                &expression,
                &context,
                &locals,
                &local_types,
                &aliases,
                &mut effects,
            );
            let reads: Vec<_> = effects.reads.iter().map(String::as_str).collect();
            let writes: Vec<_> = effects.writes.iter().map(String::as_str).collect();
            assert_eq!(reads, expected_reads, "{target} reads");
            assert_eq!(writes, expected_writes, "{target} writes");
            assert!(effects.calls.is_empty(), "{target} became an internal call");
            assert!(effects.host_calls.is_empty(), "{target} leaked a host call");
            assert!(
                effects.host_effects.is_empty(),
                "{target} leaked a host capability"
            );
        }

        for target in ["insert", "remove", "count", "capacity", "clear"] {
            let args = match target {
                "insert" => vec![
                    SimpleExpr::Identifier("empty_actors".to_string()),
                    SimpleExpr::Int(7),
                ],
                "remove" => vec![
                    SimpleExpr::Identifier("empty_actors".to_string()),
                    SimpleExpr::Int(0),
                ],
                _ => vec![SimpleExpr::Identifier("empty_actors".to_string())],
            };
            typed_collection_operation(target, &args, &context, &local_types)
                .expect("zero-capacity stable pool operation must be valid")
                .expect("stable-pool operation should be compiler-owned");
        }
    }

    #[test]
    fn typed_queue_operations_have_fixed_return_types_and_explicit_effects() {
        let (types, files) = typed_queue_fixture("");
        let context = queue_context(&types, &files);
        assert!(context
            .typed_collection_descriptors
            .contains_key("empty_events"));
        assert_eq!(
            context.typed_collection_descriptors["empty_events"].capacity,
            0
        );
        let local_types = BTreeMap::new();
        let locals = BTreeSet::new();
        let aliases = BTreeMap::new();
        let operations = [
            (
                "push",
                vec![
                    SimpleExpr::Identifier("events".to_string()),
                    SimpleExpr::Int(7),
                ],
                TYPE_ID_VOID,
                vec!["events.count", "events.head"],
                vec!["events.count", "events.head", "events.values[*]"],
            ),
            (
                "pop",
                vec![SimpleExpr::Identifier("events".to_string())],
                TYPE_ID_VOID,
                vec!["events.count", "events.head", "events.values[*]"],
                vec!["events.count", "events.head", "events.values[*]"],
            ),
            (
                "peek",
                vec![
                    SimpleExpr::Identifier("events".to_string()),
                    SimpleExpr::Int(0),
                ],
                TYPE_ID_I32,
                vec!["events.count", "events.head", "events.values[*]"],
                Vec::<&str>::new(),
            ),
            (
                "physical_index",
                vec![
                    SimpleExpr::Identifier("events".to_string()),
                    SimpleExpr::Int(0),
                ],
                TYPE_ID_I32,
                vec!["events.count", "events.head"],
                Vec::<&str>::new(),
            ),
            (
                "count",
                vec![SimpleExpr::Identifier("events".to_string())],
                TYPE_ID_I32,
                vec!["events.count"],
                Vec::<&str>::new(),
            ),
            (
                "capacity",
                vec![SimpleExpr::Identifier("events".to_string())],
                TYPE_ID_I32,
                Vec::<&str>::new(),
                Vec::<&str>::new(),
            ),
            (
                "clear",
                vec![SimpleExpr::Identifier("events".to_string())],
                TYPE_ID_VOID,
                Vec::<&str>::new(),
                vec!["events.count", "events.head", "events.values[*]"],
            ),
        ];

        for (target, args, expected_type, expected_reads, expected_writes) in operations {
            let expression = pool_call(target, args);
            assert_eq!(
                expression_type(&expression, &context, &local_types, &aliases),
                Some(expected_type),
                "{target} return type"
            );
            validate_expression_access(&expression, &context, &local_types)
                .expect("valid typed queue operation");
            let mut effects = EffectSets::default();
            analyze_expression(
                &expression,
                &context,
                &locals,
                &local_types,
                &aliases,
                &mut effects,
            );
            let reads: Vec<_> = effects.reads.iter().map(String::as_str).collect();
            let writes: Vec<_> = effects.writes.iter().map(String::as_str).collect();
            assert_eq!(reads, expected_reads, "{target} reads");
            assert_eq!(writes, expected_writes, "{target} writes");
            assert!(effects.calls.is_empty(), "{target} became an internal call");
            assert!(effects.host_calls.is_empty(), "{target} leaked a host call");
            assert!(
                effects.host_effects.is_empty(),
                "{target} leaked a host capability"
            );
        }
    }

    #[test]
    fn typed_queue_operations_require_exact_persistent_queue_paths_and_i32_indices() {
        let (types, files) = typed_queue_fixture("");
        let context = queue_context(&types, &files);
        let mut local_types = BTreeMap::new();
        local_types.insert(
            "events".to_string(),
            types.resolve("queue<i32, 2>").expect("queue type id"),
        );

        let local_result = typed_collection_operation(
            "count",
            &[SimpleExpr::Identifier("events".to_string())],
            &context,
            &local_types,
        )
        .expect("local queue value must remain an ordinary receiver");
        assert!(local_result.is_none());

        local_types.clear();
        let field_result = typed_collection_operation(
            "count",
            &[SimpleExpr::Identifier("events.values".to_string())],
            &context,
            &local_types,
        )
        .expect("queue field receiver must remain an ordinary call");
        assert!(field_result.is_none());

        let other_kind_result = typed_collection_operation(
            "count",
            &[SimpleExpr::Identifier("actors".to_string())],
            &context,
            &local_types,
        )
        .expect("count may claim another valid collection kind");
        assert_eq!(other_kind_result, Some(TypedCollectionOperation::PoolCount));

        let wrong_kind_result = typed_collection_operation(
            "peek",
            &[SimpleExpr::Identifier("actors".to_string())],
            &context,
            &local_types,
        )
        .expect("pool receiver must remain ordinary for a queue-only method");
        assert!(wrong_kind_result.is_none());

        let peek_index = typed_collection_operation(
            "peek",
            &[
                SimpleExpr::Identifier("events".to_string()),
                SimpleExpr::Float(0.0),
            ],
            &context,
            &local_types,
        )
        .expect_err("queue_peek logical index must be i32");
        assert!(peek_index.contains("peek logical_index argument"));

        let physical_index = typed_collection_operation(
            "physical_index",
            &[
                SimpleExpr::Identifier("events".to_string()),
                SimpleExpr::Bool(true),
            ],
            &context,
            &local_types,
        )
        .expect_err("queue_physical_index logical index must be i32");
        assert!(physical_index.contains("physical_index logical_index argument"));

        let pop_arity = typed_collection_operation(
            "pop",
            &[
                SimpleExpr::Identifier("events".to_string()),
                SimpleExpr::Int(0),
            ],
            &context,
            &local_types,
        )
        .expect_err("queue_pop must not accept an output argument");
        assert!(pop_arity.contains("pop expects 1 arguments"));
    }

    #[test]
    fn typed_priority_queue_operations_have_fixed_return_types_and_explicit_effects() {
        let (types, files) = typed_priority_queue_fixture("");
        let context = priority_queue_context(&types, &files);
        assert!(context
            .typed_collection_descriptors
            .contains_key("empty_events"));
        assert_eq!(
            context.typed_collection_descriptors["empty_events"].capacity,
            0
        );
        let local_types = BTreeMap::new();
        let locals = BTreeSet::new();
        let aliases = BTreeMap::new();
        let operations = [
            (
                "push",
                vec![
                    SimpleExpr::Identifier("events".to_string()),
                    SimpleExpr::Int(1),
                    SimpleExpr::Int(7),
                ],
                TYPE_ID_VOID,
                vec![
                    "events.count",
                    "events.next_order",
                    "events.order[*]",
                    "events.priority[*]",
                ],
                vec![
                    "events.count",
                    "events.next_order",
                    "events.order[*]",
                    "events.priority[*]",
                    "events.values[*]",
                ],
            ),
            (
                "pop",
                vec![SimpleExpr::Identifier("events".to_string())],
                TYPE_ID_VOID,
                vec![
                    "events.count",
                    "events.order[*]",
                    "events.priority[*]",
                    "events.values[*]",
                ],
                vec![
                    "events.count",
                    "events.order[*]",
                    "events.priority[*]",
                    "events.values[*]",
                ],
            ),
            (
                "peek",
                vec![SimpleExpr::Identifier("events".to_string())],
                TYPE_ID_I32,
                vec![
                    "events.count",
                    "events.order[*]",
                    "events.priority[*]",
                    "events.values[*]",
                ],
                Vec::<&str>::new(),
            ),
            (
                "peek_priority",
                vec![SimpleExpr::Identifier("events".to_string())],
                TYPE_ID_I32,
                vec!["events.count", "events.order[*]", "events.priority[*]"],
                Vec::<&str>::new(),
            ),
            (
                "count",
                vec![SimpleExpr::Identifier("events".to_string())],
                TYPE_ID_I32,
                vec!["events.count"],
                Vec::<&str>::new(),
            ),
            (
                "capacity",
                vec![SimpleExpr::Identifier("events".to_string())],
                TYPE_ID_I32,
                Vec::<&str>::new(),
                Vec::<&str>::new(),
            ),
            (
                "clear",
                vec![SimpleExpr::Identifier("events".to_string())],
                TYPE_ID_VOID,
                Vec::<&str>::new(),
                vec![
                    "events.count",
                    "events.next_order",
                    "events.order[*]",
                    "events.priority[*]",
                    "events.values[*]",
                ],
            ),
            (
                "can_push",
                vec![SimpleExpr::Identifier("events".to_string())],
                TYPE_ID_BOOL,
                vec!["events.count", "events.next_order"],
                Vec::<&str>::new(),
            ),
            (
                "can_pop",
                vec![SimpleExpr::Identifier("events".to_string())],
                TYPE_ID_BOOL,
                vec!["events.count"],
                Vec::<&str>::new(),
            ),
            (
                "can_peek",
                vec![SimpleExpr::Identifier("events".to_string())],
                TYPE_ID_BOOL,
                vec!["events.count"],
                Vec::<&str>::new(),
            ),
        ];

        for (target, args, expected_type, expected_reads, expected_writes) in operations {
            let expression = pool_call(target, args);
            assert_eq!(
                expression_type(&expression, &context, &local_types, &aliases),
                Some(expected_type),
                "{target} return type"
            );
            validate_expression_access(&expression, &context, &local_types)
                .expect("valid typed priority queue operation");
            let mut effects = EffectSets::default();
            analyze_expression(
                &expression,
                &context,
                &locals,
                &local_types,
                &aliases,
                &mut effects,
            );
            let reads: Vec<_> = effects.reads.iter().map(String::as_str).collect();
            let writes: Vec<_> = effects.writes.iter().map(String::as_str).collect();
            assert_eq!(reads, expected_reads, "{target} reads");
            assert_eq!(writes, expected_writes, "{target} writes");
            assert!(effects.calls.is_empty(), "{target} became an internal call");
            assert!(effects.host_calls.is_empty(), "{target} leaked a host call");
            assert!(
                effects.host_effects.is_empty(),
                "{target} leaked a host capability"
            );
        }

        for target in [
            "push",
            "pop",
            "peek",
            "peek_priority",
            "count",
            "capacity",
            "clear",
            "can_push",
            "can_pop",
            "can_peek",
        ] {
            let args = if target == "push" {
                vec![
                    SimpleExpr::Identifier("empty_events".to_string()),
                    SimpleExpr::Int(1),
                    SimpleExpr::Int(7),
                ]
            } else {
                vec![SimpleExpr::Identifier("empty_events".to_string())]
            };
            typed_collection_operation(target, &args, &context, &local_types)
                .expect("zero-capacity priority queue operation must be valid")
                .expect("priority queue operation should be compiler-owned");
        }
    }

    #[test]
    fn typed_priority_queue_operations_require_exact_persistent_i32_paths_and_arguments() {
        let (types, files) = typed_priority_queue_fixture("");
        let context = priority_queue_context(&types, &files);
        let mut local_types = BTreeMap::new();
        local_types.insert(
            "events".to_string(),
            types
                .resolve("priority_queue<i32, 2>")
                .expect("priority queue type id"),
        );

        let local_result = typed_collection_operation(
            "count",
            &[SimpleExpr::Identifier("events".to_string())],
            &context,
            &local_types,
        )
        .expect("local priority queue value must remain an ordinary receiver");
        assert!(local_result.is_none());

        local_types.clear();
        assert!(typed_collection_operation(
            "count",
            &[SimpleExpr::Identifier("events.values".to_string())],
            &context,
            &local_types,
        )
        .expect("descendant receiver must remain an ordinary call")
        .is_none());
        let other_kind_result = typed_collection_operation(
            "count",
            &[SimpleExpr::Identifier("queue".to_string())],
            &context,
            &local_types,
        )
        .expect("count may claim another valid collection kind");
        assert_eq!(
            other_kind_result,
            Some(TypedCollectionOperation::QueueCount)
        );
        let payload_error = typed_collection_operation(
            "push",
            &[
                SimpleExpr::Identifier("float_events".to_string()),
                SimpleExpr::Int(1),
                SimpleExpr::Int(7),
            ],
            &context,
            &local_types,
        )
        .expect_err("invalid priority queue payload must be rejected");
        assert!(payload_error.contains("with i32 payload"));

        for (target, args, expected) in [
            (
                "push",
                vec![
                    SimpleExpr::Identifier("events".to_string()),
                    SimpleExpr::Int(1),
                ],
                "push expects 3 arguments",
            ),
            (
                "push",
                vec![
                    SimpleExpr::Identifier("events".to_string()),
                    SimpleExpr::Bool(true),
                    SimpleExpr::Int(7),
                ],
                "push priority argument",
            ),
            (
                "push",
                vec![
                    SimpleExpr::Identifier("events".to_string()),
                    SimpleExpr::Int(1),
                    SimpleExpr::Bool(true),
                ],
                "push value argument",
            ),
            (
                "pop",
                vec![
                    SimpleExpr::Identifier("events".to_string()),
                    SimpleExpr::Int(0),
                ],
                "pop expects 1 arguments",
            ),
            (
                "peek",
                vec![
                    SimpleExpr::Identifier("events".to_string()),
                    SimpleExpr::Int(0),
                ],
                "peek expects 1 arguments",
            ),
            (
                "can_push",
                vec![
                    SimpleExpr::Identifier("events".to_string()),
                    SimpleExpr::Int(0),
                ],
                "can_push expects 1 arguments",
            ),
        ] {
            let error = typed_collection_operation(target, &args, &context, &local_types)
                .expect_err("invalid priority queue operation must be rejected");
            assert!(error.contains(expected), "{target}: {error}");
        }

        assert!(typed_collection_operation(
            "module.count",
            &[SimpleExpr::Identifier("events".to_string())],
            &context,
            &local_types,
        )
        .expect("qualified name should be treated as an ordinary target")
        .is_none());
        assert!(typed_collection_operation(
            "priority_queue_count",
            &[SimpleExpr::Identifier("events".to_string())],
            &context,
            &local_types,
        )
        .expect("prefixed priority queue aliases are not compiler-owned")
        .is_none());
        let queue_push_error = typed_collection_operation(
            "push",
            &[
                SimpleExpr::Identifier("queue".to_string()),
                SimpleExpr::Int(1),
                SimpleExpr::Int(7),
            ],
            &context,
            &local_types,
        )
        .expect_err("queue push arity must remain collection-specific");
        assert!(queue_push_error.contains("push expects 2 arguments"));
        assert!(typed_collection_operation(
            "peek_priority",
            &[SimpleExpr::Identifier("queue".to_string())],
            &context,
            &local_types,
        )
        .expect("queue receiver must remain ordinary for a priority-only method")
        .is_none());
        assert!(typed_collection_operation(
            "push",
            &[
                SimpleExpr::Identifier("ordinary_value".to_string()),
                SimpleExpr::Int(1),
                SimpleExpr::Int(7),
            ],
            &context,
            &local_types,
        )
        .expect("noncollection receiver must not be hijacked")
        .is_none());
    }

    #[test]
    fn typed_ring_buffer_operations_have_fixed_return_types_and_explicit_effects() {
        let (types, files) = typed_ring_buffer_fixture("");
        let context = ring_buffer_context(&types, &files);
        assert!(context
            .typed_collection_descriptors
            .contains_key("empty_history"));
        assert_eq!(
            context.typed_collection_descriptors["empty_history"].capacity,
            0
        );
        assert_eq!(
            context.typed_collection_descriptors["history"].kind,
            TypedCollectionKind::RingBuffer
        );
        let local_types = BTreeMap::new();
        let locals = BTreeSet::new();
        let aliases = BTreeMap::new();
        let operations = [
            (
                "push",
                vec![
                    SimpleExpr::Identifier("history".to_string()),
                    SimpleExpr::Int(7),
                ],
                TYPE_ID_VOID,
                vec!["history.count", "history.head"],
                vec!["history.count", "history.head", "history.values[*]"],
            ),
            (
                "pop",
                vec![SimpleExpr::Identifier("history".to_string())],
                TYPE_ID_VOID,
                vec!["history.count", "history.head", "history.values[*]"],
                vec!["history.count", "history.head", "history.values[*]"],
            ),
            (
                "peek",
                vec![
                    SimpleExpr::Identifier("history".to_string()),
                    SimpleExpr::Int(0),
                ],
                TYPE_ID_I32,
                vec!["history.count", "history.head", "history.values[*]"],
                Vec::<&str>::new(),
            ),
            (
                "physical_index",
                vec![
                    SimpleExpr::Identifier("history".to_string()),
                    SimpleExpr::Int(0),
                ],
                TYPE_ID_I32,
                vec!["history.count", "history.head"],
                Vec::<&str>::new(),
            ),
            (
                "count",
                vec![SimpleExpr::Identifier("history".to_string())],
                TYPE_ID_I32,
                vec!["history.count"],
                Vec::<&str>::new(),
            ),
            (
                "capacity",
                vec![SimpleExpr::Identifier("history".to_string())],
                TYPE_ID_I32,
                Vec::<&str>::new(),
                Vec::<&str>::new(),
            ),
            (
                "clear",
                vec![SimpleExpr::Identifier("history".to_string())],
                TYPE_ID_VOID,
                Vec::<&str>::new(),
                vec!["history.count", "history.head", "history.values[*]"],
            ),
        ];

        for (target, args, expected_type, expected_reads, expected_writes) in operations {
            let expression = pool_call(target, args);
            assert_eq!(
                expression_type(&expression, &context, &local_types, &aliases),
                Some(expected_type),
                "{target} return type"
            );
            validate_expression_access(&expression, &context, &local_types)
                .expect("valid typed ring buffer operation");
            let mut effects = EffectSets::default();
            analyze_expression(
                &expression,
                &context,
                &locals,
                &local_types,
                &aliases,
                &mut effects,
            );
            let reads: Vec<_> = effects.reads.iter().map(String::as_str).collect();
            let writes: Vec<_> = effects.writes.iter().map(String::as_str).collect();
            assert_eq!(reads, expected_reads, "{target} reads");
            assert_eq!(writes, expected_writes, "{target} writes");
            assert!(effects.calls.is_empty(), "{target} became an internal call");
            assert!(effects.host_calls.is_empty(), "{target} leaked a host call");
            assert!(
                effects.host_effects.is_empty(),
                "{target} leaked a host capability"
            );
        }

        for target in [
            "push",
            "pop",
            "peek",
            "physical_index",
            "count",
            "capacity",
            "clear",
        ] {
            let args = match target {
                "push" => vec![
                    SimpleExpr::Identifier("empty_history".to_string()),
                    SimpleExpr::Int(7),
                ],
                "peek" | "physical_index" => vec![
                    SimpleExpr::Identifier("empty_history".to_string()),
                    SimpleExpr::Int(0),
                ],
                _ => vec![SimpleExpr::Identifier("empty_history".to_string())],
            };
            typed_collection_operation(target, &args, &context, &local_types)
                .expect("zero-capacity ring buffer operation must be valid")
                .expect("ring-buffer operation should be compiler-owned");
        }
    }

    #[test]
    fn typed_ring_buffer_operations_require_exact_persistent_paths_and_i32_arguments() {
        let (types, files) = typed_ring_buffer_fixture("global floats: ring_buffer<f32, 2>;\n");
        let context = ring_buffer_context(&types, &files);
        let mut local_types = BTreeMap::new();
        local_types.insert(
            "history".to_string(),
            types
                .resolve("ring_buffer<i32, 2>")
                .expect("ring buffer type id"),
        );

        let local_result = typed_collection_operation(
            "count",
            &[SimpleExpr::Identifier("history".to_string())],
            &context,
            &local_types,
        )
        .expect("local ring buffer value must remain an ordinary receiver");
        assert!(local_result.is_none());

        local_types.clear();
        let field_result = typed_collection_operation(
            "count",
            &[SimpleExpr::Identifier("history.values".to_string())],
            &context,
            &local_types,
        )
        .expect("ring buffer field receiver must remain an ordinary call");
        assert!(field_result.is_none());

        let other_kind_result = typed_collection_operation(
            "count",
            &[SimpleExpr::Identifier("events".to_string())],
            &context,
            &local_types,
        )
        .expect("count may claim another valid collection kind");
        assert_eq!(
            other_kind_result,
            Some(TypedCollectionOperation::QueueCount)
        );

        let wrong_kind_result = typed_collection_operation(
            "peek",
            &[SimpleExpr::Identifier("actors".to_string())],
            &context,
            &local_types,
        )
        .expect("pool receiver must remain ordinary for a ring-buffer-only method");
        assert!(wrong_kind_result.is_none());

        let wrong_payload = typed_collection_operation(
            "count",
            &[SimpleExpr::Identifier("floats".to_string())],
            &context,
            &local_types,
        )
        .expect_err("non-i32 ring buffer must be rejected");
        assert!(wrong_payload.contains("with i32 payload"));

        let peek_index = typed_collection_operation(
            "peek",
            &[
                SimpleExpr::Identifier("history".to_string()),
                SimpleExpr::Float(0.0),
            ],
            &context,
            &local_types,
        )
        .expect_err("ring_buffer_peek logical index must be i32");
        assert!(peek_index.contains("peek logical_index argument"));

        let physical_index = typed_collection_operation(
            "physical_index",
            &[
                SimpleExpr::Identifier("history".to_string()),
                SimpleExpr::Bool(true),
            ],
            &context,
            &local_types,
        )
        .expect_err("ring_buffer_physical_index logical index must be i32");
        assert!(physical_index.contains("physical_index logical_index argument"));

        let push_value = typed_collection_operation(
            "push",
            &[
                SimpleExpr::Identifier("history".to_string()),
                SimpleExpr::Bool(true),
            ],
            &context,
            &local_types,
        )
        .expect_err("ring_buffer_push value must be i32");
        assert!(push_value.contains("push value argument"));

        let pop_arity = typed_collection_operation(
            "pop",
            &[
                SimpleExpr::Identifier("history".to_string()),
                SimpleExpr::Int(0),
            ],
            &context,
            &local_types,
        )
        .expect_err("ring_buffer_pop must not accept an output argument");
        assert!(pop_arity.contains("pop expects 1 arguments"));
    }

    #[test]
    fn qualified_ring_buffer_operation_names_are_not_hijacked() {
        let (types, files) = typed_ring_buffer_fixture("");
        let context = ring_buffer_context(&types, &files);
        let local_types = BTreeMap::new();
        assert!(typed_collection_operation(
            "module.ring_buffer_count",
            &[SimpleExpr::Identifier("history".to_string())],
            &context,
            &local_types,
        )
        .expect("qualified name should be treated as an ordinary target")
        .is_none());
    }

    #[test]
    fn qualified_collection_operation_names_are_not_hijacked() {
        let (types, files) = typed_queue_fixture("");
        let context = queue_context(&types, &files);
        let local_types = BTreeMap::new();
        assert!(typed_collection_operation(
            "module.queue_count",
            &[SimpleExpr::Identifier("events".to_string())],
            &context,
            &local_types,
        )
        .expect("qualified name should be treated as an ordinary target")
        .is_none());
    }

    #[test]
    fn typed_pool_operations_require_exact_persistent_pool_paths() {
        let (types, files) = typed_pool_fixture("");
        let context = pool_context(&types, &files);
        let mut local_types = BTreeMap::new();
        local_types.insert(
            "actors".to_string(),
            types.resolve("pool<i32, 2>").expect("pool type id"),
        );

        let local_result = typed_pool_operation(
            "count",
            &[SimpleExpr::Identifier("actors".to_string())],
            &context,
            &local_types,
        )
        .expect("local pool value must remain an ordinary receiver");
        assert!(local_result.is_none());

        local_types.clear();
        let field_result = typed_pool_operation(
            "count",
            &[SimpleExpr::Identifier("actors.values".to_string())],
            &context,
            &local_types,
        )
        .expect("pool field receiver must remain an ordinary call");
        assert!(field_result.is_none());

        let other_kind_result = typed_pool_operation(
            "count",
            &[SimpleExpr::Identifier("queue".to_string())],
            &context,
            &local_types,
        )
        .expect("count may claim another valid collection kind");
        assert_eq!(
            other_kind_result,
            Some(TypedCollectionOperation::QueueCount)
        );

        let wrong_kind_result = typed_pool_operation(
            "remove",
            &[SimpleExpr::Identifier("queue".to_string())],
            &context,
            &local_types,
        )
        .expect("queue receiver must remain ordinary for a pool-only method");
        assert!(wrong_kind_result.is_none());

        let payload_error = typed_pool_operation(
            "count",
            &[SimpleExpr::Identifier("floats".to_string())],
            &context,
            &local_types,
        )
        .expect_err("non-i32 payload pool must be rejected");
        assert!(payload_error.contains("with i32 payload"));
    }

    #[test]
    fn typed_pool_operations_reject_arity_and_strict_index_or_value_types() {
        let (types, files) = typed_pool_fixture("");
        let context = pool_context(&types, &files);
        let local_types = BTreeMap::new();
        let actor = || SimpleExpr::Identifier("actors".to_string());

        let push_arity = typed_pool_operation("push", &[actor()], &context, &local_types)
            .expect_err("pool_push arity must be checked");
        assert!(push_arity.contains("expects 2 arguments"));

        let remove_arity = typed_pool_operation(
            "remove",
            &[actor(), SimpleExpr::Int(0), SimpleExpr::Int(1)],
            &context,
            &local_types,
        )
        .expect_err("pool_remove arity must be checked");
        assert!(remove_arity.contains("expects 2 arguments"));

        let remove_index = typed_pool_operation(
            "remove",
            &[actor(), SimpleExpr::Float(0.0)],
            &context,
            &local_types,
        )
        .expect_err("pool_remove index type must be checked");
        assert!(remove_index.contains("remove index argument"));
        assert!(!remove_index.contains("remove value argument"));

        let push_value = typed_pool_operation(
            "push",
            &[actor(), SimpleExpr::Bool(true)],
            &context,
            &local_types,
        )
        .expect_err("pool_push value type must be checked");
        assert!(push_value.contains("push value argument"));
    }

    #[test]
    fn typed_stable_pool_operations_require_exact_persistent_paths_and_i32_arguments() {
        let (types, files) = typed_stable_pool_fixture("");
        let context = stable_pool_context(&types, &files);
        let mut local_types = BTreeMap::new();
        local_types.insert(
            "actors".to_string(),
            types
                .resolve("stable_pool<i32, 2>")
                .expect("stable pool type id"),
        );

        let local_result = typed_collection_operation(
            "count",
            &[SimpleExpr::Identifier("actors".to_string())],
            &context,
            &local_types,
        )
        .expect("local stable pool value must remain an ordinary receiver");
        assert!(local_result.is_none());

        local_types.clear();
        let field_result = typed_collection_operation(
            "count",
            &[SimpleExpr::Identifier("actors.occupied".to_string())],
            &context,
            &local_types,
        )
        .expect("stable pool lane receiver must remain an ordinary call");
        assert!(field_result.is_none());

        let other_kind_result = typed_collection_operation(
            "count",
            &[SimpleExpr::Identifier("queue".to_string())],
            &context,
            &local_types,
        )
        .expect("count may claim another valid collection kind");
        assert_eq!(
            other_kind_result,
            Some(TypedCollectionOperation::QueueCount)
        );

        let wrong_kind_result = typed_collection_operation(
            "insert",
            &[
                SimpleExpr::Identifier("queue".to_string()),
                SimpleExpr::Int(0),
            ],
            &context,
            &local_types,
        )
        .expect("queue receiver must remain ordinary for a stable-pool-only method");
        assert!(wrong_kind_result.is_none());

        let wrong_payload = typed_collection_operation(
            "count",
            &[SimpleExpr::Identifier("floats".to_string())],
            &context,
            &local_types,
        )
        .expect_err("non-i32 stable pool must be rejected");
        assert!(wrong_payload.contains("with i32 payload"));

        let insert_arity = typed_collection_operation(
            "insert",
            &[SimpleExpr::Identifier("actors".to_string())],
            &context,
            &local_types,
        )
        .expect_err("stable_pool_insert arity must be checked");
        assert!(insert_arity.contains("insert expects 2 arguments"));

        let remove_arity = typed_collection_operation(
            "remove",
            &[
                SimpleExpr::Identifier("actors".to_string()),
                SimpleExpr::Int(0),
                SimpleExpr::Int(1),
            ],
            &context,
            &local_types,
        )
        .expect_err("stable_pool_remove arity must be checked");
        assert!(remove_arity.contains("remove expects 2 arguments"));

        let remove_index = typed_collection_operation(
            "remove",
            &[
                SimpleExpr::Identifier("actors".to_string()),
                SimpleExpr::Float(0.0),
            ],
            &context,
            &local_types,
        )
        .expect_err("stable_pool_remove index must be i32");
        assert!(remove_index.contains("remove index argument"));

        let insert_value = typed_collection_operation(
            "insert",
            &[
                SimpleExpr::Identifier("actors".to_string()),
                SimpleExpr::Bool(true),
            ],
            &context,
            &local_types,
        )
        .expect_err("stable_pool_insert value must be i32");
        assert!(insert_value.contains("insert value argument"));

        assert!(typed_collection_operation(
            "module.stable_pool_count",
            &[SimpleExpr::Identifier("actors".to_string())],
            &context,
            &local_types,
        )
        .expect("qualified name should be treated as an ordinary target")
        .is_none());
    }

    #[test]
    fn typed_collection_values_are_not_ordinary_expression_values() {
        let (types, files) = typed_pool_fixture("");
        let context = pool_context(&types, &files);
        let local_types = BTreeMap::new();
        let direct = validate_expression_access(
            &SimpleExpr::Identifier("actors".to_string()),
            &context,
            &local_types,
        )
        .expect_err("direct typed collection value use must be rejected");
        assert!(direct.contains("only be used as the first argument"));

        let arithmetic = validate_expression_access(
            &SimpleExpr::Binary {
                lhs: Box::new(SimpleExpr::Identifier("actors".to_string())),
                op: '+',
                rhs: Box::new(SimpleExpr::Int(1)),
            },
            &context,
            &local_types,
        )
        .expect_err("typed collection arithmetic must be rejected");
        assert!(arithmetic.contains("only be used as the first argument"));

        let ordinary_call = validate_expression_access(
            &pool_call(
                "unrelated",
                vec![SimpleExpr::Identifier("actors".to_string())],
            ),
            &context,
            &local_types,
        )
        .expect_err("ordinary calls must not consume typed collections");
        assert!(ordinary_call.contains("only be used as the first argument"));
    }

    #[test]
    fn typed_collection_roots_and_descendant_lanes_cannot_be_read_or_written() {
        let (types, files) = typed_pool_fixture("");
        let context = pool_context(&types, &files);
        let local_types = BTreeMap::new();

        for expression in [
            SimpleExpr::Identifier("actors.count".to_string()),
            SimpleExpr::Identifier("actors.values".to_string()),
            SimpleExpr::IndexedPath {
                collection_path: "actors".to_string(),
                index: Box::new(SimpleExpr::Int(0)),
                suffix: "values".to_string(),
            },
        ] {
            let error = validate_expression_access(&expression, &context, &local_types)
                .expect_err("typed collection lane read must be rejected");
            assert!(error.contains("typed collection paths"), "{error}");
        }

        for target in [
            AssignTarget::GlobalPath("actors.count".to_string()),
            AssignTarget::GlobalPath("actors.values".to_string()),
            AssignTarget::IndexedPath {
                collection_path: "actors".to_string(),
                index: SimpleExpr::Int(0),
                suffix: "values".to_string(),
            },
        ] {
            let error = validate_assignment_target_access(&target, &context, &local_types)
                .expect_err("typed collection lane write must be rejected");
            assert!(error.contains("typed collection paths"), "{error}");
        }

        let mut locals = BTreeMap::new();
        let error = validate_statements(
            &[SimpleStmt::Foreach {
                item_name: "item".to_string(),
                index_name: None,
                collection_path: "actors".to_string(),
                body_statements: Vec::new(),
            }],
            TYPE_ID_VOID,
            &context,
            &mut locals,
            0,
        )
        .expect_err("typed collection foreach source must be rejected");
        assert!(error.contains("typed collection paths"), "{error}");
    }

    #[test]
    fn typed_collection_function_params_and_returns_are_rejected() {
        let (types, files) = typed_pool_fixture("");
        let pool_type = types.resolve("pool<i32, 2>").expect("pool type id");
        let statements = vec![vec![SimpleStmt::Return(SimpleExpr::Int(0))]];

        let parameter_error = validate_program_semantics(
            &files,
            &[test_function(
                "takes_pool",
                vec!["items".to_string()],
                vec![pool_type],
                TYPE_ID_I32,
            )],
            &statements,
            &types,
        )
        .expect_err("typed collection parameter must be rejected")
        .1;
        assert!(parameter_error.contains("cannot accept typed collection parameter"));

        let return_error = validate_program_semantics(
            &files,
            &[test_function(
                "returns_pool",
                Vec::new(),
                Vec::new(),
                pool_type,
            )],
            &statements,
            &types,
        )
        .expect_err("typed collection return must be rejected")
        .1;
        assert!(return_error.contains("cannot return a typed collection value"));
    }

    #[test]
    fn compiler_rejects_pool_semantic_errors_before_emission() {
        for (source, expected) in [
            (
                "global actors: pool<i32, 2>;\nfunction main(): i32 { return actors.count(1); }",
                "count expects 1 arguments",
            ),
            (
                "global actors: pool<i32, 2>;\nfunction main(): i32 { return actors.remove(1.0); }",
                "remove index argument",
            ),
            (
                "global actors: pool<i32, 2>;\nfunction main(): i32 { let p: i32 = 0; return p.count(); }",
                "cannot resolve call 'count'",
            ),
            (
                "global actors: pool<i32, 2>;\nfunction main(): i32 { return actors; }",
                "only be used as the first argument",
            ),
            (
                "global actors: pool<i32, 2, overwrite_oldest>;\nfunction main(): i32 { return 0; }",
                "uses legacy trailing overflow policy 'overwrite_oldest'",
            ),
        ] {
            compiler_rejects(source, expected);
        }
    }

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
