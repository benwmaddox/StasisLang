use std::collections::{BTreeMap, BTreeSet};

use crate::backend::emit::{
    eval_const_i64, AssignOp, AssignTarget, ComparisonOp, SimpleCondition, SimpleExpr, SimpleStmt,
};
use crate::compiler::{FunctionMeta, SourceFile};
use crate::frontend::parser::{parse_top_level_extern_functions, parse_top_level_type_layout};
use crate::frontend::types::{TypeCategory, TypeTable};

pub const FUNCTION_DATA_FLOW_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FunctionDataFlowEffects {
    pub reads: Vec<String>,
    pub writes: Vec<String>,
    pub parameter_reads: Vec<String>,
    pub parameter_writes: Vec<String>,
    pub calls: Vec<String>,
    pub host_calls: Vec<String>,
    pub bounded_iterations: Vec<FunctionBoundedIteration>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct FunctionBoundedIteration {
    pub function: String,
    pub kind: String,
    pub bound: String,
    pub max_iterations: Option<u64>,
    pub reads: Vec<String>,
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
}

#[derive(Default)]
struct EffectSets {
    reads: BTreeSet<String>,
    writes: BTreeSet<String>,
    parameter_reads: BTreeSet<String>,
    parameter_writes: BTreeSet<String>,
    calls: BTreeSet<String>,
    host_calls: BTreeSet<String>,
    bounded_iterations: BTreeSet<FunctionBoundedIteration>,
    call_sites: Vec<CallSite>,
}

#[derive(Debug, Clone)]
struct CallSite {
    target: String,
    arguments: Vec<Option<String>>,
}

impl EffectSets {
    fn merge(&mut self, other: &Self) {
        self.reads.extend(other.reads.iter().cloned());
        self.writes.extend(other.writes.iter().cloned());
        self.parameter_reads
            .extend(other.parameter_reads.iter().cloned());
        self.parameter_writes
            .extend(other.parameter_writes.iter().cloned());
        self.calls.extend(other.calls.iter().cloned());
        self.host_calls.extend(other.host_calls.iter().cloned());
        self.bounded_iterations
            .extend(other.bounded_iterations.iter().cloned());
        self.call_sites.extend(other.call_sites.iter().cloned());
    }

    fn insert_read(&mut self, path: String) {
        if path.starts_with('$') {
            self.parameter_reads.insert(path);
        } else {
            self.reads.insert(path);
        }
    }

    fn insert_write(&mut self, path: String) {
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
            bounded_iterations: self
                .bounded_iterations
                .iter()
                .map(|iteration| public_iteration(iteration, parameter_names))
                .collect(),
        }
    }
}

struct AnalysisContext {
    globals: BTreeSet<String>,
    constants: BTreeMap<String, i64>,
    internal_functions: BTreeSet<String>,
    internal_view_parameters: BTreeMap<String, BTreeSet<usize>>,
    view_parameters_by_function: BTreeMap<u32, BTreeSet<usize>>,
    fixed_parameter_capacities: BTreeMap<u32, BTreeMap<usize, u64>>,
    extern_functions: BTreeSet<String>,
    collection_capacities: BTreeMap<String, u64>,
}

pub(crate) fn build_function_data_flow_summaries(
    files: &[SourceFile],
    functions: &[FunctionMeta],
    statements_by_id: &[Vec<SimpleStmt>],
    types: &TypeTable,
) -> Result<Vec<FunctionDataFlowSummary>, String> {
    let context = build_context(files, functions, types)?;
    let mut direct_by_id = Vec::with_capacity(functions.len());
    for function in functions {
        let statements = statements_by_id
            .get(function.id as usize)
            .ok_or_else(|| format!("function '{}' has no statement artifact", function.name))?;
        let mut effects = EffectSets::default();
        let mut locals = function.param_names.iter().cloned().collect();
        let view_parameters = context.view_parameters_by_function.get(&function.id);
        let aliases = function
            .param_names
            .iter()
            .enumerate()
            .filter(|(index, _)| view_parameters.is_some_and(|positions| positions.contains(index)))
            .map(|(index, name)| (name.clone(), format!("${index}")))
            .collect();
        analyze_statements(
            &statements,
            function.id,
            &function.name,
            &context,
            &mut locals,
            &aliases,
            &mut effects,
        );
        direct_by_id.push(effects);
    }

    let mut out = Vec::with_capacity(functions.len());
    for function in functions {
        let mut aggregate = EffectSets::default();
        collect_aggregate_effects(
            function.id,
            functions,
            &direct_by_id,
            &mut BTreeSet::new(),
            &BTreeMap::new(),
            true,
            &mut aggregate,
        );
        let file = &files[function.file_id as usize];
        out.push(FunctionDataFlowSummary {
            schema_version: FUNCTION_DATA_FLOW_SCHEMA_VERSION,
            function: function.name.clone(),
            file: file.path.clone(),
            source_start: function.source_range.start,
            source_end: function.source_range.end,
            signature_hash: format!("{:016x}", function.signature_hash),
            direct: direct_by_id[function.id as usize].to_effects(&function.param_names),
            aggregate: aggregate.to_effects(&function.param_names),
        });
    }
    out.sort_by(|left, right| {
        left.file
            .cmp(&right.file)
            .then(left.source_start.cmp(&right.source_start))
            .then(left.function.cmp(&right.function))
    });
    Ok(out)
}

fn build_context(
    files: &[SourceFile],
    functions: &[FunctionMeta],
    types: &TypeTable,
) -> Result<AnalysisContext, String> {
    let mut globals = BTreeSet::new();
    let mut constants = BTreeMap::new();
    let mut extern_functions = BTreeSet::new();
    let mut structs = BTreeMap::new();
    let mut global_types = Vec::new();
    for file in files {
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
        for external in parse_top_level_extern_functions(&file.content)? {
            extern_functions.insert(external.name);
        }
    }
    let mut collection_capacities = BTreeMap::new();
    for (path, type_name) in global_types {
        collect_collection_capacities(
            &path,
            &type_name,
            &structs,
            &constants,
            &mut collection_capacities,
            &mut BTreeSet::new(),
        );
    }
    let mut internal_view_parameters: BTreeMap<String, BTreeSet<usize>> = BTreeMap::new();
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
                internal_view_parameters
                    .entry(function.name.clone())
                    .or_default()
                    .insert(index);
            }
            if let Some(capacity) = types
                .fixed_collection_len(*type_id)
                .and_then(|capacity| u64::try_from(capacity).ok())
            {
                capacities.insert(index, capacity);
            }
        }
        view_parameters_by_function.insert(function.id, positions);
        fixed_parameter_capacities.insert(function.id, capacities);
    }
    Ok(AnalysisContext {
        globals,
        constants,
        internal_functions: functions
            .iter()
            .map(|function| function.name.clone())
            .collect(),
        internal_view_parameters,
        view_parameters_by_function,
        fixed_parameter_capacities,
        extern_functions,
        collection_capacities,
    })
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
    context: &AnalysisContext,
    locals: &mut BTreeSet<String>,
    aliases: &BTreeMap<String, String>,
    effects: &mut EffectSets,
) {
    for statement in statements {
        match statement {
            SimpleStmt::Noop | SimpleStmt::Continue | SimpleStmt::ReturnVoid => {}
            SimpleStmt::Let {
                name, expression, ..
            } => {
                analyze_expression(expression, context, locals, aliases, effects);
                locals.insert(name.clone());
            }
            SimpleStmt::Assign {
                target,
                op,
                expression,
            } => {
                analyze_expression(expression, context, locals, aliases, effects);
                analyze_assignment_target(
                    target,
                    *op != AssignOp::Set,
                    context,
                    locals,
                    aliases,
                    effects,
                );
            }
            SimpleStmt::Convert { target, source, .. } => {
                analyze_expression(source, context, locals, aliases, effects);
                analyze_assignment_target(target, false, context, locals, aliases, effects);
            }
            SimpleStmt::If {
                condition,
                then_statements,
                else_statements,
            } => {
                analyze_condition(condition, context, locals, aliases, effects);
                analyze_nested_statements(
                    then_statements,
                    function_id,
                    function,
                    context,
                    locals,
                    aliases,
                    effects,
                );
                if let Some(else_statements) = else_statements {
                    analyze_nested_statements(
                        else_statements,
                        function_id,
                        function,
                        context,
                        locals,
                        aliases,
                        effects,
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
                analyze_statements(
                    std::slice::from_ref(init.as_ref()),
                    function_id,
                    function,
                    context,
                    &mut loop_locals,
                    aliases,
                    effects,
                );
                let mut bound_reads = EffectSets::default();
                analyze_condition(condition, context, &loop_locals, aliases, &mut bound_reads);
                effects.merge(&bound_reads);
                effects.bounded_iterations.insert(FunctionBoundedIteration {
                    function: function.to_string(),
                    kind: "for".to_string(),
                    bound: display_condition(condition),
                    max_iterations: static_for_max_iterations(
                        init,
                        condition,
                        step,
                        &context.constants,
                    ),
                    reads: bound_reads.reads.into_iter().collect(),
                });
                analyze_nested_statements(
                    body_statements,
                    function_id,
                    function,
                    context,
                    &loop_locals,
                    aliases,
                    effects,
                );
                analyze_statements(
                    std::slice::from_ref(step.as_ref()),
                    function_id,
                    function,
                    context,
                    &mut loop_locals,
                    aliases,
                    effects,
                );
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
                effects.bounded_iterations.insert(FunctionBoundedIteration {
                    function: function.to_string(),
                    kind: "foreach".to_string(),
                    bound: normalized.clone(),
                    max_iterations: context
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
                        }),
                    reads: (context.globals.contains(root_name(&normalized))
                        || normalized.starts_with('$'))
                    .then_some(vec![bound_path])
                    .unwrap_or_default(),
                });
                let mut loop_locals = locals.clone();
                loop_locals.insert(item_name.clone());
                if let Some(index_name) = index_name {
                    loop_locals.insert(index_name.clone());
                }
                let mut loop_aliases = aliases.clone();
                if state_collection.is_some() {
                    loop_aliases.insert(item_name.clone(), format!("{normalized}[*]"));
                }
                analyze_nested_statements(
                    body_statements,
                    function_id,
                    function,
                    context,
                    &loop_locals,
                    &loop_aliases,
                    effects,
                );
            }
            SimpleStmt::Expr(expression) | SimpleStmt::Return(expression) => {
                analyze_expression(expression, context, locals, aliases, effects);
            }
        }
    }
}

fn analyze_nested_statements(
    statements: &[SimpleStmt],
    function_id: u32,
    function: &str,
    context: &AnalysisContext,
    locals: &BTreeSet<String>,
    aliases: &BTreeMap<String, String>,
    effects: &mut EffectSets,
) {
    analyze_statements(
        statements,
        function_id,
        function,
        context,
        &mut locals.clone(),
        aliases,
        effects,
    );
}

fn analyze_assignment_target(
    target: &AssignTarget,
    reads_existing: bool,
    context: &AnalysisContext,
    locals: &BTreeSet<String>,
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
            analyze_expression(index, context, locals, aliases, effects);
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
    context: &AnalysisContext,
    locals: &BTreeSet<String>,
    aliases: &BTreeMap<String, String>,
    effects: &mut EffectSets,
) {
    match expression {
        SimpleExpr::Int(_)
        | SimpleExpr::Float(_)
        | SimpleExpr::Bool(_)
        | SimpleExpr::StringLiteral(_) => {}
        SimpleExpr::Condition(condition) => {
            analyze_condition(condition, context, locals, aliases, effects)
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
            analyze_expression(index, context, locals, aliases, effects);
            if let Some(path) = normalize_state_path(collection_path, context, locals, aliases) {
                effects.insert_read(indexed_state_path(&path, suffix));
            }
        }
        SimpleExpr::Call { target, args } => {
            if context.internal_functions.contains(target) {
                effects.calls.insert(target.clone());
                effects.call_sites.push(CallSite {
                    target: target.clone(),
                    arguments: args
                        .iter()
                        .map(|argument| expression_effect_path(argument, context, locals, aliases))
                        .collect(),
                });
            } else if context.extern_functions.contains(target) {
                effects.host_calls.insert(target.clone());
            }
            for (index, argument) in args.iter().enumerate() {
                let is_view = context
                    .internal_view_parameters
                    .get(target)
                    .is_some_and(|positions| positions.contains(&index));
                if is_view {
                    analyze_view_argument(argument, context, locals, aliases, effects);
                } else {
                    analyze_expression(argument, context, locals, aliases, effects);
                }
            }
        }
        SimpleExpr::Binary { lhs, rhs, .. } => {
            analyze_expression(lhs, context, locals, aliases, effects);
            analyze_expression(rhs, context, locals, aliases, effects);
        }
    }
}

fn analyze_view_argument(
    expression: &SimpleExpr,
    context: &AnalysisContext,
    locals: &BTreeSet<String>,
    aliases: &BTreeMap<String, String>,
    effects: &mut EffectSets,
) {
    match expression {
        SimpleExpr::Identifier(_) => {}
        SimpleExpr::IndexedPath { index, .. } => {
            analyze_expression(index, context, locals, aliases, effects)
        }
        _ => analyze_expression(expression, context, locals, aliases, effects),
    }
}

fn analyze_condition(
    condition: &SimpleCondition,
    context: &AnalysisContext,
    locals: &BTreeSet<String>,
    aliases: &BTreeMap<String, String>,
    effects: &mut EffectSets,
) {
    match condition {
        SimpleCondition::Comparison { lhs, rhs, .. } => {
            analyze_expression(lhs, context, locals, aliases, effects);
            analyze_expression(rhs, context, locals, aliases, effects);
        }
        SimpleCondition::Expr(expression) => {
            analyze_expression(expression, context, locals, aliases, effects)
        }
        SimpleCondition::And(lhs, rhs) | SimpleCondition::Or(lhs, rhs) => {
            analyze_condition(lhs, context, locals, aliases, effects);
            analyze_condition(rhs, context, locals, aliases, effects);
        }
        SimpleCondition::Not(inner) => analyze_condition(inner, context, locals, aliases, effects),
    }
}

fn normalize_state_path(
    path: &str,
    context: &AnalysisContext,
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

fn collect_aggregate_effects(
    function_id: u32,
    functions: &[FunctionMeta],
    direct_by_id: &[EffectSets],
    visiting: &mut BTreeSet<u32>,
    substitutions: &BTreeMap<usize, String>,
    retain_unmapped_parameters: bool,
    aggregate: &mut EffectSets,
) {
    if !visiting.insert(function_id) {
        return;
    }
    let Some(function) = functions.get(function_id as usize) else {
        visiting.remove(&function_id);
        return;
    };
    if let Some(direct) = direct_by_id.get(function_id as usize) {
        merge_substituted_effects(direct, substitutions, retain_unmapped_parameters, aggregate);
        for call_site in &direct.call_sites {
            for dependency in function.dependencies.iter().filter(|dependency| {
                functions
                    .get(**dependency as usize)
                    .is_some_and(|callee| callee.name == call_site.target)
            }) {
                let child_substitutions = call_site
                    .arguments
                    .iter()
                    .enumerate()
                    .filter_map(|(index, path)| {
                        substitute_path(path.as_deref()?, substitutions, retain_unmapped_parameters)
                            .map(|path| (index, path))
                    })
                    .collect();
                collect_aggregate_effects(
                    *dependency,
                    functions,
                    direct_by_id,
                    visiting,
                    &child_substitutions,
                    false,
                    aggregate,
                );
            }
        }
    }
    visiting.remove(&function_id);
}

fn merge_substituted_effects(
    direct: &EffectSets,
    substitutions: &BTreeMap<usize, String>,
    retain_unmapped_parameters: bool,
    aggregate: &mut EffectSets,
) {
    aggregate.reads.extend(direct.reads.iter().cloned());
    aggregate.writes.extend(direct.writes.iter().cloned());
    aggregate.calls.extend(direct.calls.iter().cloned());
    aggregate
        .host_calls
        .extend(direct.host_calls.iter().cloned());
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
        reads: iteration
            .reads
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
}
