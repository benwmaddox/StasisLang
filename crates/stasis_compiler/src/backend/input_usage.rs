//! Whole-program HostFrame input usage analysis.
//!
//! The replay host owns the raw `host_i32`/`host_f32` arrays.  This module
//! reports the subset of those arrays that reachable Stasis code can read.
//! It deliberately works from accepted HIR rather than source text so calls
//! lowered through wrappers and imported modules have the same reachability
//! rules as code generation.

use std::collections::{BTreeMap, BTreeSet};

use crate::backend::compile_analysis::ConstantValue;
use crate::compiler::{FunctionId, FunctionMeta};
use crate::ir::hir::{
    AssignOp, AssignTarget, ComparisonOp, FunctionHIR, SimpleCondition, SimpleExpr, SimpleStmt,
};

pub const HOST_I32_COUNT: usize = 768;
pub const HOST_F32_COUNT: usize = 64;

const HOST_I_KEY_BASE: usize = 32;
const HOST_I_KEY_COUNT: usize = 512;
const HOST_I_POINTER_BASE: usize = 544;
const HOST_I_POINTER_STRIDE: usize = 4;
const HOST_I_POINTER_COUNT: usize = 8;

const HOST_F_POINTER_BASE: usize = 0;
const HOST_F_POINTER_STRIDE: usize = 6;
const HOST_F_POINTER_COUNT: usize = 8;

// Exact integer provenance is a useful precision optimization, but it must
// remain a finite lattice for recursive calls such as `depth - 1`.
const MAX_EXACT_INTEGER_VALUES: usize = 16;

/// The ABI lane containing an observed HostFrame value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HostFrameInputLane {
    I32,
    F32,
}

impl HostFrameInputLane {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::I32 => "i32",
            Self::F32 => "f32",
        }
    }
}

/// Stable semantic family for one HostFrame slot.
///
/// The family is intentionally coarser than the exact slot.  A dynamic index
/// can therefore conservatively select an entire keyboard, pointer, display,
/// or raw lane family while exact accesses retain their individual slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HostFrameInputFamily {
    Keyboard,
    Pointer,
    Display,
    RawI32,
    RawF32,
}

impl HostFrameInputFamily {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Keyboard => "keyboard",
            Self::Pointer => "pointer",
            Self::Display => "display",
            Self::RawI32 => "i32",
            Self::RawF32 => "f32",
        }
    }
}

/// One canonical observed raw HostFrame value.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct HostFrameInputField {
    /// Zero-based position in the compact replay lane for this type.
    pub slot: usize,
    /// Zero-based position in the raw HostFrame lane.
    pub index: usize,
    /// Stable semantic path used by replay identities and diagnostics.
    pub path: String,
    pub lane: HostFrameInputLane,
    pub family: HostFrameInputFamily,
}

/// Deterministic whole-game union of raw HostFrame reads.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HostFrameInputUsage {
    i32_fields: Vec<HostFrameInputField>,
    f32_fields: Vec<HostFrameInputField>,
}

impl HostFrameInputUsage {
    pub fn i32_fields(&self) -> &[HostFrameInputField] {
        &self.i32_fields
    }

    pub fn f32_fields(&self) -> &[HostFrameInputField] {
        &self.f32_fields
    }

    pub fn is_empty(&self) -> bool {
        self.i32_fields.is_empty() && self.f32_fields.is_empty()
    }

    fn add_i32_range(&mut self, start: usize, end: usize, family: HostFrameInputFamily) {
        for index in start..end {
            self.add_i32(index, family);
        }
    }

    fn add_f32_range(&mut self, start: usize, end: usize, family: HostFrameInputFamily) {
        for index in start..end {
            self.add_f32(index, family);
        }
    }

    fn add_i32(&mut self, index: usize, family: HostFrameInputFamily) {
        if index >= HOST_I32_COUNT {
            return;
        }
        insert_field(
            &mut self.i32_fields,
            HostFrameInputField {
                slot: 0,
                index,
                path: i32_path(index),
                lane: HostFrameInputLane::I32,
                family,
            },
        );
    }

    fn add_f32(&mut self, index: usize, family: HostFrameInputFamily) {
        if index >= HOST_F32_COUNT {
            return;
        }
        insert_field(
            &mut self.f32_fields,
            HostFrameInputField {
                slot: 0,
                index,
                path: f32_path(index),
                lane: HostFrameInputLane::F32,
                family,
            },
        );
    }

    fn finalize(&mut self) {
        self.i32_fields.sort_by_key(|field| field.index);
        self.f32_fields.sort_by_key(|field| field.index);
        for (slot, field) in self.i32_fields.iter_mut().enumerate() {
            field.slot = slot;
        }
        for (slot, field) in self.f32_fields.iter_mut().enumerate() {
            field.slot = slot;
        }
    }
}

fn insert_field(fields: &mut Vec<HostFrameInputField>, field: HostFrameInputField) {
    if let Some(existing) = fields
        .iter_mut()
        .find(|existing| existing.index == field.index)
    {
        // The first classification is normally the most specific one.  If an
        // earlier dynamic access marked a slot raw and a later exact access
        // gives it a known family, preserve the more useful classification.
        if existing.family == HostFrameInputFamily::RawI32
            || existing.family == HostFrameInputFamily::RawF32
        {
            *existing = field;
        }
        return;
    }
    fields.push(field);
}

fn i32_path(index: usize) -> String {
    if (HOST_I_KEY_BASE..HOST_I_KEY_BASE + HOST_I_KEY_COUNT).contains(&index) {
        return format!("keys[{}]", index - HOST_I_KEY_BASE);
    }
    if (HOST_I_POINTER_BASE..HOST_I_POINTER_BASE + HOST_I_POINTER_STRIDE * HOST_I_POINTER_COUNT)
        .contains(&index)
    {
        let relative = index - HOST_I_POINTER_BASE;
        let pointer = relative / HOST_I_POINTER_STRIDE;
        let field = match relative % HOST_I_POINTER_STRIDE {
            0 => "id",
            1 => "is_down",
            2 => "went_down",
            _ => "went_up",
        };
        return format!("pointers[{pointer}].{field}");
    }
    match index {
        0 => "time_ms".to_string(),
        7 => "pointer_count".to_string(),
        8 => "dropped_pointer_count".to_string(),
        9 => "quit_requested".to_string(),
        10 => "tick_index".to_string(),
        11 => "resized".to_string(),
        12 => "screen_width_px".to_string(),
        13 => "screen_height_px".to_string(),
        14 => "version".to_string(),
        15 => "flags".to_string(),
        16 => "tick_hz".to_string(),
        17 => "window_focused".to_string(),
        18 => "window_minimized".to_string(),
        19 => "time_us".to_string(),
        22 => "native_width_px".to_string(),
        23 => "native_height_px".to_string(),
        24 => "drawable_width_px".to_string(),
        25 => "drawable_height_px".to_string(),
        30 => "display.generation".to_string(),
        31 => "display.density_generation".to_string(),
        _ => format!("host_i32[{index}]"),
    }
}

fn f32_path(index: usize) -> String {
    if (HOST_F_POINTER_BASE..HOST_F_POINTER_BASE + HOST_F_POINTER_STRIDE * HOST_F_POINTER_COUNT)
        .contains(&index)
    {
        let pointer = index / HOST_F_POINTER_STRIDE;
        let field = match index % HOST_F_POINTER_STRIDE {
            0 => "x_logical",
            1 => "y_logical",
            2 => "dx_logical",
            3 => "dy_logical",
            4 => "x_normalized",
            _ => "y_normalized",
        };
        return format!("pointers[{pointer}].{field}");
    }
    match index {
        48 => "display.content_scale".to_string(),
        49 => "display.raster_scale".to_string(),
        50 => "display.logical_width".to_string(),
        51 => "display.logical_height".to_string(),
        52 => "display.safe_x".to_string(),
        53 => "display.safe_y".to_string(),
        54 => "display.safe_width".to_string(),
        55 => "display.safe_height".to_string(),
        56 => "display.available_width".to_string(),
        57 => "display.available_height".to_string(),
        _ => format!("host_f32[{index}]"),
    }
}

/// Analyze HostFrame reads while carrying raw-lane provenance through local
/// aliases and calls to internal helpers.
///
/// `FunctionHIR` intentionally contains only lowered bodies, so the function
/// metadata is supplied separately for call argument-to-parameter mapping.
/// The compatibility wrapper above still supports callers that only need
/// direct reads; compiler snapshot construction should use this entry point.
pub(crate) fn analyze_host_frame_input_usage_with_functions(
    function_hirs: &BTreeMap<FunctionId, FunctionHIR>,
    reachable_function_ids: &BTreeSet<FunctionId>,
    constants: &BTreeMap<String, ConstantValue>,
    functions: &[FunctionMeta],
) -> HostFrameInputUsage {
    let mut usage = HostFrameInputUsage::default();
    let mut states = reachable_function_ids
        .iter()
        .copied()
        .map(|function_id| (function_id, FunctionInputState::default()))
        .collect::<BTreeMap<_, _>>();
    let mut pending = reachable_function_ids.iter().copied().collect::<Vec<_>>();

    while let Some(function_id) = pending.pop() {
        let Some(hir) = function_hirs.get(&function_id) else {
            continue;
        };
        let Some(function) = functions.iter().find(|function| function.id == function_id) else {
            continue;
        };
        let state = states.get(&function_id).cloned().unwrap_or_default();
        let mut environment = LocalInputEnvironment::from_function(function, &state);
        let mut calls = Vec::new();
        visit_statements_with_environment(
            &hir.statements,
            constants,
            &mut environment,
            &mut usage,
            &mut calls,
        );

        for call in calls {
            for callee in matching_callees(&call.target, call.args.len(), functions) {
                if !reachable_function_ids.contains(&callee.id) {
                    continue;
                }
                let callee_state = states.entry(callee.id).or_default();
                let mut changed = false;
                for (parameter, argument) in callee.param_names.iter().zip(&call.args) {
                    if !argument.collections.is_empty() {
                        let origins = callee_state
                            .collections
                            .entry(parameter.clone())
                            .or_default();
                        let previous_len = origins.len();
                        origins.extend(argument.collections.iter().copied());
                        changed |= origins.len() != previous_len;
                    }
                    match &argument.integer_values {
                        Some(values) => {
                            let entry = callee_state
                                .integer_values
                                .entry(parameter.clone())
                                .or_insert_with(|| Some(BTreeSet::new()));
                            changed |= union_integer_values(entry, values);
                        }
                        None => {
                            let was_known = callee_state
                                .integer_values
                                .insert(parameter.clone(), None)
                                .is_some_and(|values| values.is_some());
                            changed |= was_known;
                        }
                    }
                }
                if changed {
                    pending.push(callee.id);
                }
            }
        }
    }

    usage.finalize();
    usage
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum CollectionOrigin {
    HostI32,
    HostF32,
}

#[derive(Debug, Clone, Default)]
struct FunctionInputState {
    collections: BTreeMap<String, BTreeSet<CollectionOrigin>>,
    integer_values: BTreeMap<String, Option<BTreeSet<i64>>>,
}

#[derive(Debug, Clone, Default)]
struct LocalInputEnvironment {
    collections: BTreeMap<String, BTreeSet<CollectionOrigin>>,
    integer_values: BTreeMap<String, Option<BTreeSet<i64>>>,
    integer_intervals: BTreeMap<String, Option<IntegerInterval>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct IntegerInterval {
    min: i64,
    max: i64,
}

impl IntegerInterval {
    fn from_values(values: &BTreeSet<i64>) -> Option<Self> {
        Some(Self {
            min: *values.first()?,
            max: *values.last()?,
        })
    }
}

impl LocalInputEnvironment {
    fn from_function(function: &FunctionMeta, state: &FunctionInputState) -> Self {
        let mut environment = Self::default();
        for parameter in &function.param_names {
            if let Some(origins) = state.collections.get(parameter) {
                environment
                    .collections
                    .insert(parameter.clone(), origins.clone());
            }
            if let Some(values) = state.integer_values.get(parameter) {
                environment
                    .integer_values
                    .insert(parameter.clone(), values.clone());
            }
        }
        environment
    }
}

#[derive(Debug, Clone, Default)]
struct CallArgumentInput {
    collections: BTreeSet<CollectionOrigin>,
    integer_values: Option<BTreeSet<i64>>,
}

#[derive(Debug, Clone)]
struct PendingCall {
    target: String,
    args: Vec<CallArgumentInput>,
}

fn matching_callees<'a>(
    target: &str,
    argument_count: usize,
    functions: &'a [FunctionMeta],
) -> Vec<&'a FunctionMeta> {
    functions
        .iter()
        .filter(|function| {
            function.params.len() == argument_count
                && (function.name == target
                    || (!function.module_alias.is_empty()
                        && format!("{}.{}", function.module_alias, function.name) == target))
        })
        .collect()
}

fn visit_statements_with_environment(
    statements: &[SimpleStmt],
    constants: &BTreeMap<String, ConstantValue>,
    environment: &mut LocalInputEnvironment,
    usage: &mut HostFrameInputUsage,
    calls: &mut Vec<PendingCall>,
) {
    for statement in statements {
        match statement {
            SimpleStmt::Noop | SimpleStmt::Continue | SimpleStmt::ReturnVoid => {}
            SimpleStmt::Let {
                name, expression, ..
            } => {
                visit_expr_with_environment(expression, constants, environment, usage, calls);
                bind_local(name, expression, constants, environment);
            }
            SimpleStmt::Assign {
                target,
                op,
                expression,
            } => {
                visit_assign_target_with_environment(
                    target,
                    *op != AssignOp::Set,
                    constants,
                    environment,
                    usage,
                    calls,
                );
                visit_expr_with_environment(expression, constants, environment, usage, calls);
                if let AssignTarget::Local(name) = target {
                    if *op == AssignOp::Set {
                        bind_local(name, expression, constants, environment);
                    } else {
                        invalidate_local(name, environment);
                    }
                }
            }
            SimpleStmt::Convert { target, source, .. } => {
                visit_assign_target_with_environment(
                    target,
                    false,
                    constants,
                    environment,
                    usage,
                    calls,
                );
                visit_expr_with_environment(source, constants, environment, usage, calls);
                if let AssignTarget::Local(name) = target {
                    bind_local(name, source, constants, environment);
                }
            }
            SimpleStmt::If {
                condition,
                then_statements,
                else_statements,
            } => {
                visit_condition_with_environment(condition, constants, environment, usage, calls);
                let before = environment.clone();
                let mut then_environment = before.clone();
                visit_statements_with_environment(
                    then_statements,
                    constants,
                    &mut then_environment,
                    usage,
                    calls,
                );
                let mut else_environment = before.clone();
                if let Some(else_statements) = else_statements {
                    visit_statements_with_environment(
                        else_statements,
                        constants,
                        &mut else_environment,
                        usage,
                        calls,
                    );
                }
                merge_environments(environment, &before, &then_environment, &else_environment);
            }
            SimpleStmt::For {
                init,
                condition,
                step,
                body_statements,
            } => {
                visit_statements_with_environment(
                    std::slice::from_ref(init),
                    constants,
                    environment,
                    usage,
                    calls,
                );
                let before = environment.clone();
                let mut loop_environment = before.clone();
                invalidate_repeated_assignments(std::slice::from_ref(step), &mut loop_environment);
                invalidate_repeated_assignments(body_statements, &mut loop_environment);
                refine_canonical_for_interval(
                    condition,
                    step,
                    constants,
                    &before,
                    &mut loop_environment,
                );
                visit_condition_with_environment(
                    condition,
                    constants,
                    &mut loop_environment,
                    usage,
                    calls,
                );
                visit_statements_with_environment(
                    body_statements,
                    constants,
                    &mut loop_environment,
                    usage,
                    calls,
                );
                visit_statements_with_environment(
                    std::slice::from_ref(step),
                    constants,
                    &mut loop_environment,
                    usage,
                    calls,
                );
                merge_environments(environment, &before, &loop_environment, &before);
            }
            SimpleStmt::Foreach {
                collection_path,
                item_name,
                index_name,
                body_statements,
            } => {
                visit_collection_with_environment(collection_path, environment, usage);
                let before = environment.clone();
                let mut body_environment = before.clone();
                body_environment.collections.remove(item_name);
                body_environment
                    .integer_values
                    .insert(item_name.clone(), None);
                body_environment
                    .integer_intervals
                    .insert(item_name.clone(), None);
                if let Some(index_name) = index_name {
                    body_environment
                        .integer_values
                        .insert(index_name.clone(), None);
                    body_environment
                        .integer_intervals
                        .insert(index_name.clone(), None);
                }
                visit_statements_with_environment(
                    body_statements,
                    constants,
                    &mut body_environment,
                    usage,
                    calls,
                );
                merge_environments(environment, &before, &body_environment, &before);
            }
            SimpleStmt::Expr(expression) | SimpleStmt::Return(expression) => {
                visit_expr_with_environment(expression, constants, environment, usage, calls)
            }
        }
    }
}

fn invalidate_repeated_assignments(
    statements: &[SimpleStmt],
    environment: &mut LocalInputEnvironment,
) {
    for statement in statements {
        match statement {
            SimpleStmt::Assign {
                target: AssignTarget::Local(name),
                ..
            }
            | SimpleStmt::Convert {
                target: AssignTarget::Local(name),
                ..
            } => {
                environment.integer_values.insert(name.clone(), None);
                environment.integer_intervals.insert(name.clone(), None);
                environment.collections.remove(name);
            }
            SimpleStmt::If {
                then_statements,
                else_statements,
                ..
            } => {
                invalidate_repeated_assignments(then_statements, environment);
                if let Some(else_statements) = else_statements {
                    invalidate_repeated_assignments(else_statements, environment);
                }
            }
            SimpleStmt::For {
                step,
                body_statements,
                ..
            } => {
                invalidate_repeated_assignments(std::slice::from_ref(step), environment);
                invalidate_repeated_assignments(body_statements, environment);
            }
            SimpleStmt::Foreach {
                body_statements, ..
            } => invalidate_repeated_assignments(body_statements, environment),
            _ => {}
        }
    }
}

fn visit_assign_target_with_environment(
    target: &AssignTarget,
    reads_current_value: bool,
    constants: &BTreeMap<String, ConstantValue>,
    environment: &LocalInputEnvironment,
    usage: &mut HostFrameInputUsage,
    calls: &mut Vec<PendingCall>,
) {
    if let AssignTarget::IndexedPath {
        collection_path,
        index,
        suffix,
    } = target
    {
        visit_expr_with_environment(index, constants, environment, usage, calls);
        if reads_current_value && suffix.is_empty() {
            observe_collection_index_with_environment(
                collection_path,
                index,
                constants,
                environment,
                usage,
            );
        }
    }
}

fn visit_condition_with_environment(
    condition: &SimpleCondition,
    constants: &BTreeMap<String, ConstantValue>,
    environment: &mut LocalInputEnvironment,
    usage: &mut HostFrameInputUsage,
    calls: &mut Vec<PendingCall>,
) {
    match condition {
        SimpleCondition::Comparison { lhs, rhs, .. } => {
            visit_expr_with_environment(lhs, constants, environment, usage, calls);
            visit_expr_with_environment(rhs, constants, environment, usage, calls);
        }
        SimpleCondition::Expr(expression) => {
            visit_expr_with_environment(expression, constants, environment, usage, calls)
        }
        SimpleCondition::And(lhs, rhs) | SimpleCondition::Or(lhs, rhs) => {
            visit_condition_with_environment(lhs, constants, environment, usage, calls);
            visit_condition_with_environment(rhs, constants, environment, usage, calls);
        }
        SimpleCondition::Not(inner) => {
            visit_condition_with_environment(inner, constants, environment, usage, calls)
        }
    }
}

fn visit_expr_with_environment(
    expression: &SimpleExpr,
    constants: &BTreeMap<String, ConstantValue>,
    environment: &LocalInputEnvironment,
    usage: &mut HostFrameInputUsage,
    calls: &mut Vec<PendingCall>,
) {
    match expression {
        SimpleExpr::IndexedPath {
            collection_path,
            index,
            suffix,
        } => {
            visit_expr_with_environment(index, constants, environment, usage, calls);
            if suffix.is_empty() {
                observe_collection_index_with_environment(
                    collection_path,
                    index,
                    constants,
                    environment,
                    usage,
                );
            }
        }
        SimpleExpr::Call { target, args } => {
            for arg in args {
                visit_expr_with_environment(arg, constants, environment, usage, calls);
            }
            observe_bulk_memory_source(target, args, constants, environment, usage);
            calls.push(PendingCall {
                target: target.clone(),
                args: args
                    .iter()
                    .map(|arg| CallArgumentInput {
                        collections: collection_origins(arg, environment),
                        integer_values: integer_values(arg, constants, environment),
                    })
                    .collect(),
            });
        }
        SimpleExpr::Binary { lhs, rhs, .. } => {
            visit_expr_with_environment(lhs, constants, environment, usage, calls);
            visit_expr_with_environment(rhs, constants, environment, usage, calls);
        }
        SimpleExpr::Condition(condition) => visit_condition_with_environment(
            condition,
            constants,
            &mut environment.clone(),
            usage,
            calls,
        ),
        SimpleExpr::DefaultValue(_)
        | SimpleExpr::Int(_)
        | SimpleExpr::Float(_)
        | SimpleExpr::Bool(_)
        | SimpleExpr::StringLiteral(_)
        | SimpleExpr::Identifier(_) => {}
    }
}

fn refine_canonical_for_interval(
    condition: &SimpleCondition,
    step: &SimpleStmt,
    constants: &BTreeMap<String, ConstantValue>,
    before: &LocalInputEnvironment,
    loop_environment: &mut LocalInputEnvironment,
) {
    let SimpleCondition::Comparison { lhs, op, rhs } = condition else {
        return;
    };
    let SimpleExpr::Identifier(name) = lhs else {
        return;
    };
    let SimpleStmt::Assign {
        target: AssignTarget::Local(step_name),
        op: AssignOp::Add,
        expression: step_value,
    } = step
    else {
        return;
    };
    if step_name != name {
        return;
    }
    let Some(step_values) = integer_values(step_value, constants, before) else {
        return;
    };
    if step_values.len() != 1 || step_values.first().is_none_or(|value| *value <= 0) {
        return;
    }
    let Some(start) = before
        .integer_intervals
        .get(name)
        .copied()
        .flatten()
        .filter(|interval| interval.min == interval.max)
        .map(|interval| interval.min)
    else {
        return;
    };
    let Some(limit_values) = integer_values(rhs, constants, before) else {
        return;
    };
    if limit_values.len() != 1 {
        return;
    }
    let limit = *limit_values.first().expect("singleton checked above");
    let max = match op {
        ComparisonOp::Lt => limit.checked_sub(1),
        ComparisonOp::Le => Some(limit),
        _ => None,
    };
    let Some(max) = max.filter(|max| *max >= start) else {
        return;
    };
    loop_environment
        .integer_intervals
        .insert(name.clone(), Some(IntegerInterval { min: start, max }));
}

fn observe_bulk_memory_source(
    target: &str,
    args: &[SimpleExpr],
    constants: &BTreeMap<String, ConstantValue>,
    environment: &LocalInputEnvironment,
    usage: &mut HostFrameInputUsage,
) {
    let operation = target.rsplit('.').next().unwrap_or(target);
    let expected_origin = match operation {
        "sys_memcpy_i32" | "sys_memmove_i32" => CollectionOrigin::HostI32,
        "sys_memcpy_f32" | "sys_memmove_f32" => CollectionOrigin::HostF32,
        _ => return,
    };
    let [_, _, source, source_index, count] = args else {
        return;
    };
    if !collection_origins(source, environment).contains(&expected_origin) {
        return;
    }
    let exact = integer_values(source_index, constants, environment)
        .filter(|values| values.len() == 1)
        .and_then(|values| values.first().copied())
        .zip(
            integer_values(count, constants, environment)
                .filter(|values| values.len() == 1)
                .and_then(|values| values.first().copied()),
        )
        .and_then(|(start, count)| {
            let start = usize::try_from(start).ok()?;
            let count = usize::try_from(count).ok()?;
            start.checked_add(count).map(|end| (start, end))
        });
    match (expected_origin, exact) {
        (CollectionOrigin::HostI32, Some((start, end))) if end <= HOST_I32_COUNT => {
            for index in start..end {
                usage.add_i32(index, classify_i32(index));
            }
        }
        (CollectionOrigin::HostF32, Some((start, end))) if end <= HOST_F32_COUNT => {
            for index in start..end {
                usage.add_f32(index, classify_f32(index));
            }
        }
        (CollectionOrigin::HostI32, _) => {
            usage.add_i32_range(0, HOST_I32_COUNT, HostFrameInputFamily::RawI32)
        }
        (CollectionOrigin::HostF32, _) => {
            usage.add_f32_range(0, HOST_F32_COUNT, HostFrameInputFamily::RawF32)
        }
    }
}

fn bind_local(
    name: &str,
    expression: &SimpleExpr,
    constants: &BTreeMap<String, ConstantValue>,
    environment: &mut LocalInputEnvironment,
) {
    let collections = collection_origins(expression, environment);
    if collections.is_empty() {
        environment.collections.remove(name);
    } else {
        environment
            .collections
            .insert(name.to_string(), collections);
    }
    if let Some(values) = integer_values(expression, constants, environment) {
        let interval = IntegerInterval::from_values(&values);
        environment
            .integer_values
            .insert(name.to_string(), Some(values));
        environment
            .integer_intervals
            .insert(name.to_string(), interval);
    } else {
        environment.integer_values.insert(name.to_string(), None);
        environment.integer_intervals.insert(
            name.to_string(),
            integer_interval(expression, constants, environment),
        );
    }
}

fn invalidate_local(name: &str, environment: &mut LocalInputEnvironment) {
    environment.integer_values.insert(name.to_string(), None);
    environment.integer_intervals.insert(name.to_string(), None);
    environment.collections.remove(name);
}

fn union_integer_values(destination: &mut Option<BTreeSet<i64>>, values: &BTreeSet<i64>) -> bool {
    let Some(existing) = destination.as_mut() else {
        // Unknown is absorbing: once precision is lost, later recursive
        // iterations must not make the state precise again.
        return false;
    };
    let previous_len = existing.len();
    existing.extend(values.iter().copied());
    if existing.len() > MAX_EXACT_INTEGER_VALUES {
        *destination = None;
        return true;
    }
    existing.len() != previous_len
}

fn merge_environments(
    destination: &mut LocalInputEnvironment,
    base: &LocalInputEnvironment,
    left: &LocalInputEnvironment,
    right: &LocalInputEnvironment,
) {
    let collection_names = base
        .collections
        .keys()
        .chain(left.collections.keys())
        .chain(right.collections.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    destination.collections.clear();
    for name in collection_names {
        let mut origins = BTreeSet::new();
        for environment in [base, left, right] {
            if let Some(values) = environment.collections.get(&name) {
                origins.extend(values.iter().copied());
            }
        }
        if !origins.is_empty() {
            destination.collections.insert(name, origins);
        }
    }

    let integer_names = base
        .integer_values
        .keys()
        .chain(left.integer_values.keys())
        .chain(right.integer_values.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    destination.integer_values.clear();
    for name in integer_names {
        let mut values = BTreeSet::new();
        let mut unknown = false;
        let mut present = false;
        for environment in [base, left, right] {
            if let Some(candidate) = environment.integer_values.get(&name) {
                present = true;
                match candidate {
                    Some(candidate) => {
                        values.extend(candidate.iter().copied());
                        if values.len() > MAX_EXACT_INTEGER_VALUES {
                            unknown = true;
                        }
                    }
                    None => unknown = true,
                }
            }
        }
        if present {
            destination
                .integer_values
                .insert(name, (!unknown).then_some(values));
        }
    }

    let interval_names = base
        .integer_intervals
        .keys()
        .chain(left.integer_intervals.keys())
        .chain(right.integer_intervals.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    destination.integer_intervals.clear();
    for name in interval_names {
        let mut interval: Option<IntegerInterval> = None;
        let mut unknown = false;
        let mut present = false;
        for environment in [base, left, right] {
            if let Some(candidate) = environment.integer_intervals.get(&name) {
                present = true;
                match candidate {
                    Some(candidate) => {
                        interval = Some(match interval {
                            Some(existing) => IntegerInterval {
                                min: existing.min.min(candidate.min),
                                max: existing.max.max(candidate.max),
                            },
                            None => *candidate,
                        });
                    }
                    None => unknown = true,
                }
            }
        }
        if present {
            destination
                .integer_intervals
                .insert(name, (!unknown).then_some(interval).flatten());
        }
    }
}

fn collection_origins(
    expression: &SimpleExpr,
    environment: &LocalInputEnvironment,
) -> BTreeSet<CollectionOrigin> {
    let SimpleExpr::Identifier(path) = expression else {
        return BTreeSet::new();
    };
    match path.as_str() {
        "host_i32" => [CollectionOrigin::HostI32].into_iter().collect(),
        "host_f32" => [CollectionOrigin::HostF32].into_iter().collect(),
        _ => environment
            .collections
            .get(path)
            .cloned()
            .unwrap_or_default(),
    }
}

fn visit_collection_with_environment(
    collection_path: &str,
    environment: &LocalInputEnvironment,
    usage: &mut HostFrameInputUsage,
) {
    let origins = match collection_path {
        "host_i32" => [CollectionOrigin::HostI32].into_iter().collect(),
        "host_f32" => [CollectionOrigin::HostF32].into_iter().collect(),
        _ => environment
            .collections
            .get(collection_path)
            .cloned()
            .unwrap_or_default(),
    };
    for origin in origins {
        match origin {
            CollectionOrigin::HostI32 => {
                usage.add_i32_range(0, HOST_I32_COUNT, HostFrameInputFamily::RawI32)
            }
            CollectionOrigin::HostF32 => {
                usage.add_f32_range(0, HOST_F32_COUNT, HostFrameInputFamily::RawF32)
            }
        }
    }
}

fn observe_collection_index_with_environment(
    collection_path: &str,
    index: &SimpleExpr,
    constants: &BTreeMap<String, ConstantValue>,
    environment: &LocalInputEnvironment,
    usage: &mut HostFrameInputUsage,
) {
    let origins = match collection_path {
        "host_i32" => [CollectionOrigin::HostI32].into_iter().collect(),
        "host_f32" => [CollectionOrigin::HostF32].into_iter().collect(),
        _ => environment
            .collections
            .get(collection_path)
            .cloned()
            .unwrap_or_default(),
    };
    for origin in origins {
        let values = integer_values(index, constants, environment);
        match origin {
            CollectionOrigin::HostI32 => {
                if let Some(values) = values {
                    for value in values {
                        if let Some(index) = as_index(value) {
                            usage.add_i32(index, classify_i32(index));
                        }
                    }
                } else if let Some(family) =
                    dynamic_family_with_environment("host_i32", index, constants, environment)
                {
                    add_i32_family(usage, family);
                } else {
                    usage.add_i32_range(0, HOST_I32_COUNT, HostFrameInputFamily::RawI32);
                }
            }
            CollectionOrigin::HostF32 => {
                if let Some(values) = values {
                    for value in values {
                        if let Some(index) = as_index(value) {
                            usage.add_f32(index, classify_f32(index));
                        }
                    }
                } else if let Some(family) =
                    dynamic_family_with_environment("host_f32", index, constants, environment)
                {
                    add_f32_family(usage, family);
                } else {
                    usage.add_f32_range(0, HOST_F32_COUNT, HostFrameInputFamily::RawF32);
                }
            }
        }
    }
}

fn integer_values(
    expression: &SimpleExpr,
    constants: &BTreeMap<String, ConstantValue>,
    environment: &LocalInputEnvironment,
) -> Option<BTreeSet<i64>> {
    match expression {
        SimpleExpr::Int(value) => Some([*value].into_iter().collect()),
        SimpleExpr::Identifier(name) => {
            if let Some(values) = environment.integer_values.get(name) {
                return values.clone();
            }
            match constants.get(name) {
                Some(ConstantValue::I32 { value, .. }) => {
                    Some([i64::from(*value)].into_iter().collect())
                }
                _ => None,
            }
        }
        SimpleExpr::Binary { lhs, op, rhs } => {
            let lhs = integer_values(lhs, constants, environment)?;
            let rhs = integer_values(rhs, constants, environment)?;
            let mut result = BTreeSet::new();
            for lhs in lhs {
                for rhs in &rhs {
                    let lhs = i32::try_from(lhs).ok()?;
                    let rhs = i32::try_from(*rhs).ok()?;
                    let value = match *op {
                        '+' => Some(lhs.wrapping_add(rhs)),
                        '-' => Some(lhs.wrapping_sub(rhs)),
                        '*' => Some(lhs.wrapping_mul(rhs)),
                        '/' if rhs != 0 => Some(lhs.wrapping_div(rhs)),
                        '%' if rhs != 0 => Some(lhs.wrapping_rem(rhs)),
                        '/' | '%' => None,
                        _ => None,
                    }?;
                    result.insert(i64::from(value));
                    if result.len() > MAX_EXACT_INTEGER_VALUES {
                        return None;
                    }
                }
            }
            Some(result)
        }
        _ => None,
    }
}

fn integer_interval(
    expression: &SimpleExpr,
    constants: &BTreeMap<String, ConstantValue>,
    environment: &LocalInputEnvironment,
) -> Option<IntegerInterval> {
    if let Some(values) = integer_values(expression, constants, environment) {
        return IntegerInterval::from_values(&values);
    }
    match expression {
        SimpleExpr::Identifier(name) => environment.integer_intervals.get(name).copied().flatten(),
        SimpleExpr::Binary { lhs, op, rhs } => {
            let lhs = integer_interval(lhs, constants, environment)?;
            let rhs = integer_interval(rhs, constants, environment)?;
            let candidates = match op {
                '+' => [
                    lhs.min.checked_add(rhs.min)?,
                    lhs.min.checked_add(rhs.max)?,
                    lhs.max.checked_add(rhs.min)?,
                    lhs.max.checked_add(rhs.max)?,
                ],
                '-' => [
                    lhs.min.checked_sub(rhs.min)?,
                    lhs.min.checked_sub(rhs.max)?,
                    lhs.max.checked_sub(rhs.min)?,
                    lhs.max.checked_sub(rhs.max)?,
                ],
                '*' => [
                    lhs.min.checked_mul(rhs.min)?,
                    lhs.min.checked_mul(rhs.max)?,
                    lhs.max.checked_mul(rhs.min)?,
                    lhs.max.checked_mul(rhs.max)?,
                ],
                _ => return None,
            };
            if candidates
                .iter()
                .any(|value| i32::try_from(*value).is_err())
            {
                return None;
            }
            Some(IntegerInterval {
                min: *candidates.iter().min()?,
                max: *candidates.iter().max()?,
            })
        }
        _ => None,
    }
}

fn dynamic_family_with_environment(
    collection_path: &str,
    expression: &SimpleExpr,
    constants: &BTreeMap<String, ConstantValue>,
    environment: &LocalInputEnvironment,
) -> Option<HostFrameInputFamily> {
    family_for_interval(
        collection_path,
        integer_interval(expression, constants, environment)?,
    )
}

fn family_for_interval(
    collection_path: &str,
    interval: IntegerInterval,
) -> Option<HostFrameInputFamily> {
    let min = usize::try_from(interval.min).ok()?;
    let max = usize::try_from(interval.max).ok()?;
    match collection_path {
        "host_i32" if min >= HOST_I_KEY_BASE && max < HOST_I_KEY_BASE + HOST_I_KEY_COUNT => {
            Some(HostFrameInputFamily::Keyboard)
        }
        "host_i32"
            if min >= HOST_I_POINTER_BASE
                && max < HOST_I_POINTER_BASE + HOST_I_POINTER_STRIDE * HOST_I_POINTER_COUNT =>
        {
            Some(HostFrameInputFamily::Pointer)
        }
        "host_f32"
            if min >= HOST_F_POINTER_BASE
                && max < HOST_F_POINTER_BASE + HOST_F_POINTER_STRIDE * HOST_F_POINTER_COUNT =>
        {
            Some(HostFrameInputFamily::Pointer)
        }
        "host_f32" if min >= 48 && max < 58 => Some(HostFrameInputFamily::Display),
        _ => None,
    }
}

fn add_i32_family(usage: &mut HostFrameInputUsage, family: HostFrameInputFamily) {
    match family {
        HostFrameInputFamily::Keyboard => {
            usage.add_i32_range(HOST_I_KEY_BASE, HOST_I_KEY_BASE + HOST_I_KEY_COUNT, family)
        }
        HostFrameInputFamily::Pointer => usage.add_i32_range(
            HOST_I_POINTER_BASE,
            HOST_I_POINTER_BASE + HOST_I_POINTER_STRIDE * HOST_I_POINTER_COUNT,
            family,
        ),
        HostFrameInputFamily::Display => {
            for index in [11, 12, 13, 22, 23, 24, 25, 30, 31] {
                usage.add_i32(index, family);
            }
        }
        HostFrameInputFamily::RawI32 | HostFrameInputFamily::RawF32 => {
            usage.add_i32_range(0, HOST_I32_COUNT, family)
        }
    }
}

fn add_f32_family(usage: &mut HostFrameInputUsage, family: HostFrameInputFamily) {
    match family {
        HostFrameInputFamily::Pointer => usage.add_f32_range(
            HOST_F_POINTER_BASE,
            HOST_F_POINTER_BASE + HOST_F_POINTER_STRIDE * HOST_F_POINTER_COUNT,
            family,
        ),
        HostFrameInputFamily::Display => usage.add_f32_range(48, 58, family),
        HostFrameInputFamily::Keyboard
        | HostFrameInputFamily::RawI32
        | HostFrameInputFamily::RawF32 => usage.add_f32_range(0, HOST_F32_COUNT, family),
    }
}

fn classify_i32(index: usize) -> HostFrameInputFamily {
    if (HOST_I_KEY_BASE..HOST_I_KEY_BASE + HOST_I_KEY_COUNT).contains(&index) {
        HostFrameInputFamily::Keyboard
    } else if (HOST_I_POINTER_BASE
        ..HOST_I_POINTER_BASE + HOST_I_POINTER_STRIDE * HOST_I_POINTER_COUNT)
        .contains(&index)
    {
        HostFrameInputFamily::Pointer
    } else if matches!(index, 11 | 12 | 13 | 22 | 23 | 24 | 25 | 30 | 31) {
        HostFrameInputFamily::Display
    } else {
        HostFrameInputFamily::RawI32
    }
}

fn classify_f32(index: usize) -> HostFrameInputFamily {
    if (HOST_F_POINTER_BASE..HOST_F_POINTER_BASE + HOST_F_POINTER_STRIDE * HOST_F_POINTER_COUNT)
        .contains(&index)
    {
        HostFrameInputFamily::Pointer
    } else if (48..58).contains(&index) {
        HostFrameInputFamily::Display
    } else {
        HostFrameInputFamily::RawF32
    }
}

fn as_index(value: i64) -> Option<usize> {
    usize::try_from(value).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::reachability::compute_reachable_function_ids;
    use crate::compiler::Compiler;

    fn usage_for(source: &str) -> HostFrameInputUsage {
        let mut compiler = Compiler::new();
        compiler.upsert_file("main.stasis", source);
        compiler.check().expect("input usage fixture compiles");
        let hirs = compiler
            .analysis_hirs(&[])
            .expect("reachable input usage HIR");
        let reachable = compute_reachable_function_ids(compiler.functions(), &[]);
        analyze_host_frame_input_usage_with_functions(
            &hirs,
            &reachable,
            &BTreeMap::new(),
            compiler.functions(),
        )
    }

    #[test]
    fn keyboard_only_game_omits_mouse_lanes() {
        let usage = usage_for(
            "global host_i32: i32[768]; global host_f32: f32[64];\n\
             function tick(): void { host_i32[32] = host_i32[32]; }\n\
             function render(): void { return; }\n",
        );
        assert_eq!(
            usage
                .i32_fields()
                .iter()
                .map(|field| field.index)
                .collect::<Vec<_>>(),
            [32]
        );
        assert!(usage.f32_fields().is_empty());
    }

    #[test]
    fn plain_host_write_is_not_a_read_but_compound_write_is() {
        let usage = usage_for(
            "global host_i32: i32[768]; global host_f32: f32[64];\n\\
             function tick(): void { host_i32[32] = 1; host_i32[33] += 1; return; }\n\\
             function render(): void { return; }\n",
        );
        assert_eq!(
            usage
                .i32_fields()
                .iter()
                .map(|field| field.index)
                .collect::<Vec<_>>(),
            [33]
        );
        assert!(usage.f32_fields().is_empty());
    }

    #[test]
    fn called_wrapper_is_included_but_uncalled_helper_is_not() {
        let usage = usage_for(
            "global host_i32: i32[768]; global host_f32: f32[64];\n\
             function called_wrapper(): i32 { return host_i32[7]; }\n\
             function unused_helper(): f32 { return host_f32[0]; }\n\
             function tick(): i32 { return called_wrapper(); }\n\
             function render(): void { return; }\n",
        );
        assert!(usage.i32_fields().iter().any(|field| field.index == 7));
        assert!(usage.f32_fields().is_empty());
    }

    #[test]
    fn render_reads_are_included() {
        let usage = usage_for(
            "global host_i32: i32[768]; global host_f32: f32[64];\n\
             function tick(): void { return; }\n\
             function render(): void { let value: f32 = host_f32[50]; return; }\n",
        );
        assert_eq!(
            usage
                .f32_fields()
                .iter()
                .map(|field| field.index)
                .collect::<Vec<_>>(),
            [50]
        );
        assert_eq!(usage.f32_fields()[0].family, HostFrameInputFamily::Display);
    }

    #[test]
    fn local_collection_alias_preserves_exact_host_slot() {
        let usage = usage_for(
            "global host_i32: i32[768]; global host_f32: f32[64];\n\\
             function tick(): i32 { let values: i32[] = host_i32; return values[32]; }\n\\
             function render(): void { return; }\n",
        );
        assert_eq!(
            usage
                .i32_fields()
                .iter()
                .map(|field| field.index)
                .collect::<Vec<_>>(),
            [32]
        );
        assert!(usage.f32_fields().is_empty());
    }

    #[test]
    fn helper_collection_and_index_arguments_preserve_exact_host_slot() {
        let usage = usage_for(
            "global host_i32: i32[768]; global host_f32: f32[64];\n\\
             function read_at(values: i32[], index: i32): i32 { return values[index]; }\n\\
             function tick(): i32 { return read_at(host_i32, 32); }\n\\
             function render(): void { return; }\n",
        );
        assert_eq!(
            usage
                .i32_fields()
                .iter()
                .map(|field| field.index)
                .collect::<Vec<_>>(),
            [32]
        );
        assert!(usage.f32_fields().is_empty());
    }

    #[test]
    fn helper_dynamic_index_conservatively_includes_raw_lane() {
        let usage = usage_for(
            "global host_i32: i32[768]; global host_f32: f32[64];\n\\
             function read_at(values: i32[], index: i32): i32 { return values[index]; }\n\\
             function tick(): i32 { let index: i32 = host_i32[0]; return read_at(host_i32, index); }\n\\
             function render(): void { return; }\n",
        );
        assert_eq!(
            usage
                .i32_fields()
                .iter()
                .map(|field| field.index)
                .collect::<Vec<_>>(),
            (0..HOST_I32_COUNT).collect::<Vec<_>>()
        );
    }

    #[test]
    fn recursive_helper_index_provenance_widens_after_bounded_values() {
        let usage = usage_for(
            "global host_i32: i32[768]; global host_f32: f32[64];\n\
             function read_depth(values: i32[], depth: i32): i32 {\n\
                 if (depth > 0) { return read_depth(values, depth - 1); }\n\
                 return values[depth];\n\
             }\n\
             function tick(): i32 { return read_depth(host_i32, 64); }\n\
             function render(): void { return; }\n",
        );
        assert_eq!(usage.i32_fields().len(), HOST_I32_COUNT);
        assert_eq!(
            usage
                .i32_fields()
                .iter()
                .map(|field| field.index)
                .collect::<Vec<_>>(),
            (0..HOST_I32_COUNT).collect::<Vec<_>>()
        );
    }

    #[test]
    fn local_constant_index_preserves_exact_slot() {
        let usage = usage_for(
            "global host_i32: i32[768]; global host_f32: f32[64];\n\
             function tick(): i32 { let index: i32 = 3; return host_i32[index]; }\n\
             function render(): void { return; }\n",
        );
        assert_eq!(
            usage
                .i32_fields()
                .iter()
                .map(|field| field.index)
                .collect::<Vec<_>>(),
            [3]
        );
    }

    #[test]
    fn compound_local_assignment_invalidates_exact_index_provenance() {
        let usage = usage_for(
            "global host_i32: i32[768]; global host_f32: f32[64];\n\\
             function tick(): i32 { let index: i32 = 32; index += 1; return host_i32[index]; }\n\\
             function render(): void { return; }\n",
        );
        assert_eq!(usage.i32_fields().len(), HOST_I32_COUNT);
        assert!(usage
            .i32_fields()
            .iter()
            .all(|field| field.family == HostFrameInputFamily::RawI32));
    }

    #[test]
    fn unbounded_index_arithmetic_falls_back_to_the_full_raw_lane() {
        for expression in ["index * 4", "32 - index", "32 + index"] {
            let usage = usage_for(&format!(
                "global host_i32: i32[768];\n\
                 function read(index: i32): i32 {{ return host_i32[{expression}]; }}\n\
                 function tick(): i32 {{ return read(host_i32[0]); }}\n\
                 function render(): void {{ return; }}\n"
            ));
            assert_eq!(
                usage
                    .i32_fields()
                    .iter()
                    .map(|field| field.index)
                    .collect::<Vec<_>>(),
                (0..HOST_I32_COUNT).collect::<Vec<_>>(),
                "{expression} must not be narrowed without a range proof"
            );
        }
    }

    #[test]
    fn wrapped_i32_index_arithmetic_preserves_runtime_slot() {
        let usage = usage_for(
            "global host_i32: i32[768];\n\
             function tick(): i32 { \
                 let left: i32 = 2147483647; \
                 let right: i32 = 2147483647; \
                 let two: i32 = 2; \
                 return host_i32[left + right + two]; \
             }\n\
             function render(): void { return; }\n",
        );
        assert_eq!(
            usage
                .i32_fields()
                .iter()
                .map(|field| field.index)
                .collect::<Vec<_>>(),
            [0]
        );
    }

    #[test]
    fn bulk_memory_externs_observe_direct_and_wrapped_host_sources() {
        let direct = usage_for(
            "global host_i32: i32[768]; global copied: i32[8];\n\
             extern function @internal @effects(memory) sys_memcpy_i32(dst: i32[], dst_index: i32, src: i32[], src_index: i32, count: i32): void;\n\
             function tick(): i32 { sys_memcpy_i32(copied, 0, host_i32, 32, 2); return copied[0]; }\n\
             function render(): void { return; }\n",
        );
        assert_eq!(
            direct
                .i32_fields()
                .iter()
                .map(|field| field.index)
                .collect::<Vec<_>>(),
            [32, 33]
        );

        let wrapped = usage_for(
            "global host_f32: f32[64]; global copied: f32[8];\n\
             extern function @internal @effects(memory) sys_memmove_f32(dst: f32[], dst_index: i32, src: f32[], src_index: i32, count: i32): void;\n\
             function copy(values: f32[], start: i32, count: i32): void { sys_memmove_f32(copied, 0, values, start, count); }\n\
             function tick(): i32 { copy(host_f32, host_i32_unknown(), 2); return 0; }\n\
             extern function host_i32_unknown(): i32;\n\
             function render(): void { return; }\n",
        );
        assert_eq!(
            wrapped
                .f32_fields()
                .iter()
                .map(|field| field.index)
                .collect::<Vec<_>>(),
            (0..HOST_F32_COUNT).collect::<Vec<_>>()
        );
    }

    #[test]
    fn repeated_loop_index_uses_the_conservative_input_family() {
        let usage = usage_for(
            "global host_i32: i32[768]; global keys: i32[512];\n\
             function copy_keys(): void { \
                 for (let index: i32 = 0; index < 512; index += 1) { \
                     keys[index] = host_i32[32 + index]; \
                 } \
             } \
             function tick(): i32 { copy_keys(); return 0; }\n\
             function render(): void { return; }\n",
        );
        assert_eq!(usage.i32_fields().len(), HOST_I_KEY_COUNT);
        assert_eq!(
            usage.i32_fields().first().map(|field| field.index),
            Some(32)
        );
        assert_eq!(
            usage.i32_fields().last().map(|field| field.index),
            Some(543)
        );
        assert!(usage
            .i32_fields()
            .iter()
            .all(|field| field.family == HostFrameInputFamily::Keyboard));
    }
}
