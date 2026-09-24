//! Deterministic, portable atlas affinity planning for realized sprites.
//!
//! The planner only reassigns sprites to the fixed page set supplied by the
//! caller. It never creates or grows a page. Coordinates in placements refer
//! to the unpadded sprite pixels; allocation coordinates include the padding.

use std::collections::{BTreeMap, BTreeSet};

pub const MAX_ATLAS_PLANNER_SPRITES: usize = 512;
pub const MAX_ATLAS_PLANNER_PAGES: usize = 256;
pub const MAX_ATLAS_PLANNER_PAIR_WEIGHTS: usize = 32_768;
pub const MAX_ATLAS_PAIR_WEIGHT: u64 = 1_000_000_000;
pub const MAX_ATLAS_CLUSTER_PACK_PROBES: usize = 1_000_000;
pub const ATLAS_RGBA32_BYTES_PER_PIXEL: u64 = 4;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AtlasCompatibilityKey {
    pub group_id: Option<u64>,
    pub format: Option<u32>,
    pub sampler: Option<u32>,
    pub color_space: Option<u32>,
    pub backend: Option<u32>,
}

impl AtlasCompatibilityKey {
    fn is_known(&self) -> bool {
        self.group_id.is_some_and(|group_id| group_id != 0)
            && self.format.is_some()
            && self.sampler.is_some()
            && self.color_space.is_some()
            && self.backend.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtlasSpriteDescriptor {
    pub id: u64,
    pub width: u32,
    pub height: u32,
    pub padding: u32,
    pub compatibility: AtlasCompatibilityKey,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtlasPageExtent {
    pub id: u32,
    pub compatibility: AtlasCompatibilityKey,
    pub width: u32,
    pub height: u32,
    /// First allocatable coordinate, including native page header reservation.
    pub usable_origin_x: u32,
    pub usable_origin_y: u32,
    /// Current RGBA texture allocation bytes for this fixed extent.
    pub allocation_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtlasBaselinePlacement {
    pub sprite_id: u64,
    pub page_id: u32,
    /// Unpadded pixel origin in the existing page.
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    /// Padding used by the baseline allocation.
    pub padding: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtlasPairWeight {
    pub from_sprite_id: u64,
    pub to_sprite_id: u64,
    pub weight: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AtlasMemoryBudget {
    /// Currently resident bytes in the same accounting domain as both budgets.
    pub current_device_bytes: u64,
    /// Maximum resident bytes after the old pages have been released.
    pub max_final_bytes: u64,
    /// Maximum resident bytes while old and replacement textures overlap.
    pub max_peak_bytes: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct AtlasPlanningInput<'a> {
    /// Generation observed when the baseline placements were captured.
    pub current_generation: u64,
    pub baseline_generation: u64,
    pub sprites: &'a [AtlasSpriteDescriptor],
    pub pages: &'a [AtlasPageExtent],
    pub baseline_placements: &'a [AtlasBaselinePlacement],
    pub pair_weights: &'a [AtlasPairWeight],
    pub budget: AtlasMemoryBudget,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtlasPlacement {
    pub sprite_id: u64,
    pub page_id: u32,
    /// Unpadded pixel origin consumed by the native sprite entry.
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    /// Padded allocation rectangle reserved on the page.
    pub allocation_x: u32,
    pub allocation_y: u32,
    pub allocation_width: u32,
    pub allocation_height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtlasPlannedPage {
    pub id: u32,
    pub compatibility: AtlasCompatibilityKey,
    pub width: u32,
    pub height: u32,
    pub usable_origin_x: u32,
    pub usable_origin_y: u32,
    pub allocation_bytes: u64,
    pub sprite_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AtlasMetrics {
    pub baseline_page_count: usize,
    pub candidate_page_count: usize,
    pub moved_sprite_count: usize,
    pub baseline_cut_weight: u64,
    pub candidate_cut_weight: u64,
    pub cut_weight_saved: u64,
    pub candidate_texture_bytes: u64,
    pub final_device_bytes: u64,
    pub peak_device_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtlasPlan {
    pub placements: Vec<AtlasPlacement>,
    pub pages: Vec<AtlasPlannedPage>,
    pub metrics: AtlasMetrics,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtlasFallbackReason {
    StaleGeneration,
    EmptyInput,
    TooManySprites,
    TooManyPages,
    TooManyPairWeights,
    NoPages,
    DuplicateSpriteId,
    DuplicatePageId,
    DuplicateBaselinePlacement,
    MissingBaselinePlacement,
    UnknownCompatibility,
    InvalidSpriteExtent,
    InvalidPageExtent,
    InvalidPageAllocation,
    InvalidBaselinePlacement,
    OverlappingBaselinePlacements,
    IncompatibleBaselinePage,
    UnknownPairSprite,
    InvalidPairWeight,
    DuplicatePairWeight,
    Overflow,
    NoFeasiblePlacement,
    InvalidMemoryAccounting,
    MemoryBudgetExceeded,
    NonImproving,
    ClusterSearchLimitExceeded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtlasPlanningResult {
    /// Present only when the candidate is complete, within budget, and improves affinity.
    pub plan: Option<AtlasPlan>,
    pub baseline_metrics: Option<AtlasMetrics>,
    /// Present when all sprites were placed, even if budget or improvement rejects the plan.
    pub candidate_metrics: Option<AtlasMetrics>,
    pub fallback_reason: Option<AtlasFallbackReason>,
}

#[derive(Clone, Copy)]
struct Shelf {
    y: u32,
    height: u32,
    next_x: u32,
}

#[derive(Clone)]
struct PagePackingState<'a> {
    page: &'a AtlasPageExtent,
    shelves: Vec<Shelf>,
}

#[derive(Clone)]
struct PlacementChoice {
    shelves: Vec<Shelf>,
    allocation_x: u32,
    allocation_y: u32,
    allocation_width: u32,
    allocation_height: u32,
    vertical_slack: u32,
    horizontal_slack: u32,
}

struct AffinityCluster {
    members: BTreeSet<u64>,
    compatibility: AtlasCompatibilityKey,
    strongest_edge_weight: u64,
    internal_edge_weight: u64,
    padded_area: u64,
    first_sprite_id: u64,
}

struct PackedCluster<'a> {
    state: PagePackingState<'a>,
    placements: Vec<AtlasPlacement>,
    external_affinity: u64,
    baseline_page_matches: usize,
    fit_slack: u64,
}

impl<'a> PagePackingState<'a> {
    fn new(page: &'a AtlasPageExtent) -> Self {
        Self {
            page,
            shelves: Vec::new(),
        }
    }

    fn try_place(&self, width: u32, height: u32) -> Option<PlacementChoice> {
        let mut best: Option<((u32, u32, u32, u32), u32, u32, Option<usize>)> = None;
        for (index, shelf) in self.shelves.iter().copied().enumerate() {
            if shelf.next_x > self.page.width
                || width > self.page.width - shelf.next_x
                || shelf.y > self.page.height
                || shelf.height > self.page.height - shelf.y
                || height > shelf.height
            {
                continue;
            }
            let right = shelf.next_x + width;
            let rank = (
                shelf.height - height,
                self.page.width - right,
                shelf.y,
                shelf.next_x,
            );
            if best
                .as_ref()
                .is_none_or(|(best_rank, _, _, _)| rank < *best_rank)
            {
                best = Some((rank, shelf.next_x, shelf.y, Some(index)));
            }
        }

        let mut next_y = self.page.usable_origin_y;
        for shelf in &self.shelves {
            let Some(bottom) = shelf.y.checked_add(shelf.height) else {
                return None;
            };
            next_y = next_y.max(bottom);
        }
        let horizontal_fits = self.page.usable_origin_x <= self.page.width
            && width <= self.page.width - self.page.usable_origin_x;
        let vertical_fits = next_y <= self.page.height && height <= self.page.height - next_y;
        if horizontal_fits && vertical_fits {
            let right = self.page.usable_origin_x + width;
            let rank = (
                0,
                self.page.width - right,
                next_y,
                self.page.usable_origin_x,
            );
            if best
                .as_ref()
                .is_none_or(|(best_rank, _, _, _)| rank < *best_rank)
            {
                best = Some((rank, self.page.usable_origin_x, next_y, None));
            }
        }

        let (rank, allocation_x, allocation_y, existing_shelf) = best?;
        let right = allocation_x.checked_add(width)?;
        let mut shelves = self.shelves.clone();
        if let Some(index) = existing_shelf {
            shelves[index].next_x = right;
        } else {
            shelves.push(Shelf {
                y: allocation_y,
                height,
                next_x: right,
            });
        }
        Some(PlacementChoice {
            shelves,
            allocation_x,
            allocation_y,
            allocation_width: width,
            allocation_height: height,
            vertical_slack: rank.0,
            horizontal_slack: rank.1,
        })
    }
}

/// Plans a deterministic affinity-aware repack within the supplied fixed page set.
///
/// Pair weights are affinity benefits: two sprites on the same candidate page
/// avoid that pair's cut weight. The planner is bounded to 512 sprites, 256
/// pages, and 32,768 weighted pairs. It rejects unknown compatibility data,
/// stale baselines, arithmetic overflow, infeasible geometry, budget failures,
/// and candidates that do not strictly reduce cut weight.
pub fn plan_atlas_affinity(input: &AtlasPlanningInput<'_>) -> AtlasPlanningResult {
    if input.current_generation != input.baseline_generation {
        return declined(AtlasFallbackReason::StaleGeneration, None, None);
    }
    if input.sprites.is_empty() {
        return declined(AtlasFallbackReason::EmptyInput, None, None);
    }
    if input.sprites.len() > MAX_ATLAS_PLANNER_SPRITES {
        return declined(AtlasFallbackReason::TooManySprites, None, None);
    }
    if input.pages.is_empty() {
        return declined(AtlasFallbackReason::NoPages, None, None);
    }
    if input.pages.len() > MAX_ATLAS_PLANNER_PAGES {
        return declined(AtlasFallbackReason::TooManyPages, None, None);
    }
    if input.pair_weights.len() > MAX_ATLAS_PLANNER_PAIR_WEIGHTS {
        return declined(AtlasFallbackReason::TooManyPairWeights, None, None);
    }
    if input.baseline_placements.len() != input.sprites.len() {
        return declined(AtlasFallbackReason::MissingBaselinePlacement, None, None);
    }

    let mut sprites = BTreeMap::<u64, &AtlasSpriteDescriptor>::new();
    let mut packed_sizes = BTreeMap::<u64, (u32, u32)>::new();
    for sprite in input.sprites {
        if sprites.insert(sprite.id, sprite).is_some() {
            return declined(AtlasFallbackReason::DuplicateSpriteId, None, None);
        }
        if !sprite.compatibility.is_known() {
            return declined(AtlasFallbackReason::UnknownCompatibility, None, None);
        }
        if sprite.width == 0 || sprite.height == 0 {
            return declined(AtlasFallbackReason::InvalidSpriteExtent, None, None);
        }
        let Some(twice_padding) = sprite.padding.checked_mul(2) else {
            return declined(AtlasFallbackReason::Overflow, None, None);
        };
        let Some(padded_width) = sprite.width.checked_add(twice_padding) else {
            return declined(AtlasFallbackReason::Overflow, None, None);
        };
        let Some(padded_height) = sprite.height.checked_add(twice_padding) else {
            return declined(AtlasFallbackReason::Overflow, None, None);
        };
        packed_sizes.insert(sprite.id, (padded_width, padded_height));
    }

    let mut pages = BTreeMap::<u32, &AtlasPageExtent>::new();
    let mut existing_page_bytes = 0_u64;
    for page in input.pages {
        if pages.insert(page.id, page).is_some() {
            return declined(AtlasFallbackReason::DuplicatePageId, None, None);
        }
        if !page.compatibility.is_known() {
            return declined(AtlasFallbackReason::UnknownCompatibility, None, None);
        }
        if page.width == 0
            || page.height == 0
            || page.usable_origin_x >= page.width
            || page.usable_origin_y >= page.height
        {
            return declined(AtlasFallbackReason::InvalidPageExtent, None, None);
        }
        let expected_bytes = u64::from(page.width)
            .checked_mul(u64::from(page.height))
            .and_then(|pixels| pixels.checked_mul(ATLAS_RGBA32_BYTES_PER_PIXEL));
        let Some(expected_bytes) = expected_bytes else {
            return declined(AtlasFallbackReason::Overflow, None, None);
        };
        if page.allocation_bytes != expected_bytes {
            return declined(AtlasFallbackReason::InvalidPageAllocation, None, None);
        }
        existing_page_bytes = match existing_page_bytes.checked_add(page.allocation_bytes) {
            Some(bytes) => bytes,
            None => return declined(AtlasFallbackReason::Overflow, None, None),
        };
    }
    if existing_page_bytes > input.budget.current_device_bytes {
        return declined(AtlasFallbackReason::InvalidMemoryAccounting, None, None);
    }

    let mut baseline = BTreeMap::<u64, &AtlasBaselinePlacement>::new();
    let mut baseline_page_counts = BTreeMap::<u32, usize>::new();
    let mut baseline_rectangles = BTreeMap::<u32, Vec<(u32, u32, u32, u32)>>::new();
    for placement in input.baseline_placements {
        if baseline.insert(placement.sprite_id, placement).is_some() {
            return declined(AtlasFallbackReason::DuplicateBaselinePlacement, None, None);
        }
        let Some(sprite) = sprites.get(&placement.sprite_id) else {
            return declined(AtlasFallbackReason::UnknownPairSprite, None, None);
        };
        let Some(page) = pages.get(&placement.page_id) else {
            return declined(AtlasFallbackReason::InvalidBaselinePlacement, None, None);
        };
        if page.compatibility != sprite.compatibility {
            return declined(AtlasFallbackReason::IncompatibleBaselinePage, None, None);
        }
        if placement.width == 0 || placement.height == 0 {
            return declined(AtlasFallbackReason::InvalidBaselinePlacement, None, None);
        }
        if placement.x < placement.padding || placement.y < placement.padding {
            return declined(AtlasFallbackReason::InvalidBaselinePlacement, None, None);
        }
        let Some((allocation_x, allocation_y, allocation_width, allocation_height)) = padded_rect(
            placement.x,
            placement.y,
            placement.width,
            placement.height,
            placement.padding,
        ) else {
            return declined(AtlasFallbackReason::Overflow, None, None);
        };
        let Some(right) = allocation_x.checked_add(allocation_width) else {
            return declined(AtlasFallbackReason::Overflow, None, None);
        };
        let Some(bottom) = allocation_y.checked_add(allocation_height) else {
            return declined(AtlasFallbackReason::Overflow, None, None);
        };
        if allocation_x < page.usable_origin_x
            || allocation_y < page.usable_origin_y
            || right > page.width
            || bottom > page.height
        {
            return declined(AtlasFallbackReason::InvalidBaselinePlacement, None, None);
        }
        let rect = (allocation_x, allocation_y, right, bottom);
        let rects = baseline_rectangles.entry(placement.page_id).or_default();
        if rects.iter().any(|other| rectangles_overlap(rect, *other)) {
            return declined(
                AtlasFallbackReason::OverlappingBaselinePlacements,
                None,
                None,
            );
        }
        rects.push(rect);
        *baseline_page_counts.entry(placement.page_id).or_default() += 1;
    }
    if baseline.len() != sprites.len() {
        return declined(AtlasFallbackReason::MissingBaselinePlacement, None, None);
    }

    let mut pair_weights = Vec::<(u64, u64, u64)>::with_capacity(input.pair_weights.len());
    let mut pair_seen = BTreeSet::<(u64, u64)>::new();
    let mut pair_total_weight = 0_u64;
    let mut adjacency = BTreeMap::<u64, Vec<(u64, u64)>>::new();
    for pair in input.pair_weights {
        if pair.from_sprite_id == pair.to_sprite_id || pair.weight == 0 {
            return declined(AtlasFallbackReason::InvalidPairWeight, None, None);
        }
        if pair.weight > MAX_ATLAS_PAIR_WEIGHT {
            return declined(AtlasFallbackReason::InvalidPairWeight, None, None);
        }
        if !sprites.contains_key(&pair.from_sprite_id) || !sprites.contains_key(&pair.to_sprite_id)
        {
            return declined(AtlasFallbackReason::UnknownPairSprite, None, None);
        }
        let from = pair.from_sprite_id;
        let to = pair.to_sprite_id;
        if !pair_seen.insert((from, to)) {
            return declined(AtlasFallbackReason::DuplicatePairWeight, None, None);
        }
        pair_total_weight = match pair_total_weight.checked_add(pair.weight) {
            Some(weight) => weight,
            None => return declined(AtlasFallbackReason::Overflow, None, None),
        };
        adjacency.entry(from).or_default().push((to, pair.weight));
        adjacency.entry(to).or_default().push((from, pair.weight));
        pair_weights.push((from, to, pair.weight));
    }
    let _bounded_total_pair_weight = pair_total_weight;
    for neighbors in adjacency.values_mut() {
        neighbors.sort_unstable_by_key(|(sprite_id, _)| *sprite_id);
    }

    let baseline_cut_weight = pair_weights
        .iter()
        .try_fold(0_u64, |sum, (left, right, weight)| {
            if baseline[left].page_id == baseline[right].page_id {
                Some(sum)
            } else {
                sum.checked_add(*weight)
            }
        });
    let Some(baseline_cut_weight) = baseline_cut_weight else {
        return declined(AtlasFallbackReason::Overflow, None, None);
    };
    let baseline_metrics = AtlasMetrics {
        baseline_page_count: baseline_page_counts.len(),
        candidate_page_count: baseline_page_counts.len(),
        moved_sprite_count: 0,
        baseline_cut_weight,
        candidate_cut_weight: baseline_cut_weight,
        cut_weight_saved: 0,
        candidate_texture_bytes: baseline_page_counts
            .keys()
            .filter_map(|page_id| pages.get(page_id))
            .map(|page| page.allocation_bytes)
            .sum(),
        final_device_bytes: input.budget.current_device_bytes,
        peak_device_bytes: input.budget.current_device_bytes,
    };

    let mut sorted_pages: Vec<&AtlasPageExtent> = pages.values().copied().collect();
    sorted_pages.sort_by_key(|page| page.id);

    let sprite_ids: Vec<u64> = sprites.keys().copied().collect();
    let mut component_for_sprite = BTreeMap::<u64, usize>::new();
    let mut component_members = BTreeMap::<usize, BTreeSet<u64>>::new();
    for (component_id, sprite_id) in sprite_ids.iter().copied().enumerate() {
        component_for_sprite.insert(sprite_id, component_id);
        component_members.insert(component_id, BTreeSet::from([sprite_id]));
    }
    let mut next_component_id = sprite_ids.len();
    let mut rejected_component_merges = BTreeSet::<(usize, usize)>::new();
    let mut pack_probes = 0_usize;
    let mut merge_edges = pair_weights.clone();
    merge_edges.sort_by(|left, right| {
        right
            .2
            .cmp(&left.2)
            .then_with(|| left.0.cmp(&right.0))
            .then_with(|| left.1.cmp(&right.1))
    });

    for (from, to, _) in merge_edges {
        let from_component = component_for_sprite[&from];
        let to_component = component_for_sprite[&to];
        if from_component == to_component {
            continue;
        }
        let component_pair = if from_component < to_component {
            (from_component, to_component)
        } else {
            (to_component, from_component)
        };
        if rejected_component_merges.contains(&component_pair) {
            continue;
        }
        let from_members = &component_members[&from_component];
        let to_members = &component_members[&to_component];
        let from_key = &sprites[from_members.first().expect("nonempty cluster")].compatibility;
        let to_key = &sprites[to_members.first().expect("nonempty cluster")].compatibility;
        if from_key != to_key {
            continue;
        }
        let mut combined_members = from_members.clone();
        combined_members.extend(to_members.iter().copied());
        let can_pack = match cluster_fits_any_page(
            &combined_members,
            from_key,
            &sprites,
            &packed_sizes,
            &sorted_pages,
            &mut pack_probes,
        ) {
            Ok(can_pack) => can_pack,
            Err(reason) => {
                return declined(reason, Some(baseline_metrics), None);
            }
        };
        if !can_pack {
            rejected_component_merges.insert(component_pair);
            continue;
        }

        let merged_component = next_component_id;
        next_component_id += 1;
        component_members.remove(&from_component);
        component_members.remove(&to_component);
        for sprite_id in &combined_members {
            component_for_sprite.insert(*sprite_id, merged_component);
        }
        component_members.insert(merged_component, combined_members);
    }

    let mut clusters = BTreeMap::<usize, AffinityCluster>::new();
    for (component_id, members) in component_members {
        let Some(first_sprite_id) = members.first().copied() else {
            continue;
        };
        let compatibility = sprites[&first_sprite_id].compatibility.clone();
        let padded_area = members.iter().try_fold(0_u64, |sum, sprite_id| {
            let (width, height) = packed_sizes[sprite_id];
            sum.checked_add(u64::from(width) * u64::from(height))
        });
        let Some(padded_area) = padded_area else {
            return declined(AtlasFallbackReason::Overflow, Some(baseline_metrics), None);
        };
        clusters.insert(
            component_id,
            AffinityCluster {
                members,
                compatibility,
                strongest_edge_weight: 0,
                internal_edge_weight: 0,
                padded_area,
                first_sprite_id,
            },
        );
    }
    for (from, to, weight) in &pair_weights {
        let from_component = component_for_sprite[from];
        if from_component != component_for_sprite[to] {
            continue;
        }
        let cluster = clusters
            .get_mut(&from_component)
            .expect("every component has a cluster");
        cluster.strongest_edge_weight = cluster.strongest_edge_weight.max(*weight);
        cluster.internal_edge_weight = match cluster.internal_edge_weight.checked_add(*weight) {
            Some(total) => total,
            None => return declined(AtlasFallbackReason::Overflow, Some(baseline_metrics), None),
        };
    }

    let mut clusters: Vec<AffinityCluster> = clusters.into_values().collect();
    clusters.sort_by(|left, right| {
        right
            .strongest_edge_weight
            .cmp(&left.strongest_edge_weight)
            .then_with(|| right.internal_edge_weight.cmp(&left.internal_edge_weight))
            .then_with(|| right.padded_area.cmp(&left.padded_area))
            .then_with(|| left.first_sprite_id.cmp(&right.first_sprite_id))
    });

    let mut page_states: BTreeMap<u32, PagePackingState<'_>> = sorted_pages
        .iter()
        .map(|page| (page.id, PagePackingState::new(page)))
        .collect();
    let mut candidate_placements = BTreeMap::<u64, AtlasPlacement>::new();
    for cluster in &clusters {
        let ordered_members = ordered_cluster_members(&cluster.members, &sprites, &packed_sizes);
        let mut best: Option<PackedCluster<'_>> = None;
        for page in &sorted_pages {
            if page.compatibility != cluster.compatibility {
                continue;
            }
            let state = &page_states[&page.id];
            let candidate = match pack_cluster_on_page(
                &ordered_members,
                state,
                &candidate_placements,
                &adjacency,
                &baseline,
                &packed_sizes,
                &mut pack_probes,
            ) {
                Ok(candidate) => candidate,
                Err(reason) => {
                    return declined(reason, Some(baseline_metrics), None);
                }
            };
            let Some(candidate) = candidate else {
                continue;
            };
            let should_replace = best.as_ref().is_none_or(|current| {
                candidate.external_affinity > current.external_affinity
                    || (candidate.external_affinity == current.external_affinity
                        && (candidate.baseline_page_matches > current.baseline_page_matches
                            || (candidate.baseline_page_matches == current.baseline_page_matches
                                && (candidate.fit_slack < current.fit_slack
                                    || (candidate.fit_slack == current.fit_slack
                                        && candidate.state.page.id < current.state.page.id)))))
            });
            if should_replace {
                best = Some(candidate);
            }
        }
        let Some(best) = best else {
            return declined(
                AtlasFallbackReason::NoFeasiblePlacement,
                Some(baseline_metrics),
                None,
            );
        };
        let page_id = best.state.page.id;
        page_states.insert(page_id, best.state);
        for placement in best.placements {
            candidate_placements.insert(placement.sprite_id, placement);
        }
    }

    let candidate_cut_weight = pair_weights
        .iter()
        .try_fold(0_u64, |sum, (left, right, weight)| {
            if candidate_placements[left].page_id == candidate_placements[right].page_id {
                Some(sum)
            } else {
                sum.checked_add(*weight)
            }
        });
    let Some(candidate_cut_weight) = candidate_cut_weight else {
        return declined(AtlasFallbackReason::Overflow, Some(baseline_metrics), None);
    };

    let mut used_page_ids = BTreeSet::new();
    let mut candidate_sprite_counts = BTreeMap::<u32, usize>::new();
    let mut moved_sprite_count = 0_usize;
    for (sprite_id, placement) in &candidate_placements {
        used_page_ids.insert(placement.page_id);
        *candidate_sprite_counts
            .entry(placement.page_id)
            .or_default() += 1;
        let previous = baseline[sprite_id];
        if previous.page_id != placement.page_id
            || previous.x != placement.x
            || previous.y != placement.y
            || previous.width != placement.width
            || previous.height != placement.height
        {
            moved_sprite_count += 1;
        }
    }

    let mut planned_pages = Vec::with_capacity(used_page_ids.len());
    let mut candidate_texture_bytes = 0_u64;
    for page_id in used_page_ids {
        let page = pages[&page_id];
        candidate_texture_bytes = match candidate_texture_bytes.checked_add(page.allocation_bytes) {
            Some(bytes) => bytes,
            None => return declined(AtlasFallbackReason::Overflow, Some(baseline_metrics), None),
        };
        planned_pages.push(AtlasPlannedPage {
            id: page.id,
            compatibility: page.compatibility.clone(),
            width: page.width,
            height: page.height,
            usable_origin_x: page.usable_origin_x,
            usable_origin_y: page.usable_origin_y,
            allocation_bytes: page.allocation_bytes,
            sprite_count: candidate_sprite_counts[&page_id],
        });
    }
    let final_device_bytes = match input
        .budget
        .current_device_bytes
        .checked_sub(existing_page_bytes)
        .and_then(|unaffected| unaffected.checked_add(candidate_texture_bytes))
    {
        Some(bytes) => bytes,
        None => return declined(AtlasFallbackReason::Overflow, Some(baseline_metrics), None),
    };
    let peak_device_bytes = match input
        .budget
        .current_device_bytes
        .checked_add(candidate_texture_bytes)
    {
        Some(bytes) => bytes,
        None => return declined(AtlasFallbackReason::Overflow, Some(baseline_metrics), None),
    };
    let candidate_metrics = AtlasMetrics {
        baseline_page_count: baseline_page_counts.len(),
        candidate_page_count: planned_pages.len(),
        moved_sprite_count,
        baseline_cut_weight,
        candidate_cut_weight,
        cut_weight_saved: baseline_cut_weight.saturating_sub(candidate_cut_weight),
        candidate_texture_bytes,
        final_device_bytes,
        peak_device_bytes,
    };
    if final_device_bytes > input.budget.max_final_bytes
        || peak_device_bytes > input.budget.max_peak_bytes
    {
        return declined(
            AtlasFallbackReason::MemoryBudgetExceeded,
            Some(baseline_metrics),
            Some(candidate_metrics),
        );
    }
    if candidate_cut_weight >= baseline_cut_weight {
        return declined(
            AtlasFallbackReason::NonImproving,
            Some(baseline_metrics),
            Some(candidate_metrics),
        );
    }

    AtlasPlanningResult {
        plan: Some(AtlasPlan {
            placements: candidate_placements.into_values().collect(),
            pages: planned_pages,
            metrics: candidate_metrics,
        }),
        baseline_metrics: Some(baseline_metrics),
        candidate_metrics: Some(candidate_metrics),
        fallback_reason: None,
    }
}

fn counted_try_place(
    state: &PagePackingState<'_>,
    size: (u32, u32),
    pack_probes: &mut usize,
) -> Result<Option<PlacementChoice>, AtlasFallbackReason> {
    if *pack_probes >= MAX_ATLAS_CLUSTER_PACK_PROBES {
        return Err(AtlasFallbackReason::ClusterSearchLimitExceeded);
    }
    *pack_probes += 1;
    Ok(state.try_place(size.0, size.1))
}

fn ordered_cluster_members<'a>(
    member_ids: &BTreeSet<u64>,
    sprites: &BTreeMap<u64, &'a AtlasSpriteDescriptor>,
    packed_sizes: &BTreeMap<u64, (u32, u32)>,
) -> Vec<&'a AtlasSpriteDescriptor> {
    let mut members: Vec<_> = member_ids
        .iter()
        .map(|sprite_id| sprites[sprite_id])
        .collect();
    members.sort_by(|left, right| {
        let (left_width, left_height) = packed_sizes[&left.id];
        let (right_width, right_height) = packed_sizes[&right.id];
        let left_area = u64::from(left_width) * u64::from(left_height);
        let right_area = u64::from(right_width) * u64::from(right_height);
        right_area
            .cmp(&left_area)
            .then_with(|| right_height.cmp(&left_height))
            .then_with(|| right_width.cmp(&left_width))
            .then_with(|| left.id.cmp(&right.id))
    });
    members
}

fn cluster_fits_any_page(
    member_ids: &BTreeSet<u64>,
    compatibility: &AtlasCompatibilityKey,
    sprites: &BTreeMap<u64, &AtlasSpriteDescriptor>,
    packed_sizes: &BTreeMap<u64, (u32, u32)>,
    pages: &[&AtlasPageExtent],
    pack_probes: &mut usize,
) -> Result<bool, AtlasFallbackReason> {
    let members = ordered_cluster_members(member_ids, sprites, packed_sizes);
    for page in pages
        .iter()
        .filter(|page| page.compatibility == *compatibility)
    {
        let mut state = PagePackingState::new(page);
        let mut fits = true;
        for sprite in &members {
            let Some(choice) = counted_try_place(&state, packed_sizes[&sprite.id], pack_probes)?
            else {
                fits = false;
                break;
            };
            state.shelves = choice.shelves;
        }
        if fits {
            return Ok(true);
        }
    }
    Ok(false)
}

fn pack_cluster_on_page<'a>(
    members: &[&AtlasSpriteDescriptor],
    current_state: &PagePackingState<'a>,
    assigned: &BTreeMap<u64, AtlasPlacement>,
    adjacency: &BTreeMap<u64, Vec<(u64, u64)>>,
    baseline: &BTreeMap<u64, &AtlasBaselinePlacement>,
    packed_sizes: &BTreeMap<u64, (u32, u32)>,
    pack_probes: &mut usize,
) -> Result<Option<PackedCluster<'a>>, AtlasFallbackReason> {
    let mut state = current_state.clone();
    let page_id = state.page.id;
    let mut placements = Vec::with_capacity(members.len());
    let mut fit_slack = 0_u64;
    for sprite in members {
        let Some(choice) = counted_try_place(&state, packed_sizes[&sprite.id], pack_probes)? else {
            return Ok(None);
        };
        let Some(x) = choice.allocation_x.checked_add(sprite.padding) else {
            return Err(AtlasFallbackReason::Overflow);
        };
        let Some(y) = choice.allocation_y.checked_add(sprite.padding) else {
            return Err(AtlasFallbackReason::Overflow);
        };
        fit_slack = fit_slack
            .checked_add(u64::from(choice.vertical_slack))
            .and_then(|slack| slack.checked_add(u64::from(choice.horizontal_slack)))
            .ok_or(AtlasFallbackReason::Overflow)?;
        state.shelves = choice.shelves;
        placements.push(AtlasPlacement {
            sprite_id: sprite.id,
            page_id,
            x,
            y,
            width: sprite.width,
            height: sprite.height,
            allocation_x: choice.allocation_x,
            allocation_y: choice.allocation_y,
            allocation_width: choice.allocation_width,
            allocation_height: choice.allocation_height,
        });
    }

    let mut external_affinity = 0_u64;
    for sprite in members {
        for (neighbor_id, weight) in adjacency.get(&sprite.id).into_iter().flatten() {
            if assigned
                .get(neighbor_id)
                .is_some_and(|placement| placement.page_id == page_id)
            {
                external_affinity = external_affinity
                    .checked_add(*weight)
                    .ok_or(AtlasFallbackReason::Overflow)?;
            }
        }
    }
    let baseline_page_matches = members
        .iter()
        .filter(|sprite| baseline[&sprite.id].page_id == page_id)
        .count();
    Ok(Some(PackedCluster {
        state,
        placements,
        external_affinity,
        baseline_page_matches,
        fit_slack,
    }))
}

fn padded_rect(
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    padding: u32,
) -> Option<(u32, u32, u32, u32)> {
    let twice_padding = padding.checked_mul(2)?;
    let allocation_x = x.checked_sub(padding)?;
    let allocation_y = y.checked_sub(padding)?;
    let allocation_width = width.checked_add(twice_padding)?;
    let allocation_height = height.checked_add(twice_padding)?;
    Some((
        allocation_x,
        allocation_y,
        allocation_width,
        allocation_height,
    ))
}

fn rectangles_overlap(left: (u32, u32, u32, u32), right: (u32, u32, u32, u32)) -> bool {
    left.0 < right.2 && right.0 < left.2 && left.1 < right.3 && right.1 < left.3
}

fn declined(
    reason: AtlasFallbackReason,
    baseline_metrics: Option<AtlasMetrics>,
    candidate_metrics: Option<AtlasMetrics>,
) -> AtlasPlanningResult {
    AtlasPlanningResult {
        plan: None,
        baseline_metrics,
        candidate_metrics,
        fallback_reason: Some(reason),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(group_id: u64) -> AtlasCompatibilityKey {
        AtlasCompatibilityKey {
            group_id: Some(group_id),
            format: Some(1),
            sampler: Some(2),
            color_space: Some(3),
            backend: Some(4),
        }
    }

    fn sprite(id: u64, group_id: u64, width: u32, height: u32) -> AtlasSpriteDescriptor {
        AtlasSpriteDescriptor {
            id,
            width,
            height,
            padding: 1,
            compatibility: key(group_id),
        }
    }

    fn page(id: u32, group_id: u64, width: u32, height: u32) -> AtlasPageExtent {
        AtlasPageExtent {
            id,
            compatibility: key(group_id),
            width,
            height,
            usable_origin_x: 1,
            usable_origin_y: 6,
            allocation_bytes: u64::from(width) * u64::from(height) * 4,
        }
    }

    fn placement(
        sprite_id: u64,
        page_id: u32,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    ) -> AtlasBaselinePlacement {
        AtlasBaselinePlacement {
            sprite_id,
            page_id,
            x,
            y,
            width,
            height,
            padding: 1,
        }
    }

    fn input<'a>(
        sprites: &'a [AtlasSpriteDescriptor],
        pages: &'a [AtlasPageExtent],
        baseline_placements: &'a [AtlasBaselinePlacement],
        pair_weights: &'a [AtlasPairWeight],
    ) -> AtlasPlanningInput<'a> {
        let current_page_bytes = pages.iter().map(|page| page.allocation_bytes).sum::<u64>();
        AtlasPlanningInput {
            current_generation: 7,
            baseline_generation: 7,
            sprites,
            pages,
            baseline_placements,
            pair_weights,
            budget: AtlasMemoryBudget {
                current_device_bytes: current_page_bytes,
                max_final_bytes: current_page_bytes,
                max_peak_bytes: current_page_bytes * 2,
            },
        }
    }

    fn fallback(result: &AtlasPlanningResult) -> AtlasFallbackReason {
        result.fallback_reason.expect("fallback reason")
    }

    #[test]
    fn high_affinity_pair_moves_to_one_existing_page() {
        let sprites = [sprite(10, 1, 8, 8), sprite(20, 1, 8, 8)];
        let pages = [page(3, 1, 32, 32), page(8, 1, 32, 32)];
        let baseline = [placement(10, 3, 2, 7, 8, 8), placement(20, 8, 2, 7, 8, 8)];
        let weights = [AtlasPairWeight {
            from_sprite_id: 10,
            to_sprite_id: 20,
            weight: 90,
        }];
        let result = plan_atlas_affinity(&input(&sprites, &pages, &baseline, &weights));
        let plan = result.plan.expect("affinity improvement");
        assert_eq!(plan.placements.len(), 2);
        assert_eq!(plan.placements[0].page_id, plan.placements[1].page_id);
        assert_eq!(plan.metrics.baseline_cut_weight, 90);
        assert_eq!(plan.metrics.candidate_cut_weight, 0);
        assert_eq!(plan.metrics.candidate_page_count, 1);
        assert_eq!(plan.pages[0].id, 3);
    }

    #[test]
    fn strongest_pair_cluster_is_reserved_before_weak_star_fillers() {
        let mut a = sprite(1, 1, 18, 8);
        let mut b = sprite(2, 1, 18, 8);
        let mut c = sprite(3, 1, 6, 6);
        let mut d = sprite(4, 1, 6, 6);
        let mut e = sprite(5, 1, 6, 6);
        let mut f = sprite(6, 1, 6, 6);
        for item in [&mut a, &mut b, &mut c, &mut d, &mut e, &mut f] {
            item.padding = 1;
        }
        let sprites = [a, b, c, d, e, f];
        let pages = [page(1, 1, 42, 16), page(2, 1, 24, 22)];
        let baseline = [
            placement(1, 1, 2, 7, 1, 1),
            placement(2, 2, 2, 7, 1, 1),
            placement(3, 1, 5, 7, 1, 1),
            placement(4, 2, 5, 7, 1, 1),
            placement(5, 2, 8, 7, 1, 1),
            placement(6, 2, 11, 7, 1, 1),
        ];
        let weights = [
            AtlasPairWeight {
                from_sprite_id: 1,
                to_sprite_id: 2,
                weight: 100,
            },
            AtlasPairWeight {
                from_sprite_id: 3,
                to_sprite_id: 4,
                weight: 40,
            },
            AtlasPairWeight {
                from_sprite_id: 3,
                to_sprite_id: 5,
                weight: 40,
            },
            AtlasPairWeight {
                from_sprite_id: 3,
                to_sprite_id: 6,
                weight: 40,
            },
        ];
        let result = plan_atlas_affinity(&input(&sprites, &pages, &baseline, &weights));
        let plan = result
            .plan
            .expect("strong pair and weak star both fit separately");
        let placements: BTreeMap<_, _> = plan
            .placements
            .iter()
            .map(|placement| (placement.sprite_id, placement.page_id))
            .collect();
        assert_eq!(placements[&1], placements[&2]);
        assert_ne!(placements[&1], placements[&3]);
        assert_eq!(placements[&3], placements[&4]);
        assert_eq!(placements[&3], placements[&5]);
        assert_eq!(placements[&3], placements[&6]);
        assert_eq!(plan.metrics.baseline_cut_weight, 220);
        assert_eq!(plan.metrics.candidate_cut_weight, 0);
    }

    #[test]
    fn affinity_clusters_merge_only_when_the_padded_union_fits_a_page() {
        let sprites = [sprite(1, 1, 22, 14), sprite(2, 1, 22, 14)];
        let pages = [page(1, 1, 32, 32), page(2, 1, 32, 32)];
        let baseline = [placement(1, 1, 2, 7, 22, 14), placement(2, 2, 2, 7, 22, 14)];
        let weights = [AtlasPairWeight {
            from_sprite_id: 1,
            to_sprite_id: 2,
            weight: 50,
        }];
        let result = plan_atlas_affinity(&input(&sprites, &pages, &baseline, &weights));
        assert_eq!(fallback(&result), AtlasFallbackReason::NonImproving);
        assert_eq!(
            result
                .candidate_metrics
                .expect("both singleton placements fit")
                .candidate_cut_weight,
            50
        );
    }

    #[test]
    fn five_hundred_twelve_sprites_spill_across_fixed_pages() {
        let sprites: Vec<_> = (0..512).map(|id| sprite(id, 1, 2, 2)).collect();
        let pages: Vec<_> = (0..16).map(|id| page(id, 1, 32, 32)).collect();
        let baseline: Vec<_> = (0..512)
            .map(|id| {
                let page_id = (id % 16) as u32;
                let slot = id / 16;
                let x = 2 + (slot % 7) as u32 * 4;
                let y = 7 + (slot / 7) as u32 * 4;
                placement(id, page_id, x, y, 2, 2)
            })
            .collect();
        let weights: Vec<_> = (0..256)
            .map(|pair| AtlasPairWeight {
                from_sprite_id: pair * 2,
                to_sprite_id: pair * 2 + 1,
                weight: 1,
            })
            .collect();
        let result = plan_atlas_affinity(&input(&sprites, &pages, &baseline, &weights));
        let plan = result.plan.expect("bounded 512-item plan");
        assert_eq!(plan.placements.len(), 512);
        assert!(plan.pages.len() > 1);
        assert!(plan.pages.len() <= pages.len());
        assert!(plan.metrics.candidate_cut_weight < plan.metrics.baseline_cut_weight);
    }

    #[test]
    fn sprite_count_above_the_bound_declines_before_planning() {
        let sprites: Vec<_> = (0..=MAX_ATLAS_PLANNER_SPRITES as u64)
            .map(|id| sprite(id, 1, 1, 1))
            .collect();
        let result = plan_atlas_affinity(&AtlasPlanningInput {
            current_generation: 1,
            baseline_generation: 1,
            sprites: &sprites,
            pages: &[],
            baseline_placements: &[],
            pair_weights: &[],
            budget: AtlasMemoryBudget {
                current_device_bytes: 0,
                max_final_bytes: 0,
                max_peak_bytes: 0,
            },
        });
        assert_eq!(fallback(&result), AtlasFallbackReason::TooManySprites);
    }

    #[test]
    fn oversized_realized_sprite_declines_when_padded_extent_cannot_fit() {
        let sprites = [sprite(1, 1, 40, 20)];
        let pages = [page(1, 1, 32, 32)];
        let baseline = [placement(1, 1, 2, 7, 8, 8)];
        let result = plan_atlas_affinity(&input(&sprites, &pages, &baseline, &[]));
        assert_eq!(fallback(&result), AtlasFallbackReason::NoFeasiblePlacement);
        assert!(result.plan.is_none());
    }

    #[test]
    fn deterministic_ties_ignore_input_slice_order() {
        let sprites = [sprite(4, 1, 8, 8), sprite(2, 1, 8, 8)];
        let pages = [page(9, 1, 32, 32), page(5, 1, 32, 32)];
        let baseline = [placement(4, 9, 2, 7, 8, 8), placement(2, 5, 2, 7, 8, 8)];
        let weights = [AtlasPairWeight {
            from_sprite_id: 2,
            to_sprite_id: 4,
            weight: 5,
        }];
        let forward = plan_atlas_affinity(&input(&sprites, &pages, &baseline, &weights));
        let reverse_sprites = [sprites[1].clone(), sprites[0].clone()];
        let reverse_pages = [pages[1].clone(), pages[0].clone()];
        let reverse_baseline = [baseline[1].clone(), baseline[0].clone()];
        let reverse = plan_atlas_affinity(&input(
            &reverse_sprites,
            &reverse_pages,
            &reverse_baseline,
            &weights,
        ));
        assert_eq!(forward, reverse);
        assert!(forward.plan.is_some());
    }

    #[test]
    fn incompatible_groups_never_share_a_page() {
        let sprites = [sprite(1, 1, 8, 8), sprite(2, 2, 8, 8)];
        let pages = [page(1, 1, 32, 32), page(2, 2, 32, 32)];
        let baseline = [placement(1, 1, 2, 7, 8, 8), placement(2, 2, 2, 7, 8, 8)];
        let weights = [AtlasPairWeight {
            from_sprite_id: 1,
            to_sprite_id: 2,
            weight: 25,
        }];
        let result = plan_atlas_affinity(&input(&sprites, &pages, &baseline, &weights));
        assert_eq!(fallback(&result), AtlasFallbackReason::NonImproving);
        assert_eq!(
            result
                .candidate_metrics
                .expect("complete candidate")
                .candidate_cut_weight,
            25
        );
    }

    #[test]
    fn unknown_compatibility_and_stale_reload_generation_decline() {
        let mut sprites = [sprite(1, 1, 8, 8)];
        let pages = [page(1, 1, 32, 32)];
        let baseline = [placement(1, 1, 2, 7, 8, 8)];
        let mut request = input(&sprites, &pages, &baseline, &[]);
        request.baseline_generation = 6;
        assert_eq!(
            fallback(&plan_atlas_affinity(&request)),
            AtlasFallbackReason::StaleGeneration
        );

        sprites[0].compatibility.backend = None;
        let request = input(&sprites, &pages, &baseline, &[]);
        assert_eq!(
            fallback(&plan_atlas_affinity(&request)),
            AtlasFallbackReason::UnknownCompatibility
        );
    }

    #[test]
    fn padded_rectangles_respect_native_header_origin() {
        let sprites = [sprite(1, 1, 7, 4), sprite(2, 1, 7, 4)];
        let pages = [page(1, 1, 19, 12), page(2, 1, 19, 12)];
        let baseline = [placement(1, 1, 2, 7, 7, 4), placement(2, 2, 2, 7, 7, 4)];
        let weights = [AtlasPairWeight {
            from_sprite_id: 1,
            to_sprite_id: 2,
            weight: 1,
        }];
        let result = plan_atlas_affinity(&input(&sprites, &pages, &baseline, &weights));
        let plan = result.plan.expect("padded page fits exactly below header");
        assert_eq!(plan.placements[0].allocation_x, 1);
        assert_eq!(plan.placements[0].allocation_y, 6);
        assert_eq!(plan.placements[0].x, 2);
        assert_eq!(plan.placements[0].y, 7);

        let too_narrow = [page(1, 1, 8, 12), page(2, 1, 8, 12)];
        let rejected = plan_atlas_affinity(&input(&sprites, &too_narrow, &baseline, &weights));
        assert_eq!(
            fallback(&rejected),
            AtlasFallbackReason::InvalidBaselinePlacement
        );
    }

    #[test]
    fn inconsistent_existing_page_allocation_declines() {
        let sprites = [sprite(1, 1, 8, 8)];
        let mut pages = [page(1, 1, 32, 32)];
        pages[0].allocation_bytes -= 4;
        let baseline = [placement(1, 1, 2, 7, 8, 8)];
        let result = plan_atlas_affinity(&input(&sprites, &pages, &baseline, &[]));
        assert_eq!(
            fallback(&result),
            AtlasFallbackReason::InvalidPageAllocation
        );
    }

    #[test]
    fn peak_memory_budget_declines_complete_candidate() {
        let sprites = [sprite(1, 1, 8, 8), sprite(2, 1, 8, 8)];
        let pages = [page(1, 1, 32, 32), page(2, 1, 32, 32)];
        let baseline = [placement(1, 1, 2, 7, 8, 8), placement(2, 2, 2, 7, 8, 8)];
        let weights = [AtlasPairWeight {
            from_sprite_id: 1,
            to_sprite_id: 2,
            weight: 10,
        }];
        let mut request = input(&sprites, &pages, &baseline, &weights);
        request.budget.max_peak_bytes = request.budget.current_device_bytes + 4095;
        let peak_result = plan_atlas_affinity(&request);
        assert_eq!(
            fallback(&peak_result),
            AtlasFallbackReason::MemoryBudgetExceeded
        );
        assert!(peak_result.plan.is_none());
        assert!(peak_result.candidate_metrics.is_some());

        request.budget.max_peak_bytes = u64::MAX;
        request.budget.max_final_bytes = 4095;
        let final_result = plan_atlas_affinity(&request);
        assert_eq!(
            fallback(&final_result),
            AtlasFallbackReason::MemoryBudgetExceeded
        );
    }

    #[test]
    fn dimension_arithmetic_overflow_declines() {
        let sprites = [sprite(1, 1, u32::MAX, 8)];
        let pages = [page(1, 1, 32, 32)];
        let baseline = [placement(1, 1, 2, 7, 8, 8)];
        let result = plan_atlas_affinity(&input(&sprites, &pages, &baseline, &[]));
        assert_eq!(fallback(&result), AtlasFallbackReason::Overflow);
    }

    #[test]
    fn pair_weight_bounds_and_duplicate_pairs_fail_closed() {
        let sprites = [sprite(1, 1, 8, 8), sprite(2, 1, 8, 8)];
        let pages = [page(1, 1, 32, 32), page(2, 1, 32, 32)];
        let baseline = [placement(1, 1, 2, 7, 8, 8), placement(2, 2, 2, 7, 8, 8)];
        let too_heavy = [AtlasPairWeight {
            from_sprite_id: 1,
            to_sprite_id: 2,
            weight: MAX_ATLAS_PAIR_WEIGHT + 1,
        }];
        assert_eq!(
            fallback(&plan_atlas_affinity(&input(
                &sprites, &pages, &baseline, &too_heavy
            ))),
            AtlasFallbackReason::InvalidPairWeight
        );
        let reverse_edges = [
            AtlasPairWeight {
                from_sprite_id: 1,
                to_sprite_id: 2,
                weight: 1,
            },
            AtlasPairWeight {
                from_sprite_id: 2,
                to_sprite_id: 1,
                weight: 2,
            },
        ];
        let result = plan_atlas_affinity(&input(&sprites, &pages, &baseline, &reverse_edges));
        let plan = result.plan.expect("independent directed edges");
        assert_eq!(plan.metrics.baseline_cut_weight, 3);
        assert_eq!(plan.metrics.candidate_cut_weight, 0);

        let duplicate = [
            AtlasPairWeight {
                from_sprite_id: 1,
                to_sprite_id: 2,
                weight: 1,
            },
            AtlasPairWeight {
                from_sprite_id: 1,
                to_sprite_id: 2,
                weight: 2,
            },
        ];
        assert_eq!(
            fallback(&plan_atlas_affinity(&input(
                &sprites, &pages, &baseline, &duplicate
            ))),
            AtlasFallbackReason::DuplicatePairWeight
        );
    }
    #[test]
    fn asset_major_aaa_bbb_layout_keeps_an_already_optimal_baseline() {
        let sprites = [
            sprite(1, 1, 8, 8),
            sprite(2, 1, 8, 8),
            sprite(3, 1, 8, 8),
            sprite(4, 1, 8, 8),
            sprite(5, 1, 8, 8),
            sprite(6, 1, 8, 8),
        ];
        let pages = [page(3, 1, 64, 32), page(8, 1, 64, 32)];
        let baseline = [
            placement(1, 3, 2, 7, 8, 8),
            placement(2, 3, 14, 7, 8, 8),
            placement(3, 3, 26, 7, 8, 8),
            placement(4, 8, 2, 7, 8, 8),
            placement(5, 8, 14, 7, 8, 8),
            placement(6, 8, 26, 7, 8, 8),
        ];
        let baseline_before = baseline.clone();
        let weights = [
            AtlasPairWeight {
                from_sprite_id: 1,
                to_sprite_id: 2,
                weight: 10,
            },
            AtlasPairWeight {
                from_sprite_id: 2,
                to_sprite_id: 3,
                weight: 10,
            },
            AtlasPairWeight {
                from_sprite_id: 4,
                to_sprite_id: 5,
                weight: 10,
            },
            AtlasPairWeight {
                from_sprite_id: 5,
                to_sprite_id: 6,
                weight: 10,
            },
        ];

        let result = plan_atlas_affinity(&input(&sprites, &pages, &baseline, &weights));

        assert_eq!(fallback(&result), AtlasFallbackReason::NonImproving);
        assert!(result.plan.is_none());
        assert_eq!(
            result
                .baseline_metrics
                .as_ref()
                .expect("baseline metrics")
                .baseline_cut_weight,
            0
        );
        assert_eq!(
            result
                .candidate_metrics
                .as_ref()
                .expect("complete candidate metrics")
                .candidate_cut_weight,
            0
        );
        assert_eq!(baseline, baseline_before);
    }

    #[test]
    fn mixed_dimensions_and_tied_weights_produce_identical_reordered_plans() {
        let sprites = [
            sprite(1, 1, 12, 6),
            sprite(2, 1, 6, 12),
            sprite(3, 1, 8, 8),
            sprite(4, 1, 4, 4),
        ];
        let pages = [page(19, 1, 32, 32), page(4, 1, 32, 32)];
        let baseline = [
            placement(1, 19, 2, 7, 12, 6),
            placement(2, 4, 2, 7, 6, 12),
            placement(3, 19, 16, 7, 8, 8),
            placement(4, 4, 10, 7, 4, 4),
        ];
        let weights = [
            AtlasPairWeight {
                from_sprite_id: 1,
                to_sprite_id: 2,
                weight: 17,
            },
            AtlasPairWeight {
                from_sprite_id: 3,
                to_sprite_id: 4,
                weight: 17,
            },
        ];
        let forward = plan_atlas_affinity(&input(&sprites, &pages, &baseline, &weights));
        let reverse_sprites = [
            sprites[3].clone(),
            sprites[2].clone(),
            sprites[1].clone(),
            sprites[0].clone(),
        ];
        let reverse_pages = [pages[1].clone(), pages[0].clone()];
        let reverse_baseline = [
            baseline[3].clone(),
            baseline[2].clone(),
            baseline[1].clone(),
            baseline[0].clone(),
        ];
        let reverse_weights = [weights[1].clone(), weights[0].clone()];
        let reverse = plan_atlas_affinity(&input(
            &reverse_sprites,
            &reverse_pages,
            &reverse_baseline,
            &reverse_weights,
        ));

        assert_eq!(forward, reverse);
        let plan = forward.plan.expect("both tied affinity pairs fit");
        assert_eq!(plan.metrics.baseline_cut_weight, 34);
        assert_eq!(plan.metrics.candidate_cut_weight, 0);
        let page_by_sprite = plan
            .placements
            .iter()
            .map(|item| (item.sprite_id, item.page_id))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(page_by_sprite[&1], page_by_sprite[&2]);
        assert_eq!(page_by_sprite[&3], page_by_sprite[&4]);
    }

    #[test]
    fn format_and_sampler_mismatches_preserve_the_uncuttable_baseline() {
        let mut format_mismatch = key(1);
        format_mismatch.format = Some(9);
        let mut sampler_mismatch = key(1);
        sampler_mismatch.sampler = Some(9);

        for (label, incompatible_key) in
            [("format", format_mismatch), ("sampler", sampler_mismatch)]
        {
            let mut sprites = [sprite(1, 1, 8, 8), sprite(2, 1, 8, 8)];
            sprites[1].compatibility = incompatible_key.clone();
            let mut pages = [page(3, 1, 32, 32), page(8, 1, 32, 32)];
            pages[1].compatibility = incompatible_key;
            let baseline = [placement(1, 3, 2, 7, 8, 8), placement(2, 8, 2, 7, 8, 8)];
            let baseline_before = baseline.clone();
            let weights = [AtlasPairWeight {
                from_sprite_id: 1,
                to_sprite_id: 2,
                weight: 29,
            }];

            let result = plan_atlas_affinity(&input(&sprites, &pages, &baseline, &weights));

            assert_eq!(
                fallback(&result),
                AtlasFallbackReason::NonImproving,
                "{label}"
            );
            assert!(result.plan.is_none(), "{label}");
            assert_eq!(
                result
                    .baseline_metrics
                    .as_ref()
                    .expect("baseline metrics")
                    .baseline_cut_weight,
                29,
                "{label}"
            );
            assert_eq!(
                result
                    .candidate_metrics
                    .as_ref()
                    .expect("complete candidate metrics")
                    .candidate_cut_weight,
                29,
                "{label}"
            );
            assert_eq!(baseline, baseline_before, "{label}");
        }
    }

    #[test]
    fn fragmented_shapes_keep_baseline_when_high_affinity_pair_cannot_fit_together() {
        let sprites = [sprite(1, 1, 24, 6), sprite(2, 1, 6, 24)];
        let pages = [page(3, 1, 32, 32), page(8, 1, 32, 32)];
        let baseline = [placement(1, 3, 2, 7, 24, 6), placement(2, 8, 2, 7, 6, 24)];
        let baseline_before = baseline.clone();
        let weights = [AtlasPairWeight {
            from_sprite_id: 1,
            to_sprite_id: 2,
            weight: 73,
        }];
        let padded_area = (24 + 2 * sprites[0].padding) * (6 + 2 * sprites[0].padding)
            + (6 + 2 * sprites[1].padding) * (24 + 2 * sprites[1].padding);
        let usable_page_area = (32 - 1) * (32 - 6);
        assert!(padded_area < usable_page_area);

        let result = plan_atlas_affinity(&input(&sprites, &pages, &baseline, &weights));

        assert_eq!(fallback(&result), AtlasFallbackReason::NonImproving);
        assert!(result.plan.is_none());
        assert_eq!(
            result
                .baseline_metrics
                .as_ref()
                .expect("baseline metrics")
                .baseline_cut_weight,
            73
        );
        assert_eq!(
            result
                .candidate_metrics
                .as_ref()
                .expect("both sprites fit separately")
                .candidate_cut_weight,
            73
        );
        assert_eq!(baseline, baseline_before);
    }
}
