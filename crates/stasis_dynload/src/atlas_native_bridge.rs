//! Optional, versioned SDL atlas inventory. An absent native export keeps conventional placement.
use std::ffi::c_void;

pub const NATIVE_ATLAS_MAX_SPRITES: usize = 65_536;
pub const NATIVE_ATLAS_MAX_PAIRS: usize = 8_192;
pub const NATIVE_ATLAS_MAX_PAGES: usize = 256;
pub const NATIVE_ATLAS_PAIR_VALID: u32 = 1;
pub const NATIVE_ATLAS_PAIR_OVERFLOW: u32 = 1 << 1;
pub const NATIVE_ATLAS_INVENTORY_OVERFLOW: u32 = 1 << 2;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct NativeAtlasResidentV1 {
    pub handle: i32,
    pub width: u32,
    pub height: u32,
    pub logical_width: u32,
    pub logical_height: u32,
    pub page_index: u32,
    pub x: u32,
    pub y: u32,
    pub allocation_width: u32,
    pub allocation_height: u32,
    pub padding: u32,
    pub flags: u32,
    pub group_id: u64,
    pub logical_pixel_area: u64,
    pub member_count: u32,
    pub max_logical_width: u32,
    pub max_logical_height: u32,
    pub normalized_path_hash: u64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct NativeAtlasPageV1 {
    pub page_index: u32,
    pub width: u32,
    pub height: u32,
    pub usable_x: u32,
    pub usable_y: u32,
    pub padding: u32,
    pub reserved_header_height: u32,
    pub flags: u32,
    pub compatibility_flags: u32,
    pub group_id: u64,
    pub allocation_bytes: u64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct NativeAtlasPairV1 {
    pub from_handle: i32,
    pub to_handle: i32,
    pub weight: u64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct NativeAtlasPlanPageV1 {
    pub source_page_index: u32,
    pub width: u32,
    pub height: u32,
    pub usable_x: u32,
    pub usable_y: u32,
    pub padding: u32,
    pub reserved_header_height: u32,
    pub flags: u32,
    pub compatibility_flags: u32,
    pub group_id: u64,
    pub allocation_bytes: u64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct NativeAtlasPlacementV1 {
    pub handle: i32,
    pub page_index: u32,
    pub x: u32,
    pub y: u32,
}

#[derive(Debug, Clone)]
pub struct NativeAtlasSnapshotV1 {
    pub token: u64,
    pub renderer_generation: u32,
    pub asset_generation: u64,
    pub flags: u32,
    pub stage_peak_cap_bytes: u64,
    pub sprites: Vec<NativeAtlasResidentV1>,
    pub pages: Vec<NativeAtlasPageV1>,
    pub pairs: Vec<NativeAtlasPairV1>,
}

#[cfg(windows)]
pub type NativeAtlasQueryV1 = unsafe extern "system" fn(
    *mut u64,
    *mut u32,
    *mut u64,
    *mut u32,
    *mut u64,
    *mut NativeAtlasResidentV1,
    u32,
    *mut u32,
    *mut NativeAtlasPageV1,
    u32,
    *mut u32,
    *mut NativeAtlasPairV1,
    u32,
    *mut u32,
) -> i32;
#[cfg(not(windows))]
pub type NativeAtlasQueryV1 = unsafe extern "C" fn(
    *mut u64,
    *mut u32,
    *mut u64,
    *mut u32,
    *mut u64,
    *mut NativeAtlasResidentV1,
    u32,
    *mut u32,
    *mut NativeAtlasPageV1,
    u32,
    *mut u32,
    *mut NativeAtlasPairV1,
    u32,
    *mut u32,
) -> i32;

#[cfg(windows)]
pub type NativeAtlasStageV1 = unsafe extern "system" fn(
    u64,
    *const NativeAtlasPlanPageV1,
    u32,
    *const NativeAtlasPlacementV1,
    u32,
) -> i32;
#[cfg(not(windows))]
pub type NativeAtlasStageV1 = unsafe extern "C" fn(
    u64,
    *const NativeAtlasPlanPageV1,
    u32,
    *const NativeAtlasPlacementV1,
    u32,
) -> i32;
#[cfg(windows)]
pub type NativeAtlasCommitV1 = unsafe extern "system" fn(u64) -> i32;
#[cfg(not(windows))]
pub type NativeAtlasCommitV1 = unsafe extern "C" fn(u64) -> i32;

#[derive(Clone, Copy)]
pub struct NativeAtlasExportsV1 {
    pub query: NativeAtlasQueryV1,
    pub stage: NativeAtlasStageV1,
    pub commit: NativeAtlasCommitV1,
}

/// Reads one bounded inventory. Counts and token must agree between both calls.
pub fn query_native_atlas_v1(exports: NativeAtlasExportsV1) -> Option<NativeAtlasSnapshotV1> {
    let mut token = 0;
    let mut renderer_generation = 0;
    let mut asset_generation = 0;
    let mut flags = 0;
    let mut stage_peak_cap_bytes = 0;
    let mut sprite_count = 0;
    let mut page_count = 0;
    let mut pair_count = 0;
    let first = unsafe {
        (exports.query)(
            &mut token,
            &mut renderer_generation,
            &mut asset_generation,
            &mut flags,
            &mut stage_peak_cap_bytes,
            std::ptr::null_mut(),
            0,
            &mut sprite_count,
            std::ptr::null_mut(),
            0,
            &mut page_count,
            std::ptr::null_mut(),
            0,
            &mut pair_count,
        )
    };
    if first != 1 && first != 2 {
        return None;
    }
    if sprite_count == 0
        || page_count == 0
        || sprite_count as usize > NATIVE_ATLAS_MAX_SPRITES
        || page_count as usize > NATIVE_ATLAS_MAX_PAGES
        || pair_count as usize > NATIVE_ATLAS_MAX_PAIRS
        || flags & (NATIVE_ATLAS_PAIR_OVERFLOW | NATIVE_ATLAS_INVENTORY_OVERFLOW) != 0
    {
        return None;
    }
    let previous = (
        token,
        renderer_generation,
        asset_generation,
        flags,
        stage_peak_cap_bytes,
        sprite_count,
        page_count,
        pair_count,
    );
    let mut sprites = vec![NativeAtlasResidentV1::default(); sprite_count as usize];
    let mut pages = vec![NativeAtlasPageV1::default(); page_count as usize];
    let mut pairs = vec![NativeAtlasPairV1::default(); pair_count as usize];
    let second = unsafe {
        (exports.query)(
            &mut token,
            &mut renderer_generation,
            &mut asset_generation,
            &mut flags,
            &mut stage_peak_cap_bytes,
            sprites.as_mut_ptr(),
            sprite_count,
            &mut sprite_count,
            pages.as_mut_ptr(),
            page_count,
            &mut page_count,
            pairs.as_mut_ptr(),
            pair_count,
            &mut pair_count,
        )
    };
    if second != 1
        || previous
            != (
                token,
                renderer_generation,
                asset_generation,
                flags,
                stage_peak_cap_bytes,
                sprite_count,
                page_count,
                pair_count,
            )
    {
        return None;
    }
    Some(NativeAtlasSnapshotV1 {
        token,
        renderer_generation,
        asset_generation,
        flags,
        stage_peak_cap_bytes,
        sprites,
        pages,
        pairs,
    })
}

/// Optional dynamic exports are resolved as a unit, never mixed across libraries.
pub fn native_atlas_exports_v1(
    query: Option<usize>,
    stage: Option<usize>,
    commit: Option<usize>,
) -> Option<NativeAtlasExportsV1> {
    let query = query.filter(|address| *address != 0)?;
    let stage = stage.filter(|address| *address != 0)?;
    let commit = commit.filter(|address| *address != 0)?;
    Some(NativeAtlasExportsV1 {
        query: unsafe { std::mem::transmute::<usize, NativeAtlasQueryV1>(query) },
        stage: unsafe { std::mem::transmute::<usize, NativeAtlasStageV1>(stage) },
        commit: unsafe { std::mem::transmute::<usize, NativeAtlasCommitV1>(commit) },
    })
}

/// Optional developer capture of the exact production inventory used by the
/// planner. The preview normalizes this with a v4 AOT manifest and its assets.
fn export_native_atlas_snapshot_v1(snapshot: &NativeAtlasSnapshotV1) {
    use std::fmt::Write as _;
    let Some(path) = std::env::var_os("STASIS_ATLAS_SNAPSHOT_OUT") else {
        return;
    };
    let mut out = String::new();
    let _ = write!(
        out,
        "{{\"schema\":\"atlas-affinity-native-query/v1\",\"snapshot_token\":{},\"renderer_generation\":{},\"asset_generation\":{},\"flags\":{},\"stage_peak_cap_bytes\":{},\"evidence_provenance\":\"bounded-runtime-histogram\",\"pages\":[",
        snapshot.token,
        snapshot.renderer_generation,
        snapshot.asset_generation,
        snapshot.flags,
        snapshot.stage_peak_cap_bytes,
    );
    for (index, page) in snapshot.pages.iter().enumerate() {
        if index != 0 {
            out.push(',');
        }
        let _ = write!(
            out,
            "{{\"page_index\":{},\"width\":{},\"height\":{},\"usable_x\":{},\"usable_y\":{},\"padding\":{},\"reserved_header_height\":{},\"flags\":{},\"compatibility_flags\":{},\"group_id\":{},\"allocation_bytes\":{}}}",
            page.page_index, page.width, page.height, page.usable_x, page.usable_y,
            page.padding, page.reserved_header_height, page.flags,
            page.compatibility_flags, page.group_id, page.allocation_bytes,
        );
    }
    out.push_str("],\"sprites\":[");
    for (index, sprite) in snapshot.sprites.iter().enumerate() {
        if index != 0 {
            out.push(',');
        }
        let _ = write!(
            out,
            "{{\"handle\":{},\"width\":{},\"height\":{},\"logical_width\":{},\"logical_height\":{},\"page_index\":{},\"x\":{},\"y\":{},\"allocation_width\":{},\"allocation_height\":{},\"padding\":{},\"flags\":{},\"group_id\":{},\"normalized_path_hash\":{}}}",
            sprite.handle, sprite.width, sprite.height, sprite.logical_width,
            sprite.logical_height, sprite.page_index, sprite.x, sprite.y,
            sprite.allocation_width, sprite.allocation_height, sprite.padding,
            sprite.flags, sprite.group_id, sprite.normalized_path_hash,
        );
    }
    out.push_str("],\"pairs\":[");
    for (index, pair) in snapshot.pairs.iter().enumerate() {
        if index != 0 {
            out.push(',');
        }
        let _ = write!(
            out,
            "{{\"from_handle\":{},\"to_handle\":{},\"weight\":{}}}",
            pair.from_handle, pair.to_handle, pair.weight,
        );
    }
    out.push_str("]}");
    let _ = std::fs::write(path, out);
}
const NATIVE_ATLAS_QUERY_INTERVAL_SUBMITS: u8 = 64;

#[derive(Default)]
struct NativeAtlasQueryCadence {
    query_address: Option<usize>,
    skipped_submits: u8,
}

impl NativeAtlasQueryCadence {
    fn query_due(&mut self, query_address: usize) -> bool {
        if self.query_address != Some(query_address)
            || self.skipped_submits >= NATIVE_ATLAS_QUERY_INTERVAL_SUBMITS - 1
        {
            self.query_address = Some(query_address);
            self.skipped_submits = 0;
            true
        } else {
            self.skipped_submits += 1;
            false
        }
    }
}

fn native_atlas_query_due(query_address: usize) -> bool {
    use std::sync::{Mutex, OnceLock};
    static CADENCE: OnceLock<Mutex<NativeAtlasQueryCadence>> = OnceLock::new();
    CADENCE
        .get_or_init(|| Mutex::new(NativeAtlasQueryCadence::default()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .query_due(query_address)
}

/// One production planner attempt per stable native inventory generation.
///
/// The native inventory is queried on the first submit and at most once every
/// 64 submits afterward. A reload or renderer-generation change is therefore
/// detected within 64 submits; conventional atlas rendering remains active
/// until a staged replacement commits successfully.
pub fn optimize_native_atlas_v1(exports: NativeAtlasExportsV1) -> bool {
    use crate::atlas_placement::{
        plan_atlas_affinity, AtlasBaselinePlacement, AtlasCompatibilityKey, AtlasMemoryBudget,
        AtlasPageExtent, AtlasPairWeight, AtlasPlanningInput, AtlasSpriteDescriptor,
        MAX_ATLAS_PLANNER_SPRITES,
    };
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::{Mutex, OnceLock};

    static LAST_ATTEMPT: OnceLock<Mutex<Option<(usize, u64)>>> = OnceLock::new();
    if !native_atlas_query_due(exports.query as usize) {
        return false;
    }
    let Some(snapshot) = query_native_atlas_v1(exports) else {
        return false;
    };
    if snapshot.token == 0
        || snapshot.flags & NATIVE_ATLAS_PAIR_VALID == 0
        || snapshot.pairs.is_empty()
        || snapshot.sprites.len() > MAX_ATLAS_PLANNER_SPRITES
    {
        return false;
    }
    let attempt = (exports.query as usize, snapshot.token);
    let mut last = LAST_ATTEMPT
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if *last == Some(attempt) {
        return false;
    }
    *last = Some(attempt);
    drop(last);
    export_native_atlas_snapshot_v1(&snapshot);

    const ELIGIBLE: u32 = 1;
    const PROTECTED_PAGE_FLAGS: u32 = (1 << 0) | (1 << 1) | (1 << 2) | (1 << 3);
    const PLAN_ELIGIBLE_PAGE_FLAG: u32 = 1 << 4;
    const SDL_COMPAT: u32 = 1;
    let mut pages_by_id = BTreeMap::new();
    for page in &snapshot.pages {
        if pages_by_id.insert(page.page_index, *page).is_some() {
            return false;
        }
    }
    let mut blocked_pages = BTreeSet::new();
    for resident in &snapshot.sprites {
        let Some(page) = pages_by_id.get(&resident.page_index) else {
            return false;
        };
        if resident.handle <= 0
            || resident.flags & ELIGIBLE == 0
            || resident.group_id == 0
            || resident.group_id != page.group_id
            || page.flags & PROTECTED_PAGE_FLAGS != 0
            || page.flags & PLAN_ELIGIBLE_PAGE_FLAG == 0
            || page.compatibility_flags != SDL_COMPAT
            || resident.padding != page.padding
        {
            blocked_pages.insert(resident.page_index);
        }
    }
    let usable_pages = snapshot
        .pages
        .iter()
        .filter(|page| {
            page.group_id != 0
                && page.flags & PROTECTED_PAGE_FLAGS == 0
                && page.flags & PLAN_ELIGIBLE_PAGE_FLAG != 0
                && page.compatibility_flags == SDL_COMPAT
                && !blocked_pages.contains(&page.page_index)
        })
        .collect::<Vec<_>>();
    if usable_pages.len() < 2 {
        return false;
    }
    let usable_ids = usable_pages
        .iter()
        .map(|page| page.page_index)
        .collect::<BTreeSet<_>>();
    let residents = snapshot
        .sprites
        .iter()
        .filter(|sprite| usable_ids.contains(&sprite.page_index))
        .collect::<Vec<_>>();
    if residents.len() < 2 {
        return false;
    }
    let compat = |group_id| AtlasCompatibilityKey {
        group_id: Some(group_id),
        format: Some(1),
        sampler: Some(1),
        color_space: Some(1),
        backend: Some(1),
    };
    let sprites = residents
        .iter()
        .map(|sprite| AtlasSpriteDescriptor {
            id: sprite.handle as u64,
            width: sprite.width,
            height: sprite.height,
            padding: sprite.padding,
            compatibility: compat(sprite.group_id),
        })
        .collect::<Vec<_>>();
    let pages = usable_pages
        .iter()
        .map(|page| AtlasPageExtent {
            id: page.page_index,
            compatibility: compat(page.group_id),
            width: page.width,
            height: page.height,
            usable_origin_x: page.usable_x,
            usable_origin_y: page.usable_y,
            allocation_bytes: page.allocation_bytes,
        })
        .collect::<Vec<_>>();
    let baseline = residents
        .iter()
        .map(|sprite| AtlasBaselinePlacement {
            sprite_id: sprite.handle as u64,
            page_id: sprite.page_index,
            x: sprite.x,
            y: sprite.y,
            width: sprite.width,
            height: sprite.height,
            padding: sprite.padding,
        })
        .collect::<Vec<_>>();
    let handles = residents
        .iter()
        .map(|sprite| sprite.handle)
        .collect::<BTreeSet<_>>();
    let pairs = snapshot
        .pairs
        .iter()
        .filter(|pair| handles.contains(&pair.from_handle) && handles.contains(&pair.to_handle))
        .map(|pair| AtlasPairWeight {
            from_sprite_id: pair.from_handle as u64,
            to_sprite_id: pair.to_handle as u64,
            weight: pair.weight,
        })
        .collect::<Vec<_>>();
    if pairs.is_empty() {
        return false;
    }
    let Some(current_device_bytes) = snapshot
        .pages
        .iter()
        .try_fold(0_u64, |sum, page| sum.checked_add(page.allocation_bytes))
    else {
        return false;
    };
    let result = plan_atlas_affinity(&AtlasPlanningInput {
        current_generation: snapshot.token,
        baseline_generation: snapshot.token,
        sprites: &sprites,
        pages: &pages,
        baseline_placements: &baseline,
        pair_weights: &pairs,
        budget: AtlasMemoryBudget {
            current_device_bytes,
            max_final_bytes: current_device_bytes,
            max_peak_bytes: snapshot.stage_peak_cap_bytes,
        },
    });
    let Some(plan) = result.plan else {
        return false;
    };
    let native_pages = plan
        .pages
        .iter()
        .filter_map(|planned| pages_by_id.get(&planned.id))
        .map(|source| NativeAtlasPlanPageV1 {
            source_page_index: source.page_index,
            width: source.width,
            height: source.height,
            usable_x: source.usable_x,
            usable_y: source.usable_y,
            padding: source.padding,
            reserved_header_height: source.reserved_header_height,
            flags: source.flags,
            compatibility_flags: source.compatibility_flags,
            group_id: source.group_id,
            allocation_bytes: source.allocation_bytes,
        })
        .collect::<Vec<_>>();
    if native_pages.len() != plan.pages.len() {
        return false;
    }
    let native_index_by_page = native_pages
        .iter()
        .enumerate()
        .map(|(index, page)| (page.source_page_index, index as u32))
        .collect::<BTreeMap<_, _>>();
    let placements = plan
        .placements
        .iter()
        .filter_map(|placement| {
            Some(NativeAtlasPlacementV1 {
                handle: placement.sprite_id as i32,
                page_index: *native_index_by_page.get(&placement.page_id)?,
                x: placement.x,
                y: placement.y,
            })
        })
        .collect::<Vec<_>>();
    if placements.len() != residents.len() {
        return false;
    }
    let staged = unsafe {
        (exports.stage)(
            snapshot.token,
            native_pages.as_ptr(),
            native_pages.len() as u32,
            placements.as_ptr(),
            placements.len() as u32,
        )
    };
    staged == 1 && unsafe { (exports.commit)(snapshot.token) } == 1
}

/// Called by the AOT runner between render and submit when both optional
/// runtime libraries expose the versioned policy bridge.
#[no_mangle]
pub extern "C" fn stasis_atlas_optimize_v1(query: usize, stage: usize, commit: usize) -> i32 {
    native_atlas_exports_v1(Some(query), Some(stage), Some(commit))
        .is_some_and(optimize_native_atlas_v1) as i32
}
pub fn native_atlas_export_address(address: *const c_void) -> Option<usize> {
    (!address.is_null()).then_some(address as usize)
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_atlas_query_cadence_retries_every_64_submits_and_resets_on_export_change() {
        const FIRST_QUERY: usize = 0x1234;
        const SECOND_QUERY: usize = 0x5678;
        let mut cadence = NativeAtlasQueryCadence::default();

        assert!(cadence.query_due(FIRST_QUERY));
        for _ in 2..=64 {
            assert!(!cadence.query_due(FIRST_QUERY));
        }
        assert!(cadence.query_due(FIRST_QUERY));
        assert!(cadence.query_due(SECOND_QUERY));
        assert!(!cadence.query_due(SECOND_QUERY));
    }
    #[test]
    fn native_atlas_v1_records_have_fixed_ffi_layouts() {
        assert_eq!(std::mem::size_of::<NativeAtlasResidentV1>(), 88);
        assert_eq!(std::mem::size_of::<NativeAtlasPageV1>(), 56);
        assert_eq!(std::mem::size_of::<NativeAtlasPairV1>(), 16);
        assert_eq!(std::mem::size_of::<NativeAtlasPlanPageV1>(), 56);
        assert_eq!(std::mem::size_of::<NativeAtlasPlacementV1>(), 16);
        assert!(native_atlas_exports_v1(Some(0), Some(1), Some(1)).is_none());
        assert!(native_atlas_exports_v1(Some(1), Some(0), Some(1)).is_none());
        assert!(native_atlas_exports_v1(Some(1), Some(1), Some(0)).is_none());
    }
    #[derive(Clone)]
    struct FakeAtlasFixture {
        snapshot: NativeAtlasSnapshotV1,
        query_calls: usize,
        stage_calls: usize,
        staged_pages: Vec<NativeAtlasPlanPageV1>,
        staged_placements: Vec<NativeAtlasPlacementV1>,
        commit_tokens: Vec<u64>,
    }

    fn fake_atlas_fixture() -> &'static std::sync::Mutex<Option<FakeAtlasFixture>> {
        static FIXTURE: std::sync::OnceLock<std::sync::Mutex<Option<FakeAtlasFixture>>> =
            std::sync::OnceLock::new();
        FIXTURE.get_or_init(|| std::sync::Mutex::new(None))
    }

    unsafe fn fake_query_impl(
        token: *mut u64,
        renderer_generation: *mut u32,
        asset_generation: *mut u64,
        flags: *mut u32,
        stage_peak_cap_bytes: *mut u64,
        sprites_out: *mut NativeAtlasResidentV1,
        sprite_capacity: u32,
        sprite_count_out: *mut u32,
        pages_out: *mut NativeAtlasPageV1,
        page_capacity: u32,
        page_count_out: *mut u32,
        pairs_out: *mut NativeAtlasPairV1,
        pair_capacity: u32,
        pair_count_out: *mut u32,
    ) -> i32 {
        if token.is_null()
            || renderer_generation.is_null()
            || asset_generation.is_null()
            || flags.is_null()
            || stage_peak_cap_bytes.is_null()
            || sprite_count_out.is_null()
            || page_count_out.is_null()
            || pair_count_out.is_null()
        {
            return 0;
        }
        let Ok(mut guard) = fake_atlas_fixture().lock() else {
            return 0;
        };
        let Some(fixture) = guard.as_mut() else {
            return 0;
        };
        fixture.query_calls += 1;
        let snapshot = fixture.snapshot.clone();
        unsafe {
            *token = snapshot.token;
            *renderer_generation = snapshot.renderer_generation;
            *asset_generation = snapshot.asset_generation;
            *flags = snapshot.flags;
            *stage_peak_cap_bytes = snapshot.stage_peak_cap_bytes;
            *sprite_count_out = snapshot.sprites.len() as u32;
            *page_count_out = snapshot.pages.len() as u32;
            *pair_count_out = snapshot.pairs.len() as u32;
        }
        if sprite_capacity < snapshot.sprites.len() as u32
            || page_capacity < snapshot.pages.len() as u32
            || pair_capacity < snapshot.pairs.len() as u32
        {
            return 2;
        }
        if (!snapshot.sprites.is_empty() && sprites_out.is_null())
            || (!snapshot.pages.is_empty() && pages_out.is_null())
            || (!snapshot.pairs.is_empty() && pairs_out.is_null())
        {
            return 0;
        }
        unsafe {
            std::ptr::copy_nonoverlapping(
                snapshot.sprites.as_ptr(),
                sprites_out,
                snapshot.sprites.len(),
            );
            std::ptr::copy_nonoverlapping(snapshot.pages.as_ptr(), pages_out, snapshot.pages.len());
            std::ptr::copy_nonoverlapping(snapshot.pairs.as_ptr(), pairs_out, snapshot.pairs.len());
        }
        1
    }

    unsafe fn fake_stage_impl(
        token: u64,
        pages: *const NativeAtlasPlanPageV1,
        page_count: u32,
        placements: *const NativeAtlasPlacementV1,
        placement_count: u32,
    ) -> i32 {
        if pages.is_null() || placements.is_null() || page_count == 0 || placement_count == 0 {
            return 0;
        }
        let Ok(mut guard) = fake_atlas_fixture().lock() else {
            return 0;
        };
        let Some(fixture) = guard.as_mut() else {
            return 0;
        };
        if token != fixture.snapshot.token || fixture.stage_calls != 0 {
            return 0;
        }
        let staged_pages =
            unsafe { std::slice::from_raw_parts(pages, page_count as usize) }.to_vec();
        let staged_placements =
            unsafe { std::slice::from_raw_parts(placements, placement_count as usize) }.to_vec();
        fixture.stage_calls += 1;
        fixture.staged_pages = staged_pages;
        fixture.staged_placements = staged_placements;
        1
    }

    unsafe fn fake_commit_impl(token: u64) -> i32 {
        let Ok(mut guard) = fake_atlas_fixture().lock() else {
            return 0;
        };
        let Some(fixture) = guard.as_mut() else {
            return 0;
        };
        if token != fixture.snapshot.token || fixture.stage_calls != 1 {
            return 0;
        }
        fixture.commit_tokens.push(token);
        1
    }

    macro_rules! define_fake_atlas_exports {
        ($abi:literal) => {
            unsafe extern $abi fn fake_query(
                token: *mut u64,
                renderer_generation: *mut u32,
                asset_generation: *mut u64,
                flags: *mut u32,
                stage_peak_cap_bytes: *mut u64,
                sprites_out: *mut NativeAtlasResidentV1,
                sprite_capacity: u32,
                sprite_count_out: *mut u32,
                pages_out: *mut NativeAtlasPageV1,
                page_capacity: u32,
                page_count_out: *mut u32,
                pairs_out: *mut NativeAtlasPairV1,
                pair_capacity: u32,
                pair_count_out: *mut u32,
            ) -> i32 {
                unsafe {
                    fake_query_impl(
                        token,
                        renderer_generation,
                        asset_generation,
                        flags,
                        stage_peak_cap_bytes,
                        sprites_out,
                        sprite_capacity,
                        sprite_count_out,
                        pages_out,
                        page_capacity,
                        page_count_out,
                        pairs_out,
                        pair_capacity,
                        pair_count_out,
                    )
                }
            }

            unsafe extern $abi fn fake_stage(
                token: u64,
                pages: *const NativeAtlasPlanPageV1,
                page_count: u32,
                placements: *const NativeAtlasPlacementV1,
                placement_count: u32,
            ) -> i32 {
                unsafe {
                    fake_stage_impl(token, pages, page_count, placements, placement_count)
                }
            }

            unsafe extern $abi fn fake_commit(token: u64) -> i32 {
                unsafe { fake_commit_impl(token) }
            }
        };
    }

    #[cfg(windows)]
    define_fake_atlas_exports!("system");
    #[cfg(not(windows))]
    define_fake_atlas_exports!("C");

    #[test]
    fn optimize_native_atlas_v1_stages_sparse_source_pages_with_zero_based_indices() {
        const GROUP_ID: u64 = 77;
        const TOKEN: u64 = 901;
        let snapshot = NativeAtlasSnapshotV1 {
            token: TOKEN,
            renderer_generation: 3,
            asset_generation: 12,
            flags: NATIVE_ATLAS_PAIR_VALID,
            // Two resident 32x32 RGBA pages plus one replacement page.
            stage_peak_cap_bytes: 12_288,
            sprites: vec![
                NativeAtlasResidentV1 {
                    handle: 101,
                    width: 8,
                    height: 8,
                    logical_width: 8,
                    logical_height: 8,
                    page_index: 17,
                    x: 2,
                    y: 7,
                    allocation_width: 10,
                    allocation_height: 10,
                    padding: 1,
                    flags: 1,
                    group_id: GROUP_ID,
                    ..NativeAtlasResidentV1::default()
                },
                NativeAtlasResidentV1 {
                    handle: 202,
                    width: 8,
                    height: 8,
                    logical_width: 8,
                    logical_height: 8,
                    page_index: 42,
                    x: 2,
                    y: 7,
                    allocation_width: 10,
                    allocation_height: 10,
                    padding: 1,
                    flags: 1,
                    group_id: GROUP_ID,
                    ..NativeAtlasResidentV1::default()
                },
            ],
            pages: vec![
                NativeAtlasPageV1 {
                    page_index: 17,
                    width: 32,
                    height: 32,
                    usable_x: 1,
                    usable_y: 6,
                    padding: 1,
                    reserved_header_height: 6,
                    flags: 1 << 4,
                    compatibility_flags: 1,
                    group_id: GROUP_ID,
                    allocation_bytes: 4_096,
                },
                NativeAtlasPageV1 {
                    page_index: 42,
                    width: 32,
                    height: 32,
                    usable_x: 1,
                    usable_y: 6,
                    padding: 1,
                    reserved_header_height: 6,
                    flags: 1 << 4,
                    compatibility_flags: 1,
                    group_id: GROUP_ID,
                    allocation_bytes: 4_096,
                },
            ],
            pairs: vec![NativeAtlasPairV1 {
                from_handle: 101,
                to_handle: 202,
                weight: 73,
            }],
        };
        *fake_atlas_fixture()
            .lock()
            .expect("fake atlas fixture mutex") = Some(FakeAtlasFixture {
            snapshot: snapshot.clone(),
            query_calls: 0,
            stage_calls: 0,
            staged_pages: Vec::new(),
            staged_placements: Vec::new(),
            commit_tokens: Vec::new(),
        });
        let exports = NativeAtlasExportsV1 {
            query: fake_query,
            stage: fake_stage,
            commit: fake_commit,
        };

        assert!(optimize_native_atlas_v1(exports));
        // Calls 2 through 64 skip native queries; call 65 refreshes inventory,
        // then LAST_ATTEMPT prevents staging the unchanged token again.
        for _ in 2..=64 {
            assert!(!optimize_native_atlas_v1(exports));
        }
        assert!(!optimize_native_atlas_v1(exports));

        let fixture = fake_atlas_fixture()
            .lock()
            .expect("fake atlas fixture mutex")
            .take()
            .expect("fake atlas fixture remains installed");
        assert_eq!(snapshot.pairs[0].weight, 73);
        assert_eq!(fixture.query_calls, 4);
        assert_eq!(fixture.stage_calls, 1);
        assert_eq!(fixture.staged_pages.len(), 1);
        assert!(matches!(fixture.staged_pages[0].source_page_index, 17 | 42));
        assert_eq!(fixture.staged_placements.len(), 2);
        assert!(fixture
            .staged_placements
            .iter()
            .all(|placement| placement.page_index == 0));
        let handles = fixture
            .staged_placements
            .iter()
            .map(|placement| placement.handle)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(handles, std::collections::BTreeSet::from([101, 202]));
        assert_eq!(fixture.commit_tokens, vec![TOKEN]);
    }
}
