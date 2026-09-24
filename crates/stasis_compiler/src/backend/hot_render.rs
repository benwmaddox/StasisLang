//! Compiler-owned hot-render metadata. The compiler publishes deterministic
//! policy facts; rasterization, texture allocation, and fallback stay runtime-owned.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::backend::compile_analysis::{ConstantValue, ConstantValueMap};
use crate::compiler::{FunctionId, FunctionMeta};
use crate::ir::hir::{eval_const_i64, AssignOp, AssignTarget, FunctionHIR, SimpleExpr, SimpleStmt};

pub const HOT_RENDER_METADATA_VERSION: u32 = 4;

const MAX_PUBLISHED_TRANSITION_PAIRS: usize = 4096;
const MAX_TRANSITIONS_PER_PAIR: u64 = 1_000_000_000;

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
    pub estimated_distinct_transitions: u64,
    pub group_member_count: u32,
    pub group_logical_pixel_area: u64,
    pub group_max_logical_width: u32,
    pub group_max_logical_height: u32,
    pub backend_constraints: String,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HotRenderTransitionProvenance {
    SequenceJoin,
    BranchMaximum,
    FixedRepeatInternal,
    FixedRepeatBoundary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HotRenderTransitionValidity {
    Finite,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HotRenderTransitionAnalysisValidity {
    Complete,
    Incomplete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HotRenderTransitionMetadata {
    pub from_identity: String,
    pub to_identity: String,
    pub max_transitions_per_render: Option<u64>,
    pub validity: HotRenderTransitionValidity,
    pub unknown_cause: Option<String>,
    pub provenance: Vec<HotRenderTransitionProvenance>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HotRenderTransitionAnalysis {
    pub validity: HotRenderTransitionAnalysisValidity,
    pub unknown_causes: Vec<String>,
    pub pair_count: Option<u32>,
    pub published_pair_count: u32,
    pub omitted_pair_count: Option<u32>,
    pub pair_limit: u32,
    pub max_pair_weight: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotRenderAnalysis {
    pub images: Vec<HotRenderImageMetadata>,
    pub transitions: Vec<HotRenderTransitionMetadata>,
    pub transition_analysis: HotRenderTransitionAnalysis,
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
) -> HotRenderAnalysis {
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
    let mut causes = BTreeMap::new();
    let mut render_flow = FlowSummary::empty();
    let mut has_render_root = false;
    let render_roots = functions
        .iter()
        .filter(|function| function.name == "render" && reachable.contains(&function.id));
    for root in render_roots {
        has_render_root = true;
        let mut stack = BTreeSet::new();
        let env = BTreeMap::new();
        let root_flow = analyze_function(
            root,
            hirs,
            &by_name,
            collection_capacities,
            &env,
            &all_images,
            &mut stack,
            &mut causes,
        );
        render_flow = render_flow.branch(root_flow);
    }
    if !has_render_root {
        render_flow = FlowSummary::empty();
    }

    if let Some(cause) = loader_uncertainty {
        render_flow.poison_all(&all_images, &mut causes, &cause);
    }

    let mut records = images
        .into_values()
        .map(|image| {
            let mut bound = render_flow
                .counts
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
                grouping_key: String::new(),
                estimated_distinct_transitions: 0,
                group_member_count: 0,
                group_logical_pixel_area: 0,
                group_max_logical_width: 0,
                group_max_logical_height: 0,
                backend_constraints: "rgba8-premultiplied;linear-filter;runtime-page-limits"
                    .to_string(),
                reason,
            }
        })
        .collect::<Vec<_>>();
    apply_profitability_policy(&mut records, &render_flow.transitions);
    let (transitions, transition_analysis) = publish_transitions(&render_flow);
    HotRenderAnalysis {
        images: records,
        transitions,
        transition_analysis,
    }
}

fn publish_transitions(
    flow: &FlowSummary,
) -> (
    Vec<HotRenderTransitionMetadata>,
    HotRenderTransitionAnalysis,
) {
    let mut unknown_causes = flow.unknown_causes.clone();
    let pair_count = if flow.unknown_causes.is_empty() {
        u32::try_from(flow.transitions.len()).ok()
    } else {
        None
    };
    if pair_count.is_none() && flow.unknown_causes.is_empty() {
        unknown_causes.insert("ordered transition pair count exceeds u32".to_string());
    }

    let mut transitions = Vec::new();
    if flow.unknown_causes.is_empty() {
        for ((from_identity, to_identity), bound) in &flow.transitions {
            let (maximum, validity, unknown_cause) = match bound {
                Bound::Finite(value) if *value <= MAX_TRANSITIONS_PER_PAIR => {
                    (Some(*value), HotRenderTransitionValidity::Finite, None)
                }
                Bound::Finite(_) => {
                    let cause = format!(
                        "transition upper bound exceeds metadata weight cap {MAX_TRANSITIONS_PER_PAIR}"
                    );
                    unknown_causes.insert(cause.clone());
                    (None, HotRenderTransitionValidity::Unknown, Some(cause))
                }
                Bound::Unknown => {
                    let cause = "transition upper bound overflowed checked arithmetic".to_string();
                    unknown_causes.insert(cause.clone());
                    (None, HotRenderTransitionValidity::Unknown, Some(cause))
                }
            };
            transitions.push(HotRenderTransitionMetadata {
                from_identity: from_identity.clone(),
                to_identity: to_identity.clone(),
                max_transitions_per_render: maximum,
                validity,
                unknown_cause,
                provenance: flow
                    .transition_provenance
                    .get(&(from_identity.clone(), to_identity.clone()))
                    .map(|provenance| provenance.iter().copied().collect())
                    .unwrap_or_default(),
            });
        }
        transitions.sort_by(|left, right| {
            let weight_order = match (
                left.max_transitions_per_render,
                right.max_transitions_per_render,
            ) {
                (Some(left), Some(right)) => right.cmp(&left),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            };
            weight_order
                .then_with(|| left.from_identity.cmp(&right.from_identity))
                .then_with(|| left.to_identity.cmp(&right.to_identity))
        });
    }

    let pair_limit =
        u32::try_from(MAX_PUBLISHED_TRANSITION_PAIRS).expect("transition pair cap fits in u32");
    let (reported_count, omitted_pair_count) = if let Some(pair_count) = pair_count {
        let reported_count = pair_count.min(pair_limit);
        let omitted = pair_count - reported_count;
        if omitted > 0 {
            unknown_causes.insert(format!(
                "ordered transition pair count exceeds metadata cap {pair_limit}"
            ));
        }
        (reported_count, Some(omitted))
    } else {
        (0, None)
    };
    transitions.truncate(reported_count as usize);

    let validity = if unknown_causes.is_empty() {
        HotRenderTransitionAnalysisValidity::Complete
    } else {
        HotRenderTransitionAnalysisValidity::Incomplete
    };
    let analysis = HotRenderTransitionAnalysis {
        validity,
        unknown_causes: unknown_causes.into_iter().collect(),
        pair_count,
        published_pair_count: reported_count,
        omitted_pair_count,
        pair_limit,
        max_pair_weight: MAX_TRANSITIONS_PER_PAIR,
    };
    (transitions, analysis)
}

fn apply_profitability_policy(
    records: &mut [HotRenderImageMetadata],
    transitions: &BTreeMap<(String, String), Bound>,
) {
    const MAX_LOGICAL_EXTENT: u32 = 4096;
    const MIN_DISTINCT_TRANSITIONS: u64 = 2;

    let eligible_identities = records
        .iter()
        .filter(|image| {
            image.max_renders_per_render.is_some_and(|count| count > 1)
                && image.logical_width > 0
                && image.logical_height > 0
                && image.logical_width <= MAX_LOGICAL_EXTENT
                && image.logical_height <= MAX_LOGICAL_EXTENT
        })
        .map(|image| image.identity.clone())
        .collect::<BTreeSet<_>>();
    let mut adjacency = BTreeMap::<String, BTreeSet<String>>::new();
    for ((left, right), weight) in transitions {
        if left != right
            && eligible_identities.contains(left)
            && eligible_identities.contains(right)
            && matches!(weight, Bound::Finite(value) if *value >= MIN_DISTINCT_TRANSITIONS)
        {
            adjacency
                .entry(left.clone())
                .or_default()
                .insert(right.clone());
            adjacency
                .entry(right.clone())
                .or_default()
                .insert(left.clone());
        }
    }

    let by_identity = records
        .iter()
        .enumerate()
        .map(|(index, image)| (image.identity.clone(), index))
        .collect::<BTreeMap<_, _>>();
    let mut visited = BTreeSet::new();
    for seed in adjacency.keys() {
        if !visited.insert(seed.clone()) {
            continue;
        }
        let mut stack = vec![seed.clone()];
        let mut members = BTreeSet::new();
        while let Some(identity) = stack.pop() {
            members.insert(identity.clone());
            for neighbor in adjacency.get(&identity).into_iter().flatten() {
                if visited.insert(neighbor.clone()) {
                    stack.push(neighbor.clone());
                }
            }
        }
        let indices = members
            .iter()
            .filter_map(|identity| by_identity.get(identity).copied())
            .collect::<Vec<_>>();
        let logical_pixels = indices.iter().try_fold(0_u64, |total, index| {
            let image = &records[*index];
            total.checked_add(u64::from(image.logical_width) * u64::from(image.logical_height))
        });
        let distinct_transitions = transitions
            .iter()
            .try_fold(0_u64, |total, ((a, b), bound)| {
                if a != b && members.contains(a) && members.contains(b) {
                    match bound {
                        Bound::Finite(value) => total.checked_add(*value),
                        Bound::Unknown => None,
                    }
                } else {
                    Some(total)
                }
            });
        let eligible = indices.len() >= 2
            && distinct_transitions.is_some_and(|value| value >= MIN_DISTINCT_TRANSITIONS)
            && logical_pixels.is_some();
        let identities = members.iter().cloned().collect::<Vec<_>>();
        let digest = Sha256::digest(identities.join("\n").as_bytes());
        let group_key = format!("batch-v3:{:x}", digest);
        let max_width = indices
            .iter()
            .map(|index| records[*index].logical_width)
            .max()
            .unwrap_or(0);
        let max_height = indices
            .iter()
            .map(|index| records[*index].logical_height)
            .max()
            .unwrap_or(0);
        for index in &indices {
            let image = &mut records[*index];
            image.atlas_eligible = eligible;
            image.grouping_key = group_key.clone();
            image.estimated_distinct_transitions = distinct_transitions.unwrap_or(0);
            image.group_member_count = u32::try_from(indices.len()).unwrap_or(u32::MAX);
            image.group_logical_pixel_area = logical_pixels.unwrap_or(0);
            image.group_max_logical_width = max_width;
            image.group_max_logical_height = max_height;
            image.reason = if distinct_transitions.is_none() || logical_pixels.is_none() {
                "profitability arithmetic overflow".to_string()
            } else if distinct_transitions.unwrap_or(0) < MIN_DISTINCT_TRANSITIONS {
                "render order has insufficient avoidable texture transitions".to_string()
            } else {
                "eligible interleaved batch; runtime rechecks realized dimensions and device limits"
                    .to_string()
            };
        }
    }

    for image in records.iter_mut().filter(|image| {
        image.max_renders_per_render.is_some_and(|count| count > 1) && image.group_member_count == 0
    }) {
        image.reason = "no repeated distinct-image render transition benefit".to_string();
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum IdentitySet {
    Known(BTreeSet<String>),
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FlowSummary {
    counts: BTreeMap<String, Bound>,
    first: BTreeSet<String>,
    last: BTreeSet<String>,
    may_be_empty: bool,
    transitions: BTreeMap<(String, String), Bound>,
    transition_provenance: BTreeMap<(String, String), BTreeSet<HotRenderTransitionProvenance>>,
    unknown_causes: BTreeSet<String>,
}

impl FlowSummary {
    fn empty() -> Self {
        Self {
            counts: BTreeMap::new(),
            first: BTreeSet::new(),
            last: BTreeSet::new(),
            may_be_empty: true,
            transitions: BTreeMap::new(),
            transition_provenance: BTreeMap::new(),
            unknown_causes: BTreeSet::new(),
        }
    }

    /// An ordered non-sprite draw or clip operation. It emits no image identity,
    /// but separates sprite runs on either side of it.
    fn barrier() -> Self {
        Self {
            counts: BTreeMap::new(),
            first: BTreeSet::new(),
            last: BTreeSet::new(),
            may_be_empty: false,
            transitions: BTreeMap::new(),
            transition_provenance: BTreeMap::new(),
            unknown_causes: BTreeSet::new(),
        }
    }

    fn has_image_activity(&self) -> bool {
        !self.counts.is_empty() || !self.transitions.is_empty() || !self.unknown_causes.is_empty()
    }

    fn draw(identities: BTreeSet<String>) -> Self {
        Self {
            counts: identities
                .iter()
                .cloned()
                .map(|identity| (identity, Bound::Finite(1)))
                .collect(),
            first: identities.clone(),
            last: identities,
            may_be_empty: false,
            transitions: BTreeMap::new(),
            transition_provenance: BTreeMap::new(),
            unknown_causes: BTreeSet::new(),
        }
    }

    fn sequential(mut self, next: Self) -> Self {
        if self.counts.is_empty()
            && self.transitions.is_empty()
            && self.unknown_causes.is_empty()
            && self.may_be_empty
        {
            return next;
        }
        if next.counts.is_empty()
            && next.transitions.is_empty()
            && next.unknown_causes.is_empty()
            && next.may_be_empty
        {
            return self;
        }
        add_bound_maps(&mut self.counts, next.counts);
        merge_transition_bounds(&mut self.transitions, next.transitions);
        merge_transition_provenance(&mut self.transition_provenance, next.transition_provenance);
        self.unknown_causes.extend(next.unknown_causes);
        for left in &self.last {
            for right in &next.first {
                let pair = (left.clone(), right.clone());
                add_bound(&mut self.transitions, pair.clone(), Bound::Finite(1));
                self.transition_provenance
                    .entry(pair)
                    .or_default()
                    .insert(HotRenderTransitionProvenance::SequenceJoin);
            }
        }
        let previous_empty = self.may_be_empty;
        let previous_first = self.first.clone();
        let previous_last = self.last.clone();
        self.first = if previous_empty {
            previous_first.union(&next.first).cloned().collect()
        } else {
            previous_first
        };
        self.last = if next.may_be_empty {
            previous_last.union(&next.last).cloned().collect()
        } else {
            next.last
        };
        self.may_be_empty = previous_empty && next.may_be_empty;
        self
    }

    fn branch(self, other: Self) -> Self {
        let mut transitions = branch_bound_maps(self.transitions, other.transitions);
        let mut transition_provenance = self.transition_provenance;
        merge_transition_provenance(&mut transition_provenance, other.transition_provenance);
        for provenance in transition_provenance.values_mut() {
            provenance.insert(HotRenderTransitionProvenance::BranchMaximum);
        }
        Self {
            counts: branch_bound_maps(self.counts, other.counts),
            first: self.first.union(&other.first).cloned().collect(),
            last: self.last.union(&other.last).cloned().collect(),
            may_be_empty: self.may_be_empty || other.may_be_empty,
            transitions: std::mem::take(&mut transitions),
            transition_provenance,
            unknown_causes: self
                .unknown_causes
                .union(&other.unknown_causes)
                .cloned()
                .collect(),
        }
    }

    fn repeat(mut self, count: u64) -> Self {
        if count == 0 {
            return Self::empty();
        }
        self.counts = multiply_bound_map(self.counts, count);
        self.transitions = multiply_bound_map(self.transitions, count);
        for provenance in self.transition_provenance.values_mut() {
            provenance.insert(HotRenderTransitionProvenance::FixedRepeatInternal);
        }
        if count > 1 {
            let repeat_edges = count - 1;
            for left in &self.last {
                for right in &self.first {
                    let pair = (left.clone(), right.clone());
                    add_bound(
                        &mut self.transitions,
                        pair.clone(),
                        Bound::Finite(repeat_edges),
                    );
                    self.transition_provenance
                        .entry(pair)
                        .or_default()
                        .insert(HotRenderTransitionProvenance::FixedRepeatBoundary);
                }
            }
        }
        self
    }

    fn poison_all(
        &mut self,
        all_images: &BTreeSet<String>,
        causes: &mut BTreeMap<String, String>,
        cause: &str,
    ) {
        self.unknown_causes.insert(cause.to_string());
        for identity in all_images {
            self.counts.insert(identity.clone(), Bound::Unknown);
            causes
                .entry(identity.clone())
                .or_insert_with(|| cause.to_string());
        }
        self.transitions.clear();
        self.transition_provenance.clear();
        self.first = all_images.clone();
        self.last = all_images.clone();
    }
}

fn merge_transition_bounds(
    out: &mut BTreeMap<(String, String), Bound>,
    other: BTreeMap<(String, String), Bound>,
) {
    for (pair, bound) in other {
        add_bound(out, pair, bound);
    }
}

fn merge_transition_provenance(
    out: &mut BTreeMap<(String, String), BTreeSet<HotRenderTransitionProvenance>>,
    other: BTreeMap<(String, String), BTreeSet<HotRenderTransitionProvenance>>,
) {
    for (pair, provenance) in other {
        out.entry(pair).or_default().extend(provenance);
    }
}

fn add_bound<K: Ord>(out: &mut BTreeMap<K, Bound>, key: K, value: Bound) {
    let next = out.remove(&key).unwrap_or(Bound::Finite(0)).add(value);
    out.insert(key, next);
}

fn add_bound_maps<K: Ord>(out: &mut BTreeMap<K, Bound>, other: BTreeMap<K, Bound>) {
    for (key, value) in other {
        add_bound(out, key, value);
    }
}

fn branch_bound_maps<K: Ord + Clone>(
    left: BTreeMap<K, Bound>,
    right: BTreeMap<K, Bound>,
) -> BTreeMap<K, Bound> {
    left.keys()
        .chain(right.keys())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
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

fn multiply_bound_map<K: Ord>(map: BTreeMap<K, Bound>, factor: u64) -> BTreeMap<K, Bound> {
    map.into_iter()
        .map(|(key, value)| (key, value.mul(factor)))
        .collect()
}

fn analyze_function(
    function: &FunctionMeta,
    hirs: &BTreeMap<FunctionId, FunctionHIR>,
    by_name: &BTreeMap<(&str, usize), Vec<&FunctionMeta>>,
    capacities: &BTreeMap<String, i32>,
    env: &BTreeMap<String, IdentitySet>,
    all_images: &BTreeSet<String>,
    stack: &mut BTreeSet<FunctionId>,
    causes: &mut BTreeMap<String, String>,
) -> FlowSummary {
    if !stack.insert(function.id) {
        let mut unknown = FlowSummary::empty();
        unknown.poison_all(
            all_images,
            causes,
            &format!("recursive call through {}", function.name),
        );
        return unknown;
    }
    let result = hirs
        .get(&function.id)
        .map_or_else(FlowSummary::empty, |hir| {
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
    env: &BTreeMap<String, IdentitySet>,
    all_images: &BTreeSet<String>,
    stack: &mut BTreeSet<FunctionId>,
    causes: &mut BTreeMap<String, String>,
) -> FlowSummary {
    let mut out = FlowSummary::empty();
    for statement in statements {
        let statement_flow = match statement {
            SimpleStmt::If {
                condition,
                then_statements,
                else_statements,
                ..
            } => {
                let condition_flow = analyze_condition_calls(
                    condition, hirs, by_name, capacities, env, all_images, stack, causes,
                );
                let then_flow = analyze_statements(
                    then_statements,
                    hirs,
                    by_name,
                    capacities,
                    env,
                    all_images,
                    stack,
                    causes,
                );
                let else_flow =
                    else_statements
                        .as_deref()
                        .map_or_else(FlowSummary::empty, |body| {
                            analyze_statements(
                                body, hirs, by_name, capacities, env, all_images, stack, causes,
                            )
                        });
                condition_flow.sequential(then_flow.branch(else_flow))
            }
            SimpleStmt::For {
                init,
                condition,
                step,
                body_statements,
            } => {
                let init_flow = analyze_statement_calls(
                    init, hirs, by_name, capacities, env, all_images, stack, causes,
                );
                let condition_flow = analyze_condition_calls(
                    condition, hirs, by_name, capacities, env, all_images, stack, causes,
                );
                let step_flow = analyze_statement_calls(
                    step, hirs, by_name, capacities, env, all_images, stack, causes,
                );
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
                        let iteration = condition_flow
                            .clone()
                            .sequential(body)
                            .sequential(step_flow)
                            .repeat(iterations);
                        init_flow.sequential(iteration).sequential(condition_flow)
                    }
                    LoopBound::Dynamic => {
                        let mut unknown = init_flow
                            .sequential(condition_flow)
                            .sequential(body)
                            .sequential(step_flow);
                        if !unknown.has_image_activity() {
                            // The loop may execute zero times. Work containing no
                            // image draw cannot make the image transition bound
                            // unknown; preserve its possible empty path.
                            FlowSummary::empty()
                        } else {
                            unknown.poison_all(all_images, causes, "unbounded or dynamic for loop");
                            unknown
                        }
                    }
                    LoopBound::Overflow => {
                        let mut unknown = init_flow
                            .sequential(condition_flow)
                            .sequential(body)
                            .sequential(step_flow);
                        unknown.poison_all(
                            all_images,
                            causes,
                            "for-loop bound arithmetic overflow",
                        );
                        unknown
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
                    Some(capacity) => body.repeat(capacity),
                    None if body.has_image_activity() => {
                        let mut unknown = body;
                        unknown.poison_all(all_images, causes, "unknown collection capacity");
                        unknown
                    }
                    None => FlowSummary::empty(),
                }
            }
            _ => analyze_statement_calls(
                statement, hirs, by_name, capacities, env, all_images, stack, causes,
            ),
        };
        out = out.sequential(statement_flow);
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn analyze_statement_calls(
    statement: &SimpleStmt,
    hirs: &BTreeMap<FunctionId, FunctionHIR>,
    by_name: &BTreeMap<(&str, usize), Vec<&FunctionMeta>>,
    capacities: &BTreeMap<String, i32>,
    env: &BTreeMap<String, IdentitySet>,
    all_images: &BTreeSet<String>,
    stack: &mut BTreeSet<FunctionId>,
    causes: &mut BTreeMap<String, String>,
) -> FlowSummary {
    let mut out = FlowSummary::empty();
    visit_stmt_exprs(statement, &mut |target, args| {
        out = std::mem::replace(&mut out, FlowSummary::empty()).sequential(analyze_call(
            target, args, hirs, by_name, capacities, env, all_images, stack, causes,
        ));
    });
    out
}

#[allow(clippy::too_many_arguments)]
fn analyze_condition_calls(
    condition: &crate::ir::hir::SimpleCondition,
    hirs: &BTreeMap<FunctionId, FunctionHIR>,
    by_name: &BTreeMap<(&str, usize), Vec<&FunctionMeta>>,
    capacities: &BTreeMap<String, i32>,
    env: &BTreeMap<String, IdentitySet>,
    all_images: &BTreeSet<String>,
    stack: &mut BTreeSet<FunctionId>,
    causes: &mut BTreeMap<String, String>,
) -> FlowSummary {
    let mut out = FlowSummary::empty();
    visit_condition_exprs(condition, &mut |target, args| {
        out = std::mem::replace(&mut out, FlowSummary::empty()).sequential(analyze_call(
            target, args, hirs, by_name, capacities, env, all_images, stack, causes,
        ));
    });
    out
}

#[allow(clippy::too_many_arguments)]
fn analyze_call(
    target: &str,
    args: &[SimpleExpr],
    hirs: &BTreeMap<FunctionId, FunctionHIR>,
    by_name: &BTreeMap<(&str, usize), Vec<&FunctionMeta>>,
    capacities: &BTreeMap<String, i32>,
    env: &BTreeMap<String, IdentitySet>,
    all_images: &BTreeSet<String>,
    stack: &mut BTreeSet<FunctionId>,
    causes: &mut BTreeMap<String, String>,
) -> FlowSummary {
    let leaf_target = target.rsplit('.').next().unwrap_or(target);
    if is_ordered_render_barrier(leaf_target) {
        return FlowSummary::barrier();
    }
    if matches!(leaf_target, "draw" | "draw_frame" | "draw_frame_scaled") {
        if let Some(receiver) = args.first() {
            match resolve_identities(receiver, env, all_images) {
                IdentitySet::Known(identities)
                    if !identities.is_empty() && identities.is_subset(all_images) =>
                {
                    return FlowSummary::draw(identities);
                }
                IdentitySet::Known(identities)
                    if leaf_target == "draw" && identities.is_disjoint(all_images) =>
                {
                    // draw is also used by TextRun and LineBatch. Resolve those
                    // helper bodies below rather than treating them as images.
                }
                _ => {
                    return unknown_render_flow(
                        all_images,
                        causes,
                        &format!("dynamic render receiver in call to {leaf_target}"),
                    );
                }
            }
        } else if leaf_target != "draw" {
            return unknown_render_flow(
                all_images,
                causes,
                &format!("dynamic render receiver in call to {leaf_target}"),
            );
        }
    }
    let Some(candidates) = by_name.get(&(leaf_target, args.len())) else {
        if is_known_nonrender_graphics_call(leaf_target) {
            return FlowSummary::empty();
        }
        let cause = if is_potential_render_call(leaf_target) || leaf_target == "draw" {
            format!("unresolved potentially rendering call {leaf_target}")
        } else {
            format!("unresolved call target {leaf_target} has no analyzable body")
        };
        return unknown_render_flow(all_images, causes, &cause);
    };
    if candidates.len() != 1 {
        return unknown_render_flow(
            all_images,
            causes,
            &format!("ambiguous render-reachable call target {leaf_target}"),
        );
    }
    let callee = candidates[0];
    let has_analyzable_body = hirs
        .get(&callee.id)
        .is_some_and(|hir| !hir.statements.is_empty());
    if !has_analyzable_body
        && (leaf_target == "draw"
            || (has_graphics_effect(callee) && !is_known_nonrender_graphics_call(leaf_target)))
    {
        return unknown_render_flow(
            all_images,
            causes,
            &format!("potentially rendering external call {leaf_target} has no analyzable body"),
        );
    }
    let callee_env = callee
        .param_names
        .iter()
        .zip(args)
        .map(|(name, arg)| (name.clone(), resolve_identities(arg, env, all_images)))
        .collect();
    analyze_function(
        callee,
        hirs,
        by_name,
        capacities,
        &callee_env,
        all_images,
        stack,
        causes,
    )
}

fn unknown_render_flow(
    all_images: &BTreeSet<String>,
    causes: &mut BTreeMap<String, String>,
    cause: &str,
) -> FlowSummary {
    let mut unknown = FlowSummary::empty();
    unknown.poison_all(all_images, causes, cause);
    unknown
}

fn is_ordered_render_barrier(target: &str) -> bool {
    matches!(
        target,
        "draw_line"
            | "draw_text"
            | "fill_rect"
            | "draw_rect"
            | "gfx_push_clip"
            | "gfx_pop_clip"
            | "gfx_cmd_line"
            | "gfx_cmd_rect"
            | "gfx_cmd_text"
            | "gfx_cmd_text_cached"
            | "gfx_cmd_clip_push"
            | "gfx_cmd_clip_pop"
    )
}

fn is_potential_render_call(target: &str) -> bool {
    target.starts_with("draw_") || target.starts_with("gfx_cmd_")
}

fn has_graphics_effect(function: &FunctionMeta) -> bool {
    function
        .effect_contract
        .as_ref()
        .is_some_and(|effects| effects.iter().any(|effect| effect == "graphics"))
}

fn is_known_nonrender_graphics_call(target: &str) -> bool {
    matches!(
        target,
        "gfx_poll_reload"
            | "gfx_dump_bmp"
            | "gfx_dump_png"
            | "load_font"
            | "gfx_release_font"
            | "gfx_font_status"
            | "measure_text"
            | "load_sprite_from"
            | "load_sprite_sheet_asset_from"
            | "load_text_from"
            | "replace_text_from"
            | "asset_request_sprite"
            | "gfx_release_sprite"
    )
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

fn resolve_identities(
    expr: &SimpleExpr,
    env: &BTreeMap<String, IdentitySet>,
    all_images: &BTreeSet<String>,
) -> IdentitySet {
    match expr {
        SimpleExpr::Identifier(path) => env
            .get(path)
            .cloned()
            .unwrap_or_else(|| IdentitySet::Known(BTreeSet::from([path.clone()]))),
        SimpleExpr::IndexedPath {
            collection_path,
            index,
            suffix,
        } => {
            if let Some(index) = eval_const_i64(index) {
                return IdentitySet::Known(BTreeSet::from([if suffix.is_empty() {
                    format!("{collection_path}[{index}]")
                } else {
                    format!("{collection_path}[{index}].{suffix}")
                }]));
            }
            let prefix = format!("{collection_path}[");
            let suffix_pattern = if suffix.is_empty() {
                "]".to_string()
            } else {
                format!("].{suffix}")
            };
            let aliases = all_images
                .iter()
                .filter(|identity| {
                    identity.starts_with(&prefix) && identity.ends_with(&suffix_pattern)
                })
                .cloned()
                .collect::<BTreeSet<_>>();
            if aliases.is_empty() {
                IdentitySet::Unknown
            } else {
                IdentitySet::Known(aliases)
            }
        }
        _ => IdentitySet::Unknown,
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
    fn ordered_flow_composes_optional_branch_and_loop_edges() {
        let a = FlowSummary::draw(BTreeSet::from(["a".to_string()]));
        let optional_b =
            FlowSummary::draw(BTreeSet::from(["b".to_string()])).branch(FlowSummary::empty());
        let repeated = a.sequential(optional_b).repeat(4);

        assert_eq!(
            repeated
                .transitions
                .get(&("a".to_string(), "b".to_string())),
            Some(&Bound::Finite(4))
        );
        assert_eq!(
            repeated
                .transitions
                .get(&("b".to_string(), "a".to_string())),
            Some(&Bound::Finite(3))
        );
        assert_eq!(repeated.counts.get("a"), Some(&Bound::Finite(4)));
        assert_eq!(repeated.counts.get("b"), Some(&Bound::Finite(4)));
    }

    #[test]
    fn ordered_barriers_separate_sprite_pairs_but_optional_barriers_preserve_possible_edges() {
        let a = FlowSummary::draw(BTreeSet::from(["a".to_string()]));
        let b = FlowSummary::draw(BTreeSet::from(["b".to_string()]));

        let separated = a
            .clone()
            .sequential(FlowSummary::barrier())
            .sequential(b.clone());
        assert!(!separated
            .transitions
            .contains_key(&("a".to_string(), "b".to_string())));

        let optional_barrier = FlowSummary::barrier().branch(FlowSummary::empty());
        let possible = a.sequential(optional_barrier).sequential(b);
        assert_eq!(
            possible
                .transitions
                .get(&("a".to_string(), "b".to_string())),
            Some(&Bound::Finite(1))
        );
    }

    #[test]
    fn dynamic_draw_receiver_poisons_every_declared_image() {
        let hirs = BTreeMap::new();
        let by_name = BTreeMap::new();
        let capacities = BTreeMap::new();
        let env = BTreeMap::new();
        let all_images = BTreeSet::from(["hero".to_string(), "enemy".to_string()]);
        let mut stack = BTreeSet::new();
        let mut causes = BTreeMap::new();
        let out = analyze_call(
            "draw",
            &[
                SimpleExpr::Call {
                    target: "choose_sprite".to_string(),
                    args: Vec::new(),
                },
                SimpleExpr::Int(0),
            ],
            &hirs,
            &by_name,
            &capacities,
            &env,
            &all_images,
            &mut stack,
            &mut causes,
        );
        assert_eq!(out.counts.get("hero"), Some(&Bound::Unknown));
        assert_eq!(out.counts.get("enemy"), Some(&Bound::Unknown));
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
                host_export: None,
                effect_contract: None,
                requires_contract: None,
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
        let mut causes: BTreeMap<String, String> = BTreeMap::new();
        let out = analyze_call(
            "paint",
            &[],
            &BTreeMap::new(),
            &by_name,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeSet::from(["hero".to_string()]),
            &mut BTreeSet::new(),
            &mut causes,
        );
        assert_eq!(out.counts.get("hero"), Some(&Bound::Unknown));
        assert!(causes["hero"].contains("ambiguous"));
    }

    #[test]
    fn bodyless_graphics_external_poisons_pair_evidence() {
        let path = CanonicalSourcePath::project_relative("main.stasis").expect("path");
        let external = FunctionMeta {
            id: 1,
            symbol_id: SymbolId::function(&path, "custom_renderer", "(i32)"),
            storage_index: 1,
            name: "custom_renderer".to_string(),
            module_alias: String::new(),
            name_hash: 0,
            file_id: 0,
            source_range: 0..0,
            signature_range: 0..0,
            signature_hash: 1,
            body_hash: 0,
            param_names: vec!["font".to_string()],
            params: vec![0],
            return_type: 0,
            inline: false,
            host_export: None,
            effect_contract: Some(vec!["graphics".to_string()]),
            requires_contract: None,
            dependencies: Vec::new(),
            dependents: Vec::new(),
            call_sites: Vec::new(),
            dirty: false,
        };
        let by_name = BTreeMap::from([(("custom_renderer", 1), vec![&external])]);
        let mut causes = BTreeMap::new();
        let out = analyze_call(
            "custom_renderer",
            &[SimpleExpr::Int(0)],
            &BTreeMap::new(),
            &by_name,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeSet::from(["hero".to_string()]),
            &mut BTreeSet::new(),
            &mut causes,
        );

        assert_eq!(out.counts.get("hero"), Some(&Bound::Unknown));
        assert!(causes["hero"].contains("potentially rendering external call custom_renderer"));
    }

    #[test]
    fn transition_publication_marks_weight_overflow_unknown_and_caps_pair_rows() {
        let mut capped = FlowSummary::empty();
        for index in 0..=MAX_PUBLISHED_TRANSITION_PAIRS {
            capped.transitions.insert(
                (format!("source-{index:04}"), "target".to_string()),
                Bound::Finite(1),
            );
        }
        let (rows, analysis) = publish_transitions(&capped);
        assert_eq!(rows.len(), MAX_PUBLISHED_TRANSITION_PAIRS);
        assert_eq!(rows[0].from_identity, "source-0000");
        assert_eq!(
            analysis.pair_count,
            Some((MAX_PUBLISHED_TRANSITION_PAIRS + 1) as u32)
        );
        assert_eq!(
            analysis.published_pair_count,
            MAX_PUBLISHED_TRANSITION_PAIRS as u32
        );
        assert_eq!(analysis.omitted_pair_count, Some(1));
        assert_eq!(
            analysis.validity,
            HotRenderTransitionAnalysisValidity::Incomplete
        );
        assert!(analysis
            .unknown_causes
            .iter()
            .any(|cause| cause.contains("pair count exceeds metadata cap")));

        let pair = ("a".to_string(), "b".to_string());
        let mut overweight = FlowSummary::empty();
        overweight
            .transitions
            .insert(pair.clone(), Bound::Finite(MAX_TRANSITIONS_PER_PAIR + 1));
        overweight.transition_provenance.insert(
            pair,
            BTreeSet::from([HotRenderTransitionProvenance::SequenceJoin]),
        );
        let (rows, analysis) = publish_transitions(&overweight);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].validity, HotRenderTransitionValidity::Unknown);
        assert_eq!(rows[0].max_transitions_per_render, None);
        assert!(rows[0]
            .unknown_cause
            .as_deref()
            .is_some_and(|cause| cause.contains("weight cap")));
        assert_eq!(
            analysis.validity,
            HotRenderTransitionAnalysisValidity::Incomplete
        );

        let mut unknown = FlowSummary::empty();
        unknown
            .transitions
            .insert(("a".to_string(), "b".to_string()), Bound::Finite(1));
        unknown
            .unknown_causes
            .insert("dynamic loop bound".to_string());
        let (rows, analysis) = publish_transitions(&unknown);
        assert!(rows.is_empty());
        assert_eq!(analysis.pair_count, None);
        assert_eq!(analysis.published_pair_count, 0);
        assert_eq!(analysis.omitted_pair_count, None);
        assert!(analysis
            .unknown_causes
            .contains(&"dynamic loop bound".to_string()));
    }

    #[test]
    fn profitability_rejects_cold_single_and_accepts_interleaved_mixed_sizes() {
        let mut records = vec![
            record("cold", 512, 512, Some(1)),
            record("single", 512, 512, Some(20)),
        ];
        records[0].grouping_key = "cold-format".to_string();
        records[1].grouping_key = "hot-format".to_string();
        apply_profitability_policy(&mut records, &BTreeMap::new());
        assert!(!records[0].atlas_eligible);
        assert!(!records[1].atlas_eligible);
        assert!(records[1].reason.contains("no repeated distinct"));

        let mut compatible = vec![
            record("a", 256, 256, Some(5)),
            record("b", 256, 256, Some(5)),
        ];
        let alternating = BTreeMap::from([
            (("a".to_string(), "b".to_string()), Bound::Finite(5)),
            (("b".to_string(), "a".to_string()), Bound::Finite(4)),
        ]);
        apply_profitability_policy(&mut compatible, &alternating);
        assert!(compatible.iter().all(|image| image.atlas_eligible));

        let mut mixed = vec![
            record("a", 256, 256, Some(20)),
            record("b", 512, 256, Some(20)),
        ];
        apply_profitability_policy(&mut mixed, &alternating);
        assert!(mixed.iter().all(|image| image.atlas_eligible));
        assert_eq!(mixed[0].grouping_key, mixed[1].grouping_key);

        let mut contiguous = vec![
            record("a", 256, 256, Some(5)),
            record("b", 256, 256, Some(5)),
        ];
        apply_profitability_policy(
            &mut contiguous,
            &BTreeMap::from([(("a".to_string(), "b".to_string()), Bound::Finite(1))]),
        );
        assert!(contiguous.iter().all(|image| !image.atlas_eligible));
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
            grouping_key: String::new(),
            estimated_distinct_transitions: 0,
            group_member_count: 0,
            group_logical_pixel_area: 0,
            group_max_logical_width: 0,
            group_max_logical_height: 0,
            backend_constraints: "rgba8-premultiplied;linear-filter;runtime-page-limits"
                .to_string(),
            reason: String::new(),
        }
    }
}
