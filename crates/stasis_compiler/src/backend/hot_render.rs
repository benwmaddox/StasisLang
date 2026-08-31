//! Compiler-owned hot-render metadata. The compiler publishes deterministic
//! policy facts; rasterization, texture allocation, and fallback stay runtime-owned.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::backend::compile_analysis::{ConstantValue, ConstantValueMap};
use crate::compiler::{FunctionId, FunctionMeta};
use crate::ir::hir::{eval_const_i64, AssignOp, AssignTarget, FunctionHIR, SimpleExpr, SimpleStmt};

pub const HOT_RENDER_METADATA_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HotRenderImageMetadata {
    pub identity: String,
    pub logical_path: String,
    pub logical_width: u32,
    pub logical_height: u32,
    pub sheet_columns: Option<u32>,
    pub sheet_rows: Option<u32>,
    pub cell_width: Option<u32>,
    pub cell_height: Option<u32>,
    pub max_renders_per_render: Option<u64>,
    pub unknown_cause: Option<String>,
    pub atlas_eligible: bool,
    pub grouping_key: String,
    pub backend_constraints: String,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bound {
    Finite(u64),
    Unknown,
}

impl Bound {
    fn add(self, other: Self) -> Self {
        match (self, other) {
            (Self::Finite(a), Self::Finite(b)) => {
                a.checked_add(b).map_or(Self::Unknown, Self::Finite)
            }
            _ => Self::Unknown,
        }
    }

    fn mul(self, other: u64) -> Self {
        match self {
            Self::Finite(value) => value.checked_mul(other).map_or(Self::Unknown, Self::Finite),
            Self::Unknown => Self::Unknown,
        }
    }

    fn branch(self, other: Self) -> Self {
        match (self, other) {
            (Self::Finite(a), Self::Finite(b)) => Self::Finite(a.max(b)),
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone)]
struct ImageDecl {
    identity: String,
    path: String,
    width: u32,
    height: u32,
    conflicting_load: bool,
    invalid_load_cause: Option<String>,
    sheet_geometry: Option<(u32, u32, u32, u32)>,
}

pub(crate) fn analyze_hot_render_images(
    functions: &[FunctionMeta],
    hirs: &BTreeMap<FunctionId, FunctionHIR>,
    reachable: &BTreeSet<FunctionId>,
    collection_capacities: &BTreeMap<String, i32>,
    constants: &ConstantValueMap,
) -> Vec<HotRenderImageMetadata> {
    let mut images = BTreeMap::<String, ImageDecl>::new();
    let mut loader_uncertainty = None;
    for function in functions.iter().filter(|f| reachable.contains(&f.id)) {
        if let Some(hir) = hirs.get(&function.id) {
            let loader_env = function
                .param_names
                .iter()
                .map(|name| (name.clone(), None))
                .collect();
            collect_image_decls(
                &hir.statements,
                constants,
                &loader_env,
                &mut images,
                &mut loader_uncertainty,
            );
        }
    }
    let all_images = images.keys().cloned().collect::<BTreeSet<_>>();

    let by_name = functions.iter().filter(|f| reachable.contains(&f.id)).fold(
        BTreeMap::<(&str, usize), Vec<&FunctionMeta>>::new(),
        |mut map, function| {
            map.entry((&function.name, function.params.len()))
                .or_default()
                .push(function);
            map
        },
    );
    let mut counts = BTreeMap::new();
    let mut causes = BTreeMap::new();
    let render_roots = functions
        .iter()
        .filter(|function| function.name == "render" && reachable.contains(&function.id));
    for root in render_roots {
        let mut stack = BTreeSet::new();
        let env = BTreeMap::new();
        let root_counts = analyze_function(
            root,
            hirs,
            &by_name,
            collection_capacities,
            &env,
            &all_images,
            &mut stack,
            &mut causes,
        );
        add_counts(&mut counts, root_counts);
    }

    if let Some(cause) = loader_uncertainty {
        poison_all(&mut counts, &all_images, &mut causes, &cause);
    }

    let mut records = images
        .into_values()
        .map(|image| {
            let mut bound = counts
                .get(&image.identity)
                .copied()
                .unwrap_or(Bound::Finite(0));
            if image.conflicting_load {
                bound = Bound::Unknown;
                causes.insert(
                    image.identity.clone(),
                    "receiver has multiple static load identities".to_string(),
                );
            }
            if let Some(load_cause) = image.invalid_load_cause.as_ref() {
                bound = Bound::Unknown;
                causes.insert(image.identity.clone(), load_cause.clone());
            }
            let maximum = match bound {
                Bound::Finite(value) => Some(value),
                Bound::Unknown => None,
            };
            let cause = (bound == Bound::Unknown).then(|| {
                causes
                    .get(&image.identity)
                    .cloned()
                    .unwrap_or_else(|| "unprovable render flow".to_string())
            });
            let eligible = false;
            let reason = match maximum {
                Some(0) => "not rendered from render()".to_string(),
                Some(1) => "maximum is one; standalone by default".to_string(),
                Some(_) => "finite hot-render candidate; profitability pending".to_string(),
                None => format!(
                    "unknown: {}",
                    cause.as_deref().unwrap_or("unprovable render flow")
                ),
            };
            HotRenderImageMetadata {
                identity: image.identity,
                logical_path: image.path,
                logical_width: image.width,
                logical_height: image.height,
                sheet_columns: image.sheet_geometry.map(|geometry| geometry.0),
                sheet_rows: image.sheet_geometry.map(|geometry| geometry.1),
                cell_width: image.sheet_geometry.map(|geometry| geometry.2),
                cell_height: image.sheet_geometry.map(|geometry| geometry.3),
                max_renders_per_render: maximum,
                unknown_cause: cause,
                atlas_eligible: eligible,
                grouping_key: "rgba8-premul:linear-filter".to_string(),
                backend_constraints: "rgba8-premultiplied;linear-filter;runtime-page-limits"
                    .to_string(),
                reason,
            }
        })
        .collect::<Vec<_>>();
    apply_profitability_policy(&mut records);
    records
}

fn apply_profitability_policy(records: &mut [HotRenderImageMetadata]) {
    const MAX_LOGICAL_EXTENT: u32 = 4096;
    const MIN_GROUP_REUSE: u64 = 8;
    const MIN_GROUP_LOGICAL_PIXELS: u64 = 65_536;

    let mut groups = BTreeMap::<(String, u32, u32), Vec<usize>>::new();
    for (index, image) in records.iter().enumerate() {
        if image.max_renders_per_render.is_some_and(|count| count > 1)
            && image.logical_width > 0
            && image.logical_height > 0
            && image.logical_width <= MAX_LOGICAL_EXTENT
            && image.logical_height <= MAX_LOGICAL_EXTENT
        {
            groups
                .entry((
                    image.grouping_key.clone(),
                    image.logical_width,
                    image.logical_height,
                ))
                .or_default()
                .push(index);
        }
    }

    for indices in groups.values() {
        let reuse = indices.iter().try_fold(0_u64, |total, index| {
            total.checked_add(
                records[*index]
                    .max_renders_per_render
                    .unwrap_or(0)
                    .saturating_sub(1),
            )
        });
        let logical_pixels = indices.iter().try_fold(0_u64, |total, index| {
            let image = &records[*index];
            total.checked_add(u64::from(image.logical_width) * u64::from(image.logical_height))
        });
        let eligible = indices.len() >= 2
            && reuse.is_some_and(|value| value >= MIN_GROUP_REUSE)
            && logical_pixels.is_some_and(|value| value >= MIN_GROUP_LOGICAL_PIXELS);
        for index in indices {
            let image = &mut records[*index];
            image.atlas_eligible = eligible;
            image.reason = if indices.len() < 2 {
                "single hot image does not amortize an atlas page".to_string()
            } else if reuse.is_none() || logical_pixels.is_none() {
                "profitability arithmetic overflow".to_string()
            } else if reuse.unwrap_or(0) < MIN_GROUP_REUSE {
                "compatible group has insufficient aggregate reuse".to_string()
            } else if logical_pixels.unwrap_or(0) < MIN_GROUP_LOGICAL_PIXELS {
                "compatible group has insufficient logical occupancy".to_string()
            } else {
                "eligible candidate; runtime rechecks realized dimensions and device limits"
                    .to_string()
            };
        }
    }

    for image in records.iter_mut().filter(|image| {
        image.max_renders_per_render.is_some_and(|count| count > 1)
            && (image.logical_width > MAX_LOGICAL_EXTENT
                || image.logical_height > MAX_LOGICAL_EXTENT)
    }) {
        image.reason = "logical dimensions exceed conservative atlas policy".to_string();
    }
}

fn collect_image_decls(
    statements: &[SimpleStmt],
    constants: &ConstantValueMap,
    env: &BTreeMap<String, Option<String>>,
    out: &mut BTreeMap<String, ImageDecl>,
    loader_uncertainty: &mut Option<String>,
) {
    visit_calls(statements, &mut |target, args| {
        let loader = target.rsplit('.').next().unwrap_or(target);
        if !matches!(loader, "load_sprite_from" | "load_sprite_sheet_from") {
            return;
        }
        let Some(receiver) = args.first() else {
            *loader_uncertainty = Some(format!("malformed {loader} call"));
            return;
        };
        let Some(identity) = stable_identity(receiver, env) else {
            *loader_uncertainty = Some(format!("dynamic receiver in {loader}"));
            return;
        };
        let path = args.get(1).and_then(|expr| const_string(expr, constants));
        let (dimensions, sheet_geometry) = match loader {
            "load_sprite_from" if args.len() == 4 => args
                .get(2)
                .and_then(|expr| const_i64(expr, constants))
                .zip(args.get(3).and_then(|expr| const_i64(expr, constants)))
                .map_or((None, None), |dimensions| (Some(dimensions), None)),
            "load_sprite_sheet_from" if args.len() == 6 => {
                let geometry = args
                    .get(2)
                    .and_then(|expr| const_u32(expr, constants))
                    .zip(args.get(3).and_then(|expr| const_u32(expr, constants)))
                    .zip(args.get(4).and_then(|expr| const_u32(expr, constants)))
                    .zip(args.get(5).and_then(|expr| const_u32(expr, constants)))
                    .map(|(((columns, rows), cell_width), cell_height)| {
                        (columns, rows, cell_width, cell_height)
                    });
                let dimensions = geometry.and_then(|(columns, rows, cell_width, cell_height)| {
                    Some((
                        i64::from(columns.checked_mul(cell_width)?),
                        i64::from(rows.checked_mul(cell_height)?),
                    ))
                });
                (dimensions, geometry)
            }
            _ => (None, None),
        };
        let dimensions = dimensions.and_then(|(width, height)| {
            Some((u32::try_from(width).ok()?, u32::try_from(height).ok()?))
        });
        let invalid_load_cause = match (&path, dimensions) {
            (None, _) => Some(format!("dynamic or unprovable asset path in {loader}")),
            (_, None) => Some(format!(
                "invalid or overflowing raster dimensions in {loader}"
            )),
            (_, Some((0, _)) | Some((_, 0))) => Some(format!("zero raster dimension in {loader}")),
            _ => None,
        };
        let path = path.unwrap_or_else(|| "<dynamic>".to_string());
        let (width, height) = dimensions.unwrap_or((0, 0));
        match out.entry(identity.clone()) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(ImageDecl {
                    identity,
                    path: path.clone(),
                    width,
                    height,
                    conflicting_load: false,
                    invalid_load_cause,
                    sheet_geometry,
                });
            }
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                let previous = entry.get_mut();
                previous.conflicting_load |=
                    previous.path != *path || previous.width != width || previous.height != height;
                if previous.invalid_load_cause.is_none() {
                    previous.invalid_load_cause = invalid_load_cause;
                }
                if previous.sheet_geometry != sheet_geometry {
                    previous.conflicting_load = true;
                }
            }
        }
    });
}

fn const_u32(expr: &SimpleExpr, constants: &ConstantValueMap) -> Option<u32> {
    u32::try_from(const_i64(expr, constants)?).ok()
}

fn const_i64(expr: &SimpleExpr, constants: &ConstantValueMap) -> Option<i64> {
    eval_const_i64(expr).or_else(|| match expr {
        SimpleExpr::Identifier(name) => match constants.get(name) {
            Some(ConstantValue::I32 { value, .. }) => Some(i64::from(*value)),
            _ => None,
        },
        _ => None,
    })
}

fn const_string(expr: &SimpleExpr, constants: &ConstantValueMap) -> Option<String> {
    match expr {
        SimpleExpr::StringLiteral(value) => Some(value.clone()),
        SimpleExpr::Identifier(name) => match constants.get(name) {
            Some(ConstantValue::String { value, .. }) => Some(value.clone()),
            _ => None,
        },
        _ => None,
    }
}

fn analyze_function(
    function: &FunctionMeta,
    hirs: &BTreeMap<FunctionId, FunctionHIR>,
    by_name: &BTreeMap<(&str, usize), Vec<&FunctionMeta>>,
    capacities: &BTreeMap<String, i32>,
    env: &BTreeMap<String, Option<String>>,
    all_images: &BTreeSet<String>,
    stack: &mut BTreeSet<FunctionId>,
    causes: &mut BTreeMap<String, String>,
) -> BTreeMap<String, Bound> {
    if !stack.insert(function.id) {
        let mut unknown = BTreeMap::new();
        for identity in all_images {
            unknown.insert(identity.clone(), Bound::Unknown);
            causes.insert(
                identity.clone(),
                format!("recursive call through {}", function.name),
            );
        }
        return unknown;
    }
    let result = hirs.get(&function.id).map_or_else(BTreeMap::new, |hir| {
        analyze_statements(
            &hir.statements,
            hirs,
            by_name,
            capacities,
            env,
            all_images,
            stack,
            causes,
        )
    });
    stack.remove(&function.id);
    result
}

fn analyze_statements(
    statements: &[SimpleStmt],
    hirs: &BTreeMap<FunctionId, FunctionHIR>,
    by_name: &BTreeMap<(&str, usize), Vec<&FunctionMeta>>,
    capacities: &BTreeMap<String, i32>,
    env: &BTreeMap<String, Option<String>>,
    all_images: &BTreeSet<String>,
    stack: &mut BTreeSet<FunctionId>,
    causes: &mut BTreeMap<String, String>,
) -> BTreeMap<String, Bound> {
    let mut out = BTreeMap::new();
    for statement in statements {
        match statement {
            SimpleStmt::If {
                condition,
                then_statements,
                else_statements,
                ..
            } => {
                visit_condition_exprs(condition, &mut |target, args| {
                    analyze_call(
                        target, args, &mut out, hirs, by_name, capacities, env, all_images, stack,
                        causes,
                    )
                });
                let then_counts = analyze_statements(
                    then_statements,
                    hirs,
                    by_name,
                    capacities,
                    env,
                    all_images,
                    stack,
                    causes,
                );
                let else_counts = else_statements
                    .as_deref()
                    .map_or_else(BTreeMap::new, |body| {
                        analyze_statements(
                            body, hirs, by_name, capacities, env, all_images, stack, causes,
                        )
                    });
                add_counts(&mut out, branch_counts(then_counts, else_counts));
            }
            SimpleStmt::For {
                init,
                condition,
                step,
                body_statements,
            } => {
                let mut init_counts = BTreeMap::new();
                visit_stmt_exprs(init, &mut |target, args| {
                    analyze_call(
                        target,
                        args,
                        &mut init_counts,
                        hirs,
                        by_name,
                        capacities,
                        env,
                        all_images,
                        stack,
                        causes,
                    )
                });
                add_counts(&mut out, init_counts);
                let mut condition_counts = BTreeMap::new();
                visit_condition_exprs(condition, &mut |target, args| {
                    analyze_call(
                        target,
                        args,
                        &mut condition_counts,
                        hirs,
                        by_name,
                        capacities,
                        env,
                        all_images,
                        stack,
                        causes,
                    )
                });
                let mut step_counts = BTreeMap::new();
                visit_stmt_exprs(step, &mut |target, args| {
                    analyze_call(
                        target,
                        args,
                        &mut step_counts,
                        hirs,
                        by_name,
                        capacities,
                        env,
                        all_images,
                        stack,
                        causes,
                    )
                });
                let body = analyze_statements(
                    body_statements,
                    hirs,
                    by_name,
                    capacities,
                    env,
                    all_images,
                    stack,
                    causes,
                );
                match fixed_for_iterations(init, condition, step) {
                    LoopBound::Finite(iterations) => {
                        add_counts(&mut out, multiply_counts(body, iterations));
                        add_counts(&mut out, multiply_counts(step_counts, iterations));
                        match iterations.checked_add(1) {
                            Some(executions) => {
                                add_counts(&mut out, multiply_counts(condition_counts, executions))
                            }
                            None => mark_unknown(
                                &mut out,
                                condition_counts,
                                causes,
                                "for-loop condition execution count overflow",
                            ),
                        }
                    }
                    LoopBound::Dynamic => {
                        add_counts(&mut condition_counts, step_counts);
                        add_counts(&mut condition_counts, body);
                        mark_unknown(
                            &mut out,
                            condition_counts,
                            causes,
                            "unbounded or dynamic for loop",
                        );
                    }
                    LoopBound::Overflow => {
                        add_counts(&mut condition_counts, step_counts);
                        add_counts(&mut condition_counts, body);
                        mark_unknown(
                            &mut out,
                            condition_counts,
                            causes,
                            "for-loop bound arithmetic overflow",
                        );
                    }
                }
            }
            SimpleStmt::Foreach {
                collection_path,
                body_statements,
                ..
            } => {
                let body = analyze_statements(
                    body_statements,
                    hirs,
                    by_name,
                    capacities,
                    env,
                    all_images,
                    stack,
                    causes,
                );
                match capacities
                    .get(collection_path)
                    .and_then(|v| u64::try_from(*v).ok())
                {
                    Some(capacity) => add_counts(&mut out, multiply_counts(body, capacity)),
                    None => mark_unknown(&mut out, body, causes, "unknown collection capacity"),
                }
            }
            _ => visit_stmt_exprs(statement, &mut |target, args| {
                analyze_call(
                    target, args, &mut out, hirs, by_name, capacities, env, all_images, stack,
                    causes,
                )
            }),
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn analyze_call(
    target: &str,
    args: &[SimpleExpr],
    out: &mut BTreeMap<String, Bound>,
    hirs: &BTreeMap<FunctionId, FunctionHIR>,
    by_name: &BTreeMap<(&str, usize), Vec<&FunctionMeta>>,
    capacities: &BTreeMap<String, i32>,
    env: &BTreeMap<String, Option<String>>,
    all_images: &BTreeSet<String>,
    stack: &mut BTreeSet<FunctionId>,
    causes: &mut BTreeMap<String, String>,
) {
    let leaf_target = target.rsplit('.').next().unwrap_or(target);
    if matches!(leaf_target, "draw" | "draw_frame" | "draw_frame_scaled") {
        if let Some(identity) = args.first().and_then(|arg| stable_identity(arg, env)) {
            add_one(out, identity);
        } else {
            poison_all(
                out,
                all_images,
                causes,
                &format!("dynamic render receiver in call to {leaf_target}"),
            );
        }
        return;
    }
    let Some(candidates) = by_name.get(&(leaf_target, args.len())) else {
        return;
    };
    if candidates.len() != 1 {
        poison_all(
            out,
            all_images,
            causes,
            &format!("ambiguous render-reachable call target {leaf_target}"),
        );
        return;
    }
    let callee = candidates[0];
    let callee_env = callee
        .param_names
        .iter()
        .zip(args)
        .map(|(name, arg)| (name.clone(), stable_identity(arg, env)))
        .collect();
    add_counts(
        out,
        analyze_function(
            callee,
            hirs,
            by_name,
            capacities,
            &callee_env,
            all_images,
            stack,
            causes,
        ),
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoopBound {
    Finite(u64),
    Dynamic,
    Overflow,
}

fn fixed_for_iterations(
    init: &SimpleStmt,
    condition: &crate::ir::hir::SimpleCondition,
    step: &SimpleStmt,
) -> LoopBound {
    use crate::ir::hir::{ComparisonOp, SimpleCondition};
    let (name, start) = match init {
        SimpleStmt::Let {
            name, expression, ..
        } => match eval_const_i64(expression) {
            Some(value) => (name, value),
            None => return LoopBound::Dynamic,
        },
        _ => return LoopBound::Dynamic,
    };
    let end = match condition {
        SimpleCondition::Comparison {
            lhs: SimpleExpr::Identifier(lhs),
            op: ComparisonOp::Lt,
            rhs,
        } if lhs == name => match eval_const_i64(rhs) {
            Some(value) => value,
            None => return LoopBound::Dynamic,
        },
        SimpleCondition::Comparison {
            lhs: SimpleExpr::Identifier(lhs),
            op: ComparisonOp::Le,
            rhs,
        } if lhs == name => match eval_const_i64(rhs) {
            Some(value) => match value.checked_add(1) {
                Some(end) => end,
                None => return LoopBound::Overflow,
            },
            None => return LoopBound::Dynamic,
        },
        _ => return LoopBound::Dynamic,
    };
    let valid_step = matches!(step, SimpleStmt::Assign { target: AssignTarget::Local(step_name), op: AssignOp::Add, expression, } if step_name == name && eval_const_i64(expression) == Some(1));
    if !valid_step {
        return LoopBound::Dynamic;
    }
    match end.checked_sub(start) {
        Some(value) => LoopBound::Finite(u64::try_from(value.max(0)).unwrap_or(0)),
        None => LoopBound::Overflow,
    }
}

fn stable_identity(expr: &SimpleExpr, env: &BTreeMap<String, Option<String>>) -> Option<String> {
    match expr {
        SimpleExpr::Identifier(path) => {
            env.get(path).cloned().unwrap_or_else(|| Some(path.clone()))
        }
        SimpleExpr::IndexedPath {
            collection_path,
            index,
            suffix,
        } => {
            let index = eval_const_i64(index)?;
            Some(if suffix.is_empty() {
                format!("{collection_path}[{index}]")
            } else {
                format!("{collection_path}[{index}].{suffix}")
            })
        }
        _ => None,
    }
}

fn add_one(out: &mut BTreeMap<String, Bound>, identity: String) {
    let next = out
        .remove(&identity)
        .unwrap_or(Bound::Finite(0))
        .add(Bound::Finite(1));
    out.insert(identity, next);
}

fn add_counts(out: &mut BTreeMap<String, Bound>, other: BTreeMap<String, Bound>) {
    for (key, value) in other {
        let next = out.remove(&key).unwrap_or(Bound::Finite(0)).add(value);
        out.insert(key, next);
    }
}

fn branch_counts(
    left: BTreeMap<String, Bound>,
    right: BTreeMap<String, Bound>,
) -> BTreeMap<String, Bound> {
    let keys = left
        .keys()
        .chain(right.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    keys.into_iter()
        .map(|key| {
            let value = left
                .get(&key)
                .copied()
                .unwrap_or(Bound::Finite(0))
                .branch(right.get(&key).copied().unwrap_or(Bound::Finite(0)));
            (key, value)
        })
        .collect()
}

fn multiply_counts(counts: BTreeMap<String, Bound>, factor: u64) -> BTreeMap<String, Bound> {
    counts
        .into_iter()
        .map(|(key, value)| (key, value.mul(factor)))
        .collect()
}

fn mark_unknown(
    out: &mut BTreeMap<String, Bound>,
    body: BTreeMap<String, Bound>,
    causes: &mut BTreeMap<String, String>,
    cause: &str,
) {
    for key in body.keys() {
        causes
            .entry(key.clone())
            .or_insert_with(|| cause.to_string());
    }
    add_counts(
        out,
        body.into_keys().map(|key| (key, Bound::Unknown)).collect(),
    );
}

fn poison_all(
    out: &mut BTreeMap<String, Bound>,
    all_images: &BTreeSet<String>,
    causes: &mut BTreeMap<String, String>,
    cause: &str,
) {
    for identity in all_images {
        out.insert(identity.clone(), Bound::Unknown);
        causes
            .entry(identity.clone())
            .or_insert_with(|| cause.to_string());
    }
}

fn visit_calls(statements: &[SimpleStmt], callback: &mut impl FnMut(&str, &[SimpleExpr])) {
    for statement in statements {
        visit_stmt_exprs(statement, callback);
        if let SimpleStmt::If {
            then_statements,
            else_statements,
            ..
        } = statement
        {
            visit_calls(then_statements, callback);
            if let Some(body) = else_statements {
                visit_calls(body, callback);
            }
        } else if let SimpleStmt::For {
            body_statements, ..
        }
        | SimpleStmt::Foreach {
            body_statements, ..
        } = statement
        {
            visit_calls(body_statements, callback);
        }
    }
}

fn visit_stmt_exprs(statement: &SimpleStmt, callback: &mut impl FnMut(&str, &[SimpleExpr])) {
    match statement {
        SimpleStmt::Let { expression, .. }
        | SimpleStmt::Assign { expression, .. }
        | SimpleStmt::Expr(expression)
        | SimpleStmt::Return(expression) => visit_expr(expression, callback),
        SimpleStmt::Convert { source, .. } => visit_expr(source, callback),
        _ => {}
    }
}

fn visit_condition_exprs(
    condition: &crate::ir::hir::SimpleCondition,
    callback: &mut impl FnMut(&str, &[SimpleExpr]),
) {
    use crate::ir::hir::SimpleCondition;
    match condition {
        SimpleCondition::Comparison { lhs, rhs, .. } => {
            visit_expr(lhs, callback);
            visit_expr(rhs, callback);
        }
        SimpleCondition::Expr(expression) => visit_expr(expression, callback),
        SimpleCondition::And(left, right) | SimpleCondition::Or(left, right) => {
            visit_condition_exprs(left, callback);
            visit_condition_exprs(right, callback);
        }
        SimpleCondition::Not(inner) => visit_condition_exprs(inner, callback),
    }
}

fn visit_expr(expr: &SimpleExpr, callback: &mut impl FnMut(&str, &[SimpleExpr])) {
    match expr {
        SimpleExpr::Call { target, args } => {
            callback(target, args);
            for arg in args {
                visit_expr(arg, callback);
            }
        }
        SimpleExpr::Binary { lhs, rhs, .. } => {
            visit_expr(lhs, callback);
            visit_expr(rhs, callback);
        }
        SimpleExpr::IndexedPath { index, .. } => visit_expr(index, callback),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{CanonicalSourcePath, SymbolId};
    use crate::ir::hir::{ComparisonOp, SimpleCondition};

    fn fixed_loop(start: i64, end: i64, inclusive: bool) -> LoopBound {
        let init = SimpleStmt::Let {
            name: "i".to_string(),
            type_id: None,
            expression: SimpleExpr::Int(start),
        };
        let condition = SimpleCondition::Comparison {
            lhs: SimpleExpr::Identifier("i".to_string()),
            op: if inclusive {
                ComparisonOp::Le
            } else {
                ComparisonOp::Lt
            },
            rhs: SimpleExpr::Int(end),
        };
        let step = SimpleStmt::Assign {
            target: AssignTarget::Local("i".to_string()),
            op: AssignOp::Add,
            expression: SimpleExpr::Int(1),
        };
        fixed_for_iterations(&init, &condition, &step)
    }

    #[test]
    fn checked_bound_arithmetic_never_wraps() {
        assert_eq!(
            Bound::Finite(u64::MAX).add(Bound::Finite(1)),
            Bound::Unknown
        );
        assert_eq!(Bound::Finite(u64::MAX).mul(2), Bound::Unknown);
        assert_eq!(fixed_loop(i64::MIN, i64::MAX, false), LoopBound::Overflow);
        assert_eq!(fixed_loop(0, i64::MAX, true), LoopBound::Overflow);
    }

    #[test]
    fn dynamic_draw_receiver_poisons_every_declared_image() {
        let mut out = BTreeMap::new();
        let hirs = BTreeMap::new();
        let by_name = BTreeMap::new();
        let capacities = BTreeMap::new();
        let env = BTreeMap::new();
        let all_images = BTreeSet::from(["hero".to_string(), "enemy".to_string()]);
        let mut stack = BTreeSet::new();
        let mut causes = BTreeMap::new();
        analyze_call(
            "draw",
            &[
                SimpleExpr::Call {
                    target: "choose_sprite".to_string(),
                    args: Vec::new(),
                },
                SimpleExpr::Int(0),
            ],
            &mut out,
            &hirs,
            &by_name,
            &capacities,
            &env,
            &all_images,
            &mut stack,
            &mut causes,
        );
        assert_eq!(out.get("hero"), Some(&Bound::Unknown));
        assert_eq!(out.get("enemy"), Some(&Bound::Unknown));
        assert!(causes
            .values()
            .all(|cause| cause.contains("dynamic render receiver")));
    }

    #[test]
    fn ambiguous_render_reachable_call_poisons_declared_images() {
        fn function(id: u32) -> FunctionMeta {
            let path = CanonicalSourcePath::project_relative("main.stasis").expect("path");
            FunctionMeta {
                id,
                symbol_id: SymbolId::function(&path, "paint", &id.to_string()),
                storage_index: id,
                name: "paint".to_string(),
                module_alias: String::new(),
                name_hash: 0,
                file_id: 0,
                source_range: 0..0,
                signature_range: 0..0,
                signature_hash: u64::from(id),
                body_hash: 0,
                param_names: Vec::new(),
                params: Vec::new(),
                return_type: 0,
                inline: false,
                effect_contract: None,
                dependencies: Vec::new(),
                dependents: Vec::new(),
                call_sites: Vec::new(),
                dirty: false,
            }
        }
        let first = function(1);
        let second = function(2);
        let mut by_name = BTreeMap::new();
        by_name.insert(("paint", 0), vec![&first, &second]);
        let mut out = BTreeMap::new();
        let mut causes = BTreeMap::new();
        analyze_call(
            "paint",
            &[],
            &mut out,
            &BTreeMap::new(),
            &by_name,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeSet::from(["hero".to_string()]),
            &mut BTreeSet::new(),
            &mut causes,
        );
        assert_eq!(out.get("hero"), Some(&Bound::Unknown));
        assert!(causes["hero"].contains("ambiguous"));
    }

    #[test]
    fn profitability_rejects_cold_single_and_mixed_small_groups() {
        let mut records = vec![
            record("cold", 512, 512, Some(1)),
            record("single", 512, 512, Some(20)),
        ];
        records[0].grouping_key = "cold-format".to_string();
        records[1].grouping_key = "hot-format".to_string();
        apply_profitability_policy(&mut records);
        assert!(!records[0].atlas_eligible);
        assert!(!records[1].atlas_eligible);
        assert!(records[1].reason.contains("single hot image"));

        let mut compatible = vec![
            record("a", 256, 256, Some(5)),
            record("b", 256, 256, Some(5)),
        ];
        apply_profitability_policy(&mut compatible);
        assert!(compatible.iter().all(|image| image.atlas_eligible));

        let mut mixed = vec![
            record("a", 256, 256, Some(20)),
            record("b", 512, 256, Some(20)),
        ];
        apply_profitability_policy(&mut mixed);
        assert!(mixed.iter().all(|image| !image.atlas_eligible));
    }

    fn record(
        identity: &str,
        width: u32,
        height: u32,
        maximum: Option<u64>,
    ) -> HotRenderImageMetadata {
        HotRenderImageMetadata {
            identity: identity.to_string(),
            logical_path: format!("{identity}.png"),
            logical_width: width,
            logical_height: height,
            sheet_columns: None,
            sheet_rows: None,
            cell_width: None,
            cell_height: None,
            max_renders_per_render: maximum,
            unknown_cause: None,
            atlas_eligible: false,
            grouping_key: "rgba8-premul:linear-filter".to_string(),
            backend_constraints: "rgba8-premultiplied;linear-filter;runtime-page-limits"
                .to_string(),
            reason: String::new(),
        }
    }
}
