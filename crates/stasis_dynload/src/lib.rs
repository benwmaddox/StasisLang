#![cfg_attr(not(debug_assertions), deny(warnings))]

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ffi::{c_char, CString};
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, RwLock};
use std::time::{Duration, Instant};

mod dynamic_library;
pub use dynamic_library::{atomic_rename_no_replace, Library};

#[cfg(feature = "cross-atlas-research")]
mod cross_atlas_research;
#[cfg(feature = "cross-atlas-research")]
pub use cross_atlas_research::*;

pub const HOT_RENDER_METADATA_VERSION: u32 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotRenderRuntimeImage {
    pub logical_path: String,
    pub logical_width: u32,
    pub logical_height: u32,
    pub max_renders_per_render: Option<u64>,
    pub atlas_eligible: bool,
    pub grouping_key: String,
    pub estimated_distinct_transitions: u64,
    pub group_member_count: u32,
    pub group_logical_pixel_area: u64,
    pub group_max_logical_width: u32,
    pub group_max_logical_height: u32,
    pub backend_constraints: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HotRenderAtlasPolicy {
    pub eligible: bool,
    pub group_id: u64,
    pub member_count: u32,
    pub logical_pixel_area: u64,
    pub max_logical_width: u32,
    pub max_logical_height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealizedHotRenderImage {
    pub image: HotRenderRuntimeImage,
    pub realized_width: u32,
    pub realized_height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotRenderLoadPlan {
    pub groups: Vec<(String, Vec<(String, u32, u32)>)>,
    pub standalone: Vec<(String, u32, u32)>,
}

fn hot_render_runtime_images() -> &'static RwLock<HashMap<(String, u32, u32), HotRenderRuntimeImage>>
{
    static IMAGES: OnceLock<RwLock<HashMap<(String, u32, u32), HotRenderRuntimeImage>>> =
        OnceLock::new();
    IMAGES.get_or_init(|| RwLock::new(HashMap::new()))
}

fn normalized_asset_key(path: &str, width: u32, height: u32) -> (String, u32, u32) {
    (path.replace('\\', "/"), width, height)
}

/// Atomically replaces the runtime policy table. Unknown contract versions
/// deliberately publish an empty table, preserving standalone-safe behavior.
pub fn replace_hot_render_metadata(version: u32, images: &[HotRenderRuntimeImage]) {
    let mut next = HashMap::new();
    if version == HOT_RENDER_METADATA_VERSION {
        for image in images {
            next.insert(
                normalized_asset_key(
                    &image.logical_path,
                    image.logical_width,
                    image.logical_height,
                ),
                image.clone(),
            );
        }
    }
    *hot_render_runtime_images()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = next;
}

/// Captures the currently accepted policy for transactional host publication.
pub fn snapshot_hot_render_metadata() -> Vec<HotRenderRuntimeImage> {
    let mut images = hot_render_runtime_images()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .values()
        .cloned()
        .collect::<Vec<_>>();
    images.sort_by(|left, right| {
        (&left.logical_path, left.logical_width, left.logical_height).cmp(&(
            &right.logical_path,
            right.logical_width,
            right.logical_height,
        ))
    });
    images
}

/// Missing, stale, unknown, and <=1 records are standalone by default.
pub fn hot_render_atlas_eligible(path: &str, width: u32, height: u32) -> bool {
    hot_render_atlas_policy(path, width, height).eligible
}

fn stable_group_id(key: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in key.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    if hash == 0 {
        1
    } else {
        hash
    }
}

pub fn hot_render_atlas_policy(path: &str, width: u32, height: u32) -> HotRenderAtlasPolicy {
    hot_render_runtime_images()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&normalized_asset_key(path, width, height))
        .filter(|image| {
            image.atlas_eligible
                && image.max_renders_per_render.is_some_and(|count| count > 1)
                && image.group_member_count > 1
                && !image.grouping_key.is_empty()
        })
        .map_or_else(HotRenderAtlasPolicy::default, |image| {
            HotRenderAtlasPolicy {
                eligible: true,
                group_id: stable_group_id(&image.grouping_key),
                member_count: image.group_member_count,
                logical_pixel_area: image.group_logical_pixel_area,
                max_logical_width: image.group_max_logical_width,
                max_logical_height: image.group_max_logical_height,
            }
        })
}

/// Deterministic candidate grouping from compiler metadata. Render backends
/// must refine this with realized dimensions before allocating pages.
pub fn plan_hot_render_groups(
    images: &[HotRenderRuntimeImage],
) -> Vec<(String, Vec<(String, u32, u32)>)> {
    let mut groups = std::collections::BTreeMap::<String, Vec<(String, u32, u32)>>::new();
    for image in images.iter().filter(|image| {
        image.atlas_eligible && image.max_renders_per_render.is_some_and(|count| count > 1)
    }) {
        let compatible_key = format!("{}:{}", image.grouping_key, image.backend_constraints);
        groups
            .entry(compatible_key)
            .or_default()
            .push(normalized_asset_key(
                &image.logical_path,
                image.logical_width,
                image.logical_height,
            ));
    }
    for entries in groups.values_mut() {
        entries.sort();
        entries.dedup();
    }
    groups.into_iter().collect()
}

/// Plans with device-realized raster extents. Logical dimensions in compiler
/// metadata never stand in for decoded/device-scaled dimensions here.
pub fn plan_realized_hot_render_loads(
    images: &[RealizedHotRenderImage],
    page_width: u32,
    page_height: u32,
    max_texture_extent: u32,
) -> HotRenderLoadPlan {
    let mut groups = std::collections::BTreeMap::<String, Vec<(String, u32, u32)>>::new();
    let mut standalone = Vec::new();
    let max_width = page_width.min(max_texture_extent);
    let max_height = page_height.min(max_texture_extent);
    for realized in images {
        let key = normalized_asset_key(
            &realized.image.logical_path,
            realized.realized_width,
            realized.realized_height,
        );
        let required_width = realized.realized_width.checked_add(4);
        let required_height = realized.realized_height.checked_add(4);
        let fits = realized.realized_width > 0
            && realized.realized_height > 0
            && required_width.is_some_and(|width| width <= max_width)
            && required_height.is_some_and(|height| height <= max_height);
        if fits
            && realized.image.atlas_eligible
            && realized
                .image
                .max_renders_per_render
                .is_some_and(|count| count > 1)
        {
            let group = format!(
                "{}:{}",
                realized.image.grouping_key, realized.image.backend_constraints
            );
            groups.entry(group).or_default().push(key);
        } else {
            standalone.push(key);
        }
    }
    for entries in groups.values_mut() {
        entries.sort();
        entries.dedup();
    }
    standalone.sort();
    standalone.dedup();
    HotRenderLoadPlan {
        groups: groups.into_iter().collect(),
        standalone,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JitHostEntryTargets {
    pub revision: u64,
    pub main: usize,
    pub tick: usize,
    pub render: usize,
    pub on_code_swap: Option<usize>,
}

static JIT_HOST_ENTRY_TARGETS: AtomicUsize = AtomicUsize::new(0);

static JIT_DEBUG_ENABLED: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "network")]
static NETWORK_HOST_HANDLE: AtomicUsize = AtomicUsize::new(0);
static JIT_PROFILE_ENABLED: AtomicBool = AtomicBool::new(false);
static JIT_PROFILE_GENERATION: AtomicU64 = AtomicU64::new(1);
static RECORDING_CLOCK_FPS: AtomicU64 = AtomicU64::new(0);
static RECORDING_CLOCK_FRAME: AtomicU64 = AtomicU64::new(0);

pub fn set_recording_clock(fps: u32, frame: u64) {
    RECORDING_CLOCK_FPS.store(u64::from(fps), Ordering::Release);
    RECORDING_CLOCK_FRAME.store(frame, Ordering::Release);
}

pub fn set_recording_clock_frame(frame: u64) {
    RECORDING_CLOCK_FRAME.store(frame, Ordering::Release);
}

pub fn clear_recording_clock() {
    RECORDING_CLOCK_FPS.store(0, Ordering::Release);
    RECORDING_CLOCK_FRAME.store(0, Ordering::Release);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JitProfileSample {
    pub function_id: u32,
    pub calls: u64,
    pub inclusive_ns: u64,
    pub exclusive_ns: u64,
    pub max_inclusive_ns: u64,
}

struct JitProfileFrame {
    function_id: u32,
    generation: u64,
    started: Instant,
    child_ns: u64,
}

#[derive(Default)]
struct JitProfileAggregate {
    calls: AtomicU64,
    inclusive_ns: AtomicU64,
    exclusive_ns: AtomicU64,
    max_inclusive_ns: AtomicU64,
}

#[derive(Default)]
struct JitProfileState {
    generation: u64,
    frames: Vec<JitProfileFrame>,
    aggregate_cache: HashMap<u32, Arc<JitProfileAggregate>>,
}

thread_local! {
    static JIT_PROFILE_STATE: RefCell<JitProfileState> = RefCell::new(JitProfileState::default());
}

fn elapsed_ns(started: Instant) -> u64 {
    started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64
}

fn jit_profile_aggregates() -> &'static RwLock<HashMap<u32, Arc<JitProfileAggregate>>> {
    static AGGREGATES: OnceLock<RwLock<HashMap<u32, Arc<JitProfileAggregate>>>> = OnceLock::new();
    AGGREGATES.get_or_init(|| RwLock::new(HashMap::new()))
}

fn sync_jit_profile_generation(state: &mut JitProfileState, generation: u64) {
    if state.generation != generation {
        state.generation = generation;
        state.frames.clear();
        state.aggregate_cache.clear();
    }
}

pub fn enable_jit_profiler() {
    reset_jit_profile();
    JIT_PROFILE_ENABLED.store(true, Ordering::Release);
}

pub fn disable_jit_profiler() {
    JIT_PROFILE_ENABLED.store(false, Ordering::Release);
    JIT_PROFILE_STATE.with(|state| state.borrow_mut().frames.clear());
}

pub fn reset_jit_profile() {
    JIT_PROFILE_GENERATION.fetch_add(1, Ordering::AcqRel);
    let aggregates = jit_profile_aggregates()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for aggregate in aggregates.values() {
        aggregate.calls.store(0, Ordering::Relaxed);
        aggregate.inclusive_ns.store(0, Ordering::Relaxed);
        aggregate.exclusive_ns.store(0, Ordering::Relaxed);
        aggregate.max_inclusive_ns.store(0, Ordering::Relaxed);
    }
}

pub fn jit_profile_snapshot() -> Vec<JitProfileSample> {
    let aggregates = jit_profile_aggregates()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut samples: Vec<JitProfileSample> = aggregates
        .iter()
        .map(|(function_id, aggregate)| JitProfileSample {
            function_id: *function_id,
            calls: aggregate.calls.load(Ordering::Relaxed),
            inclusive_ns: aggregate.inclusive_ns.load(Ordering::Relaxed),
            exclusive_ns: aggregate.exclusive_ns.load(Ordering::Relaxed),
            max_inclusive_ns: aggregate.max_inclusive_ns.load(Ordering::Relaxed),
        })
        .filter(|sample| sample.calls > 0)
        .collect();
    samples.sort_by_key(|sample| sample.function_id);
    samples
}

#[no_mangle]
pub extern "C" fn stasis_jit_profile_frame_enter(function_id: i32) {
    if !JIT_PROFILE_ENABLED.load(Ordering::Acquire) {
        return;
    }
    let generation = JIT_PROFILE_GENERATION.load(Ordering::Acquire);
    let started = Instant::now();
    JIT_PROFILE_STATE.with(|state| {
        let mut state = state.borrow_mut();
        sync_jit_profile_generation(&mut state, generation);
        state.frames.push(JitProfileFrame {
            function_id: function_id as u32,
            generation,
            started,
            child_ns: 0,
        });
    });
}

#[no_mangle]
pub extern "C" fn stasis_jit_profile_frame_leave(function_id: i32) {
    if !JIT_PROFILE_ENABLED.load(Ordering::Acquire) {
        return;
    }
    let generation = JIT_PROFILE_GENERATION.load(Ordering::Acquire);
    JIT_PROFILE_STATE.with(|state| {
        let mut state = state.borrow_mut();
        sync_jit_profile_generation(&mut state, generation);
        let Some(frame) = state.frames.pop() else {
            return;
        };
        if frame.generation != generation || frame.function_id != function_id as u32 {
            state.frames.clear();
            return;
        }
        let inclusive_ns = elapsed_ns(frame.started);
        let exclusive_ns = inclusive_ns.saturating_sub(frame.child_ns);
        if let Some(parent) = state.frames.last_mut() {
            parent.child_ns = parent.child_ns.saturating_add(inclusive_ns);
        }
        let aggregate = if let Some(aggregate) = state.aggregate_cache.get(&frame.function_id) {
            Arc::clone(aggregate)
        } else {
            let mut aggregates = jit_profile_aggregates()
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let aggregate = Arc::clone(
                aggregates
                    .entry(frame.function_id)
                    .or_insert_with(|| Arc::new(JitProfileAggregate::default())),
            );
            state
                .aggregate_cache
                .insert(frame.function_id, Arc::clone(&aggregate));
            aggregate
        };
        aggregate.calls.fetch_add(1, Ordering::Relaxed);
        aggregate
            .inclusive_ns
            .fetch_add(inclusive_ns, Ordering::Relaxed);
        aggregate
            .exclusive_ns
            .fetch_add(exclusive_ns, Ordering::Relaxed);
        aggregate
            .max_inclusive_ns
            .fetch_max(inclusive_ns, Ordering::Relaxed);
    });
}

fn jit_output_capture() -> &'static Mutex<Option<String>> {
    static CAPTURE: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    CAPTURE.get_or_init(|| Mutex::new(None))
}

pub fn set_jit_output_capture(enabled: bool) {
    *lock_unpoisoned(jit_output_capture()) = enabled.then(String::new);
}

pub fn drain_jit_output() -> String {
    let mut capture = lock_unpoisoned(jit_output_capture());
    capture.as_mut().map(std::mem::take).unwrap_or_default()
}

fn write_jit_output(text: &str) {
    let mut capture = lock_unpoisoned(jit_output_capture());
    if let Some(capture) = capture.as_mut() {
        capture.push_str(text);
    } else {
        print!("{text}");
        let _ = std::io::stdout().flush();
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum JitDebugValue {
    I64 { type_tag: i32, value: i64 },
    F64 { type_tag: i32, value: f64 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct JitDebugFrame {
    pub function_id: u32,
    pub site_id: u32,
    pub values: HashMap<u32, JitDebugValue>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct JitDebugStop {
    pub sequence: u64,
    pub function_id: u32,
    pub site_id: u32,
    pub frames: Vec<JitDebugFrame>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JitDebugResume {
    Continue,
    StepIn,
    StepOver,
    StepOut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JitDebugRunMode {
    Continue,
    StepIn,
    StepOver { depth: usize },
    StepOut { depth: usize },
}

struct JitDebugState {
    enabled: bool,
    breakpoints: HashSet<(u32, u32)>,
    frames: Vec<JitDebugFrame>,
    mode: JitDebugRunMode,
    stopped: Option<JitDebugStop>,
    sequence: u64,
}

impl Default for JitDebugState {
    fn default() -> Self {
        Self {
            enabled: false,
            breakpoints: HashSet::new(),
            frames: Vec::new(),
            mode: JitDebugRunMode::Continue,
            stopped: None,
            sequence: 0,
        }
    }
}

fn jit_debug_controller() -> &'static (Mutex<JitDebugState>, Condvar) {
    static CONTROLLER: OnceLock<(Mutex<JitDebugState>, Condvar)> = OnceLock::new();
    CONTROLLER.get_or_init(|| (Mutex::new(JitDebugState::default()), Condvar::new()))
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn enable_jit_debugger(breakpoints: impl IntoIterator<Item = (u32, u32)>) {
    let (state, wake) = jit_debug_controller();
    let mut state = lock_unpoisoned(state);
    state.enabled = true;
    state.breakpoints = breakpoints.into_iter().collect();
    state.frames.clear();
    state.mode = JitDebugRunMode::Continue;
    state.stopped = None;
    JIT_DEBUG_ENABLED.store(true, Ordering::Release);
    wake.notify_all();
}

pub fn set_jit_debug_breakpoints(breakpoints: impl IntoIterator<Item = (u32, u32)>) {
    let (state, _) = jit_debug_controller();
    lock_unpoisoned(state).breakpoints = breakpoints.into_iter().collect();
}

pub fn disable_jit_debugger() {
    JIT_DEBUG_ENABLED.store(false, Ordering::Release);
    let (state, wake) = jit_debug_controller();
    let mut state = lock_unpoisoned(state);
    state.enabled = false;
    state.frames.clear();
    state.stopped = None;
    state.mode = JitDebugRunMode::Continue;
    wake.notify_all();
}

pub fn jit_debug_stop() -> Option<JitDebugStop> {
    let (state, _) = jit_debug_controller();
    lock_unpoisoned(state).stopped.clone()
}

pub fn wait_for_jit_debug_stop(after_sequence: u64, timeout: Duration) -> Option<JitDebugStop> {
    let (state, wake) = jit_debug_controller();
    let mut state = lock_unpoisoned(state);
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(stop) = state
            .stopped
            .as_ref()
            .filter(|stop| stop.sequence > after_sequence)
        {
            return Some(stop.clone());
        }
        if !state.enabled {
            return None;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return None;
        }
        let (next, result) = wake
            .wait_timeout(state, remaining)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state = next;
        if result.timed_out() {
            return state
                .stopped
                .as_ref()
                .filter(|stop| stop.sequence > after_sequence)
                .cloned();
        }
    }
}

pub fn resume_jit_debugger(resume: JitDebugResume) -> Result<(), String> {
    let (state, wake) = jit_debug_controller();
    let mut state = lock_unpoisoned(state);
    if !state.enabled {
        return Err("JIT debugger is not enabled".to_string());
    }
    let depth = state.frames.len();
    state.mode = match resume {
        JitDebugResume::Continue => JitDebugRunMode::Continue,
        JitDebugResume::StepIn => JitDebugRunMode::StepIn,
        JitDebugResume::StepOver => JitDebugRunMode::StepOver { depth },
        JitDebugResume::StepOut => JitDebugRunMode::StepOut { depth },
    };
    state.stopped = None;
    wake.notify_all();
    Ok(())
}

pub fn pause_jit_debugger() -> Result<(), String> {
    let (state, _) = jit_debug_controller();
    let mut state = lock_unpoisoned(state);
    if !state.enabled {
        return Err("JIT debugger is not enabled".to_string());
    }
    state.mode = JitDebugRunMode::StepIn;
    Ok(())
}

#[no_mangle]
pub extern "C" fn stasis_jit_debug_frame_enter(function_id: i32) {
    if !JIT_DEBUG_ENABLED.load(Ordering::Acquire) {
        return;
    }
    let (state, _) = jit_debug_controller();
    let mut state = lock_unpoisoned(state);
    if state.enabled {
        state.frames.push(JitDebugFrame {
            function_id: function_id as u32,
            site_id: 0,
            values: HashMap::new(),
        });
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_debug_frame_leave(function_id: i32) {
    if !JIT_DEBUG_ENABLED.load(Ordering::Acquire) {
        return;
    }
    let (state, _) = jit_debug_controller();
    let mut state = lock_unpoisoned(state);
    if !state.enabled {
        return;
    }
    if state
        .frames
        .last()
        .is_some_and(|frame| frame.function_id == function_id as u32)
    {
        state.frames.pop();
    } else {
        state.frames.clear();
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_debug_values_begin() {
    if !JIT_DEBUG_ENABLED.load(Ordering::Acquire) {
        return;
    }
    let (state, _) = jit_debug_controller();
    let mut state = lock_unpoisoned(state);
    if let Some(frame) = state.frames.last_mut() {
        frame.values.clear();
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_debug_value_i64(slot: i32, type_tag: i32, value: i64) {
    if !JIT_DEBUG_ENABLED.load(Ordering::Acquire) {
        return;
    }
    let (state, _) = jit_debug_controller();
    let mut state = lock_unpoisoned(state);
    if let Some(frame) = state.frames.last_mut() {
        frame
            .values
            .insert(slot as u32, JitDebugValue::I64 { type_tag, value });
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_debug_value_f64(slot: i32, type_tag: i32, value: f64) {
    if !JIT_DEBUG_ENABLED.load(Ordering::Acquire) {
        return;
    }
    let (state, _) = jit_debug_controller();
    let mut state = lock_unpoisoned(state);
    if let Some(frame) = state.frames.last_mut() {
        frame
            .values
            .insert(slot as u32, JitDebugValue::F64 { type_tag, value });
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_debug_statement(function_id: i32, site_id: i32) {
    if !JIT_DEBUG_ENABLED.load(Ordering::Acquire) {
        return;
    }
    let (state, wake) = jit_debug_controller();
    let mut state = lock_unpoisoned(state);
    if !state.enabled {
        return;
    }
    let function_id = function_id as u32;
    let site_id = site_id as u32;
    let depth = state.frames.len();
    if let Some(frame) = state.frames.last_mut() {
        frame.site_id = site_id;
    }
    let should_stop = state.breakpoints.contains(&(function_id, site_id))
        || match state.mode {
            JitDebugRunMode::Continue => false,
            JitDebugRunMode::StepIn => true,
            JitDebugRunMode::StepOver { depth: target } => depth <= target,
            JitDebugRunMode::StepOut { depth: target } => depth < target,
        };
    if !should_stop {
        return;
    }
    state.sequence = state.sequence.saturating_add(1);
    state.mode = JitDebugRunMode::Continue;
    state.stopped = Some(JitDebugStop {
        sequence: state.sequence,
        function_id,
        site_id,
        frames: state.frames.clone(),
    });
    wake.notify_all();
    while state.enabled && state.stopped.is_some() {
        state = wake
            .wait(state)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
    }
}

pub fn publish_jit_host_entry_targets(targets: JitHostEntryTargets) -> Result<(), String> {
    validate_jit_host_entry_targets(&targets)?;
    if let Some(active) = jit_host_entry_targets() {
        if targets.revision <= active.revision {
            return Err(format!(
                "stale JIT host-entry revision {} cannot replace active revision {}",
                targets.revision, active.revision
            ));
        }
    }
    install_jit_host_entry_targets(targets);
    Ok(())
}

pub fn begin_jit_host_entry_session(targets: JitHostEntryTargets) -> Result<(), String> {
    validate_jit_host_entry_targets(&targets)?;
    install_jit_host_entry_targets(targets);
    Ok(())
}

fn install_jit_host_entry_targets(targets: JitHostEntryTargets) {
    let published = Box::into_raw(Box::new(targets)) as usize;
    JIT_HOST_ENTRY_TARGETS.store(published, Ordering::Release);
}

pub fn validate_jit_host_entry_targets(targets: &JitHostEntryTargets) -> Result<(), String> {
    if targets.main == 0 || targets.tick == 0 || targets.render == 0 {
        return Err(
            "JIT host-entry publication requires non-zero main, tick, and render targets"
                .to_string(),
        );
    }
    Ok(())
}

pub fn jit_host_entry_targets() -> Option<JitHostEntryTargets> {
    let published = JIT_HOST_ENTRY_TARGETS.load(Ordering::Acquire);
    if published == 0 {
        return None;
    }
    Some(unsafe { *(published as *const JitHostEntryTargets) })
}

pub fn jit_host_main_trampoline_ptr() -> usize {
    stasis_jit_host_main_trampoline as *const () as usize
}

pub fn jit_host_tick_trampoline_ptr() -> usize {
    stasis_jit_host_tick_trampoline as *const () as usize
}

pub fn jit_host_render_trampoline_ptr() -> usize {
    stasis_jit_host_render_trampoline as *const () as usize
}

pub fn jit_host_on_code_swap_trampoline_ptr() -> usize {
    stasis_jit_host_on_code_swap_trampoline as *const () as usize
}

fn active_jit_host_target(select: impl FnOnce(JitHostEntryTargets) -> Option<usize>) -> usize {
    jit_host_entry_targets().and_then(select).unwrap_or(0)
}

extern "C" fn stasis_jit_host_main_trampoline() -> i32 {
    call_jit_host_i32_target(active_jit_host_target(|targets| Some(targets.main)))
}

extern "C" fn stasis_jit_host_tick_trampoline() -> i32 {
    call_jit_host_i32_target(active_jit_host_target(|targets| Some(targets.tick)))
}

extern "C" fn stasis_jit_host_render_trampoline() -> i32 {
    call_jit_host_i32_target(active_jit_host_target(|targets| Some(targets.render)))
}

extern "C" fn stasis_jit_host_on_code_swap_trampoline() {
    if let Some(target) = jit_host_entry_targets().and_then(|targets| targets.on_code_swap) {
        call_jit_host_void_target(target);
    }
}

fn call_jit_host_i32_target(address: usize) -> i32 {
    if address == 0 {
        return -1;
    }
    #[cfg(windows)]
    let callback: extern "system" fn() -> i32 = unsafe { std::mem::transmute(address) };
    #[cfg(not(windows))]
    let callback: extern "C" fn() -> i32 = unsafe { std::mem::transmute(address) };
    callback()
}

fn call_jit_host_void_target(address: usize) {
    if address == 0 {
        return;
    }
    #[cfg(windows)]
    let callback: extern "system" fn() = unsafe { std::mem::transmute(address) };
    #[cfg(not(windows))]
    let callback: extern "C" fn() = unsafe { std::mem::transmute(address) };
    callback();
}

pub fn invoke_noarg_u64(address: usize) -> Result<u64, String> {
    if address == 0 {
        return Err("cannot invoke null function pointer".to_string());
    }
    let _dispatch_lock = jit_dispatch_lock()
        .lock()
        .expect("jit dispatch lock mutex poisoned");
    let _execution = JitExecutionGuard::enter();
    #[cfg(windows)]
    {
        let callback: extern "system" fn() -> u64 = unsafe { std::mem::transmute(address) };
        return Ok(callback());
    }
    #[cfg(not(windows))]
    {
        let callback: extern "C" fn() -> u64 = unsafe { std::mem::transmute(address) };
        Ok(callback())
    }
}

pub fn invoke_noarg_i32(address: usize) -> Result<i32, String> {
    if address == 0 {
        return Err("cannot invoke null function pointer".to_string());
    }
    let _dispatch_lock = jit_dispatch_lock()
        .lock()
        .expect("jit dispatch lock mutex poisoned");
    let _execution = JitExecutionGuard::enter();
    #[cfg(windows)]
    {
        let callback: extern "system" fn() -> i32 = unsafe { std::mem::transmute(address) };
        return Ok(callback());
    }
    #[cfg(not(windows))]
    {
        let callback: extern "C" fn() -> i32 = unsafe { std::mem::transmute(address) };
        Ok(callback())
    }
}

pub fn invoke_noarg_void(address: usize) -> Result<(), String> {
    if address == 0 {
        return Err("cannot invoke null function pointer".to_string());
    }
    let _dispatch_lock = jit_dispatch_lock()
        .lock()
        .expect("jit dispatch lock mutex poisoned");
    let _execution = JitExecutionGuard::enter();
    #[cfg(windows)]
    {
        let callback: extern "system" fn() = unsafe { std::mem::transmute(address) };
        callback();
        return Ok(());
    }
    #[cfg(not(windows))]
    {
        let callback: extern "C" fn() = unsafe { std::mem::transmute(address) };
        callback();
        Ok(())
    }
}

thread_local! {
    static CODE_SWAP_REJECTION: RefCell<Option<String>> = const { RefCell::new(None) };
}

pub fn invoke_code_swap_hook(address: usize) -> Result<(), String> {
    let _ = CODE_SWAP_REJECTION.with(|rejection| rejection.borrow_mut().take());
    invoke_noarg_void(address)?;
    CODE_SWAP_REJECTION.with(|rejection| rejection.borrow_mut().take().map_or(Ok(()), Err))
}

#[no_mangle]
pub extern "C" fn stasis_jit_reject_code_swap() {
    CODE_SWAP_REJECTION.with(|rejection| {
        *rejection.borrow_mut() = Some("hook requested rejection".to_string());
    });
}

pub fn invoke_i32_to_void(address: usize, arg0: i32) -> Result<(), String> {
    if address == 0 {
        return Err("cannot invoke null function pointer".to_string());
    }
    let _dispatch_lock = jit_dispatch_lock()
        .lock()
        .expect("jit dispatch lock mutex poisoned");
    let _execution = JitExecutionGuard::enter();
    #[cfg(windows)]
    {
        let callback: extern "system" fn(i32) = unsafe { std::mem::transmute(address) };
        callback(arg0);
        return Ok(());
    }
    #[cfg(not(windows))]
    {
        let callback: extern "C" fn(i32) = unsafe { std::mem::transmute(address) };
        callback(arg0);
        Ok(())
    }
}

/// Invoke a guest ABI function with one i32 argument and an i32 result.
///
/// The address is produced by the JIT, so keeping the platform calling
/// convention and the dispatch/execution guards in one helper prevents hosts
/// from accidentally introducing a second callback ABI.
pub fn invoke_i32_to_i32(address: usize, arg0: i32) -> Result<i32, String> {
    if address == 0 {
        return Err("cannot invoke null function pointer".to_string());
    }
    let _dispatch_lock = jit_dispatch_lock()
        .lock()
        .expect("jit dispatch lock mutex poisoned");
    let _execution = JitExecutionGuard::enter();
    #[cfg(windows)]
    {
        let callback: extern "system" fn(i32) -> i32 = unsafe { std::mem::transmute(address) };
        return Ok(callback(arg0));
    }
    #[cfg(not(windows))]
    {
        let callback: extern "C" fn(i32) -> i32 = unsafe { std::mem::transmute(address) };
        Ok(callback(arg0))
    }
}

pub fn invoke_i32_i32_to_i32(address: usize, left: i32, right: i32) -> Result<i32, String> {
    if address == 0 {
        return Err("cannot invoke null function pointer".to_string());
    }
    let _dispatch_lock = jit_dispatch_lock()
        .lock()
        .expect("jit dispatch lock mutex poisoned");
    let _execution = JitExecutionGuard::enter();
    #[cfg(windows)]
    {
        let callback: extern "system" fn(i32, i32) -> i32 = unsafe { std::mem::transmute(address) };
        return Ok(callback(left, right));
    }
    #[cfg(not(windows))]
    {
        let callback: extern "C" fn(i32, i32) -> i32 = unsafe { std::mem::transmute(address) };
        Ok(callback(left, right))
    }
}

pub fn invoke_i32_i32_i32_to_i32(
    address: usize,
    arg0: i32,
    arg1: i32,
    arg2: i32,
) -> Result<i32, String> {
    if address == 0 {
        return Err("cannot invoke null function pointer".to_string());
    }
    let _dispatch_lock = jit_dispatch_lock()
        .lock()
        .expect("jit dispatch lock mutex poisoned");
    let _execution = JitExecutionGuard::enter();
    #[cfg(windows)]
    {
        let callback: extern "system" fn(i32, i32, i32) -> i32 =
            unsafe { std::mem::transmute(address) };
        return Ok(callback(arg0, arg1, arg2));
    }
    #[cfg(not(windows))]
    {
        let callback: extern "C" fn(i32, i32, i32) -> i32 = unsafe { std::mem::transmute(address) };
        Ok(callback(arg0, arg1, arg2))
    }
}

pub fn invoke_i32_i32_i32_i32_to_void(
    address: usize,
    arg0: i32,
    arg1: i32,
    arg2: i32,
    arg3: i32,
) -> Result<(), String> {
    if address == 0 {
        return Err("cannot invoke null function pointer".to_string());
    }
    let _dispatch_lock = jit_dispatch_lock()
        .lock()
        .expect("jit dispatch lock mutex poisoned");
    let _execution = JitExecutionGuard::enter();
    #[cfg(windows)]
    {
        let callback: extern "system" fn(i32, i32, i32, i32) =
            unsafe { std::mem::transmute(address) };
        callback(arg0, arg1, arg2, arg3);
        return Ok(());
    }
    #[cfg(not(windows))]
    {
        let callback: extern "C" fn(i32, i32, i32, i32) = unsafe { std::mem::transmute(address) };
        callback(arg0, arg1, arg2, arg3);
        Ok(())
    }
}

pub fn invoke_i32_i32_i32_f32_to_void(
    address: usize,
    arg0: i32,
    arg1: i32,
    arg2: i32,
    arg3: f32,
) -> Result<(), String> {
    if address == 0 {
        return Err("cannot invoke null function pointer".to_string());
    }
    let _dispatch_lock = jit_dispatch_lock()
        .lock()
        .expect("jit dispatch lock mutex poisoned");
    let _execution = JitExecutionGuard::enter();
    #[cfg(windows)]
    {
        let callback: extern "system" fn(i32, i32, i32, f32) =
            unsafe { std::mem::transmute(address) };
        callback(arg0, arg1, arg2, arg3);
        return Ok(());
    }
    #[cfg(not(windows))]
    {
        let callback: extern "C" fn(i32, i32, i32, f32) = unsafe { std::mem::transmute(address) };
        callback(arg0, arg1, arg2, arg3);
        Ok(())
    }
}

// ============================================================
// stasis_graphics host API (dev in-process runner)
// ============================================================

const STASIS_GRAPHICS_RUNTIME_ABI_VERSION: i32 = 3;

fn verify_graphics_runtime_abi(lib: &Library, path: &Path) -> Result<(), String> {
    let address = lib
        .symbol_address("stasis_graphics_runtime_abi_version")
        .map_err(|_| {
            format!(
                "incompatible stasis graphics runtime {}: missing ABI version",
                path.display()
            )
        })?;
    #[cfg(windows)]
    let actual = {
        let version: extern "system" fn() -> i32 = unsafe { std::mem::transmute(address) };
        version()
    };
    #[cfg(not(windows))]
    let actual = {
        let version: extern "C" fn() -> i32 = unsafe { std::mem::transmute(address) };
        version()
    };
    if actual != STASIS_GRAPHICS_RUNTIME_ABI_VERSION {
        return Err(format!(
            "incompatible stasis graphics runtime {}: expected ABI {}, found {}",
            path.display(),
            STASIS_GRAPHICS_RUNTIME_ABI_VERSION,
            actual
        ));
    }
    Ok(())
}

pub fn graphics_runtime_release_id(path: &Path) -> Result<String, String> {
    let lib = Library::load(path)?;
    verify_graphics_runtime_abi(&lib, path)?;
    read_graphics_runtime_string(&lib, path, "stasis_graphics_release_id", "release identity")
}

pub fn graphics_runtime_build_fingerprint(path: &Path) -> Result<String, String> {
    let lib = Library::load(path)?;
    verify_graphics_runtime_abi(&lib, path)?;
    read_graphics_runtime_string(
        &lib,
        path,
        "stasis_graphics_build_fingerprint",
        "build fingerprint",
    )
}

pub fn graphics_runtime_identity(path: &Path) -> Result<(String, String), String> {
    let lib = Library::load(path)?;
    verify_graphics_runtime_abi(&lib, path)?;
    let release_id =
        read_graphics_runtime_string(&lib, path, "stasis_graphics_release_id", "release identity")?;
    let fingerprint = read_graphics_runtime_string(
        &lib,
        path,
        "stasis_graphics_build_fingerprint",
        "build fingerprint",
    )?;
    Ok((release_id, fingerprint))
}

/// Returns the fingerprint compiled into the Rust side of this toolchain.
///
/// An absent value is expected for ordinary source-tree development builds and
/// is deliberately not treated as an installed-toolchain identity.
pub fn compiled_build_fingerprint() -> Option<&'static str> {
    option_env!("STASIS_BUILD_FINGERPRINT")
        .map(str::trim)
        .filter(|value| is_verified_build_fingerprint(value))
}

pub fn is_verified_build_fingerprint(value: &str) -> bool {
    let value = value.trim();
    value.len() == 64
        && value.bytes().all(|byte| byte.is_ascii_hexdigit())
        && !value.eq_ignore_ascii_case("development")
}

pub fn verify_graphics_runtime_build_fingerprint(
    path: &Path,
    expected: &str,
) -> Result<(), String> {
    let lib = Library::load(path)?;
    verify_graphics_runtime_build_fingerprint_on_library(&lib, path, expected)
}

fn verify_graphics_runtime_build_fingerprint_on_library(
    lib: &Library,
    path: &Path,
    expected: &str,
) -> Result<(), String> {
    if !is_verified_build_fingerprint(expected) {
        return Err(
            "stasis CLI has no verified build fingerprint; refusing installed runtime startup"
                .to_string(),
        );
    }
    let actual = read_graphics_runtime_string(
        lib,
        path,
        "stasis_graphics_build_fingerprint",
        "build fingerprint",
    )?;
    if actual != expected {
        return Err(format!(
            "toolchain build fingerprint mismatch: stasis is '{expected}' but {} is '{actual}'",
            path.display()
        ));
    }
    Ok(())
}

fn read_graphics_runtime_string(
    lib: &Library,
    path: &Path,
    symbol: &str,
    label: &str,
) -> Result<String, String> {
    let address = lib.symbol_address(symbol).map_err(|_| {
        format!(
            "incompatible stasis graphics runtime {}: missing {label}",
            path.display()
        )
    })?;
    #[cfg(windows)]
    let value = {
        let identity: extern "system" fn() -> *const c_char =
            unsafe { std::mem::transmute(address) };
        identity()
    };
    #[cfg(not(windows))]
    let value = {
        let identity: extern "C" fn() -> *const c_char = unsafe { std::mem::transmute(address) };
        identity()
    };
    if value.is_null() {
        return Err(format!(
            "incompatible stasis graphics runtime {}: empty {label}",
            path.display(),
        ));
    }
    let value = unsafe { std::ffi::CStr::from_ptr(value) }
        .to_str()
        .map_err(|_| {
            format!(
                "incompatible stasis graphics runtime {}: {label} is not UTF-8",
                path.display(),
            )
        })?;
    if value.trim().is_empty() {
        return Err(format!(
            "incompatible stasis graphics runtime {}: empty {label}",
            path.display()
        ));
    }
    Ok(value.to_string())
}

pub struct StasisGraphicsApi {
    _lib: Library,
    stasis_init_window: usize,
    stasis_set_asset_root: usize,
    stasis_host_get_frame: usize,
    stasis_host_bulk_init: usize,
    stasis_host_bulk_apply_requests: usize,
    stasis_host_performance_metrics_enabled: usize,
    stasis_host_set_performance_metrics: usize,
    stasis_gfx_submit_u8: usize,
    stasis_set_recording_config: Option<usize>,
    stasis_set_recording_audio_config: Option<usize>,
    stasis_recording_audio_pull_f32_interleaved: Option<usize>,
    stasis_test_get_render_submission_state: Option<usize>,
    stasis_gfx_notify_file_changed: Option<usize>,
    stasis_load_font: Option<usize>,
    stasis_sleep_ms: usize,
}

impl StasisGraphicsApi {
    pub fn load_default() -> Result<Self, String> {
        let mut last_error = None;
        for candidate in runtime_library_candidate_paths() {
            if !candidate.exists() {
                continue;
            }
            match Self::load(&candidate) {
                Ok(api) => return Ok(api),
                Err(error) => last_error = Some(error),
            }
        }
        Err(last_error.unwrap_or_else(|| {
            "failed to load stasis_graphics runtime library (set STASIS_RUNTIME_LIBRARY_PATH or build runtime)"
                .to_string()
        }))
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let lib = Library::load(path)?;
        verify_graphics_runtime_abi(&lib, path)?;
        if let Some(expected) = option_env!("STASIS_BUILD_FINGERPRINT") {
            verify_graphics_runtime_build_fingerprint_on_library(&lib, path, expected)?;
        }
        if let Some(expected) = option_env!("STASIS_RELEASE_ID") {
            let actual = read_graphics_runtime_string(
                &lib,
                path,
                "stasis_graphics_release_id",
                "release identity",
            )?;
            if actual != expected {
                return Err(format!(
                    "toolchain release mismatch: stasis is '{expected}' but {} is '{actual}'",
                    path.display()
                ));
            }
        }
        let stasis_init_window = lib.symbol_address("stasis_init_window")?;
        let stasis_set_asset_root = lib.symbol_address("stasis_set_asset_root")?;
        let stasis_host_get_frame = lib.symbol_address("stasis_host_get_frame")?;
        let stasis_host_bulk_init = lib.symbol_address("stasis_host_bulk_init")?;
        let stasis_host_bulk_apply_requests =
            lib.symbol_address("stasis_host_bulk_apply_requests")?;
        let stasis_host_set_performance_metrics =
            lib.symbol_address("stasis_host_set_performance_metrics")?;
        let stasis_host_performance_metrics_enabled =
            lib.symbol_address("stasis_host_performance_metrics_enabled")?;
        let stasis_gfx_submit_u8 = lib.symbol_address("stasis_gfx_submit_u8")?;
        let stasis_set_recording_config = lib.symbol_address("stasis_set_recording_config").ok();
        let stasis_set_recording_audio_config =
            lib.symbol_address("stasis_set_recording_audio_config").ok();
        let stasis_recording_audio_pull_f32_interleaved = lib
            .symbol_address("stasis_recording_audio_pull_f32_interleaved")
            .ok();
        let stasis_test_get_render_submission_state = lib
            .symbol_address("stasis_test_get_render_submission_state")
            .ok();
        let stasis_gfx_notify_file_changed =
            lib.symbol_address("stasis_gfx_notify_file_changed").ok();
        let stasis_load_font = lib.symbol_address("stasis_load_font").ok();
        let stasis_sleep_ms = lib.symbol_address("stasis_sleep_ms")?;
        Ok(Self {
            _lib: lib,
            stasis_init_window,
            stasis_set_asset_root,
            stasis_host_get_frame,
            stasis_host_bulk_init,
            stasis_host_bulk_apply_requests,
            stasis_host_performance_metrics_enabled,
            stasis_host_set_performance_metrics,
            stasis_gfx_submit_u8,
            stasis_set_recording_config,
            stasis_set_recording_audio_config,
            stasis_recording_audio_pull_f32_interleaved,
            stasis_test_get_render_submission_state,
            stasis_gfx_notify_file_changed,
            stasis_load_font,
            stasis_sleep_ms,
        })
    }

    pub fn init_window(&self, width: i32, height: i32, title: &str) -> Result<bool, String> {
        #[cfg(windows)]
        {
            let title = CString::new(title)
                .map_err(|_| "window title contains interior NUL byte".to_string())?;
            let callback: extern "system" fn(i32, i32, *const c_char) -> i32 =
                unsafe { std::mem::transmute(self.stasis_init_window) };
            let rc = callback(width, height, title.as_ptr());
            return Ok(rc != 0);
        }
        #[cfg(not(windows))]
        {
            let title = CString::new(title)
                .map_err(|_| "window title contains interior NUL byte".to_string())?;
            let callback: extern "C" fn(i32, i32, *const c_char) -> i32 =
                unsafe { std::mem::transmute(self.stasis_init_window) };
            Ok(callback(width, height, title.as_ptr()) != 0)
        }
    }

    pub fn set_recording_config(&self, width: u32, height: u32, fps: u32) -> Result<(), String> {
        let symbol = self.stasis_set_recording_config.ok_or_else(|| {
            "graphics runtime lacks typed headless recording configuration support".to_string()
        })?;
        #[cfg(windows)]
        {
            let callback: extern "system" fn(i32, i32, u32) -> i32 =
                unsafe { std::mem::transmute(symbol) };
            if callback(width as i32, height as i32, fps) == 0 {
                return Err("graphics runtime rejected typed recording configuration".to_string());
            }
            return Ok(());
        }
        #[cfg(not(windows))]
        {
            let callback: extern "C" fn(i32, i32, u32) -> i32 =
                unsafe { std::mem::transmute(symbol) };
            if callback(width as i32, height as i32, fps) == 0 {
                return Err("graphics runtime rejected typed recording configuration".to_string());
            }
            Ok(())
        }
    }

    pub fn set_recording_audio_config(&self, enabled: bool) -> Result<(), String> {
        let symbol = self.stasis_set_recording_audio_config.ok_or_else(|| {
            "graphics runtime lacks offline recording audio configuration support".to_string()
        })?;
        #[cfg(windows)]
        {
            let callback: extern "system" fn(i32) -> i32 = unsafe { std::mem::transmute(symbol) };
            if callback(if enabled { 1 } else { 0 }) == 0 {
                return Err(
                    "graphics runtime rejected offline recording audio configuration".to_string(),
                );
            }
            return Ok(());
        }
        #[cfg(not(windows))]
        {
            let callback: extern "C" fn(i32) -> i32 = unsafe { std::mem::transmute(symbol) };
            if callback(if enabled { 1 } else { 0 }) == 0 {
                return Err(
                    "graphics runtime rejected offline recording audio configuration".to_string(),
                );
            }
            Ok(())
        }
    }

    pub fn pull_recording_audio_f32_interleaved(
        &self,
        output: &mut [f32],
    ) -> Result<usize, String> {
        if output.len() % 2 != 0 {
            return Err("recording audio output must contain stereo samples".to_string());
        }
        let symbol = self
            .stasis_recording_audio_pull_f32_interleaved
            .ok_or_else(|| {
                "graphics runtime lacks offline recording audio pull support".to_string()
            })?;
        let frame_count = output.len() / 2;
        if frame_count > i32::MAX as usize {
            return Err("recording audio pull exceeds runtime frame-count bound".to_string());
        }
        #[cfg(windows)]
        let callback: extern "system" fn(*mut f32, i32) -> i32 =
            unsafe { std::mem::transmute(symbol) };
        #[cfg(not(windows))]
        let callback: extern "C" fn(*mut f32, i32) -> i32 = unsafe { std::mem::transmute(symbol) };
        let accepted = callback(output.as_mut_ptr(), frame_count as i32);
        if accepted < 0 || accepted as usize != frame_count {
            return Err(format!(
                "graphics runtime returned {accepted} recording audio frames, expected {frame_count}"
            ));
        }
        Ok(accepted as usize)
    }

    pub fn set_asset_root(&self, path: &Path) -> Result<(), String> {
        let path = CString::new(path.to_string_lossy().as_bytes())
            .map_err(|_| "asset root contains an interior NUL byte".to_string())?;
        #[cfg(windows)]
        {
            let callback: extern "system" fn(*const c_char) -> i32 =
                unsafe { std::mem::transmute(self.stasis_set_asset_root) };
            if callback(path.as_ptr()) == 0 {
                return Err(format!(
                    "graphics runtime rejected asset root {}",
                    path.to_string_lossy()
                ));
            }
            return Ok(());
        }
        #[cfg(not(windows))]
        {
            let callback: extern "C" fn(*const c_char) -> i32 =
                unsafe { std::mem::transmute(self.stasis_set_asset_root) };
            if callback(path.as_ptr()) == 0 {
                return Err(format!(
                    "graphics runtime rejected asset root {}",
                    path.to_string_lossy()
                ));
            }
            Ok(())
        }
    }

    pub fn host_get_frame(&self, out_i32: &mut [i32], out_f32: &mut [f32]) -> Result<(), String> {
        #[cfg(windows)]
        {
            let callback: extern "system" fn(*mut i32, *mut f32) =
                unsafe { std::mem::transmute(self.stasis_host_get_frame) };
            callback(out_i32.as_mut_ptr(), out_f32.as_mut_ptr());
            return Ok(());
        }
        #[cfg(not(windows))]
        {
            let callback: extern "C" fn(*mut i32, *mut f32) =
                unsafe { std::mem::transmute(self.stasis_host_get_frame) };
            callback(out_i32.as_mut_ptr(), out_f32.as_mut_ptr());
            Ok(())
        }
    }

    pub fn host_bulk_init(&self, host_req_seq: &i32) -> Result<(), String> {
        #[cfg(windows)]
        {
            let callback: extern "system" fn(*const i32) =
                unsafe { std::mem::transmute(self.stasis_host_bulk_init) };
            callback(host_req_seq as *const i32);
            return Ok(());
        }
        #[cfg(not(windows))]
        {
            let callback: extern "C" fn(*const i32) =
                unsafe { std::mem::transmute(self.stasis_host_bulk_init) };
            callback(host_req_seq as *const i32);
            Ok(())
        }
    }

    pub fn host_bulk_apply_requests(
        &self,
        host_req_seq: &i32,
        host_req_flags: &i32,
        host_req_window_w_px: &i32,
        host_req_window_h_px: &i32,
    ) -> Result<(), String> {
        #[cfg(windows)]
        {
            let callback: extern "system" fn(*const i32, *const i32, *const i32, *const i32) =
                unsafe { std::mem::transmute(self.stasis_host_bulk_apply_requests) };
            callback(
                host_req_seq as *const i32,
                host_req_flags as *const i32,
                host_req_window_w_px as *const i32,
                host_req_window_h_px as *const i32,
            );
            return Ok(());
        }
        #[cfg(not(windows))]
        {
            let callback: extern "C" fn(*const i32, *const i32, *const i32, *const i32) =
                unsafe { std::mem::transmute(self.stasis_host_bulk_apply_requests) };
            callback(
                host_req_seq as *const i32,
                host_req_flags as *const i32,
                host_req_window_w_px as *const i32,
                host_req_window_h_px as *const i32,
            );
            Ok(())
        }
    }

    pub fn host_set_performance_metrics(
        &self,
        tick_micros: u64,
        render_micros: u64,
    ) -> Result<(), String> {
        #[cfg(windows)]
        {
            let callback: extern "system" fn(u64, u64) =
                unsafe { std::mem::transmute(self.stasis_host_set_performance_metrics) };
            callback(tick_micros, render_micros);
            return Ok(());
        }
        #[cfg(not(windows))]
        {
            let callback: extern "C" fn(u64, u64) =
                unsafe { std::mem::transmute(self.stasis_host_set_performance_metrics) };
            callback(tick_micros, render_micros);
            Ok(())
        }
    }

    pub fn host_performance_metrics_enabled(&self) -> Result<bool, String> {
        #[cfg(windows)]
        {
            let callback: extern "system" fn() -> i32 =
                unsafe { std::mem::transmute(self.stasis_host_performance_metrics_enabled) };
            return Ok(callback() != 0);
        }
        #[cfg(not(windows))]
        {
            let callback: extern "C" fn() -> i32 =
                unsafe { std::mem::transmute(self.stasis_host_performance_metrics_enabled) };
            Ok(callback() != 0)
        }
    }

    pub fn gfx_submit_u8(
        &self,
        cmd_i32: &mut [i32],
        cmd_f32: &[f32],
        cmd_u8: &[u8],
    ) -> Result<(), String> {
        #[cfg(windows)]
        {
            let callback: extern "system" fn(*mut i32, *const f32, *const u8) =
                unsafe { std::mem::transmute(self.stasis_gfx_submit_u8) };
            callback(cmd_i32.as_mut_ptr(), cmd_f32.as_ptr(), cmd_u8.as_ptr());
            return Ok(());
        }
        #[cfg(not(windows))]
        {
            let callback: extern "C" fn(*mut i32, *const f32, *const u8) =
                unsafe { std::mem::transmute(self.stasis_gfx_submit_u8) };
            callback(cmd_i32.as_mut_ptr(), cmd_f32.as_ptr(), cmd_u8.as_ptr());
            Ok(())
        }
    }

    pub fn test_render_submission_state(&self) -> Result<Option<[i32; 5]>, String> {
        let Some(address) = self.stasis_test_get_render_submission_state else {
            return Ok(None);
        };
        let mut state = [0; 5];
        #[cfg(windows)]
        {
            let callback: extern "system" fn(*mut i32, i32) -> i32 =
                unsafe { std::mem::transmute(address) };
            return Ok((callback(state.as_mut_ptr(), state.len() as i32) != 0).then_some(state));
        }
        #[cfg(not(windows))]
        {
            let callback: extern "C" fn(*mut i32, i32) -> i32 =
                unsafe { std::mem::transmute(address) };
            Ok((callback(state.as_mut_ptr(), state.len() as i32) != 0).then_some(state))
        }
    }

    pub fn notify_file_changed(&self, path: &Path) -> Result<(), String> {
        let Some(address) = self.stasis_gfx_notify_file_changed else {
            return Ok(());
        };
        let path = CString::new(path.to_string_lossy().as_bytes())
            .map_err(|_| "changed asset path contains interior NUL byte".to_string())?;
        #[cfg(windows)]
        {
            let callback: extern "system" fn(*const c_char) =
                unsafe { std::mem::transmute(address) };
            callback(path.as_ptr());
            return Ok(());
        }
        #[cfg(not(windows))]
        {
            let callback: extern "C" fn(*const c_char) = unsafe { std::mem::transmute(address) };
            callback(path.as_ptr());
            Ok(())
        }
    }

    /// Load a font through the existing path-based graphics runtime symbol.
    pub fn load_font(&self, path: &Path, size: i32) -> Result<i32, String> {
        let address = self
            .stasis_load_font
            .ok_or_else(|| "graphics runtime lacks font loading support".to_string())?;
        let path = CString::new(path.to_string_lossy().as_bytes())
            .map_err(|_| "font path contains an interior NUL byte".to_string())?;
        #[cfg(windows)]
        let callback: extern "system" fn(*const c_char, i32) -> i32 =
            unsafe { std::mem::transmute(address) };
        #[cfg(not(windows))]
        let callback: extern "C" fn(*const c_char, i32) -> i32 =
            unsafe { std::mem::transmute(address) };
        let handle = callback(path.as_ptr(), size);
        if handle <= 0 {
            return Err(format!(
                "graphics runtime rejected font {}",
                path.to_string_lossy()
            ));
        }
        Ok(handle)
    }

    pub fn sleep_ms(&self, ms: i32) -> Result<(), String> {
        #[cfg(windows)]
        {
            let callback: extern "system" fn(i32) =
                unsafe { std::mem::transmute(self.stasis_sleep_ms) };
            callback(ms);
            return Ok(());
        }
        #[cfg(not(windows))]
        {
            let callback: extern "C" fn(i32) = unsafe { std::mem::transmute(self.stasis_sleep_ms) };
            callback(ms);
            Ok(())
        }
    }
}

// ============================================================
// stasis_graphics asset API (JIT extern call bridge)
// ============================================================

struct StasisGraphicsAssetsApi {
    _lib: Library,
    stasis_gfx_load_sprite: usize,
    stasis_gfx_set_next_sprite_atlas_policy_v3: Option<usize>,
    stasis_asset_request_sprite_with_policy_v3: Option<usize>,
    stasis_asset_request_sprite: Option<usize>,
    stasis_asset_request_audio: Option<usize>,
    stasis_asset_task_poll: Option<usize>,
    stasis_asset_task_take_handle: Option<usize>,
    stasis_asset_task_cancel: Option<usize>,
    stasis_gfx_release_sprite: usize,
    stasis_gfx_dump_bmp: usize,
    stasis_gfx_dump_png: Option<usize>,
    stasis_host_schedule_screenshot: Option<usize>,
    stasis_gfx_poll_reload: usize,
    stasis_load_font: usize,
    stasis_measure_text: usize,
    stasis_gfx_cache_text: usize,
    stasis_gfx_replace_text: Option<usize>,
    stasis_gfx_measure_text_cached: usize,
    stasis_gfx_measure_text_cached_height: usize,
    stasis_clipboard_load_ascii: Option<usize>,
    stasis_clipboard_save_ascii: Option<usize>,
    stasis_audio_init: Option<usize>,
    stasis_audio_shutdown: Option<usize>,
    stasis_audio_is_available: Option<usize>,
    stasis_audio_get_sample_rate: Option<usize>,
    stasis_audio_get_channels: Option<usize>,
    stasis_audio_get_queued_frames: Option<usize>,
    stasis_audio_get_underruns: Option<usize>,
    stasis_audio_push_f32_interleaved: Option<usize>,
    stasis_audio_load_wav: Option<usize>,
    stasis_audio_release: Option<usize>,
    stasis_audio_play: Option<usize>,
    stasis_audio_stop: Option<usize>,
    stasis_audio_voice_is_playing: Option<usize>,
    stasis_audio_voice_set_paused: Option<usize>,
    stasis_audio_voice_set_volume_pan: Option<usize>,
    stasis_audio_load_music: Option<usize>,
    stasis_audio_load_effect: Option<usize>,
    stasis_audio_play_music: Option<usize>,
    stasis_audio_stop_music: Option<usize>,
    stasis_audio_pause_music: Option<usize>,
    stasis_audio_set_music_volume: Option<usize>,
    stasis_audio_play_effect: Option<usize>,
    #[cfg(windows)]
    stasis_storage_load_ascii: Option<usize>,
    #[cfg(windows)]
    stasis_storage_load_i32: Option<usize>,
    #[cfg(windows)]
    stasis_storage_save_ascii: Option<usize>,
    #[cfg(windows)]
    stasis_storage_save_i32: Option<usize>,
}

impl StasisGraphicsAssetsApi {
    fn load_default() -> Result<Self, String> {
        let mut last_error = None;
        for candidate in runtime_library_candidate_paths() {
            if !candidate.exists() {
                continue;
            }
            match Self::load(&candidate) {
                Ok(api) => return Ok(api),
                Err(error) => last_error = Some(error),
            }
        }
        Err(last_error.unwrap_or_else(|| {
            "failed to load stasis_graphics runtime library for asset calls".to_string()
        }))
    }

    fn load(path: &Path) -> Result<Self, String> {
        let lib = Library::load(path)?;
        verify_graphics_runtime_abi(&lib, path)?;
        Ok(Self {
            stasis_gfx_load_sprite: lib.symbol_address("stasis_gfx_load_sprite")?,
            stasis_gfx_set_next_sprite_atlas_policy_v3: lib
                .symbol_address("stasis_gfx_set_next_sprite_atlas_policy_v3")
                .ok(),
            stasis_asset_request_sprite_with_policy_v3: lib
                .symbol_address("stasis_asset_request_sprite_with_policy_v3")
                .ok(),
            stasis_asset_request_sprite: lib.symbol_address("stasis_asset_request_sprite").ok(),
            stasis_asset_request_audio: lib.symbol_address("stasis_asset_request_audio").ok(),
            stasis_asset_task_poll: lib.symbol_address("stasis_asset_task_poll").ok(),
            stasis_asset_task_take_handle: lib.symbol_address("stasis_asset_task_take_handle").ok(),
            stasis_asset_task_cancel: lib.symbol_address("stasis_asset_task_cancel").ok(),
            stasis_gfx_release_sprite: lib.symbol_address("stasis_gfx_release_sprite")?,
            stasis_gfx_dump_bmp: lib.symbol_address("stasis_gfx_dump_bmp")?,
            // PNG capture was added after the original asset ABI. Keep older runtimes usable for
            // all pre-existing calls and report PNG as unsupported.
            stasis_gfx_dump_png: lib.symbol_address("stasis_gfx_dump_png").ok(),
            stasis_host_schedule_screenshot: lib
                .symbol_address("stasis_host_schedule_screenshot")
                .ok(),
            stasis_gfx_poll_reload: lib.symbol_address("stasis_gfx_poll_reload")?,
            stasis_load_font: lib.symbol_address("stasis_load_font")?,
            stasis_measure_text: lib.symbol_address("stasis_measure_text")?,
            stasis_gfx_cache_text: lib.symbol_address("stasis_gfx_cache_text")?,
            // Replaceable runs are additive to graphics ABI 3. Older runtimes remain usable for
            // immutable TextRuns and report replacement as unsupported.
            stasis_gfx_replace_text: lib.symbol_address("stasis_gfx_replace_text").ok(),
            stasis_gfx_measure_text_cached: lib.symbol_address("stasis_gfx_measure_text_cached")?,
            stasis_gfx_measure_text_cached_height: lib
                .symbol_address("stasis_gfx_measure_text_cached_height")?,
            stasis_clipboard_load_ascii: lib.symbol_address("stasis_clipboard_load_ascii").ok(),
            stasis_clipboard_save_ascii: lib.symbol_address("stasis_clipboard_save_ascii").ok(),
            stasis_audio_init: lib.symbol_address("stasis_audio_init").ok(),
            stasis_audio_shutdown: lib.symbol_address("stasis_audio_shutdown").ok(),
            stasis_audio_is_available: lib.symbol_address("stasis_audio_is_available").ok(),
            stasis_audio_get_sample_rate: lib.symbol_address("stasis_audio_get_sample_rate").ok(),
            stasis_audio_get_channels: lib.symbol_address("stasis_audio_get_channels").ok(),
            stasis_audio_get_queued_frames: lib
                .symbol_address("stasis_audio_get_queued_frames")
                .ok(),
            stasis_audio_get_underruns: lib.symbol_address("stasis_audio_get_underruns").ok(),
            stasis_audio_push_f32_interleaved: lib
                .symbol_address("stasis_audio_push_f32_interleaved")
                .ok(),
            stasis_audio_load_wav: lib.symbol_address("stasis_audio_load_wav").ok(),
            stasis_audio_release: lib.symbol_address("stasis_audio_release").ok(),
            stasis_audio_play: lib.symbol_address("stasis_audio_play").ok(),
            stasis_audio_stop: lib.symbol_address("stasis_audio_stop").ok(),
            stasis_audio_voice_is_playing: lib.symbol_address("stasis_audio_voice_is_playing").ok(),
            stasis_audio_voice_set_paused: lib.symbol_address("stasis_audio_voice_set_paused").ok(),
            stasis_audio_voice_set_volume_pan: lib
                .symbol_address("stasis_audio_voice_set_volume_pan")
                .ok(),
            stasis_audio_load_music: lib.symbol_address("stasis_audio_load_music").ok(),
            stasis_audio_load_effect: lib.symbol_address("stasis_audio_load_effect").ok(),
            stasis_audio_play_music: lib.symbol_address("stasis_audio_play_music").ok(),
            stasis_audio_stop_music: lib.symbol_address("stasis_audio_stop_music").ok(),
            stasis_audio_pause_music: lib.symbol_address("stasis_audio_pause_music").ok(),
            stasis_audio_set_music_volume: lib.symbol_address("stasis_audio_set_music_volume").ok(),
            stasis_audio_play_effect: lib.symbol_address("stasis_audio_play_effect").ok(),
            #[cfg(windows)]
            stasis_storage_load_ascii: lib.symbol_address("stasis_storage_load_ascii").ok(),
            #[cfg(windows)]
            stasis_storage_load_i32: lib.symbol_address("stasis_storage_load_i32").ok(),
            #[cfg(windows)]
            stasis_storage_save_ascii: lib.symbol_address("stasis_storage_save_ascii").ok(),
            #[cfg(windows)]
            stasis_storage_save_i32: lib.symbol_address("stasis_storage_save_i32").ok(),
            _lib: lib,
        })
    }
}

pub fn schedule_runtime_screenshot(path: &Path) -> Result<(), String> {
    let api = stasis_graphics_assets_api()?;
    let address = api.stasis_host_schedule_screenshot.ok_or_else(|| {
        "the loaded graphics runtime does not support dynamic screenshots".to_string()
    })?;
    let path = CString::new(path.to_string_lossy().as_bytes())
        .map_err(|_| "screenshot path contains an interior NUL byte".to_string())?;
    #[cfg(windows)]
    let callback: extern "system" fn(*const c_char) -> i32 =
        unsafe { std::mem::transmute(address) };
    #[cfg(not(windows))]
    let callback: extern "C" fn(*const c_char) -> i32 = unsafe { std::mem::transmute(address) };
    if callback(path.as_ptr()) == 0 {
        return Err("graphics runtime rejected the screenshot path".to_string());
    }
    Ok(())
}

fn stasis_graphics_assets_api() -> Result<&'static StasisGraphicsAssetsApi, String> {
    static API: OnceLock<Result<StasisGraphicsAssetsApi, String>> = OnceLock::new();
    match API.get_or_init(StasisGraphicsAssetsApi::load_default) {
        Ok(api) => Ok(api),
        Err(error) => Err(error.clone()),
    }
}

pub fn runtime_library_candidate_paths() -> Vec<PathBuf> {
    let configured = [
        std::env::var_os("STASIS_RUNTIME_LIBRARY_PATH"),
        // Preserve the original variable as a compatibility alias for existing Windows workflows.
        std::env::var_os("STASIS_RUNTIME_DLL_PATH"),
    ]
    .into_iter()
    .flatten()
    .map(PathBuf::from)
    .collect::<Vec<_>>();
    let executable_dir = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf));
    runtime_library_candidate_paths_for(executable_dir.as_deref(), &configured)
}

fn runtime_library_candidate_paths_for(
    executable_dir: Option<&Path>,
    configured: &[PathBuf],
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(exe_dir) = executable_dir {
        // A release bundle is one unit. Never let an environment override replace its sibling
        // runtime with a different build.
        for file_name in runtime_library_file_names() {
            out.push(exe_dir.join(file_name));
        }
        out.extend(configured.iter().cloned());

        // Dev-friendly default: locate the runtime built under the repo tree by
        // walking a few parents from the executable location.
        for ancestor in exe_dir.ancestors().take(6) {
            for file_name in runtime_library_file_names() {
                for configuration in [None, Some("Release"), Some("Debug")] {
                    let mut candidate = ancestor.join("runtime").join("build").join("bin");
                    if let Some(configuration) = configuration {
                        candidate.push(configuration);
                    }
                    candidate.push(file_name);
                    out.push(candidate);
                }
            }
        }
    } else {
        // If there is no executable directory, the configured path is the only explicit fallback.
        out.extend(configured.iter().cloned());
    }

    // Allow loading from the current working directory too (handy for ad-hoc runs).
    for file_name in runtime_library_file_names() {
        out.push(PathBuf::from(file_name));
    }

    // Dev-friendly fallback: if the runtime exists under this repo checkout, include it.
    // This helps when `CARGO_TARGET_DIR` points outside the workspace (so `current_exe()` ancestry
    // won't include the repo root).
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    for file_name in runtime_library_file_names() {
        for configuration in [None, Some("Release"), Some("Debug")] {
            let mut candidate = repo_root.join("runtime").join("build").join("bin");
            if let Some(configuration) = configuration {
                candidate.push(configuration);
            }
            candidate.push(file_name);
            if candidate.exists() {
                out.push(candidate);
            }
        }
    }
    out
}

#[cfg(windows)]
fn runtime_library_file_names() -> &'static [&'static str] {
    &["stasis_graphics.dll"]
}

#[cfg(target_os = "macos")]
fn runtime_library_file_names() -> &'static [&'static str] {
    &["libstasis_graphics.dylib", "stasis_graphics.dylib"]
}

#[cfg(all(unix, not(target_os = "macos")))]
fn runtime_library_file_names() -> &'static [&'static str] {
    &["libstasis_graphics.so", "stasis_graphics.so"]
}

pub fn clear_jit_string_literal_table() {
    let table = jit_string_literal_table();
    let mut guard = table
        .lock()
        .expect("jit string literal table mutex poisoned");
    guard.clear();
}

pub fn begin_jit_string_literal_staging() -> Result<(), String> {
    JIT_STRING_LITERAL_STAGE.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_some() {
            return Err("JIT string literal staging is already active on this thread".to_string());
        }
        *slot = Some(JitStringLiteralStage::default());
        Ok(())
    })
}

pub fn finish_jit_string_literal_staging() -> Result<HashMap<i32, String>, String> {
    JIT_STRING_LITERAL_STAGE.with(|slot| {
        let stage = slot
            .borrow_mut()
            .take()
            .ok_or_else(|| "JIT string literal staging is not active".to_string())?;
        if let Some(error) = stage.collision {
            Err(error)
        } else {
            Ok(stage.literals)
        }
    })
}

pub fn replace_jit_string_literal_table(literals: &HashMap<i32, String>) {
    let table = jit_string_literal_table();
    let mut guard = table
        .lock()
        .expect("jit string literal table mutex poisoned");
    guard.clear();
    guard.extend(literals.iter().map(|(id, value)| (*id, value.clone())));
}

pub fn upsert_jit_string_literal(id: i32, value: &str) {
    let staged = JIT_STRING_LITERAL_STAGE.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(stage) = slot.as_mut() else {
            return false;
        };
        if let Some(previous) = stage.literals.get(&id) {
            if previous != value && stage.collision.is_none() {
                stage.collision = Some(format!(
                    "JIT string literal hash collision for id {id}: '{previous}' vs '{value}'"
                ));
            }
        } else {
            stage.literals.insert(id, value.to_string());
        }
        true
    });
    if staged {
        return;
    }
    let table = jit_string_literal_table();
    let mut guard = table
        .lock()
        .expect("jit string literal table mutex poisoned");
    guard.insert(id, value.to_string());
}

#[no_mangle]
pub extern "C" fn stasis_jit_clear_string_literal_table() {
    clear_jit_string_literal_table();
}

#[no_mangle]
pub extern "C" fn stasis_jit_upsert_string_literal(id: i32, value: *const c_char) {
    if value.is_null() {
        return;
    }
    #[cfg(windows)]
    let text = unsafe { std::ffi::CStr::from_ptr(value) };
    #[cfg(not(windows))]
    let text = unsafe { std::ffi::CStr::from_ptr(value) };
    if let Ok(text) = text.to_str() {
        upsert_jit_string_literal(id, text);
    }
}

pub fn jit_string_literal_value(id: i32) -> Option<String> {
    let table = jit_string_literal_table();
    let guard = table
        .lock()
        .expect("jit string literal table mutex poisoned");
    guard.get(&id).cloned()
}

// ============================================================
// Registered global memory (in-process engine)
// ============================================================

type ArrayKey = (i32, i32); // (collection_hash, field_hash)

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum JitStorageKind {
    I32,
    F32,
    F64,
    U8,
    U16,
}

#[repr(C)]
pub struct JitStorageSlot {
    data: usize,
    len: usize,
}

impl JitStorageSlot {
    pub const DATA_OFFSET: i32 = 0;
    pub const LEN_OFFSET: i32 = std::mem::size_of::<usize>() as i32;
}

// Slot fields are only rebound while no guest execution window is active. Generated code reads
// them directly, so the registry mutex is deliberately absent from the load/store hot path.
unsafe impl Send for JitStorageSlot {}
unsafe impl Sync for JitStorageSlot {}

type StorageKey = (JitStorageKind, i32, i32);

fn direct_storage_slots() -> &'static Mutex<HashMap<StorageKey, Box<JitStorageSlot>>> {
    static TABLE: OnceLock<Mutex<HashMap<StorageKey, Box<JitStorageSlot>>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn direct_array_required_lengths() -> &'static Mutex<HashMap<StorageKey, usize>> {
    static TABLE: OnceLock<Mutex<HashMap<StorageKey, usize>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn accepts_direct_array_rebind(key: StorageKey, len: usize) -> bool {
    direct_array_required_lengths()
        .lock()
        .expect("direct array required-length table mutex poisoned")
        .get(&key)
        .is_none_or(|required| len >= *required)
}

fn guest_execution_count() -> &'static AtomicUsize {
    static COUNT: AtomicUsize = AtomicUsize::new(0);
    &COUNT
}

pub struct JitExecutionGuard;

impl JitExecutionGuard {
    pub fn enter() -> Self {
        guest_execution_count().fetch_add(1, Ordering::AcqRel);
        Self
    }
}

impl Drop for JitExecutionGuard {
    fn drop(&mut self) {
        guest_execution_count().fetch_sub(1, Ordering::AcqRel);
    }
}

fn ensure_rebind_allowed() -> Result<(), String> {
    (guest_execution_count().load(Ordering::Acquire) == 0)
        .then_some(())
        .ok_or_else(|| {
            "storage rebinding is only allowed between guest execution windows".to_string()
        })
}

fn acquire_rebind_guard() -> Result<MutexGuard<'static, ()>, String> {
    ensure_rebind_allowed()?;
    let guard = jit_dispatch_lock()
        .lock()
        .expect("jit dispatch lock mutex poisoned");
    ensure_rebind_allowed()?;
    Ok(guard)
}

fn update_direct_storage_slot(key: StorageKey, data: usize, len: usize) {
    if let Some(slot) = direct_storage_slots()
        .lock()
        .expect("direct storage slot table mutex poisoned")
        .get_mut(&key)
    {
        slot.data = data;
        slot.len = len;
    }
}

pub fn direct_scalar_storage_slot_address(
    kind: JitStorageKind,
    path_hash: i32,
) -> Result<usize, String> {
    direct_storage_slot_address(kind, path_hash, 0)
}

pub fn direct_array_storage_slot_address(
    kind: JitStorageKind,
    collection_hash: i32,
    field_hash: i32,
) -> Result<usize, String> {
    direct_storage_slot_address(kind, collection_hash, field_hash)
}

#[doc(hidden)]
pub fn direct_array_storage_slot_len_for_test(
    kind: JitStorageKind,
    collection_hash: i32,
    field_hash: i32,
) -> Option<usize> {
    direct_storage_slots()
        .lock()
        .expect("direct storage slot table mutex poisoned")
        .get(&(kind, collection_hash, field_hash))
        .map(|slot| slot.len)
}

fn direct_storage_slot_address(
    kind: JitStorageKind,
    collection_hash: i32,
    field_hash: i32,
) -> Result<usize, String> {
    let key = (kind, collection_hash, field_hash);
    let current = registered_storage(kind, collection_hash, field_hash);
    let (data, actual_len) = current.unwrap_or((0, 0));
    let mut slots = direct_storage_slots()
        .lock()
        .expect("direct storage slot table mutex poisoned");
    let slot = slots.entry(key).or_insert_with(|| {
        Box::new(JitStorageSlot {
            data,
            len: actual_len,
        })
    });
    Ok(slot.as_ref() as *const JitStorageSlot as usize)
}

pub fn provision_direct_scalar_storage(kind: JitStorageKind, path_hash: i32) -> Result<(), String> {
    let _rebind = acquire_rebind_guard()?;
    match kind {
        JitStorageKind::I32 => ensure_owned_i32_scalar(path_hash)?,
        JitStorageKind::F32 => ensure_owned_f32_scalar(path_hash)?,
        JitStorageKind::F64 => ensure_owned_f64_scalar(path_hash)?,
        JitStorageKind::U8 => ensure_jit_u8_array_capacity_unlocked(path_hash, 0, 1)?,
        JitStorageKind::U16 => ensure_jit_u16_array_capacity_unlocked(path_hash, 0, 1)?,
    }
    let (data, len) = registered_storage(kind, path_hash, 0)?;
    update_direct_storage_slot((kind, path_hash, 0), data, len);
    Ok(())
}

pub fn provision_direct_array_storage(
    kind: JitStorageKind,
    collection_hash: i32,
    field_hash: i32,
    len: usize,
) -> Result<(), String> {
    let _rebind = acquire_rebind_guard()?;
    let key = (kind, collection_hash, field_hash);
    match kind {
        JitStorageKind::I32 => {
            ensure_jit_i32_array_capacity_unlocked(collection_hash, field_hash, len)?
        }
        JitStorageKind::F32 => {
            ensure_jit_f32_array_capacity_unlocked(collection_hash, field_hash, len)?
        }
        JitStorageKind::F64 => {
            ensure_jit_f64_array_capacity_unlocked(collection_hash, field_hash, len)?
        }
        JitStorageKind::U8 => {
            ensure_jit_u8_array_capacity_unlocked(collection_hash, field_hash, len)?
        }
        JitStorageKind::U16 => {
            ensure_jit_u16_array_capacity_unlocked(collection_hash, field_hash, len)?
        }
    }
    let (data, actual_len) = registered_storage(kind, collection_hash, field_hash)?;
    direct_array_required_lengths()
        .lock()
        .expect("direct array required-length table mutex poisoned")
        .insert(key, len);
    update_direct_storage_slot(key, data, actual_len);
    Ok(())
}

fn registered_i32_ptrs() -> &'static Mutex<HashMap<i32, usize>> {
    static TABLE: OnceLock<Mutex<HashMap<i32, usize>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn registered_f32_ptrs() -> &'static Mutex<HashMap<i32, usize>> {
    static TABLE: OnceLock<Mutex<HashMap<i32, usize>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn registered_f64_ptrs() -> &'static Mutex<HashMap<i32, usize>> {
    static TABLE: OnceLock<Mutex<HashMap<i32, usize>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn registered_i32_arrays() -> &'static Mutex<HashMap<ArrayKey, (usize, usize)>> {
    static TABLE: OnceLock<Mutex<HashMap<ArrayKey, (usize, usize)>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn registered_f32_arrays() -> &'static Mutex<HashMap<ArrayKey, (usize, usize)>> {
    static TABLE: OnceLock<Mutex<HashMap<ArrayKey, (usize, usize)>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn registered_f64_arrays() -> &'static Mutex<HashMap<ArrayKey, (usize, usize)>> {
    static TABLE: OnceLock<Mutex<HashMap<ArrayKey, (usize, usize)>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn registered_u8_arrays() -> &'static Mutex<HashMap<ArrayKey, (usize, usize)>> {
    static TABLE: OnceLock<Mutex<HashMap<ArrayKey, (usize, usize)>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn registered_u16_arrays() -> &'static Mutex<HashMap<ArrayKey, (usize, usize)>> {
    static TABLE: OnceLock<Mutex<HashMap<ArrayKey, (usize, usize)>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn owned_f32_arrays() -> &'static Mutex<HashMap<ArrayKey, Vec<f32>>> {
    static TABLE: OnceLock<Mutex<HashMap<ArrayKey, Vec<f32>>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn owned_i32_arrays() -> &'static Mutex<HashMap<ArrayKey, Vec<i32>>> {
    static TABLE: OnceLock<Mutex<HashMap<ArrayKey, Vec<i32>>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn owned_u8_arrays() -> &'static Mutex<HashMap<ArrayKey, Vec<u8>>> {
    static TABLE: OnceLock<Mutex<HashMap<ArrayKey, Vec<u8>>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn owned_u16_arrays() -> &'static Mutex<HashMap<ArrayKey, Vec<u16>>> {
    static TABLE: OnceLock<Mutex<HashMap<ArrayKey, Vec<u16>>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn owned_f64_arrays() -> &'static Mutex<HashMap<ArrayKey, Vec<f64>>> {
    static TABLE: OnceLock<Mutex<HashMap<ArrayKey, Vec<f64>>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn owned_i32_scalars() -> &'static Mutex<HashMap<i32, Box<i32>>> {
    static TABLE: OnceLock<Mutex<HashMap<i32, Box<i32>>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn owned_f32_scalars() -> &'static Mutex<HashMap<i32, Box<f32>>> {
    static TABLE: OnceLock<Mutex<HashMap<i32, Box<f32>>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn owned_f64_scalars() -> &'static Mutex<HashMap<i32, Box<f64>>> {
    static TABLE: OnceLock<Mutex<HashMap<i32, Box<f64>>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn ensure_owned_scalar<T: Copy + Default>(
    path_hash: i32,
    owned: &Mutex<HashMap<i32, Box<T>>>,
    registered: &Mutex<HashMap<i32, usize>>,
    fallback: &Mutex<HashMap<i32, T>>,
) -> Result<(), String> {
    if registered
        .lock()
        .expect("registered scalar table mutex poisoned")
        .contains_key(&path_hash)
    {
        return Ok(());
    }
    ensure_rebind_allowed()?;
    let initial = fallback
        .lock()
        .expect("fallback scalar table mutex poisoned")
        .remove(&path_hash)
        .unwrap_or_default();
    let mut owned = owned.lock().expect("owned scalar table mutex poisoned");
    let value = owned.entry(path_hash).or_insert_with(|| Box::new(initial));
    let address = value.as_mut() as *mut T as usize;
    registered
        .lock()
        .expect("registered scalar table mutex poisoned")
        .insert(path_hash, address);
    Ok(())
}

fn ensure_owned_i32_scalar(path_hash: i32) -> Result<(), String> {
    ensure_owned_scalar(
        path_hash,
        owned_i32_scalars(),
        registered_i32_ptrs(),
        jit_i32_global_table(),
    )
}

fn ensure_owned_f32_scalar(path_hash: i32) -> Result<(), String> {
    ensure_owned_scalar(
        path_hash,
        owned_f32_scalars(),
        registered_f32_ptrs(),
        jit_f32_global_table(),
    )
}

fn ensure_owned_f64_scalar(path_hash: i32) -> Result<(), String> {
    ensure_owned_scalar(
        path_hash,
        owned_f64_scalars(),
        registered_f64_ptrs(),
        jit_f64_global_table(),
    )
}

fn registered_storage(
    kind: JitStorageKind,
    collection_hash: i32,
    field_hash: i32,
) -> Result<(usize, usize), String> {
    let value = match kind {
        JitStorageKind::I32 if field_hash == 0 => registered_i32_ptrs()
            .lock()
            .expect("registered i32 ptr table mutex poisoned")
            .get(&collection_hash)
            .copied()
            .map(|ptr| (ptr, 1))
            .or_else(|| {
                registered_i32_arrays()
                    .lock()
                    .expect("registered i32 array table mutex poisoned")
                    .get(&(collection_hash, field_hash))
                    .copied()
            }),
        JitStorageKind::I32 => registered_i32_arrays()
            .lock()
            .expect("registered i32 array table mutex poisoned")
            .get(&(collection_hash, field_hash))
            .copied(),
        JitStorageKind::F32 if field_hash == 0 => registered_f32_ptrs()
            .lock()
            .expect("registered f32 ptr table mutex poisoned")
            .get(&collection_hash)
            .copied()
            .map(|ptr| (ptr, 1))
            .or_else(|| {
                registered_f32_arrays()
                    .lock()
                    .expect("registered f32 array table mutex poisoned")
                    .get(&(collection_hash, field_hash))
                    .copied()
            }),
        JitStorageKind::F32 => registered_f32_arrays()
            .lock()
            .expect("registered f32 array table mutex poisoned")
            .get(&(collection_hash, field_hash))
            .copied(),
        JitStorageKind::F64 if field_hash == 0 => registered_f64_ptrs()
            .lock()
            .expect("registered f64 ptr table mutex poisoned")
            .get(&collection_hash)
            .copied()
            .map(|ptr| (ptr, 1))
            .or_else(|| {
                registered_f64_arrays()
                    .lock()
                    .expect("registered f64 array table mutex poisoned")
                    .get(&(collection_hash, field_hash))
                    .copied()
            }),
        JitStorageKind::F64 => registered_f64_arrays()
            .lock()
            .expect("registered f64 array table mutex poisoned")
            .get(&(collection_hash, field_hash))
            .copied(),
        JitStorageKind::U8 => registered_u8_arrays()
            .lock()
            .expect("registered u8 array table mutex poisoned")
            .get(&(collection_hash, field_hash))
            .copied(),
        JitStorageKind::U16 => registered_u16_arrays()
            .lock()
            .expect("registered u16 array table mutex poisoned")
            .get(&(collection_hash, field_hash))
            .copied(),
    };
    value.ok_or_else(|| "direct storage backing was not provisioned".to_string())
}

fn ensure_owned_array_capacity<T: Copy>(
    key: ArrayKey,
    requested_len: usize,
    default: T,
    owned: &Mutex<HashMap<ArrayKey, Vec<T>>>,
    registered: &Mutex<HashMap<ArrayKey, (usize, usize)>>,
) -> Result<(), String> {
    let mut owned_guard = owned.lock().expect("owned array table mutex poisoned");
    if let Some(array) = owned_guard.get_mut(&key) {
        if let Some((registered_ptr, registered_len)) = registered
            .lock()
            .expect("registered array table mutex poisoned")
            .get(&key)
            .copied()
        {
            if registered_ptr != array.as_mut_ptr() as usize {
                return (registered_len >= requested_len)
                    .then_some(())
                    .ok_or_else(|| {
                        format!(
                            "cannot grow host-owned collection storage from {registered_len} to {requested_len}"
                        )
                    });
            }
        }
        array.resize(array.len().max(requested_len), default);
        let mut registered_guard = registered
            .lock()
            .expect("registered array table mutex poisoned");
        registered_guard.insert(key, (array.as_mut_ptr() as usize, array.len()));
        return Ok(());
    }
    if let Some((_, registered_len)) = registered
        .lock()
        .expect("registered array table mutex poisoned")
        .get(&key)
        .copied()
    {
        return (registered_len >= requested_len)
            .then_some(())
            .ok_or_else(|| {
                format!(
                "cannot grow host-owned collection storage from {registered_len} to {requested_len}"
            )
            });
    }
    let mut array = vec![default; requested_len];
    let ptr = array.as_mut_ptr() as usize;
    owned_guard.insert(key, array);
    registered
        .lock()
        .expect("registered array table mutex poisoned")
        .insert(key, (ptr, requested_len));
    Ok(())
}

fn preflight_owned_array_capacity<T>(
    key: ArrayKey,
    requested_len: usize,
    owned: &Mutex<HashMap<ArrayKey, Vec<T>>>,
    registered: &Mutex<HashMap<ArrayKey, (usize, usize)>>,
) -> Result<(), String> {
    let owned_guard = owned.lock().expect("owned array table mutex poisoned");
    let registered_entry = registered
        .lock()
        .expect("registered array table mutex poisoned")
        .get(&key)
        .copied();
    if let Some(array) = owned_guard.get(&key) {
        if registered_entry
            .is_none_or(|(registered_ptr, _)| registered_ptr == array.as_ptr() as usize)
        {
            return Ok(());
        }
    }
    if let Some((_, registered_len)) = registered_entry {
        return (registered_len >= requested_len)
            .then_some(())
            .ok_or_else(|| {
                format!(
                    "cannot grow host-owned collection storage from {registered_len} to {requested_len}"
                )
            });
    }
    Ok(())
}

pub fn preflight_jit_i32_array_capacity(
    collection_hash: i32,
    field_hash: i32,
    requested_len: usize,
) -> Result<(), String> {
    preflight_owned_array_capacity(
        (collection_hash, field_hash),
        requested_len,
        owned_i32_arrays(),
        registered_i32_arrays(),
    )
}

/// Returns the registered/owned capacity without growing or mutating a JIT
/// collection. Realtime ABI wrappers use this before every bounded load/store.
pub fn jit_i32_array_capacity(collection_hash: i32, field_hash: i32) -> Option<usize> {
    if let Some((_, len)) = registered_i32_arrays()
        .lock()
        .expect("registered i32 array table mutex poisoned")
        .get(&(collection_hash, field_hash))
        .copied()
    {
        return Some(len);
    }
    if let Some(len) = owned_i32_arrays()
        .lock()
        .expect("owned i32 array table mutex poisoned")
        .get(&(collection_hash, field_hash))
        .map(Vec::len)
    {
        return Some(len);
    }
    let fallback = jit_i32_array_global_table()
        .lock()
        .expect("jit i32 array table mutex poisoned");
    let max_index = fallback
        .keys()
        .filter(|(collection, field, _)| *collection == collection_hash && *field == field_hash)
        .map(|(_, _, index)| *index)
        .max()?;
    usize::try_from(max_index).ok()?.checked_add(1)
}

pub fn preflight_jit_f32_array_capacity(
    collection_hash: i32,
    field_hash: i32,
    requested_len: usize,
) -> Result<(), String> {
    preflight_owned_array_capacity(
        (collection_hash, field_hash),
        requested_len,
        owned_f32_arrays(),
        registered_f32_arrays(),
    )
}

pub fn preflight_jit_f64_array_capacity(
    collection_hash: i32,
    field_hash: i32,
    requested_len: usize,
) -> Result<(), String> {
    preflight_owned_array_capacity(
        (collection_hash, field_hash),
        requested_len,
        owned_f64_arrays(),
        registered_f64_arrays(),
    )
}

fn migrate_fallback_array<T: Copy>(
    key: ArrayKey,
    owned: &Mutex<HashMap<ArrayKey, Vec<T>>>,
    fallback: &Mutex<HashMap<(i32, i32, i32), T>>,
) {
    let mut owned_guard = owned.lock().expect("owned array table mutex poisoned");
    let Some(array) = owned_guard.get_mut(&key) else {
        return;
    };
    let mut fallback = fallback
        .lock()
        .expect("fallback array table mutex poisoned");
    for (index, slot) in array.iter_mut().enumerate() {
        if let Some(value) = fallback.remove(&(key.0, key.1, index as i32)) {
            *slot = value;
        }
    }
}

fn direct_array_rebind_guard(
    kind: JitStorageKind,
    collection_hash: i32,
    field_hash: i32,
) -> Result<Option<MutexGuard<'static, ()>>, String> {
    let has_direct_slot = direct_storage_slots()
        .lock()
        .expect("direct storage slot table mutex poisoned")
        .contains_key(&(kind, collection_hash, field_hash));
    has_direct_slot.then(acquire_rebind_guard).transpose()
}

pub fn ensure_jit_i32_array_capacity(
    collection_hash: i32,
    field_hash: i32,
    requested_len: usize,
) -> Result<(), String> {
    let _rebind = direct_array_rebind_guard(JitStorageKind::I32, collection_hash, field_hash)?;
    ensure_jit_i32_array_capacity_unlocked(collection_hash, field_hash, requested_len)
}

fn ensure_jit_i32_array_capacity_unlocked(
    collection_hash: i32,
    field_hash: i32,
    requested_len: usize,
) -> Result<(), String> {
    let key = (collection_hash, field_hash);
    discard_integer_array_lane(key, JitStorageKind::I32);
    ensure_owned_array_capacity(
        key,
        requested_len,
        0,
        owned_i32_arrays(),
        registered_i32_arrays(),
    )?;
    migrate_fallback_array(key, owned_i32_arrays(), jit_i32_array_global_table());
    if let Some((data, len)) = registered_i32_arrays()
        .lock()
        .expect("registered i32 array table mutex poisoned")
        .get(&key)
        .copied()
    {
        update_direct_storage_slot((JitStorageKind::I32, key.0, key.1), data, len);
    }
    Ok(())
}

pub fn ensure_jit_f32_array_capacity(
    collection_hash: i32,
    field_hash: i32,
    requested_len: usize,
) -> Result<(), String> {
    let _rebind = direct_array_rebind_guard(JitStorageKind::F32, collection_hash, field_hash)?;
    ensure_jit_f32_array_capacity_unlocked(collection_hash, field_hash, requested_len)
}

fn ensure_jit_f32_array_capacity_unlocked(
    collection_hash: i32,
    field_hash: i32,
    requested_len: usize,
) -> Result<(), String> {
    let key = (collection_hash, field_hash);
    ensure_owned_array_capacity(
        key,
        requested_len,
        0.0,
        owned_f32_arrays(),
        registered_f32_arrays(),
    )?;
    migrate_fallback_array(key, owned_f32_arrays(), jit_f32_array_global_table());
    if let Some((data, len)) = registered_f32_arrays()
        .lock()
        .expect("registered f32 array table mutex poisoned")
        .get(&key)
        .copied()
    {
        update_direct_storage_slot((JitStorageKind::F32, key.0, key.1), data, len);
    }
    Ok(())
}

pub fn ensure_jit_f64_array_capacity(
    collection_hash: i32,
    field_hash: i32,
    requested_len: usize,
) -> Result<(), String> {
    let _rebind = direct_array_rebind_guard(JitStorageKind::F64, collection_hash, field_hash)?;
    ensure_jit_f64_array_capacity_unlocked(collection_hash, field_hash, requested_len)
}

fn ensure_jit_f64_array_capacity_unlocked(
    collection_hash: i32,
    field_hash: i32,
    requested_len: usize,
) -> Result<(), String> {
    let key = (collection_hash, field_hash);
    ensure_owned_array_capacity(
        key,
        requested_len,
        0.0,
        owned_f64_arrays(),
        registered_f64_arrays(),
    )?;
    migrate_fallback_array(key, owned_f64_arrays(), jit_f64_array_global_table());
    if let Some((data, len)) = registered_f64_arrays()
        .lock()
        .expect("registered f64 array table mutex poisoned")
        .get(&key)
        .copied()
    {
        update_direct_storage_slot((JitStorageKind::F64, key.0, key.1), data, len);
    }
    Ok(())
}

pub fn ensure_jit_u8_array_capacity(
    collection_hash: i32,
    field_hash: i32,
    requested_len: usize,
) -> Result<(), String> {
    let _rebind = direct_array_rebind_guard(JitStorageKind::U8, collection_hash, field_hash)?;
    ensure_jit_u8_array_capacity_unlocked(collection_hash, field_hash, requested_len)
}

fn ensure_jit_u8_array_capacity_unlocked(
    collection_hash: i32,
    field_hash: i32,
    requested_len: usize,
) -> Result<(), String> {
    let key = (collection_hash, field_hash);
    discard_integer_array_lane(key, JitStorageKind::U8);
    ensure_owned_array_capacity(
        key,
        requested_len,
        0,
        owned_u8_arrays(),
        registered_u8_arrays(),
    )?;
    let (data, len) = registered_u8_arrays()
        .lock()
        .expect("registered u8 array table mutex poisoned")
        .get(&key)
        .copied()
        .ok_or_else(|| "u8 storage was not provisioned".to_string())?;
    update_direct_storage_slot((JitStorageKind::U8, key.0, key.1), data, len);
    Ok(())
}

pub fn preflight_jit_u8_array_capacity(
    collection_hash: i32,
    field_hash: i32,
    requested_len: usize,
) -> Result<(), String> {
    preflight_owned_array_capacity(
        (collection_hash, field_hash),
        requested_len,
        owned_u8_arrays(),
        registered_u8_arrays(),
    )
}

pub fn preflight_jit_u16_array_capacity(
    collection_hash: i32,
    field_hash: i32,
    requested_len: usize,
) -> Result<(), String> {
    preflight_owned_array_capacity(
        (collection_hash, field_hash),
        requested_len,
        owned_u16_arrays(),
        registered_u16_arrays(),
    )
}

pub fn ensure_jit_u16_array_capacity(
    collection_hash: i32,
    field_hash: i32,
    requested_len: usize,
) -> Result<(), String> {
    let _rebind = direct_array_rebind_guard(JitStorageKind::U16, collection_hash, field_hash)?;
    ensure_jit_u16_array_capacity_unlocked(collection_hash, field_hash, requested_len)
}

fn ensure_jit_u16_array_capacity_unlocked(
    collection_hash: i32,
    field_hash: i32,
    requested_len: usize,
) -> Result<(), String> {
    let key = (collection_hash, field_hash);
    discard_integer_array_lane(key, JitStorageKind::U16);
    ensure_owned_array_capacity(
        key,
        requested_len,
        0,
        owned_u16_arrays(),
        registered_u16_arrays(),
    )?;
    let (data, len) = registered_u16_arrays()
        .lock()
        .expect("registered u16 array table mutex poisoned")
        .get(&key)
        .copied()
        .ok_or_else(|| "u16 storage was not provisioned".to_string())?;
    update_direct_storage_slot((JitStorageKind::U16, key.0, key.1), data, len);
    Ok(())
}

pub fn clear_registered_global_memory() {
    let Ok(_rebind) = acquire_rebind_guard() else {
        return;
    };
    registered_i32_ptrs()
        .lock()
        .expect("registered i32 ptr table mutex poisoned")
        .clear();
    registered_f32_ptrs()
        .lock()
        .expect("registered f32 ptr table mutex poisoned")
        .clear();
    registered_f64_ptrs()
        .lock()
        .expect("registered f64 ptr table mutex poisoned")
        .clear();
    registered_i32_arrays()
        .lock()
        .expect("registered i32 array table mutex poisoned")
        .clear();
    registered_f32_arrays()
        .lock()
        .expect("registered f32 array table mutex poisoned")
        .clear();
    registered_f64_arrays()
        .lock()
        .expect("registered f64 array table mutex poisoned")
        .clear();
    registered_u8_arrays()
        .lock()
        .expect("registered u8 array table mutex poisoned")
        .clear();
    registered_u16_arrays()
        .lock()
        .expect("registered u16 array table mutex poisoned")
        .clear();

    owned_f32_arrays()
        .lock()
        .expect("owned f32 array table mutex poisoned")
        .clear();
    owned_i32_arrays()
        .lock()
        .expect("owned i32 array table mutex poisoned")
        .clear();
    owned_f64_arrays()
        .lock()
        .expect("owned f64 array table mutex poisoned")
        .clear();
    owned_u8_arrays()
        .lock()
        .expect("owned u8 array table mutex poisoned")
        .clear();
    owned_u16_arrays()
        .lock()
        .expect("owned u16 array table mutex poisoned")
        .clear();
    owned_i32_scalars()
        .lock()
        .expect("owned i32 scalar table mutex poisoned")
        .clear();
    owned_f32_scalars()
        .lock()
        .expect("owned f32 scalar table mutex poisoned")
        .clear();
    owned_f64_scalars()
        .lock()
        .expect("owned f64 scalar table mutex poisoned")
        .clear();
    direct_storage_slots()
        .lock()
        .expect("direct storage slot table mutex poisoned")
        .clear();
    direct_array_required_lengths()
        .lock()
        .expect("direct array required-length table mutex poisoned")
        .clear();
}

#[derive(Debug, Clone)]
pub struct JitRuntimeStateSnapshot {
    i32_globals: JitI32GlobalMap,
    f32_globals: JitF32GlobalMap,
    f64_globals: JitF64GlobalMap,
    i32_array_globals: JitI32ArrayGlobalMap,
    f32_array_globals: JitF32ArrayGlobalMap,
    f64_array_globals: JitF64ArrayGlobalMap,
    owned_i32_scalars: HashMap<i32, i32>,
    owned_f32_scalars: HashMap<i32, f32>,
    owned_f64_scalars: HashMap<i32, f64>,
    owned_i32_arrays: HashMap<ArrayKey, Vec<i32>>,
    owned_f32_arrays: HashMap<ArrayKey, Vec<f32>>,
    owned_f64_arrays: HashMap<ArrayKey, Vec<f64>>,
    owned_u8_arrays: HashMap<ArrayKey, Vec<u8>>,
    owned_u16_arrays: HashMap<ArrayKey, Vec<u16>>,
}

/// Captures state owned by the Stasis runtime.
///
/// Host-registered pointers are borrowed FFI memory. They are deliberately excluded: the
/// registry cannot prove that those allocations still exist when a snapshot is restored.
pub fn snapshot_jit_runtime_state() -> JitRuntimeStateSnapshot {
    snapshot_jit_runtime_state_bounded(usize::MAX)
        .expect("unbounded JIT runtime snapshot cannot exceed usize")
}

pub fn snapshot_jit_runtime_state_bounded(
    max_bytes: usize,
) -> Result<JitRuntimeStateSnapshot, String> {
    let snapshot_bytes = runtime_snapshot_bytes()?;
    if snapshot_bytes > max_bytes {
        return Err(format!(
            "live runtime snapshot requires {snapshot_bytes} bytes; limit is {max_bytes} bytes"
        ));
    }
    Ok(JitRuntimeStateSnapshot {
        i32_globals: jit_i32_global_table()
            .lock()
            .expect("jit i32 global table mutex poisoned")
            .clone(),
        f32_globals: jit_f32_global_table()
            .lock()
            .expect("jit f32 global table mutex poisoned")
            .clone(),
        f64_globals: jit_f64_global_table()
            .lock()
            .expect("jit f64 global table mutex poisoned")
            .clone(),
        i32_array_globals: jit_i32_array_global_table()
            .lock()
            .expect("jit i32 array global table mutex poisoned")
            .clone(),
        f32_array_globals: jit_f32_array_global_table()
            .lock()
            .expect("jit f32 array global table mutex poisoned")
            .clone(),
        f64_array_globals: jit_f64_array_global_table()
            .lock()
            .expect("jit f64 array global table mutex poisoned")
            .clone(),
        owned_i32_scalars: snapshot_owned_scalars(owned_i32_scalars(), registered_i32_ptrs()),
        owned_f32_scalars: snapshot_owned_scalars(owned_f32_scalars(), registered_f32_ptrs()),
        owned_f64_scalars: snapshot_owned_scalars(owned_f64_scalars(), registered_f64_ptrs()),
        owned_i32_arrays: snapshot_owned_arrays(owned_i32_arrays()),
        owned_f32_arrays: snapshot_owned_arrays(owned_f32_arrays()),
        owned_f64_arrays: snapshot_owned_arrays(owned_f64_arrays()),
        owned_u8_arrays: snapshot_owned_arrays(owned_u8_arrays()),
        owned_u16_arrays: snapshot_owned_arrays(owned_u16_arrays()),
    })
}

fn snapshot_owned_scalars<T: Copy>(
    owned: &Mutex<HashMap<i32, Box<T>>>,
    registered: &Mutex<HashMap<i32, usize>>,
) -> HashMap<i32, T> {
    let owned = owned.lock().expect("owned scalar table mutex poisoned");
    let registered = registered
        .lock()
        .expect("registered scalar table mutex poisoned");
    owned
        .iter()
        .filter(|(key, value)| registered.get(key) == Some(&(value.as_ref() as *const T as usize)))
        .map(|(key, value)| (*key, **value))
        .collect()
}

fn snapshot_owned_arrays<T: Clone>(
    owned: &Mutex<HashMap<ArrayKey, Vec<T>>>,
) -> HashMap<ArrayKey, Vec<T>> {
    owned
        .lock()
        .expect("owned array table mutex poisoned")
        .clone()
}

fn runtime_snapshot_bytes() -> Result<usize, String> {
    let mut bytes = 0usize;
    let mut add = |count: usize, item_bytes: usize| -> Result<(), String> {
        bytes = bytes
            .checked_add(
                count
                    .checked_mul(item_bytes)
                    .ok_or_else(|| "live runtime snapshot size overflow".to_string())?,
            )
            .ok_or_else(|| "live runtime snapshot size overflow".to_string())?;
        Ok(())
    };
    add(
        jit_i32_global_table()
            .lock()
            .expect("jit i32 global table mutex poisoned")
            .len(),
        std::mem::size_of::<(i32, i32)>(),
    )?;
    add(
        jit_f32_global_table()
            .lock()
            .expect("jit f32 global table mutex poisoned")
            .len(),
        std::mem::size_of::<(i32, f32)>(),
    )?;
    add(
        jit_f64_global_table()
            .lock()
            .expect("jit f64 global table mutex poisoned")
            .len(),
        std::mem::size_of::<(i32, f64)>(),
    )?;
    add(
        jit_i32_array_global_table()
            .lock()
            .expect("jit i32 array global table mutex poisoned")
            .len(),
        std::mem::size_of::<((i32, i32, i32), i32)>(),
    )?;
    add(
        jit_f32_array_global_table()
            .lock()
            .expect("jit f32 array global table mutex poisoned")
            .len(),
        std::mem::size_of::<((i32, i32, i32), f32)>(),
    )?;
    add(
        jit_f64_array_global_table()
            .lock()
            .expect("jit f64 array global table mutex poisoned")
            .len(),
        std::mem::size_of::<((i32, i32, i32), f64)>(),
    )?;
    add_owned_scalar_bytes(&mut add, owned_i32_scalars(), std::mem::size_of::<i32>())?;
    add_owned_scalar_bytes(&mut add, owned_f32_scalars(), std::mem::size_of::<f32>())?;
    add_owned_scalar_bytes(&mut add, owned_f64_scalars(), std::mem::size_of::<f64>())?;
    add_owned_array_bytes(&mut add, owned_i32_arrays(), std::mem::size_of::<i32>())?;
    add_owned_array_bytes(&mut add, owned_f32_arrays(), std::mem::size_of::<f32>())?;
    add_owned_array_bytes(&mut add, owned_f64_arrays(), std::mem::size_of::<f64>())?;
    add_owned_array_bytes(&mut add, owned_u8_arrays(), std::mem::size_of::<u8>())?;
    add_owned_array_bytes(&mut add, owned_u16_arrays(), std::mem::size_of::<u16>())?;
    Ok(bytes)
}

fn add_owned_scalar_bytes<T>(
    add: &mut impl FnMut(usize, usize) -> Result<(), String>,
    table: &Mutex<HashMap<i32, Box<T>>>,
    item_bytes: usize,
) -> Result<(), String> {
    add(
        table
            .lock()
            .expect("owned scalar table mutex poisoned")
            .len(),
        item_bytes,
    )
}

fn add_owned_array_bytes<T>(
    add: &mut impl FnMut(usize, usize) -> Result<(), String>,
    table: &Mutex<HashMap<ArrayKey, Vec<T>>>,
    item_bytes: usize,
) -> Result<(), String> {
    let elements = table
        .lock()
        .expect("owned array table mutex poisoned")
        .values()
        .try_fold(0usize, |total, values| {
            total
                .checked_add(values.len())
                .ok_or_else(|| "live runtime snapshot size overflow".to_string())
        })?;
    add(elements, item_bytes)
}

pub fn restore_jit_runtime_state(snapshot: &JitRuntimeStateSnapshot) {
    let Ok(_rebind) = acquire_rebind_guard() else {
        return;
    };
    *jit_i32_global_table()
        .lock()
        .expect("jit i32 global table mutex poisoned") = snapshot.i32_globals.clone();
    *jit_f32_global_table()
        .lock()
        .expect("jit f32 global table mutex poisoned") = snapshot.f32_globals.clone();
    *jit_f64_global_table()
        .lock()
        .expect("jit f64 global table mutex poisoned") = snapshot.f64_globals.clone();
    *jit_i32_array_global_table()
        .lock()
        .expect("jit i32 array global table mutex poisoned") = snapshot.i32_array_globals.clone();
    *jit_f32_array_global_table()
        .lock()
        .expect("jit f32 array global table mutex poisoned") = snapshot.f32_array_globals.clone();
    *jit_f64_array_global_table()
        .lock()
        .expect("jit f64 array global table mutex poisoned") = snapshot.f64_array_globals.clone();
    restore_owned_scalars(
        &snapshot.owned_i32_scalars,
        owned_i32_scalars(),
        registered_i32_ptrs(),
    );
    restore_owned_scalars(
        &snapshot.owned_f32_scalars,
        owned_f32_scalars(),
        registered_f32_ptrs(),
    );
    restore_owned_scalars(
        &snapshot.owned_f64_scalars,
        owned_f64_scalars(),
        registered_f64_ptrs(),
    );
    restore_owned_arrays(
        &snapshot.owned_i32_arrays,
        owned_i32_arrays(),
        registered_i32_arrays(),
    );
    restore_owned_arrays(
        &snapshot.owned_f32_arrays,
        owned_f32_arrays(),
        registered_f32_arrays(),
    );
    restore_owned_arrays(
        &snapshot.owned_f64_arrays,
        owned_f64_arrays(),
        registered_f64_arrays(),
    );
    restore_owned_arrays(
        &snapshot.owned_u8_arrays,
        owned_u8_arrays(),
        registered_u8_arrays(),
    );
    restore_owned_arrays(
        &snapshot.owned_u16_arrays,
        owned_u16_arrays(),
        registered_u16_arrays(),
    );
    refresh_direct_storage_slots();
}

fn refresh_direct_storage_slots() {
    let keys = direct_storage_slots()
        .lock()
        .expect("direct storage slot table mutex poisoned")
        .keys()
        .copied()
        .collect::<Vec<_>>();
    for (kind, collection_hash, field_hash) in keys {
        let (data, len) = registered_storage(kind, collection_hash, field_hash).unwrap_or((0, 0));
        update_direct_storage_slot((kind, collection_hash, field_hash), data, len);
    }
}

fn restore_owned_scalars<T: Copy>(
    snapshot: &HashMap<i32, T>,
    owned: &Mutex<HashMap<i32, Box<T>>>,
    registered: &Mutex<HashMap<i32, usize>>,
) {
    let mut owned = owned.lock().expect("owned scalar table mutex poisoned");
    let previous_addresses = owned
        .iter()
        .map(|(key, value)| (*key, value.as_ref() as *const T as usize))
        .collect::<HashMap<_, _>>();
    owned.retain(|key, _| snapshot.contains_key(key));
    for (key, value) in snapshot {
        **owned.entry(*key).or_insert_with(|| Box::new(*value)) = *value;
    }
    let mut registered = registered
        .lock()
        .expect("registered scalar table mutex poisoned");
    registered.retain(|key, address| {
        snapshot.contains_key(key) || previous_addresses.get(key) != Some(address)
    });
    for (key, value) in owned.iter_mut() {
        registered.insert(*key, value.as_mut() as *mut T as usize);
    }
}

fn restore_owned_arrays<T: Clone>(
    snapshot: &HashMap<ArrayKey, Vec<T>>,
    owned: &Mutex<HashMap<ArrayKey, Vec<T>>>,
    registered: &Mutex<HashMap<ArrayKey, (usize, usize)>>,
) {
    let mut owned = owned.lock().expect("owned array table mutex poisoned");
    let previous_addresses = owned
        .iter()
        .map(|(key, values)| (*key, values.as_ptr() as usize))
        .collect::<HashMap<_, _>>();
    owned.retain(|key, _| snapshot.contains_key(key));
    for (key, values) in snapshot {
        let target = owned.entry(*key).or_default();
        target.clear();
        target.extend_from_slice(values);
    }
    let mut registered = registered
        .lock()
        .expect("registered array table mutex poisoned");
    registered.retain(|key, (address, _)| {
        snapshot.contains_key(key) || previous_addresses.get(key) != Some(address)
    });
    for (key, values) in owned.iter_mut() {
        registered.insert(*key, (values.as_mut_ptr() as usize, values.len()));
    }
}

pub fn register_global_i32_ptr(path_hash: i32, ptr: *mut i32) {
    if ptr.is_null() {
        return;
    }
    let Ok(_rebind) = acquire_rebind_guard() else {
        return;
    };
    remove_replaced_owned_scalar(path_hash, ptr as usize, owned_i32_scalars());
    let table = registered_i32_ptrs();
    let mut guard = table
        .lock()
        .expect("registered i32 ptr table mutex poisoned");
    guard.insert(path_hash, ptr as usize);
    update_direct_storage_slot((JitStorageKind::I32, path_hash, 0), ptr as usize, 1);
}

pub fn register_global_f32_ptr(path_hash: i32, ptr: *mut f32) {
    if ptr.is_null() {
        return;
    }
    let Ok(_rebind) = acquire_rebind_guard() else {
        return;
    };
    remove_replaced_owned_scalar(path_hash, ptr as usize, owned_f32_scalars());
    let table = registered_f32_ptrs();
    let mut guard = table
        .lock()
        .expect("registered f32 ptr table mutex poisoned");
    guard.insert(path_hash, ptr as usize);
    update_direct_storage_slot((JitStorageKind::F32, path_hash, 0), ptr as usize, 1);
}

pub fn register_global_f64_ptr(path_hash: i32, ptr: *mut f64) {
    if ptr.is_null() {
        return;
    }
    let Ok(_rebind) = acquire_rebind_guard() else {
        return;
    };
    remove_replaced_owned_scalar(path_hash, ptr as usize, owned_f64_scalars());
    let table = registered_f64_ptrs();
    let mut guard = table
        .lock()
        .expect("registered f64 ptr table mutex poisoned");
    guard.insert(path_hash, ptr as usize);
    update_direct_storage_slot((JitStorageKind::F64, path_hash, 0), ptr as usize, 1);
}

pub fn register_global_i32_array(collection_hash: i32, field_hash: i32, ptr: *mut i32, len: usize) {
    if ptr.is_null() {
        return;
    }
    let Ok(_rebind) = acquire_rebind_guard() else {
        return;
    };
    if !accepts_direct_array_rebind((JitStorageKind::I32, collection_hash, field_hash), len) {
        return;
    }
    let key = (collection_hash, field_hash);
    discard_integer_array_lane(key, JitStorageKind::I32);
    remove_replaced_owned_array(key, ptr as usize, owned_i32_arrays());
    let table = registered_i32_arrays();
    let mut guard = table
        .lock()
        .expect("registered i32 array table mutex poisoned");
    guard.insert(key, (ptr as usize, len));
    update_direct_storage_slot(
        (JitStorageKind::I32, collection_hash, field_hash),
        ptr as usize,
        len,
    );
}

pub fn fill_registered_global_i32_array(
    collection_hash: i32,
    field_hash: i32,
    start: usize,
    len: usize,
    value: i32,
) -> Result<(), String> {
    let end = start
        .checked_add(len)
        .ok_or_else(|| "registered i32 array range overflowed".to_string())?;
    let table = registered_i32_arrays();
    let guard = table
        .lock()
        .map_err(|_| "registered i32 array table mutex poisoned".to_string())?;
    let (ptr, registered_len) = guard
        .get(&(collection_hash, field_hash))
        .copied()
        .ok_or_else(|| "registered i32 array was not found".to_string())?;
    if end > registered_len {
        return Err(format!(
            "registered i32 array range {start}..{end} exceeds length {registered_len}"
        ));
    }
    // Registration owns the pointer lifetime, and the table lock prevents a
    // rebind while this bounded range is written.
    let values = unsafe { std::slice::from_raw_parts_mut(ptr as *mut i32, registered_len) };
    values[start..end].fill(value);
    Ok(())
}

pub fn register_global_f32_array(collection_hash: i32, field_hash: i32, ptr: *mut f32, len: usize) {
    if ptr.is_null() {
        return;
    }
    let Ok(_rebind) = acquire_rebind_guard() else {
        return;
    };
    if !accepts_direct_array_rebind((JitStorageKind::F32, collection_hash, field_hash), len) {
        return;
    }
    let key = (collection_hash, field_hash);
    remove_replaced_owned_array(key, ptr as usize, owned_f32_arrays());
    let table = registered_f32_arrays();
    let mut guard = table
        .lock()
        .expect("registered f32 array table mutex poisoned");
    guard.insert(key, (ptr as usize, len));
    update_direct_storage_slot(
        (JitStorageKind::F32, collection_hash, field_hash),
        ptr as usize,
        len,
    );
}

pub fn register_global_f64_array(collection_hash: i32, field_hash: i32, ptr: *mut f64, len: usize) {
    if ptr.is_null() {
        return;
    }
    let Ok(_rebind) = acquire_rebind_guard() else {
        return;
    };
    if !accepts_direct_array_rebind((JitStorageKind::F64, collection_hash, field_hash), len) {
        return;
    }
    let key = (collection_hash, field_hash);
    remove_replaced_owned_array(key, ptr as usize, owned_f64_arrays());
    let table = registered_f64_arrays();
    let mut guard = table
        .lock()
        .expect("registered f64 array table mutex poisoned");
    guard.insert(key, (ptr as usize, len));
    update_direct_storage_slot(
        (JitStorageKind::F64, collection_hash, field_hash),
        ptr as usize,
        len,
    );
}

fn remove_replaced_owned_array<T>(
    key: ArrayKey,
    registered_ptr: usize,
    owned: &Mutex<HashMap<ArrayKey, Vec<T>>>,
) {
    let mut guard = owned.lock().expect("owned array table mutex poisoned");
    if guard
        .get(&key)
        .is_some_and(|values| values.as_ptr() as usize != registered_ptr)
    {
        guard.remove(&key);
    }
}

fn remove_replaced_owned_scalar<T>(
    key: i32,
    registered_ptr: usize,
    owned: &Mutex<HashMap<i32, Box<T>>>,
) {
    let mut guard = owned.lock().expect("owned scalar table mutex poisoned");
    if guard
        .get(&key)
        .is_some_and(|value| value.as_ref() as *const T as usize != registered_ptr)
    {
        guard.remove(&key);
    }
}

fn discard_integer_array_lane(key: ArrayKey, retained: JitStorageKind) {
    if retained != JitStorageKind::I32 {
        registered_i32_arrays()
            .lock()
            .expect("registered i32 array table mutex poisoned")
            .remove(&key);
        owned_i32_arrays()
            .lock()
            .expect("owned i32 array table mutex poisoned")
            .remove(&key);
    }
    if retained != JitStorageKind::U8 {
        registered_u8_arrays()
            .lock()
            .expect("registered u8 array table mutex poisoned")
            .remove(&key);
        owned_u8_arrays()
            .lock()
            .expect("owned u8 array table mutex poisoned")
            .remove(&key);
    }
    if retained != JitStorageKind::U16 {
        registered_u16_arrays()
            .lock()
            .expect("registered u16 array table mutex poisoned")
            .remove(&key);
        owned_u16_arrays()
            .lock()
            .expect("owned u16 array table mutex poisoned")
            .remove(&key);
    }
}

pub fn register_global_u16_array(collection_hash: i32, field_hash: i32, ptr: *mut u16, len: usize) {
    if ptr.is_null() {
        return;
    }
    let Ok(_rebind) = acquire_rebind_guard() else {
        return;
    };
    if !accepts_direct_array_rebind((JitStorageKind::U16, collection_hash, field_hash), len) {
        return;
    }
    let key = (collection_hash, field_hash);
    discard_integer_array_lane(key, JitStorageKind::U16);
    remove_replaced_owned_array(key, ptr as usize, owned_u16_arrays());
    registered_u16_arrays()
        .lock()
        .expect("registered u16 array table mutex poisoned")
        .insert(key, (ptr as usize, len));
    update_direct_storage_slot(
        (JitStorageKind::U16, collection_hash, field_hash),
        ptr as usize,
        len,
    );
}

pub fn register_global_u8_array(collection_hash: i32, field_hash: i32, ptr: *mut u8, len: usize) {
    if ptr.is_null() {
        return;
    }
    let Ok(_rebind) = acquire_rebind_guard() else {
        return;
    };
    if !accepts_direct_array_rebind((JitStorageKind::U8, collection_hash, field_hash), len) {
        return;
    }
    let key = (collection_hash, field_hash);
    discard_integer_array_lane(key, JitStorageKind::U8);
    remove_replaced_owned_array(key, ptr as usize, owned_u8_arrays());
    let table = registered_u8_arrays();
    let mut guard = table
        .lock()
        .expect("registered u8 array table mutex poisoned");
    guard.insert(key, (ptr as usize, len));
    update_direct_storage_slot(
        (JitStorageKind::U8, collection_hash, field_hash),
        ptr as usize,
        len,
    );
}

#[no_mangle]
pub extern "C" fn stasis_jit_register_global_i32_ptr(path_hash: i32, ptr: *mut i32) {
    register_global_i32_ptr(path_hash, ptr);
}

#[no_mangle]
pub extern "C" fn stasis_jit_register_global_f32_ptr(path_hash: i32, ptr: *mut f32) {
    register_global_f32_ptr(path_hash, ptr);
}

#[no_mangle]
pub extern "C" fn stasis_jit_register_global_f64_ptr(path_hash: i32, ptr: *mut f64) {
    register_global_f64_ptr(path_hash, ptr);
}

#[no_mangle]
pub extern "C" fn stasis_jit_register_global_i32_array(
    collection_hash: i32,
    field_hash: i32,
    ptr: *mut i32,
    len: i32,
) {
    if len <= 0 {
        return;
    }
    register_global_i32_array(collection_hash, field_hash, ptr, len as usize);
}

#[no_mangle]
pub extern "C" fn stasis_jit_register_global_f32_array(
    collection_hash: i32,
    field_hash: i32,
    ptr: *mut f32,
    len: i32,
) {
    if len <= 0 {
        return;
    }
    register_global_f32_array(collection_hash, field_hash, ptr, len as usize);
}

#[no_mangle]
pub extern "C" fn stasis_jit_register_global_f64_array(
    collection_hash: i32,
    field_hash: i32,
    ptr: *mut f64,
    len: i32,
) {
    if len <= 0 {
        return;
    }
    register_global_f64_array(collection_hash, field_hash, ptr, len as usize);
}

#[no_mangle]
pub extern "C" fn stasis_jit_register_global_u8_array(
    collection_hash: i32,
    field_hash: i32,
    ptr: *mut u8,
    len: i32,
) {
    if len <= 0 {
        return;
    }
    register_global_u8_array(collection_hash, field_hash, ptr, len as usize);
}

#[no_mangle]
pub extern "C" fn stasis_jit_register_global_u16_array(
    collection_hash: i32,
    field_hash: i32,
    ptr: *mut u16,
    len: i32,
) {
    if len <= 0 {
        return;
    }
    register_global_u16_array(collection_hash, field_hash, ptr, len as usize);
}

#[no_mangle]
pub extern "C" fn stasis_jit_global_i32_array_ptr(
    collection_hash: i32,
    field_hash: i32,
    len: i32,
) -> *mut i32 {
    if len <= 0 {
        return std::ptr::null_mut();
    }

    let requested_len = len as usize;
    let key = (collection_hash, field_hash);
    if let Some((ptr, registered_len)) = registered_i32_arrays()
        .lock()
        .expect("registered i32 array table mutex poisoned")
        .get(&key)
        .copied()
    {
        if registered_len >= requested_len {
            return ptr as *mut i32;
        }
    }
    if ensure_jit_i32_array_capacity(collection_hash, field_hash, requested_len).is_err() {
        return std::ptr::null_mut();
    }
    registered_i32_arrays()
        .lock()
        .expect("registered i32 array table mutex poisoned")
        .get(&key)
        .map_or(std::ptr::null_mut(), |(ptr, _)| *ptr as *mut i32)
}

#[no_mangle]
pub extern "C" fn stasis_jit_global_f32_array_ptr(
    collection_hash: i32,
    field_hash: i32,
    len: i32,
) -> *mut f32 {
    if len <= 0 {
        return std::ptr::null_mut();
    }

    let requested_len = len as usize;
    let key = (collection_hash, field_hash);
    if let Some((ptr, registered_len)) = registered_f32_arrays()
        .lock()
        .expect("registered f32 array table mutex poisoned")
        .get(&key)
        .copied()
    {
        if registered_len >= requested_len {
            return ptr as *mut f32;
        }
    }
    if ensure_jit_f32_array_capacity(collection_hash, field_hash, requested_len).is_err() {
        return std::ptr::null_mut();
    }
    registered_f32_arrays()
        .lock()
        .expect("registered f32 array table mutex poisoned")
        .get(&key)
        .map_or(std::ptr::null_mut(), |(ptr, _)| *ptr as *mut f32)
}

fn global_u8_array_ptr(collection_hash: i32, field_hash: i32, len: i32) -> *mut u8 {
    if len <= 0 {
        return std::ptr::null_mut();
    }

    let requested_len = len as usize;
    let key = (collection_hash, field_hash);
    if let Some((ptr, registered_len)) = registered_u8_arrays()
        .lock()
        .expect("registered u8 array table mutex poisoned")
        .get(&key)
        .copied()
    {
        if registered_len >= requested_len {
            return ptr as *mut u8;
        }
    }
    if ensure_jit_u8_array_capacity(collection_hash, field_hash, requested_len).is_err() {
        return std::ptr::null_mut();
    }
    registered_u8_arrays()
        .lock()
        .expect("registered u8 array table mutex poisoned")
        .get(&key)
        .map_or(std::ptr::null_mut(), |(ptr, _)| *ptr as *mut u8)
}

#[no_mangle]
pub extern "C" fn stasis_jit_global_f64_array_ptr(
    collection_hash: i32,
    field_hash: i32,
    len: i32,
) -> *mut f64 {
    if len <= 0 {
        return std::ptr::null_mut();
    }

    let requested_len = len as usize;
    let key = (collection_hash, field_hash);
    if let Some((ptr, registered_len)) = registered_f64_arrays()
        .lock()
        .expect("registered f64 array table mutex poisoned")
        .get(&key)
        .copied()
    {
        if registered_len >= requested_len {
            return ptr as *mut f64;
        }
    }
    if ensure_jit_f64_array_capacity(collection_hash, field_hash, requested_len).is_err() {
        return std::ptr::null_mut();
    }
    registered_f64_arrays()
        .lock()
        .expect("registered f64 array table mutex poisoned")
        .get(&key)
        .map_or(std::ptr::null_mut(), |(ptr, _)| *ptr as *mut f64)
}

#[no_mangle]
pub extern "C" fn stasis_jit_print_i32(value: i32) {
    write_jit_output(&value.to_string());
}

#[no_mangle]
pub extern "C" fn stasis_jit_print_string(value_id: i32) {
    if let Some(bytes) = jit_text_arg_bytes(value_id) {
        write_jit_output(&String::from_utf8_lossy(&bytes));
        return;
    }

    let table = jit_string_literal_table();
    let guard = table
        .lock()
        .expect("jit string literal table mutex poisoned");
    if let Some(text) = guard.get(&value_id) {
        write_jit_output(text);
    }
}

pub const STASIS_RENDER_I32_COUNT: usize = 67_888;
pub const STASIS_RENDER_F32_COUNT: usize = 146_564;
pub const STASIS_RENDER_U8_COUNT: usize = 65_536;
pub const STASIS_RENDER_MAGIC: i32 = 0x4758_4631;
pub const STASIS_RENDER_VERSION: i32 = 7;
const STASIS_RENDER_HEADER_I32_COUNT: usize = 10;
const STASIS_RENDER_ORDER_COUNT_INDEX: usize = 22;
const STASIS_RENDER_ORDER_HEADER_END: usize = 24;
const STASIS_RENDER_RECT_COUNT_INDEX: usize = 24;
const STASIS_RENDER_RECT_HEADER_END: usize = 26;
const STASIS_RENDER_CLIP_COUNT_INDEX: usize = 27;
const STASIS_RENDER_CLIP_HEADER_END: usize = 29;
const STASIS_RENDER_SPRITE_RUN_HEADER_END: usize = 31;
const STASIS_RENDER_F_CLEAR_BASE: usize = 0;
const STASIS_RENDER_F_LINE_BASE: usize = 4;
pub const STASIS_RENDER_ORDER_BASE: usize = 51_232;
const STASIS_RENDER_MAX_CLIPS: usize = 256;
const STASIS_RENDER_MAX_ORDER: usize = 16_656;
const STASIS_RENDER_SPRITE_BASE: usize = 32;
const STASIS_RENDER_MAX_GEOMETRY: usize = 10_000;
const STASIS_RENDER_MAX_LINES: usize = 10_000;
const STASIS_RENDER_GEOMETRY_STRIDE_F32: usize = 8;
const STASIS_RENDER_LINE_STRIDE: usize = 8;
pub const STASIS_RENDER_MAX_SPRITES: usize = 4_096;
const STASIS_RENDER_SPRITE_STRIDE_I32: usize = 3;
const STASIS_RENDER_SPRITE_RUN_COUNT_INDEX: usize = 29;
const STASIS_RENDER_SPRITE_RUN_BASE: usize = 18_464;
const STASIS_RENDER_MAX_SPRITE_RUNS: usize = 4_096;
const STASIS_RENDER_SPRITE_RUN_STRIDE_I32: usize = 8;
const STASIS_RENDER_SPRITE_BASE_F32: usize = 80_004;
pub const STASIS_RENDER_RECT_REVERSE_BASE_F32: usize = 79_996;
const STASIS_RENDER_SPRITE_STRIDE_F32: usize = 13;
pub const STASIS_RENDER_TEXT_BASE_I32: usize = 12_320;
const STASIS_RENDER_TEXT_BASE_F32: usize = 133_252;
const STASIS_RENDER_MAX_TEXT: usize = 2_048;
const STASIS_RENDER_TEXT_STRIDE_I32: usize = 3;
const STASIS_RENDER_TEXT_STRIDE_F32: usize = 6;
const STASIS_RENDER_CLIP_BASE_F32: usize = 145_540;
const STASIS_RENDER_CLIP_STRIDE_F32: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderActiveCounts {
    pub lines: usize,
    pub rects: usize,
    pub sprites: usize,
    pub sprite_runs: usize,
    pub text: usize,
    pub text_bytes: usize,
    pub order: usize,
    pub clips: usize,
}

/// Returns the stable identifier used by JIT global registration and lookup.
pub fn global_path_hash(path: &str) -> i32 {
    let mut hash = 2_166_136_261u32;
    for byte in path.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(16_777_619);
    }
    hash as i32
}

/// Copies only the active spans of the canonical JIT render buffers.
///
/// The destination retains production offsets so the Android GLES adapter consumes the exact
/// same ABI as the SDL renderer without copying unused command capacity.
pub fn copy_jit_render_active(
    out_i32: &mut [i32],
    out_f32: &mut [f32],
    out_u8: &mut [u8],
) -> Result<RenderActiveCounts, String> {
    if out_i32.len() < STASIS_RENDER_I32_COUNT
        || out_f32.len() < STASIS_RENDER_F32_COUNT
        || out_u8.len() < STASIS_RENDER_U8_COUNT
    {
        return Err("production render destination has the wrong capacity".to_string());
    }
    let i32_id = global_path_hash("gfx_cmd_i32");
    let i32_header_ptr = stasis_jit_global_i32_array_ptr(i32_id, 0, 2);
    if i32_header_ptr.is_null() {
        return Err("production render buffers were not registered by the JIT".to_string());
    }
    let version = unsafe { *i32_header_ptr.add(1) };
    if version != STASIS_RENDER_VERSION {
        return Err(format!(
            "JIT frame has unsupported gfx_cmd version {version}; expected {STASIS_RENDER_VERSION}"
        ));
    }
    let i32_ptr = stasis_jit_global_i32_array_ptr(i32_id, 0, STASIS_RENDER_I32_COUNT as i32);
    let f32_ptr = stasis_jit_global_f32_array_ptr(
        global_path_hash("gfx_cmd_f32"),
        0,
        STASIS_RENDER_F32_COUNT as i32,
    );
    let u8_ptr = global_u8_array_ptr(
        global_path_hash("gfx_cmd_u8"),
        0,
        STASIS_RENDER_U8_COUNT as i32,
    );
    if i32_ptr.is_null() || f32_ptr.is_null() || u8_ptr.is_null() {
        return Err("production render buffers were not registered by the JIT".to_string());
    }
    let source_i32 = unsafe { std::slice::from_raw_parts(i32_ptr, STASIS_RENDER_I32_COUNT) };
    let source_f32 = unsafe { std::slice::from_raw_parts(f32_ptr, STASIS_RENDER_F32_COUNT) };
    let source_u8 = unsafe { std::slice::from_raw_parts(u8_ptr, STASIS_RENDER_U8_COUNT) };
    if source_i32[0] != STASIS_RENDER_MAGIC || source_i32[1] != STASIS_RENDER_VERSION {
        return Err("JIT frame is not a supported production gfx_cmd frame".to_string());
    }

    let lines = source_i32[3].clamp(0, STASIS_RENDER_MAX_LINES as i32) as usize;
    let counts = RenderActiveCounts {
        lines,
        rects: source_i32[STASIS_RENDER_RECT_COUNT_INDEX]
            .clamp(0, (STASIS_RENDER_MAX_GEOMETRY - lines) as i32) as usize,
        sprites: source_i32[4].clamp(0, STASIS_RENDER_MAX_SPRITES as i32) as usize,
        sprite_runs: source_i32[STASIS_RENDER_SPRITE_RUN_COUNT_INDEX]
            .clamp(0, STASIS_RENDER_MAX_SPRITE_RUNS as i32) as usize,
        text: source_i32[7].clamp(0, STASIS_RENDER_MAX_TEXT as i32) as usize,
        text_bytes: source_i32[9].clamp(0, STASIS_RENDER_U8_COUNT as i32) as usize,
        order: source_i32[STASIS_RENDER_ORDER_COUNT_INDEX].clamp(0, STASIS_RENDER_MAX_ORDER as i32)
            as usize,
        clips: source_i32[STASIS_RENDER_CLIP_COUNT_INDEX].clamp(0, STASIS_RENDER_MAX_CLIPS as i32)
            as usize,
    };
    out_i32[..STASIS_RENDER_HEADER_I32_COUNT]
        .copy_from_slice(&source_i32[..STASIS_RENDER_HEADER_I32_COUNT]);
    out_i32[STASIS_RENDER_ORDER_COUNT_INDEX..STASIS_RENDER_ORDER_HEADER_END].fill(0);
    out_i32[STASIS_RENDER_ORDER_COUNT_INDEX] = counts.order as i32;
    out_i32[STASIS_RENDER_RECT_COUNT_INDEX..STASIS_RENDER_RECT_HEADER_END].fill(0);
    out_i32[STASIS_RENDER_RECT_COUNT_INDEX..STASIS_RENDER_RECT_HEADER_END].copy_from_slice(
        &source_i32[STASIS_RENDER_RECT_COUNT_INDEX..STASIS_RENDER_RECT_HEADER_END],
    );
    out_i32[STASIS_RENDER_RECT_COUNT_INDEX] = counts.rects as i32;
    out_i32[STASIS_RENDER_CLIP_COUNT_INDEX..STASIS_RENDER_CLIP_HEADER_END].fill(0);
    out_i32[STASIS_RENDER_CLIP_COUNT_INDEX..STASIS_RENDER_CLIP_HEADER_END].copy_from_slice(
        &source_i32[STASIS_RENDER_CLIP_COUNT_INDEX..STASIS_RENDER_CLIP_HEADER_END],
    );
    out_i32[STASIS_RENDER_CLIP_COUNT_INDEX] = counts.clips as i32;
    out_i32[STASIS_RENDER_SPRITE_RUN_COUNT_INDEX..STASIS_RENDER_SPRITE_RUN_HEADER_END]
        .copy_from_slice(
            &source_i32[STASIS_RENDER_SPRITE_RUN_COUNT_INDEX..STASIS_RENDER_SPRITE_RUN_HEADER_END],
        );
    out_i32[STASIS_RENDER_SPRITE_RUN_COUNT_INDEX] = counts.sprite_runs as i32;
    let sprite_end = STASIS_RENDER_SPRITE_BASE + counts.sprites * STASIS_RENDER_SPRITE_STRIDE_I32;
    out_i32[STASIS_RENDER_SPRITE_BASE..sprite_end]
        .copy_from_slice(&source_i32[STASIS_RENDER_SPRITE_BASE..sprite_end]);
    let text_i32_end = STASIS_RENDER_TEXT_BASE_I32 + counts.text * STASIS_RENDER_TEXT_STRIDE_I32;
    out_i32[STASIS_RENDER_TEXT_BASE_I32..text_i32_end]
        .copy_from_slice(&source_i32[STASIS_RENDER_TEXT_BASE_I32..text_i32_end]);
    let sprite_run_end =
        STASIS_RENDER_SPRITE_RUN_BASE + counts.sprite_runs * STASIS_RENDER_SPRITE_RUN_STRIDE_I32;
    out_i32[STASIS_RENDER_SPRITE_RUN_BASE..sprite_run_end]
        .copy_from_slice(&source_i32[STASIS_RENDER_SPRITE_RUN_BASE..sprite_run_end]);
    let order_end = STASIS_RENDER_ORDER_BASE + counts.order;
    out_i32[STASIS_RENDER_ORDER_BASE..order_end]
        .copy_from_slice(&source_i32[STASIS_RENDER_ORDER_BASE..order_end]);

    out_f32[STASIS_RENDER_F_CLEAR_BASE..STASIS_RENDER_F_LINE_BASE]
        .copy_from_slice(&source_f32[STASIS_RENDER_F_CLEAR_BASE..STASIS_RENDER_F_LINE_BASE]);
    let line_end = STASIS_RENDER_F_LINE_BASE + counts.lines * STASIS_RENDER_LINE_STRIDE;
    out_f32[STASIS_RENDER_F_LINE_BASE..line_end]
        .copy_from_slice(&source_f32[STASIS_RENDER_F_LINE_BASE..line_end]);
    let rect_start = if counts.rects == 0 {
        STASIS_RENDER_SPRITE_BASE_F32
    } else {
        STASIS_RENDER_RECT_REVERSE_BASE_F32 - (counts.rects - 1) * STASIS_RENDER_GEOMETRY_STRIDE_F32
    };
    out_f32[rect_start..STASIS_RENDER_SPRITE_BASE_F32]
        .copy_from_slice(&source_f32[rect_start..STASIS_RENDER_SPRITE_BASE_F32]);
    let sprite_f32_end =
        STASIS_RENDER_SPRITE_BASE_F32 + counts.sprites * STASIS_RENDER_SPRITE_STRIDE_F32;
    out_f32[STASIS_RENDER_SPRITE_BASE_F32..sprite_f32_end]
        .copy_from_slice(&source_f32[STASIS_RENDER_SPRITE_BASE_F32..sprite_f32_end]);
    let source_text_base = STASIS_RENDER_TEXT_BASE_F32;
    let text_values = counts.text * STASIS_RENDER_TEXT_STRIDE_F32;
    out_f32[STASIS_RENDER_TEXT_BASE_F32..STASIS_RENDER_TEXT_BASE_F32 + text_values]
        .copy_from_slice(&source_f32[source_text_base..source_text_base + text_values]);
    if counts.clips > 0 {
        let clip_values = counts.clips * STASIS_RENDER_CLIP_STRIDE_F32;
        out_f32[STASIS_RENDER_CLIP_BASE_F32..STASIS_RENDER_CLIP_BASE_F32 + clip_values]
            .copy_from_slice(
                &source_f32[STASIS_RENDER_CLIP_BASE_F32..STASIS_RENDER_CLIP_BASE_F32 + clip_values],
            );
    }
    out_u8[..counts.text_bytes].copy_from_slice(&source_u8[..counts.text_bytes]);
    Ok(counts)
}

unsafe extern "C" {
    fn stasis_render_trace_native(
        cmd_i32: *const i32,
        cmd_f32: *const f32,
        cmd_u8: *const u8,
    ) -> u32;
}

/// Computes the command trace for the current render ABI from host-owned buffers.
///
/// This is an internal current-build seam: inputs with non-canonical capacities,
/// magic, or version are rejected rather than interpreted as an older layout.
pub fn current_render_trace(cmd_i32: &[i32], cmd_f32: &[f32], cmd_u8: &[u8]) -> u32 {
    if cmd_i32.len() != STASIS_RENDER_I32_COUNT
        || cmd_f32.len() != STASIS_RENDER_F32_COUNT
        || cmd_u8.len() != STASIS_RENDER_U8_COUNT
        || cmd_i32.first().copied() != Some(STASIS_RENDER_MAGIC)
        || cmd_i32.get(1).copied() != Some(STASIS_RENDER_VERSION)
    {
        return 0;
    }
    unsafe { stasis_render_trace_native(cmd_i32.as_ptr(), cmd_f32.as_ptr(), cmd_u8.as_ptr()) }
}

#[no_mangle]
pub unsafe extern "C" fn stasis_jit_render_trace(
    cmd_i32_id: i32,
    cmd_i32_len: i32,
    cmd_f32_id: i32,
    cmd_f32_len: i32,
    cmd_u8_id: i32,
    cmd_u8_len: i32,
) -> i32 {
    if cmd_i32_len as usize != STASIS_RENDER_I32_COUNT
        || cmd_f32_len as usize != STASIS_RENDER_F32_COUNT
        || cmd_u8_len != STASIS_RENDER_U8_COUNT as i32
    {
        return 0;
    }
    let cmd_i32_header = stasis_jit_global_i32_array_ptr(cmd_i32_id, 0, 2);
    if cmd_i32_header.is_null() {
        return 0;
    }
    let magic = *cmd_i32_header;
    let version = *cmd_i32_header.add(1);
    if magic != STASIS_RENDER_MAGIC || version != STASIS_RENDER_VERSION {
        return 0;
    }
    let cmd_i32 = stasis_jit_global_i32_array_ptr(cmd_i32_id, 0, cmd_i32_len);
    let cmd_f32 = stasis_jit_global_f32_array_ptr(cmd_f32_id, 0, cmd_f32_len);
    let cmd_u8 = global_u8_array_ptr(cmd_u8_id, 0, STASIS_RENDER_U8_COUNT as i32);
    if cmd_i32.is_null() || cmd_f32.is_null() || cmd_u8.is_null() {
        return 0;
    }
    stasis_render_trace_native(cmd_i32, cmd_f32, cmd_u8) as i32
}
fn jit_text_buffer_is_registered(value_id: i32) -> bool {
    if jit_collection_runtime_metadata_is_registered(value_id) {
        return true;
    }
    if registered_u8_arrays()
        .lock()
        .expect("registered u8 array table mutex poisoned")
        .contains_key(&(value_id, 0))
    {
        return true;
    }
    if registered_i32_arrays()
        .lock()
        .expect("registered i32 array table mutex poisoned")
        .contains_key(&(value_id, 0))
    {
        return true;
    }
    jit_i32_array_global_table()
        .lock()
        .expect("jit i32 array global table mutex poisoned")
        .keys()
        .any(|(collection_hash, field_hash, _)| *collection_hash == value_id && *field_hash == 0)
}

fn jit_global_text_bytes(value_id: i32) -> Option<Vec<u8>> {
    if !jit_text_buffer_is_registered(value_id) {
        return None;
    }
    let byte_len = stasis_jit_collection_i32_load(value_id, 1);
    if byte_len < 0 {
        return None;
    }

    let mut bytes = Vec::with_capacity(byte_len as usize);
    for index in 0..byte_len {
        let byte = stasis_jit_global_i32_array_load(value_id, 0, index);
        let Ok(byte) = u8::try_from(byte) else {
            return None;
        };
        bytes.push(byte);
    }
    Some(bytes)
}

#[no_mangle]
pub extern "C" fn stasis_jit_network_supported() -> i32 {
    #[cfg(feature = "network")]
    {
        return stasis_network::stasis_network_supported();
    }
    #[cfg(not(feature = "network"))]
    {
        0
    }
}

// The browser network-client ABI is intentionally unavailable to ordinary
// native JIT sessions. Deterministic recording opts into these inert bridges
// so a guest may import the shared stdlib without opening sockets, creating
// credentials, reading storage, or mutating a payload buffer.
const OFFLINE_WEB_NETWORK_MAX_MESSAGE_BYTES: i32 = 64 * 1024;

#[no_mangle]
pub extern "C" fn stasis_jit_offline_web_network_supported() -> i32 {
    0
}

#[no_mangle]
pub extern "C" fn stasis_jit_offline_web_network_connect() -> i32 {
    -4
}

#[no_mangle]
pub extern "C" fn stasis_jit_offline_web_network_status() -> i32 {
    -4
}

#[no_mangle]
pub extern "C" fn stasis_jit_offline_web_network_poll(_out_id: i32, capacity: i32) -> i32 {
    if !(0..=OFFLINE_WEB_NETWORK_MAX_MESSAGE_BYTES).contains(&capacity) {
        return -1;
    }
    -4
}

#[no_mangle]
pub extern "C" fn stasis_jit_offline_web_network_send(_payload_id: i32, length: i32) -> i32 {
    if !(0..=OFFLINE_WEB_NETWORK_MAX_MESSAGE_BYTES).contains(&length) {
        return -1;
    }
    -4
}

#[no_mangle]
pub extern "C" fn stasis_jit_offline_web_network_resume_seat() -> i32 {
    -1
}

#[no_mangle]
pub extern "C" fn stasis_jit_offline_web_network_last_sequence() -> i32 {
    0
}

#[no_mangle]
pub extern "C" fn stasis_jit_offline_web_network_checkpoint(seat: i32, last_sequence: i32) -> i32 {
    if !(-1..8).contains(&seat) || last_sequence < 0 {
        return -1;
    }
    -4
}

#[no_mangle]
pub extern "C" fn stasis_jit_network_host_random_seed() -> i32 {
    #[cfg(feature = "network")]
    {
        return stasis_network::stasis_network_random_seed();
    }
    #[cfg(not(feature = "network"))]
    {
        0
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_network_host_start(content_id: i32, content_length: i32) -> i32 {
    stasis_jit_network_host_start_bind(content_id, content_length, 0x7f000001)
}

#[no_mangle]
pub extern "C" fn stasis_jit_network_host_start_bind(
    content_id: i32,
    content_length: i32,
    bind_ipv4: i32,
) -> i32 {
    if content_length <= 0 || content_length as usize > 32 * 1024 * 1024 {
        return -1;
    }
    #[cfg(not(feature = "network"))]
    {
        let _ = (content_id, content_length, bind_ipv4);
        return -4;
    }
    #[cfg(feature = "network")]
    {
        if NETWORK_HOST_HANDLE.load(Ordering::Acquire) != 0 {
            return -2;
        }
        let Some(content) = jit_text_arg_bytes(content_id) else {
            return -1;
        };
        let length = content_length as usize;
        if length > content.len() {
            return -1;
        }
        let mut port = 0_u16;
        let handle = stasis_network::stasis_network_host_start_bind(
            0,
            bind_ipv4 as u32,
            content[..length].as_ptr(),
            length,
            &mut port,
        );
        if handle.is_null() {
            return -3;
        }
        NETWORK_HOST_HANDLE.store(handle as usize, Ordering::Release);
        i32::from(port)
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_network_host_start_text(content_id: i32) -> i32 {
    #[cfg(not(feature = "network"))]
    {
        let _ = content_id;
        return -4;
    }
    #[cfg(feature = "network")]
    {
        let Some(content) = jit_text_arg_bytes(content_id) else {
            return -1;
        };
        stasis_jit_network_host_start_bind(content_id, content.len() as i32, 0x7f000001)
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_network_host_start_bind_text(content_id: i32, bind_ipv4: i32) -> i32 {
    #[cfg(not(feature = "network"))]
    {
        let _ = (content_id, bind_ipv4);
        return -4;
    }
    #[cfg(feature = "network")]
    {
        let Some(content) = jit_text_arg_bytes(content_id) else {
            return -1;
        };
        stasis_jit_network_host_start_bind(content_id, content.len() as i32, bind_ipv4)
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_network_host_status() -> i32 {
    #[cfg(feature = "network")]
    {
        let handle = NETWORK_HOST_HANDLE.load(Ordering::Acquire);
        if handle == 0 {
            0
        } else {
            unsafe {
                stasis_network::stasis_network_host_status(
                    handle as *mut stasis_network::NetworkHost,
                )
            }
        }
    }
    #[cfg(not(feature = "network"))]
    {
        0
    }
}
#[no_mangle]
pub extern "C" fn stasis_jit_network_host_overflow_count() -> i32 {
    #[cfg(feature = "network")]
    {
        let handle = NETWORK_HOST_HANDLE.load(Ordering::Acquire);
        if handle == 0 {
            0
        } else {
            unsafe {
                stasis_network::stasis_network_host_overflow_count(
                    handle as *mut stasis_network::NetworkHost,
                ) as i32
            }
        }
    }
    #[cfg(not(feature = "network"))]
    {
        0
    }
}
#[no_mangle]
pub extern "C" fn stasis_jit_network_host_port() -> i32 {
    #[cfg(feature = "network")]
    {
        let handle = NETWORK_HOST_HANDLE.load(Ordering::Acquire);
        if handle == 0 {
            0
        } else {
            unsafe {
                stasis_network::stasis_network_host_port(handle as *mut stasis_network::NetworkHost)
                    as i32
            }
        }
    }
    #[cfg(not(feature = "network"))]
    {
        0
    }
}
#[no_mangle]
pub extern "C" fn stasis_jit_network_host_poll(
    out_fields_id: i32,
    out_field_capacity: i32,
    out_payload_id: i32,
    out_payload_capacity: i32,
) -> i32 {
    if out_field_capacity < 3 || out_payload_capacity < 0 {
        return -1;
    }
    #[cfg(not(feature = "network"))]
    {
        let _ = (out_fields_id, out_payload_id);
        return -4;
    }
    #[cfg(feature = "network")]
    {
        let handle = NETWORK_HOST_HANDLE.load(Ordering::Acquire);
        if handle == 0 {
            return -3;
        }
        let mut event = stasis_network::StasisNetworkEvent {
            kind: 0,
            connection: 0,
            length: 0,
            payload: [0; stasis_network::MAX_MESSAGE_BYTES],
        };
        let result = unsafe {
            stasis_network::stasis_network_host_poll(
                handle as *mut stasis_network::NetworkHost,
                &mut event,
            )
        };
        if result <= 0 {
            return result;
        }
        if event.length as usize > out_payload_capacity as usize {
            return -1;
        }
        for (index, value) in [
            event.kind as i32,
            event.connection as i32,
            event.length as i32,
        ]
        .into_iter()
        .enumerate()
        {
            stasis_jit_global_i32_array_store(out_fields_id, 0, index as i32, value);
        }
        for index in 0..event.length as usize {
            stasis_jit_global_i32_array_store(
                out_payload_id,
                0,
                index as i32,
                i32::from(event.payload[index]),
            );
        }
        result
    }
}
#[no_mangle]
pub extern "C" fn stasis_jit_network_host_send(
    connection: i32,
    payload_id: i32,
    payload_length: i32,
) -> i32 {
    if connection <= 0 || payload_length < 0 || payload_length as usize > 64 * 1024 {
        return -1;
    }
    #[cfg(not(feature = "network"))]
    {
        let _ = payload_id;
        return -4;
    }
    #[cfg(feature = "network")]
    {
        let handle = NETWORK_HOST_HANDLE.load(Ordering::Acquire);
        if handle == 0 {
            return -3;
        }
        let mut payload = Vec::with_capacity(payload_length as usize);
        for index in 0..payload_length {
            let value = stasis_jit_global_i32_array_load(payload_id, 0, index);
            let Ok(value) = u8::try_from(value) else {
                return -30 - index;
            };
            payload.push(value);
        }
        unsafe {
            stasis_network::stasis_network_host_send(
                handle as *mut stasis_network::NetworkHost,
                connection as u32,
                payload.as_ptr(),
                payload.len(),
            )
        }
    }
}
#[no_mangle]
pub extern "C" fn stasis_jit_network_host_stop() {
    #[cfg(feature = "network")]
    {
        let handle = NETWORK_HOST_HANDLE.swap(0, Ordering::AcqRel);
        if handle != 0 {
            unsafe {
                stasis_network::stasis_network_host_stop(
                    handle as *mut stasis_network::NetworkHost,
                );
            }
        }
    }
}

// Bounded realtime controls use the same Rust scheduler for native and JIT
// guests.  These wrappers intentionally expose only scalar operations; game
// state and rendering remain owned by the guest.
fn stasis_network_payload_limit() -> usize {
    #[cfg(feature = "network")]
    {
        stasis_network::REALTIME_NATIVE_MAX_PAYLOAD
    }
    #[cfg(not(feature = "network"))]
    {
        0
    }
}

const REALTIME_GUEST_MAX_TICK: u64 = i32::MAX as u64;
const REALTIME_GUEST_MAX_EPOCH: i32 = i32::MAX;

#[no_mangle]
pub extern "C" fn stasis_jit_realtime_start(
    simulation_hz: i32,
    presentation_hz: i32,
    control_hz: i32,
    input_delay_ticks: i32,
    seats: i32,
) -> i32 {
    #[cfg(feature = "network")]
    {
        if simulation_hz < 0
            || presentation_hz < 0
            || control_hz < 0
            || input_delay_ticks < 0
            || seats < 0
        {
            return -1;
        }
        return stasis_network::stasis_realtime_start(
            simulation_hz,
            presentation_hz,
            control_hz,
            input_delay_ticks,
            seats,
        );
    }
    #[cfg(not(feature = "network"))]
    {
        let _ = (
            simulation_hz,
            presentation_hz,
            control_hz,
            input_delay_ticks,
            seats,
        );
        -4
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_realtime_stop() -> i32 {
    #[cfg(feature = "network")]
    {
        return stasis_network::stasis_realtime_stop();
    }
    #[cfg(not(feature = "network"))]
    {
        -4
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_realtime_submit_payload(payload_id: i32, payload_length: i32) -> i32 {
    if payload_length < 0 || payload_length as usize > stasis_network_payload_limit() {
        return -1;
    }
    if jit_i32_array_capacity(payload_id, 0)
        .is_none_or(|capacity| capacity < payload_length as usize)
    {
        return -11;
    }
    #[cfg(feature = "network")]
    {
        let mut payload = Vec::with_capacity(payload_length as usize);
        for index in 0..payload_length {
            let value = stasis_jit_global_i32_array_load(payload_id, 0, index);
            let Ok(value) = u8::try_from(value) else {
                return -1;
            };
            payload.push(value);
        }
        return stasis_network::submit_realtime_payload_bytes(&payload);
    }
    #[cfg(not(feature = "network"))]
    {
        let _ = payload_id;
        -4
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_realtime_build_payload(
    out_payload_id: i32,
    capacity: i32,
    seat: i32,
    epoch: i32,
    sequence: i32,
    apply_tick: i32,
    buttons: i32,
    axis_x: i32,
    axis_y: i32,
) -> i32 {
    if seat < 0
        || seat >= 8
        || epoch <= 0
        || sequence <= 0
        || apply_tick < 0
        || apply_tick as u64 > REALTIME_GUEST_MAX_TICK
        || epoch > REALTIME_GUEST_MAX_EPOCH
        || buttons < 0
        || buttons > i32::from(u16::MAX)
        || !(-128..=127).contains(&axis_x)
        || !(-128..=127).contains(&axis_y)
        || capacity < 0
        || capacity as usize > stasis_network_payload_limit()
    {
        return -1;
    }
    #[cfg(feature = "network")]
    {
        if jit_i32_array_capacity(out_payload_id, 0)
            .is_none_or(|registered| registered < capacity as usize)
        {
            return -11;
        }
        let mut payload = vec![0_i32; capacity as usize];
        let length = unsafe {
            stasis_network::stasis_realtime_build_payload(
                payload.as_mut_ptr(),
                capacity,
                seat,
                epoch,
                sequence,
                apply_tick,
                buttons,
                axis_x,
                axis_y,
            )
        };
        if length < 0 {
            return length;
        }
        for (index, value) in payload.into_iter().take(length as usize).enumerate() {
            stasis_jit_global_i32_array_store(out_payload_id, 0, index as i32, value);
        }
        return length;
    }
    #[cfg(not(feature = "network"))]
    {
        let _ = out_payload_id;
        -4
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_realtime_resync_required() -> i32 {
    #[cfg(feature = "network")]
    {
        return stasis_network::stasis_realtime_resync_required();
    }
    #[cfg(not(feature = "network"))]
    {
        -1
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_realtime_record_hash(tick: i32, hash_low: i32, hash_high: i32) -> i32 {
    #[cfg(feature = "network")]
    {
        return stasis_network::stasis_realtime_record_hash(tick, hash_low, hash_high);
    }
    #[cfg(not(feature = "network"))]
    {
        let _ = (tick, hash_low, hash_high);
        -4
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_realtime_apply_snapshot(
    revision: i32,
    tick: i32,
    seat_count: i32,
    buttons_id: i32,
    axis_x_id: i32,
    axis_y_id: i32,
    sequences_id: i32,
    epochs_id: i32,
    active_id: i32,
) -> i32 {
    if revision <= 0 || tick < 0 || seat_count <= 0 || seat_count > 8 {
        return -1;
    }
    let count = seat_count as usize;
    for id in [
        buttons_id,
        axis_x_id,
        axis_y_id,
        sequences_id,
        epochs_id,
        active_id,
    ] {
        if jit_i32_array_capacity(id, 0).is_none_or(|capacity| capacity < count) {
            return -11;
        }
    }
    #[cfg(feature = "network")]
    {
        let mut buttons = Vec::<i32>::with_capacity(count);
        let mut axis_x = Vec::<i32>::with_capacity(count);
        let mut axis_y = Vec::<i32>::with_capacity(count);
        let mut sequences = Vec::<i32>::with_capacity(count);
        let mut epochs = Vec::<i32>::with_capacity(count);
        let mut active = Vec::<i32>::with_capacity(count);
        for index in 0..seat_count {
            let button = stasis_jit_global_i32_array_load(buttons_id, 0, index);
            let x = stasis_jit_global_i32_array_load(axis_x_id, 0, index);
            let y = stasis_jit_global_i32_array_load(axis_y_id, 0, index);
            let sequence = stasis_jit_global_i32_array_load(sequences_id, 0, index);
            let epoch = stasis_jit_global_i32_array_load(epochs_id, 0, index);
            let is_active = stasis_jit_global_i32_array_load(active_id, 0, index);
            if button < 0
                || button > i32::from(u16::MAX)
                || !(-128..=127).contains(&x)
                || !(-128..=127).contains(&y)
                || sequence < 0
                || epoch <= 0
                || epoch > REALTIME_GUEST_MAX_EPOCH
                || !matches!(is_active, 0 | 1)
            {
                return -1;
            }
            buttons.push(button);
            axis_x.push(x);
            axis_y.push(y);
            sequences.push(sequence);
            epochs.push(epoch);
            active.push(is_active);
        }
        return unsafe {
            stasis_network::stasis_realtime_apply_snapshot(
                revision,
                tick,
                seat_count,
                buttons.as_ptr(),
                axis_x.as_ptr(),
                axis_y.as_ptr(),
                sequences.as_ptr(),
                epochs.as_ptr(),
                active.as_ptr(),
            )
        };
    }
    #[cfg(not(feature = "network"))]
    {
        let _ = (
            buttons_id,
            axis_x_id,
            axis_y_id,
            sequences_id,
            epochs_id,
            active_id,
        );
        -4
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_realtime_current_tick() -> i32 {
    #[cfg(feature = "network")]
    {
        return stasis_network::stasis_realtime_current_tick();
    }
    #[cfg(not(feature = "network"))]
    {
        -1
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_realtime_current_epoch(seat: i32) -> i32 {
    #[cfg(feature = "network")]
    {
        return if seat < 0 {
            -1
        } else {
            stasis_network::stasis_realtime_current_epoch(seat)
        };
    }
    #[cfg(not(feature = "network"))]
    {
        let _ = seat;
        -1
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_realtime_schedule(
    seat: i32,
    epoch: i32,
    sequence: i32,
    apply_tick: i32,
    buttons: i32,
    axis_x: i32,
    axis_y: i32,
) -> i32 {
    #[cfg(feature = "network")]
    {
        if seat < 0 || epoch < 0 || sequence < 0 || buttons < 0 {
            return -1;
        }
        return stasis_network::stasis_realtime_schedule(
            seat, epoch, sequence, apply_tick, buttons, axis_x, axis_y,
        );
    }
    #[cfg(not(feature = "network"))]
    {
        let _ = (seat, epoch, sequence, apply_tick, buttons, axis_x, axis_y);
        -4
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_realtime_advance() -> i32 {
    #[cfg(feature = "network")]
    {
        return stasis_network::stasis_realtime_advance();
    }
    #[cfg(not(feature = "network"))]
    {
        -4
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_realtime_read_control(
    seat: i32,
    out_buttons_id: i32,
    out_axis_x_id: i32,
    out_axis_y_id: i32,
) -> i32 {
    if seat < 0 {
        return -1;
    }
    if [out_buttons_id, out_axis_x_id, out_axis_y_id]
        .into_iter()
        .any(|id| jit_i32_array_capacity(id, 0).is_none_or(|capacity| capacity < 1))
    {
        return -11;
    }
    #[cfg(feature = "network")]
    {
        let mut buttons = 0_i32;
        let mut axis_x = 0_i32;
        let mut axis_y = 0_i32;
        let result = unsafe {
            stasis_network::stasis_realtime_read_control(
                seat,
                &mut buttons,
                &mut axis_x,
                &mut axis_y,
            )
        };
        if result == 0 {
            stasis_jit_global_i32_array_store(out_buttons_id, 0, 0, buttons as i32);
            stasis_jit_global_i32_array_store(out_axis_x_id, 0, 0, axis_x);
            stasis_jit_global_i32_array_store(out_axis_y_id, 0, 0, axis_y);
        }
        return result;
    }
    #[cfg(not(feature = "network"))]
    {
        let _ = (out_buttons_id, out_axis_x_id, out_axis_y_id);
        -4
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_realtime_disconnect(seat: i32) -> i32 {
    #[cfg(feature = "network")]
    {
        return if seat < 0 {
            -1
        } else {
            stasis_network::stasis_realtime_disconnect(seat)
        };
    }
    #[cfg(not(feature = "network"))]
    {
        let _ = seat;
        -4
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_realtime_reconnect(seat: i32) -> i32 {
    #[cfg(feature = "network")]
    {
        return if seat < 0 {
            -1
        } else {
            stasis_network::stasis_realtime_reconnect(seat)
        };
    }
    #[cfg(not(feature = "network"))]
    {
        let _ = seat;
        -4
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_realtime_pause() -> i32 {
    #[cfg(feature = "network")]
    {
        return stasis_network::stasis_realtime_pause();
    }
    #[cfg(not(feature = "network"))]
    {
        -4
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_realtime_focus_lost() -> i32 {
    #[cfg(feature = "network")]
    {
        return stasis_network::stasis_realtime_focus_lost();
    }
    #[cfg(not(feature = "network"))]
    {
        -4
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_realtime_rematch() -> i32 {
    #[cfg(feature = "network")]
    {
        return stasis_network::stasis_realtime_rematch();
    }
    #[cfg(not(feature = "network"))]
    {
        -4
    }
}

fn jit_text_arg_bytes(value_id: i32) -> Option<Vec<u8>> {
    if let Some(bytes) = jit_global_text_bytes(value_id) {
        return Some(bytes);
    }

    let table = jit_string_literal_table();
    let guard = table
        .lock()
        .expect("jit string literal table mutex poisoned");
    guard.get(&value_id).map(|text| text.as_bytes().to_vec())
}

thread_local! {
    static JIT_TEXT_SCRATCH: std::cell::RefCell<Vec<u8>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

fn with_jit_text_arg_bytes<R>(value_id: i32, mut callback: impl FnMut(&[u8]) -> R) -> Option<R> {
    if jit_text_buffer_is_registered(value_id) {
        let byte_len = usize::try_from(stasis_jit_collection_i32_load(value_id, 1)).ok()?;
        return JIT_TEXT_SCRATCH.with(|scratch| {
            let mut bytes = scratch.borrow_mut();
            bytes.clear();
            bytes.try_reserve(byte_len).ok()?;
            for index in 0..byte_len {
                let byte = stasis_jit_global_i32_array_load(value_id, 0, index as i32);
                bytes.push(u8::try_from(byte).ok()?);
            }
            Some(callback(&bytes))
        });
    }

    let table = jit_string_literal_table();
    let guard = table
        .lock()
        .expect("jit string literal table mutex poisoned");
    guard.get(&value_id).map(|text| callback(text.as_bytes()))
}

#[derive(Clone, Copy)]
pub struct EmbeddedGraphicsHost {
    pub load_sprite: fn(&[u8], i32, i32) -> i32,
    pub release_sprite: fn(i32),
    pub load_font: fn(&[u8], i32) -> i32,
    pub measure_text: fn(i32, &[u8]) -> f32,
    pub cache_text: fn(i32, &[u8]) -> i32,
    pub replace_text: fn(i32, i32, &[u8]) -> i32,
    pub measure_text_cached: fn(i32) -> f32,
    pub measure_text_cached_height: fn(i32) -> f32,
    pub poll_reload: fn(i32) -> i32,
}

pub const ASSET_EXTERN_SEAM_EVIDENCE_ENV: &str = "STASIS_ASSET_EXTERN_SEAM_EVIDENCE";

fn asset_extern_seam_evidence_path() -> Option<PathBuf> {
    std::env::var_os(ASSET_EXTERN_SEAM_EVIDENCE_ENV).map(PathBuf::from)
}

fn asset_extern_seam_text_hex(text: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(text.len() * 2);
    for byte in text {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn record_asset_extern_seam_call(kind: &str, fields: &[String]) -> Option<bool> {
    let path = asset_extern_seam_evidence_path()?;
    let mut file = match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        Ok(file) => file,
        Err(error) => {
            eprintln!(
                "asset extern seam recorder failed to open {}: {error}",
                path.display()
            );
            return Some(false);
        }
    };
    let mut line = format!("stasis.asset_extern.v1\t{kind}");
    for field in fields {
        line.push('\t');
        line.push_str(field);
    }
    Some(writeln!(file, "{line}").is_ok())
}

thread_local! {
    static EMBEDDED_GRAPHICS_HOST: std::cell::Cell<Option<EmbeddedGraphicsHost>> =
        const { std::cell::Cell::new(None) };
}

pub fn set_embedded_graphics_host(host: Option<EmbeddedGraphicsHost>) {
    EMBEDDED_GRAPHICS_HOST.with(|slot| slot.set(host));
}

pub fn embedded_graphics_host_is_set() -> bool {
    EMBEDDED_GRAPHICS_HOST.with(|slot| slot.get().is_some())
}

fn embedded_graphics_host() -> Option<EmbeddedGraphicsHost> {
    EMBEDDED_GRAPHICS_HOST.with(|slot| slot.get())
}

fn jit_text_arg_to_cstring(value_id: i32) -> Result<CString, String> {
    let bytes = jit_text_arg_bytes(value_id).ok_or_else(|| {
        format!("missing jit text handle for id={value_id} (neither literal nor utf8 buffer)")
    })?;
    CString::new(bytes)
        .map_err(|_| "string contains interior NUL byte; cannot pass to C runtime".to_string())
}

// JIT string->C bridge for startup/asset extern calls.
// The language-level `string` currently lowers to an i32 handle in the JIT path, so we must
// translate that into a stable `const char*` when calling the C runtime.
#[no_mangle]
pub extern "C" fn stasis_jit_gfx_load_sprite(path_id: i32, max_w: i32, max_h: i32) -> i32 {
    if asset_extern_seam_evidence_path().is_some() {
        let Some(path) = jit_text_arg_bytes(path_id) else {
            return 0;
        };
        return if record_asset_extern_seam_call(
            "load_sprite",
            &[
                asset_extern_seam_text_hex(&path),
                max_w.to_string(),
                max_h.to_string(),
                "101".to_string(),
            ],
        ) == Some(true)
        {
            101
        } else {
            0
        };
    }
    let atlas_policy = if let Some(path) = jit_text_arg_bytes(path_id) {
        let path_text = String::from_utf8_lossy(&path);
        u32::try_from(max_w)
            .ok()
            .zip(u32::try_from(max_h).ok())
            .map_or_else(HotRenderAtlasPolicy::default, |(width, height)| {
                hot_render_atlas_policy(&path_text, width, height)
            })
    } else {
        HotRenderAtlasPolicy::default()
    };
    if let (Some(host), Some(path)) = (embedded_graphics_host(), jit_text_arg_bytes(path_id)) {
        return (host.load_sprite)(&path, max_w, max_h);
    }
    let Ok(path) = jit_text_arg_to_cstring(path_id) else {
        eprintln!("gfx_load_sprite failed: unknown string handle {path_id}");
        return 0;
    };
    let api = match stasis_graphics_assets_api() {
        Ok(api) => api,
        Err(error) => {
            eprintln!("gfx_load_sprite failed: {error}");
            return 0;
        }
    };
    if let Some(address) = api.stasis_gfx_set_next_sprite_atlas_policy_v3 {
        #[cfg(windows)]
        let set_policy: extern "system" fn(i32, u64, u32, u64, u32, u32) =
            unsafe { std::mem::transmute(address) };
        #[cfg(not(windows))]
        let set_policy: extern "C" fn(i32, u64, u32, u64, u32, u32) =
            unsafe { std::mem::transmute(address) };
        set_policy(
            i32::from(atlas_policy.eligible),
            atlas_policy.group_id,
            atlas_policy.member_count,
            atlas_policy.logical_pixel_area,
            atlas_policy.max_logical_width,
            atlas_policy.max_logical_height,
        );
    }
    #[cfg(windows)]
    let callback: extern "system" fn(*const c_char, i32, i32) -> i32 =
        unsafe { std::mem::transmute(api.stasis_gfx_load_sprite) };
    #[cfg(not(windows))]
    let callback: extern "C" fn(*const c_char, i32, i32) -> i32 =
        unsafe { std::mem::transmute(api.stasis_gfx_load_sprite) };
    let handle = callback(path.as_ptr(), max_w, max_h);
    if handle == 0 {
        eprintln!("gfx_load_sprite failed for {}", path.to_string_lossy());
    }
    handle
}

#[no_mangle]
pub extern "C" fn stasis_jit_asset_request_sprite(path_id: i32, max_w: i32, max_h: i32) -> i32 {
    let atlas_policy =
        jit_text_arg_bytes(path_id).map_or_else(HotRenderAtlasPolicy::default, |path| {
            let path_text = String::from_utf8_lossy(&path);
            u32::try_from(max_w)
                .ok()
                .zip(u32::try_from(max_h).ok())
                .map_or_else(HotRenderAtlasPolicy::default, |(width, height)| {
                    hot_render_atlas_policy(&path_text, width, height)
                })
        });
    let Ok(path) = jit_text_arg_to_cstring(path_id) else {
        return 0;
    };
    let Ok(api) = stasis_graphics_assets_api() else {
        return 0;
    };
    if let Some(address) = api.stasis_asset_request_sprite_with_policy_v3 {
        #[cfg(windows)]
        let callback: extern "system" fn(
            *const c_char,
            i32,
            i32,
            i32,
            u64,
            u32,
            u64,
            u32,
            u32,
        ) -> i32 = unsafe { std::mem::transmute(address) };
        #[cfg(not(windows))]
        let callback: extern "C" fn(
            *const c_char,
            i32,
            i32,
            i32,
            u64,
            u32,
            u64,
            u32,
            u32,
        ) -> i32 = unsafe { std::mem::transmute(address) };
        return callback(
            path.as_ptr(),
            max_w,
            max_h,
            i32::from(atlas_policy.eligible),
            atlas_policy.group_id,
            atlas_policy.member_count,
            atlas_policy.logical_pixel_area,
            atlas_policy.max_logical_width,
            atlas_policy.max_logical_height,
        );
    }
    let Some(address) = api.stasis_asset_request_sprite else {
        return 0;
    };
    #[cfg(windows)]
    let callback: extern "system" fn(*const c_char, i32, i32) -> i32 =
        unsafe { std::mem::transmute(address) };
    #[cfg(not(windows))]
    let callback: extern "C" fn(*const c_char, i32, i32) -> i32 =
        unsafe { std::mem::transmute(address) };
    callback(path.as_ptr(), max_w, max_h)
}

#[no_mangle]
pub extern "C" fn stasis_jit_asset_request_audio(path_id: i32) -> i32 {
    let Ok(path) = jit_text_arg_to_cstring(path_id) else {
        return 0;
    };
    let Ok(api) = stasis_graphics_assets_api() else {
        return 0;
    };
    let Some(address) = api.stasis_asset_request_audio else {
        return 0;
    };
    #[cfg(windows)]
    let callback: extern "system" fn(*const c_char) -> i32 =
        unsafe { std::mem::transmute(address) };
    #[cfg(not(windows))]
    let callback: extern "C" fn(*const c_char) -> i32 = unsafe { std::mem::transmute(address) };
    callback(path.as_ptr())
}

#[no_mangle]
pub extern "C" fn stasis_jit_asset_task_poll(task: i32) -> i32 {
    let Ok(api) = stasis_graphics_assets_api() else {
        return 0;
    };
    let Some(address) = api.stasis_asset_task_poll else {
        return 0;
    };
    #[cfg(windows)]
    let callback: extern "system" fn(i32) -> i32 = unsafe { std::mem::transmute(address) };
    #[cfg(not(windows))]
    let callback: extern "C" fn(i32) -> i32 = unsafe { std::mem::transmute(address) };
    callback(task)
}

#[no_mangle]
pub extern "C" fn stasis_jit_asset_task_take_handle(task: i32) -> i32 {
    let Ok(api) = stasis_graphics_assets_api() else {
        return 0;
    };
    let Some(address) = api.stasis_asset_task_take_handle else {
        return 0;
    };
    #[cfg(windows)]
    let callback: extern "system" fn(i32) -> i32 = unsafe { std::mem::transmute(address) };
    #[cfg(not(windows))]
    let callback: extern "C" fn(i32) -> i32 = unsafe { std::mem::transmute(address) };
    callback(task)
}

#[no_mangle]
pub extern "C" fn stasis_jit_asset_task_cancel(task: i32) {
    let Ok(api) = stasis_graphics_assets_api() else {
        return;
    };
    let Some(address) = api.stasis_asset_task_cancel else {
        return;
    };
    #[cfg(windows)]
    let callback: extern "system" fn(i32) = unsafe { std::mem::transmute(address) };
    #[cfg(not(windows))]
    let callback: extern "C" fn(i32) = unsafe { std::mem::transmute(address) };
    callback(task);
}

#[no_mangle]
pub extern "C" fn stasis_jit_gfx_release_sprite(handle: i32) {
    if let Some(host) = embedded_graphics_host() {
        (host.release_sprite)(handle);
        return;
    }
    let Ok(api) = stasis_graphics_assets_api() else {
        return;
    };
    #[cfg(windows)]
    let callback: extern "system" fn(i32) =
        unsafe { std::mem::transmute(api.stasis_gfx_release_sprite) };
    #[cfg(not(windows))]
    let callback: extern "C" fn(i32) =
        unsafe { std::mem::transmute(api.stasis_gfx_release_sprite) };
    callback(handle);
}

fn configured_preference_storage_root() -> &'static Mutex<Option<PathBuf>> {
    static ROOT: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();
    ROOT.get_or_init(|| Mutex::new(None))
}

#[cfg(not(windows))]
fn preference_storage_root() -> Option<PathBuf> {
    if let Some(root) = configured_preference_storage_root()
        .lock()
        .expect("preference storage root mutex poisoned")
        .clone()
    {
        return Some(root);
    }
    if let Some(root) = std::env::var_os("STASIS_STORAGE_DIR").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(root));
    }
    #[cfg(target_os = "macos")]
    {
        return std::env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .map(|root| root.join("Library/Application Support/StasisLang"));
    }
    #[cfg(not(target_os = "macos"))]
    {
        if let Some(root) = std::env::var_os("XDG_DATA_HOME").filter(|value| !value.is_empty()) {
            return Some(PathBuf::from(root).join("StasisLang"));
        }
        std::env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .map(|root| root.join(".local/share/StasisLang"))
    }
}

pub fn set_preference_storage_root(root: Option<PathBuf>) {
    *configured_preference_storage_root()
        .lock()
        .expect("preference storage root mutex poisoned") = root;
}

#[cfg(not(windows))]
fn preference_component_valid(value: &[u8]) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && value
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

#[cfg(not(windows))]
fn preference_path(scope: &[u8], key: &[u8], extension: &str) -> Option<PathBuf> {
    if !preference_component_valid(scope) || !preference_component_valid(key) {
        return None;
    }
    let scope = std::str::from_utf8(scope).ok()?;
    let key = std::str::from_utf8(key).ok()?;
    Some(
        preference_storage_root()?
            .join(scope)
            .join(format!("{key}.{extension}")),
    )
}

#[cfg(not(windows))]
fn portable_storage_load_i32(scope: &[u8], key: &[u8], fallback: i32) -> i32 {
    let Some(path) = preference_path(scope, key, "i32") else {
        return fallback;
    };
    let Ok(bytes) = std::fs::read(path) else {
        return fallback;
    };
    if bytes.len() > 63 {
        return fallback;
    }
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return fallback;
    };
    text.trim().parse::<i32>().unwrap_or(fallback)
}

#[cfg(not(windows))]
fn portable_storage_save_i32(scope: &[u8], key: &[u8], value: i32) -> bool {
    let Some(path) = preference_path(scope, key, "i32") else {
        return false;
    };
    let Some(directory) = path.parent() else {
        return false;
    };
    if std::fs::create_dir_all(directory).is_err() {
        return false;
    }
    let temporary = path.with_extension("i32.tmp");
    let result = (|| -> std::io::Result<()> {
        let mut file = std::fs::File::create(&temporary)?;
        writeln!(file, "{value}")?;
        file.sync_all()?;
        std::fs::rename(&temporary, &path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
        return false;
    }
    true
}

fn printable_ascii(bytes: &[u8]) -> bool {
    bytes.iter().all(|byte| (32..=126).contains(byte))
}

fn write_jit_ascii_buffer(out_id: i32, capacity: i32, bytes: &[u8]) -> i32 {
    let Ok(capacity) = usize::try_from(capacity) else {
        return -1;
    };
    if bytes.len() > capacity || !printable_ascii(bytes) {
        return -1;
    }
    for (index, byte) in bytes.iter().enumerate() {
        stasis_jit_global_i32_array_store(out_id, 0, index as i32, i32::from(*byte));
    }
    bytes.len() as i32
}

#[cfg(not(windows))]
fn portable_storage_load_ascii(scope: &[u8], key: &[u8], capacity: i32) -> Option<Vec<u8>> {
    let path = preference_path(scope, key, "ascii")?;
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() > usize::try_from(capacity).ok()? || !printable_ascii(&bytes) {
        return None;
    }
    Some(bytes)
}

#[cfg(not(windows))]
fn portable_storage_save_ascii(scope: &[u8], key: &[u8], value: &[u8]) -> bool {
    if !printable_ascii(value) {
        return false;
    }
    let Some(path) = preference_path(scope, key, "ascii") else {
        return false;
    };
    let Some(directory) = path.parent() else {
        return false;
    };
    if std::fs::create_dir_all(directory).is_err() {
        return false;
    }
    let temporary = path.with_extension("ascii.tmp");
    let result = (|| -> std::io::Result<()> {
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(value)?;
        file.sync_all()?;
        std::fs::rename(&temporary, &path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
        return false;
    }
    true
}

#[no_mangle]
pub extern "C" fn stasis_jit_storage_load_ascii(
    scope_id: i32,
    key_id: i32,
    out_id: i32,
    capacity: i32,
) -> i32 {
    if capacity <= 0 {
        return -1;
    }
    #[cfg(windows)]
    {
        let (Ok(scope), Ok(key)) = (
            jit_text_arg_to_cstring(scope_id),
            jit_text_arg_to_cstring(key_id),
        ) else {
            return -1;
        };
        let Ok(api) = stasis_graphics_assets_api() else {
            return -1;
        };
        let Some(address) = api.stasis_storage_load_ascii else {
            return -1;
        };
        let mut bytes = vec![0_u8; capacity as usize];
        let callback: extern "system" fn(*const c_char, *const c_char, *mut c_char, i32) -> i32 =
            unsafe { std::mem::transmute(address) };
        let loaded = callback(
            scope.as_ptr(),
            key.as_ptr(),
            bytes.as_mut_ptr().cast::<c_char>(),
            capacity,
        );
        if loaded < 0 || loaded > capacity {
            return -1;
        }
        return write_jit_ascii_buffer(out_id, capacity, &bytes[..loaded as usize]);
    }
    #[cfg(not(windows))]
    {
        let (Some(scope), Some(key)) = (jit_text_arg_bytes(scope_id), jit_text_arg_bytes(key_id))
        else {
            return -1;
        };
        let Some(bytes) = portable_storage_load_ascii(&scope, &key, capacity) else {
            return -1;
        };
        write_jit_ascii_buffer(out_id, capacity, &bytes)
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_storage_save_ascii(
    scope_id: i32,
    key_id: i32,
    value_id: i32,
    length: i32,
) -> i32 {
    let (Some(scope), Some(key), Some(value)) = (
        jit_text_arg_bytes(scope_id),
        jit_text_arg_bytes(key_id),
        jit_text_arg_bytes(value_id),
    ) else {
        return 0;
    };
    let Ok(length) = usize::try_from(length) else {
        return 0;
    };
    if length > value.len() || !printable_ascii(&value[..length]) {
        return 0;
    }
    #[cfg(windows)]
    {
        let (Ok(scope), Ok(key)) = (CString::new(scope), CString::new(key)) else {
            return 0;
        };
        let Ok(api) = stasis_graphics_assets_api() else {
            return 0;
        };
        let Some(address) = api.stasis_storage_save_ascii else {
            return 0;
        };
        let callback: extern "system" fn(*const c_char, *const c_char, *const c_char, i32) -> i32 =
            unsafe { std::mem::transmute(address) };
        return callback(
            scope.as_ptr(),
            key.as_ptr(),
            value.as_ptr().cast::<c_char>(),
            length as i32,
        );
    }
    #[cfg(not(windows))]
    {
        portable_storage_save_ascii(&scope, &key, &value[..length]) as i32
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_clipboard_load_ascii(out_id: i32, capacity: i32) -> i32 {
    if capacity <= 0 {
        return -1;
    }
    let Ok(api) = stasis_graphics_assets_api() else {
        return -1;
    };
    let Some(address) = api.stasis_clipboard_load_ascii else {
        return -1;
    };
    let mut bytes = vec![0_u8; capacity as usize];
    #[cfg(windows)]
    let callback: extern "system" fn(*mut c_char, i32) -> i32 =
        unsafe { std::mem::transmute(address) };
    #[cfg(not(windows))]
    let callback: extern "C" fn(*mut c_char, i32) -> i32 = unsafe { std::mem::transmute(address) };
    let loaded = callback(bytes.as_mut_ptr().cast::<c_char>(), capacity);
    if loaded < 0 || loaded > capacity {
        return -1;
    }
    write_jit_ascii_buffer(out_id, capacity, &bytes[..loaded as usize])
}

#[no_mangle]
pub extern "C" fn stasis_jit_clipboard_save_ascii(value_id: i32, length: i32) -> i32 {
    let Some(value) = jit_text_arg_bytes(value_id) else {
        return 0;
    };
    let Ok(length) = usize::try_from(length) else {
        return 0;
    };
    if length > value.len() || !printable_ascii(&value[..length]) {
        return 0;
    }
    let Ok(api) = stasis_graphics_assets_api() else {
        return 0;
    };
    let Some(address) = api.stasis_clipboard_save_ascii else {
        return 0;
    };
    #[cfg(windows)]
    let callback: extern "system" fn(*const c_char, i32) -> i32 =
        unsafe { std::mem::transmute(address) };
    #[cfg(not(windows))]
    let callback: extern "C" fn(*const c_char, i32) -> i32 =
        unsafe { std::mem::transmute(address) };
    callback(value.as_ptr().cast::<c_char>(), length as i32)
}

#[no_mangle]
pub extern "C" fn stasis_jit_storage_load_i32(scope_id: i32, key_id: i32, fallback: i32) -> i32 {
    #[cfg(windows)]
    {
        let (Ok(scope), Ok(key)) = (
            jit_text_arg_to_cstring(scope_id),
            jit_text_arg_to_cstring(key_id),
        ) else {
            return fallback;
        };
        let Ok(api) = stasis_graphics_assets_api() else {
            return fallback;
        };
        let Some(address) = api.stasis_storage_load_i32 else {
            return fallback;
        };
        let callback: extern "system" fn(*const c_char, *const c_char, i32) -> i32 =
            unsafe { std::mem::transmute(address) };
        return callback(scope.as_ptr(), key.as_ptr(), fallback);
    }
    #[cfg(not(windows))]
    {
        let (Some(scope), Some(key)) = (jit_text_arg_bytes(scope_id), jit_text_arg_bytes(key_id))
        else {
            return fallback;
        };
        portable_storage_load_i32(&scope, &key, fallback)
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_storage_save_i32(scope_id: i32, key_id: i32, value: i32) -> i32 {
    #[cfg(windows)]
    {
        let (Ok(scope), Ok(key)) = (
            jit_text_arg_to_cstring(scope_id),
            jit_text_arg_to_cstring(key_id),
        ) else {
            return 0;
        };
        let Ok(api) = stasis_graphics_assets_api() else {
            return 0;
        };
        let Some(address) = api.stasis_storage_save_i32 else {
            return 0;
        };
        let callback: extern "system" fn(*const c_char, *const c_char, i32) -> i32 =
            unsafe { std::mem::transmute(address) };
        return callback(scope.as_ptr(), key.as_ptr(), value);
    }
    #[cfg(not(windows))]
    {
        let (Some(scope), Some(key)) = (jit_text_arg_bytes(scope_id), jit_text_arg_bytes(key_id))
        else {
            return 0;
        };
        portable_storage_save_i32(&scope, &key, value) as i32
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_gfx_dump_bmp(path_id: i32) -> i32 {
    if asset_extern_seam_evidence_path().is_some() {
        let Some(path) = jit_text_arg_bytes(path_id) else {
            return 0;
        };
        return if record_asset_extern_seam_call(
            "dump_bmp",
            &[asset_extern_seam_text_hex(&path), "11".to_string()],
        ) == Some(true)
        {
            11
        } else {
            0
        };
    }
    let Ok(path) = jit_text_arg_to_cstring(path_id) else {
        return 0;
    };
    let Ok(api) = stasis_graphics_assets_api() else {
        return 0;
    };
    #[cfg(windows)]
    let callback: extern "system" fn(*const c_char) -> i32 =
        unsafe { std::mem::transmute(api.stasis_gfx_dump_bmp) };
    #[cfg(not(windows))]
    let callback: extern "C" fn(*const c_char) -> i32 =
        unsafe { std::mem::transmute(api.stasis_gfx_dump_bmp) };
    callback(path.as_ptr())
}

#[no_mangle]
pub extern "C" fn stasis_jit_gfx_dump_png(path_id: i32) -> i32 {
    if asset_extern_seam_evidence_path().is_some() {
        let Some(path) = jit_text_arg_bytes(path_id) else {
            return 0;
        };
        return if record_asset_extern_seam_call(
            "dump_png",
            &[asset_extern_seam_text_hex(&path), "12".to_string()],
        ) == Some(true)
        {
            12
        } else {
            0
        };
    }
    let Ok(path) = jit_text_arg_to_cstring(path_id) else {
        return 0;
    };
    let Ok(api) = stasis_graphics_assets_api() else {
        return 0;
    };
    let Some(address) = api.stasis_gfx_dump_png else {
        return 0;
    };
    #[cfg(windows)]
    let callback: extern "system" fn(*const c_char) -> i32 =
        unsafe { std::mem::transmute(address) };
    #[cfg(not(windows))]
    let callback: extern "C" fn(*const c_char) -> i32 = unsafe { std::mem::transmute(address) };
    callback(path.as_ptr())
}

#[no_mangle]
pub extern "C" fn stasis_jit_load_font(path_id: i32, size: i32) -> i32 {
    if asset_extern_seam_evidence_path().is_some() {
        let Some(path) = jit_text_arg_bytes(path_id) else {
            return 0;
        };
        return if record_asset_extern_seam_call(
            "load_font",
            &[
                asset_extern_seam_text_hex(&path),
                size.to_string(),
                "202".to_string(),
            ],
        ) == Some(true)
        {
            202
        } else {
            0
        };
    }
    if let (Some(host), Some(path)) = (embedded_graphics_host(), jit_text_arg_bytes(path_id)) {
        return (host.load_font)(&path, size);
    }
    let Ok(path) = jit_text_arg_to_cstring(path_id) else {
        return 0;
    };
    let Ok(api) = stasis_graphics_assets_api() else {
        return 0;
    };
    #[cfg(windows)]
    let callback: extern "system" fn(*const c_char, i32) -> i32 =
        unsafe { std::mem::transmute(api.stasis_load_font) };
    #[cfg(not(windows))]
    let callback: extern "C" fn(*const c_char, i32) -> i32 =
        unsafe { std::mem::transmute(api.stasis_load_font) };
    callback(path.as_ptr(), size)
}

#[no_mangle]
pub extern "C" fn stasis_jit_measure_text(font: i32, text_id: i32) -> f32 {
    if asset_extern_seam_evidence_path().is_some() {
        let Some(text) = jit_text_arg_bytes(text_id) else {
            return 0.0;
        };
        return if record_asset_extern_seam_call(
            "measure_text",
            &[
                font.to_string(),
                asset_extern_seam_text_hex(&text),
                18.75_f32.to_bits().to_string(),
            ],
        ) == Some(true)
        {
            18.75
        } else {
            0.0
        };
    }
    if let Some(host) = embedded_graphics_host() {
        if let Some(width) =
            with_jit_text_arg_bytes(text_id, |text| (host.measure_text)(font, text))
        {
            return width;
        }
    }
    let Ok(text) = jit_text_arg_to_cstring(text_id) else {
        return 0.0;
    };
    let Ok(api) = stasis_graphics_assets_api() else {
        return 0.0;
    };
    #[cfg(windows)]
    let callback: extern "system" fn(i32, *const c_char) -> f32 =
        unsafe { std::mem::transmute(api.stasis_measure_text) };
    #[cfg(not(windows))]
    let callback: extern "C" fn(i32, *const c_char) -> f32 =
        unsafe { std::mem::transmute(api.stasis_measure_text) };
    callback(font, text.as_ptr())
}

#[no_mangle]
pub extern "C" fn stasis_jit_gfx_cache_text(font: i32, text_id: i32) -> i32 {
    if asset_extern_seam_evidence_path().is_some() {
        let Some(text) = jit_text_arg_bytes(text_id) else {
            return 0;
        };
        return if record_asset_extern_seam_call(
            "cache_text",
            &[
                font.to_string(),
                asset_extern_seam_text_hex(&text),
                "303".to_string(),
            ],
        ) == Some(true)
        {
            303
        } else {
            0
        };
    }
    if let (Some(host), Some(text)) = (embedded_graphics_host(), jit_text_arg_bytes(text_id)) {
        return (host.cache_text)(font, &text);
    }
    let Ok(text) = jit_text_arg_to_cstring(text_id) else {
        return 0;
    };
    let Ok(api) = stasis_graphics_assets_api() else {
        return 0;
    };
    #[cfg(windows)]
    let callback: extern "system" fn(i32, *const c_char) -> i32 =
        unsafe { std::mem::transmute(api.stasis_gfx_cache_text) };
    #[cfg(not(windows))]
    let callback: extern "C" fn(i32, *const c_char) -> i32 =
        unsafe { std::mem::transmute(api.stasis_gfx_cache_text) };
    callback(font, text.as_ptr())
}

fn replace_cached_text(run_handle: i32, font: i32, text_id: i32) -> i32 {
    let Some(text) = jit_text_arg_bytes(text_id) else {
        return 0;
    };
    if asset_extern_seam_evidence_path().is_some() {
        let result = if std::str::from_utf8(&text).is_ok() {
            404
        } else {
            0
        };
        return if record_asset_extern_seam_call(
            "replace_text",
            &[
                run_handle.to_string(),
                font.to_string(),
                asset_extern_seam_text_hex(&text),
                result.to_string(),
            ],
        ) == Some(true)
        {
            result
        } else {
            0
        };
    }
    if let Some(host) = embedded_graphics_host() {
        return (host.replace_text)(run_handle, font, &text);
    }
    let Ok(text) = CString::new(text) else {
        return 0;
    };
    let Ok(api) = stasis_graphics_assets_api() else {
        return 0;
    };
    let Some(address) = api.stasis_gfx_replace_text else {
        return 0;
    };
    #[cfg(windows)]
    let callback: extern "system" fn(i32, i32, *const c_char) -> i32 =
        unsafe { std::mem::transmute(address) };
    #[cfg(not(windows))]
    let callback: extern "C" fn(i32, i32, *const c_char) -> i32 =
        unsafe { std::mem::transmute(address) };
    callback(run_handle, font, text.as_ptr())
}

#[no_mangle]
pub extern "C" fn stasis_jit_gfx_poll_reload(handle: i32) -> i32 {
    if asset_extern_seam_evidence_path().is_some() {
        let result = i32::from(handle == 101);
        return if record_asset_extern_seam_call(
            "poll_reload",
            &[handle.to_string(), result.to_string()],
        ) == Some(true)
        {
            result
        } else {
            0
        };
    }
    if let Some(host) = embedded_graphics_host() {
        return (host.poll_reload)(handle);
    }
    let Ok(api) = stasis_graphics_assets_api() else {
        return 0;
    };
    #[cfg(windows)]
    let callback: extern "system" fn(i32) -> i32 =
        unsafe { std::mem::transmute(api.stasis_gfx_poll_reload) };
    #[cfg(not(windows))]
    let callback: extern "C" fn(i32) -> i32 =
        unsafe { std::mem::transmute(api.stasis_gfx_poll_reload) };
    callback(handle)
}

#[no_mangle]
pub extern "C" fn stasis_jit_gfx_measure_text_cached(run_handle: i32) -> f32 {
    if asset_extern_seam_evidence_path().is_some() {
        return if record_asset_extern_seam_call(
            "measure_text_cached",
            &[run_handle.to_string(), 44.5_f32.to_bits().to_string()],
        ) == Some(true)
        {
            44.5
        } else {
            0.0
        };
    }
    if let Some(host) = embedded_graphics_host() {
        return (host.measure_text_cached)(run_handle);
    }
    let Ok(api) = stasis_graphics_assets_api() else {
        return 0.0;
    };
    #[cfg(windows)]
    let callback: extern "system" fn(i32) -> f32 =
        unsafe { std::mem::transmute(api.stasis_gfx_measure_text_cached) };
    #[cfg(not(windows))]
    let callback: extern "C" fn(i32) -> f32 =
        unsafe { std::mem::transmute(api.stasis_gfx_measure_text_cached) };
    callback(run_handle)
}

#[no_mangle]
pub extern "C" fn stasis_jit_gfx_measure_text_cached_height(run_handle: i32) -> f32 {
    if asset_extern_seam_evidence_path().is_some() {
        return if record_asset_extern_seam_call(
            "measure_text_cached_height",
            &[run_handle.to_string(), 12.25_f32.to_bits().to_string()],
        ) == Some(true)
        {
            12.25
        } else {
            0.0
        };
    }
    if let Some(host) = embedded_graphics_host() {
        return (host.measure_text_cached_height)(run_handle);
    }
    let Ok(api) = stasis_graphics_assets_api() else {
        return 0.0;
    };
    #[cfg(windows)]
    let callback: extern "system" fn(i32) -> f32 =
        unsafe { std::mem::transmute(api.stasis_gfx_measure_text_cached_height) };
    #[cfg(not(windows))]
    let callback: extern "C" fn(i32) -> f32 =
        unsafe { std::mem::transmute(api.stasis_gfx_measure_text_cached_height) };
    callback(run_handle)
}

fn struct_field_path_hash(base_hash: i32, suffix: &str) -> i32 {
    let mut hash = base_hash as u32;
    hash ^= u32::from(b'.');
    hash = hash.wrapping_mul(16_777_619);
    for byte in suffix.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(16_777_619);
    }
    hash as i32
}

fn struct_view_i32_load(base: i32, index: i32, suffix: &str) -> i32 {
    if index < 0 {
        stasis_jit_global_i32_load(struct_field_path_hash(base, suffix))
    } else {
        stasis_jit_global_i32_array_load(base, global_path_hash(suffix), index)
    }
}

fn struct_view_i32_store(base: i32, index: i32, len: i32, suffix: &str, value: i32) {
    if index < 0 {
        stasis_jit_global_i32_store(struct_field_path_hash(base, suffix), value);
    } else if index < len {
        stasis_jit_global_i32_array_store(base, global_path_hash(suffix), index, value);
    }
}

fn struct_view_f32_store(base: i32, index: i32, len: i32, suffix: &str, value: f32) {
    if index < 0 {
        stasis_jit_global_f32_store(struct_field_path_hash(base, suffix), value);
    } else if index < len {
        stasis_jit_global_f32_array_store(base, global_path_hash(suffix), index, value);
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_sprite_load_from(
    base: i32,
    index: i32,
    len: i32,
    path_id: i32,
    width: i32,
    height: i32,
) -> i32 {
    if width <= 0 || height <= 0 || (index >= 0 && index >= len) {
        return 0;
    }
    let loaded_handle = stasis_jit_gfx_load_sprite(path_id, width, height);
    if loaded_handle == 0 {
        return 0;
    }
    let old_handle = struct_view_i32_load(base, index, "handle");
    struct_view_i32_store(base, index, len, "handle", loaded_handle);
    struct_view_i32_store(base, index, len, "width", width);
    struct_view_i32_store(base, index, len, "height", height);
    if old_handle != 0 {
        stasis_jit_gfx_release_sprite(old_handle);
    }
    1
}

#[no_mangle]
pub extern "C" fn stasis_jit_text_run_load_from(
    base: i32,
    index: i32,
    len: i32,
    font: i32,
    text_id: i32,
) -> i32 {
    if font <= 0 || (index >= 0 && index >= len) {
        return 0;
    }
    let loaded_handle = stasis_jit_gfx_cache_text(font, text_id);
    if loaded_handle <= 0 {
        return 0;
    }
    struct_view_i32_store(base, index, len, "font", font);
    struct_view_i32_store(base, index, len, "handle", loaded_handle);
    struct_view_f32_store(
        base,
        index,
        len,
        "width",
        stasis_jit_gfx_measure_text_cached(loaded_handle),
    );
    struct_view_f32_store(
        base,
        index,
        len,
        "height",
        stasis_jit_gfx_measure_text_cached_height(loaded_handle),
    );
    1
}

#[no_mangle]
pub extern "C" fn stasis_jit_text_run_replace_from(
    base: i32,
    index: i32,
    len: i32,
    font: i32,
    text_id: i32,
) -> i32 {
    if font <= 0 || (index >= 0 && index >= len) {
        return 0;
    }
    let old_handle = struct_view_i32_load(base, index, "handle");
    let loaded_handle = replace_cached_text(old_handle, font, text_id);
    if loaded_handle <= 0 {
        return 0;
    }
    let width = stasis_jit_gfx_measure_text_cached(loaded_handle);
    let height = stasis_jit_gfx_measure_text_cached_height(loaded_handle);
    struct_view_i32_store(base, index, len, "font", font);
    struct_view_i32_store(base, index, len, "handle", loaded_handle);
    struct_view_f32_store(base, index, len, "width", width);
    struct_view_f32_store(base, index, len, "height", height);
    1
}

// AOT engine bundles may be linked and executed headlessly; keep this as a no-op so tests don't
// block on sleeps during deterministic quality-gate runs.
#[no_mangle]
pub extern "C" fn stasis_jit_sleep_ms(ms: i32) {
    let _ = ms;
}

// Runtime-compatible time APIs used by `extern function time()`/`time_us()` expansion.
fn recording_clock_us() -> Option<u64> {
    let fps = RECORDING_CLOCK_FPS.load(Ordering::Acquire);
    (fps != 0).then(|| {
        RECORDING_CLOCK_FRAME
            .load(Ordering::Acquire)
            .saturating_mul(1_000_000)
            / fps
    })
}

#[no_mangle]
pub extern "C" fn stasis_get_time_ms() -> i32 {
    if let Some(micros) = recording_clock_us() {
        return (micros / 1_000).min(i32::MAX as u64) as i32;
    }

    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(duration) => duration.as_millis() as i32,
        Err(_) => 0,
    }
}

#[no_mangle]
pub extern "C" fn stasis_get_time_us() -> i32 {
    if let Some(micros) = recording_clock_us() {
        return micros.min(i32::MAX as u64) as i32;
    }
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(duration) => duration.as_micros() as i32,
        Err(_) => 0,
    }
}

// Some stdlib externs use explicit `stasis_gfx_*` symbol names. Provide aliases so AOT bundles can
// link against the same shim layer used by the JIT runner.
#[no_mangle]
pub extern "C" fn stasis_gfx_cache_text(font: i32, text_id: i32) -> i32 {
    stasis_jit_gfx_cache_text(font, text_id)
}

#[no_mangle]
pub extern "C" fn stasis_gfx_measure_text_cached(run_handle: i32) -> f32 {
    stasis_jit_gfx_measure_text_cached(run_handle)
}

#[no_mangle]
pub extern "C" fn stasis_gfx_measure_text_cached_height(run_handle: i32) -> f32 {
    stasis_jit_gfx_measure_text_cached_height(run_handle)
}

#[no_mangle]
pub extern "C" fn stasis_gfx_poll_reload(handle: i32) -> i32 {
    stasis_jit_gfx_poll_reload(handle)
}

#[no_mangle]
pub extern "C" fn stasis_jit_sin_fast(value: f32) -> f32 {
    value.sin()
}

#[no_mangle]
pub extern "C" fn stasis_jit_cos_fast(value: f32) -> f32 {
    value.cos()
}

fn jit_dispatch_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[no_mangle]
pub extern "C" fn stasis_jit_global_i32_load(path_hash: i32) -> i32 {
    {
        let table = registered_i32_ptrs();
        let guard = table
            .lock()
            .expect("registered i32 ptr table mutex poisoned");
        if let Some(ptr) = guard.get(&path_hash).copied() {
            // Safety: caller owns lifetime; this is a process-global registration.
            return unsafe { *(ptr as *mut i32) };
        }
    }
    let table = jit_i32_global_table();
    let guard = table.lock().expect("jit global table mutex poisoned");
    guard.get(&path_hash).copied().unwrap_or_default()
}

#[no_mangle]
pub extern "C" fn stasis_jit_global_i32_store(path_hash: i32, value: i32) {
    {
        let table = registered_i32_ptrs();
        let guard = table
            .lock()
            .expect("registered i32 ptr table mutex poisoned");
        if let Some(ptr) = guard.get(&path_hash).copied() {
            // Safety: caller owns lifetime; this is a process-global registration.
            unsafe { *(ptr as *mut i32) = value };
            return;
        }
    }
    let table = jit_i32_global_table();
    let mut guard = table.lock().expect("jit global table mutex poisoned");
    guard.insert(path_hash, value);
}

fn fnv1a_extend_u32(mut hash: u32, suffix: &[u8]) -> u32 {
    for byte in suffix {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(16777619);
    }
    hash
}

fn jit_i32_slot_is_registered(path_hash: i32) -> bool {
    if registered_i32_ptrs()
        .lock()
        .expect("registered i32 ptr table mutex poisoned")
        .contains_key(&path_hash)
    {
        return true;
    }
    jit_i32_global_table()
        .lock()
        .expect("jit global table mutex poisoned")
        .contains_key(&path_hash)
}

fn jit_collection_runtime_metadata_is_registered(collection_hash: i32) -> bool {
    let max_length_hash = fnv1a_extend_u32(collection_hash as u32, b".max_length") as i32;
    jit_i32_slot_is_registered(max_length_hash)
}

fn stasis_meta_suffix_bytes(meta_kind: i32) -> Option<&'static [u8]> {
    // NOTE: These are intentionally hardcoded so Stasis can access collection header-like
    // fields (length/max_length/char_length) via a simple i32 handle (the base path hash)
    // without exposing negative indexing in Stasis source.
    match meta_kind {
        1 => Some(b".length"),
        2 => Some(b".max_length"),
        3 => Some(b".char_length"),
        _ => None,
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_collection_i32_load(collection_hash: i32, meta_kind: i32) -> i32 {
    let Some(suffix) = stasis_meta_suffix_bytes(meta_kind) else {
        return 0;
    };
    let derived = fnv1a_extend_u32(collection_hash as u32, suffix) as i32;
    if jit_i32_slot_is_registered(derived) {
        return stasis_jit_global_i32_load(derived);
    }
    let table = jit_string_literal_table();
    let guard = table
        .lock()
        .expect("jit string literal table mutex poisoned");
    let Some(text) = guard.get(&collection_hash) else {
        return 0;
    };
    match meta_kind {
        1 | 2 => i32::try_from(text.len()).unwrap_or(i32::MAX),
        3 => i32::try_from(text.chars().count()).unwrap_or(i32::MAX),
        _ => 0,
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_collection_i32_store(
    collection_hash: i32,
    meta_kind: i32,
    value: i32,
) {
    let Some(suffix) = stasis_meta_suffix_bytes(meta_kind) else {
        return;
    };
    let derived = fnv1a_extend_u32(collection_hash as u32, suffix) as i32;
    stasis_jit_global_i32_store(derived, value);
}

#[no_mangle]
pub extern "C" fn stasis_jit_global_f32_load(path_hash: i32) -> f32 {
    {
        let table = registered_f32_ptrs();
        let guard = table
            .lock()
            .expect("registered f32 ptr table mutex poisoned");
        if let Some(ptr) = guard.get(&path_hash).copied() {
            // Safety: caller owns lifetime; this is a process-global registration.
            return unsafe { *(ptr as *mut f32) };
        }
    }
    let table = jit_f32_global_table();
    let guard = table.lock().expect("jit global table mutex poisoned");
    guard.get(&path_hash).copied().unwrap_or_default()
}

#[no_mangle]
pub extern "C" fn stasis_jit_global_f32_store(path_hash: i32, value: f32) {
    {
        let table = registered_f32_ptrs();
        let guard = table
            .lock()
            .expect("registered f32 ptr table mutex poisoned");
        if let Some(ptr) = guard.get(&path_hash).copied() {
            // Safety: caller owns lifetime; this is a process-global registration.
            unsafe { *(ptr as *mut f32) = value };
            return;
        }
    }
    let table = jit_f32_global_table();
    let mut guard = table.lock().expect("jit global table mutex poisoned");
    guard.insert(path_hash, value);
}

#[no_mangle]
pub extern "C" fn stasis_jit_global_f64_load(path_hash: i32) -> f64 {
    {
        let table = registered_f64_ptrs();
        let guard = table
            .lock()
            .expect("registered f64 ptr table mutex poisoned");
        if let Some(ptr) = guard.get(&path_hash).copied() {
            // Safety: caller owns lifetime; this is a process-global registration.
            return unsafe { *(ptr as *mut f64) };
        }
    }
    let table = jit_f64_global_table();
    let guard = table.lock().expect("jit global table mutex poisoned");
    guard.get(&path_hash).copied().unwrap_or_default()
}

#[no_mangle]
pub extern "C" fn stasis_jit_global_f64_store(path_hash: i32, value: f64) {
    {
        let table = registered_f64_ptrs();
        let guard = table
            .lock()
            .expect("registered f64 ptr table mutex poisoned");
        if let Some(ptr) = guard.get(&path_hash).copied() {
            // Safety: caller owns lifetime; this is a process-global registration.
            unsafe { *(ptr as *mut f64) = value };
            return;
        }
    }
    let table = jit_f64_global_table();
    let mut guard = table.lock().expect("jit global table mutex poisoned");
    guard.insert(path_hash, value);
}

#[no_mangle]
pub extern "C" fn stasis_jit_global_i32_array_load(
    collection_hash: i32,
    field_hash: i32,
    index: i32,
) -> i32 {
    if index < 0 {
        return 0;
    }

    let idx = index as usize;
    {
        let table = registered_i32_arrays();
        let guard = table
            .lock()
            .expect("registered i32 array table mutex poisoned");
        if let Some((ptr, len)) = guard.get(&(collection_hash, field_hash)).copied() {
            if idx < len {
                // Safety: caller owns lifetime; this is a process-global registration.
                return unsafe { *((ptr as *mut i32).add(idx)) };
            }
            return 0;
        }
    }
    {
        let table = registered_u8_arrays();
        let guard = table
            .lock()
            .expect("registered u8 array table mutex poisoned");
        if let Some((ptr, len)) = guard.get(&(collection_hash, field_hash)).copied() {
            if idx < len {
                // Safety: caller owns lifetime; this is a process-global registration.
                return i32::from(unsafe { *((ptr as *mut u8).add(idx)) });
            }
            return 0;
        }
    }
    {
        let table = registered_u16_arrays();
        let guard = table
            .lock()
            .expect("registered u16 array table mutex poisoned");
        if let Some((ptr, len)) = guard.get(&(collection_hash, field_hash)).copied() {
            if idx < len {
                return i32::from(unsafe { *((ptr as *mut u16).add(idx)) });
            }
            return 0;
        }
    }
    let (runtime_value, runtime_array_exists) = {
        let table = jit_i32_array_global_table();
        let guard = table.lock().expect("jit global table mutex poisoned");
        (
            guard.get(&(collection_hash, field_hash, index)).copied(),
            guard.keys().any(|(collection, field, _)| {
                *collection == collection_hash && *field == field_hash
            }),
        )
    };
    if let Some(value) = runtime_value {
        return value;
    }
    if runtime_array_exists {
        return 0;
    }

    // A string literal is represented by its hash while lowering text-view
    // indexing. Registered/dynamic runtime arrays above must win when they
    // use the same hash; only an unbound byte view can fall back to the
    // literal table.
    if field_hash != 0 {
        return 0;
    }
    let table = jit_string_literal_table();
    let guard = table
        .lock()
        .expect("jit string literal table mutex poisoned");
    guard
        .get(&collection_hash)
        .and_then(|text| text.as_bytes().get(idx))
        .map(|byte| i32::from(*byte))
        .unwrap_or_default()
}

#[no_mangle]
pub extern "C" fn stasis_jit_global_i32_array_store(
    collection_hash: i32,
    field_hash: i32,
    index: i32,
    value: i32,
) {
    if index < 0 {
        return;
    }

    let idx = index as usize;
    {
        let table = registered_i32_arrays();
        let guard = table
            .lock()
            .expect("registered i32 array table mutex poisoned");
        if let Some((ptr, len)) = guard.get(&(collection_hash, field_hash)).copied() {
            if idx < len {
                // Safety: caller owns lifetime; this is a process-global registration.
                unsafe { *((ptr as *mut i32).add(idx)) = value };
            }
            return;
        }
    }
    {
        let table = registered_u8_arrays();
        let guard = table
            .lock()
            .expect("registered u8 array table mutex poisoned");
        if let Some((ptr, len)) = guard.get(&(collection_hash, field_hash)).copied() {
            if idx < len {
                // Safety: caller owns lifetime; this is a process-global registration.
                unsafe { *((ptr as *mut u8).add(idx)) = value as u8 };
            }
            return;
        }
    }
    {
        let table = registered_u16_arrays();
        let guard = table
            .lock()
            .expect("registered u16 array table mutex poisoned");
        if let Some((ptr, len)) = guard.get(&(collection_hash, field_hash)).copied() {
            if idx < len {
                unsafe { *((ptr as *mut u16).add(idx)) = value as u16 };
            }
            return;
        }
    }
    let table = jit_i32_array_global_table();
    let mut guard = table.lock().expect("jit global table mutex poisoned");
    guard.insert((collection_hash, field_hash, index), value);
}

#[no_mangle]
pub extern "C" fn stasis_jit_global_f32_array_load(
    collection_hash: i32,
    field_hash: i32,
    index: i32,
) -> f32 {
    if index < 0 {
        return 0.0;
    }

    let idx = index as usize;
    {
        let table = registered_f32_arrays();
        let guard = table
            .lock()
            .expect("registered f32 array table mutex poisoned");
        if let Some((ptr, len)) = guard.get(&(collection_hash, field_hash)).copied() {
            if idx < len {
                // Safety: caller owns lifetime; this is a process-global registration.
                return unsafe { *((ptr as *mut f32).add(idx)) };
            }
            return 0.0;
        }
    }
    let table = jit_f32_array_global_table();
    let guard = table.lock().expect("jit global table mutex poisoned");
    guard
        .get(&(collection_hash, field_hash, index))
        .copied()
        .unwrap_or_default()
}

#[no_mangle]
pub extern "C" fn stasis_jit_global_f32_array_store(
    collection_hash: i32,
    field_hash: i32,
    index: i32,
    value: f32,
) {
    if index < 0 {
        return;
    }

    let idx = index as usize;
    {
        let table = registered_f32_arrays();
        let guard = table
            .lock()
            .expect("registered f32 array table mutex poisoned");
        if let Some((ptr, len)) = guard.get(&(collection_hash, field_hash)).copied() {
            if idx < len {
                // Safety: caller owns lifetime; this is a process-global registration.
                unsafe { *((ptr as *mut f32).add(idx)) = value };
            }
            return;
        }
    }
    let table = jit_f32_array_global_table();
    let mut guard = table.lock().expect("jit global table mutex poisoned");
    guard.insert((collection_hash, field_hash, index), value);
}

#[no_mangle]
pub extern "C" fn stasis_jit_global_f64_array_load(
    collection_hash: i32,
    field_hash: i32,
    index: i32,
) -> f64 {
    if index < 0 {
        return 0.0;
    }

    let idx = index as usize;
    {
        let table = registered_f64_arrays();
        let guard = table
            .lock()
            .expect("registered f64 array table mutex poisoned");
        if let Some((ptr, len)) = guard.get(&(collection_hash, field_hash)).copied() {
            if idx < len {
                // Safety: caller owns lifetime; this is a process-global registration.
                return unsafe { *((ptr as *mut f64).add(idx)) };
            }
            return 0.0;
        }
    }
    let table = jit_f64_array_global_table();
    let guard = table.lock().expect("jit global table mutex poisoned");
    guard
        .get(&(collection_hash, field_hash, index))
        .copied()
        .unwrap_or_default()
}

#[no_mangle]
pub extern "C" fn stasis_jit_global_f64_array_store(
    collection_hash: i32,
    field_hash: i32,
    index: i32,
    value: f64,
) {
    if index < 0 {
        return;
    }

    let idx = index as usize;
    {
        let table = registered_f64_arrays();
        let guard = table
            .lock()
            .expect("registered f64 array table mutex poisoned");
        if let Some((ptr, len)) = guard.get(&(collection_hash, field_hash)).copied() {
            if idx < len {
                // Safety: caller owns lifetime; this is a process-global registration.
                unsafe { *((ptr as *mut f64).add(idx)) = value };
            }
            return;
        }
    }
    let table = jit_f64_array_global_table();
    let mut guard = table.lock().expect("jit global table mutex poisoned");
    guard.insert((collection_hash, field_hash, index), value);
}

pub fn clear_jit_i32_global_table() {
    let table = jit_i32_global_table();
    let mut guard = table.lock().expect("jit global table mutex poisoned");
    guard.clear();
    for value in owned_i32_scalars()
        .lock()
        .expect("owned i32 scalar table mutex poisoned")
        .values_mut()
    {
        **value = 0;
    }
}

pub fn clear_jit_f32_global_table() {
    let table = jit_f32_global_table();
    let mut guard = table.lock().expect("jit global table mutex poisoned");
    guard.clear();
    for value in owned_f32_scalars()
        .lock()
        .expect("owned f32 scalar table mutex poisoned")
        .values_mut()
    {
        **value = 0.0;
    }
}

pub fn clear_jit_f64_global_table() {
    let table = jit_f64_global_table();
    let mut guard = table.lock().expect("jit global table mutex poisoned");
    guard.clear();
    for value in owned_f64_scalars()
        .lock()
        .expect("owned f64 scalar table mutex poisoned")
        .values_mut()
    {
        **value = 0.0;
    }
}

pub fn clear_jit_i32_array_global_table() {
    let table = jit_i32_array_global_table();
    let mut guard = table.lock().expect("jit global table mutex poisoned");
    guard.clear();
    for values in owned_i32_arrays()
        .lock()
        .expect("owned i32 array table mutex poisoned")
        .values_mut()
    {
        values.fill(0);
    }
}

pub fn clear_jit_f32_array_global_table() {
    let table = jit_f32_array_global_table();
    let mut guard = table.lock().expect("jit global table mutex poisoned");
    guard.clear();
    for values in owned_f32_arrays()
        .lock()
        .expect("owned f32 array table mutex poisoned")
        .values_mut()
    {
        values.fill(0.0);
    }
}

pub fn clear_jit_f64_array_global_table() {
    let table = jit_f64_array_global_table();
    let mut guard = table.lock().expect("jit global table mutex poisoned");
    guard.clear();
    for values in owned_f64_arrays()
        .lock()
        .expect("owned f64 array table mutex poisoned")
        .values_mut()
    {
        values.fill(0.0);
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_sys_memcpy_u8(
    dst: i32,
    dst_index: i32,
    src: i32,
    src_index: i32,
    count: i32,
) {
    if count <= 0 {
        return;
    }
    let literal_bytes = if jit_text_buffer_is_registered(src) {
        None
    } else {
        jit_string_literal_table()
            .lock()
            .expect("jit string literal table mutex poisoned")
            .get(&src)
            .map(|text| text.as_bytes().to_vec())
    };
    if let Some(bytes) = literal_bytes {
        for offset in 0..count {
            let source_index = src_index.saturating_add(offset);
            let value = usize::try_from(source_index)
                .ok()
                .and_then(|index| bytes.get(index))
                .copied()
                .unwrap_or_default();
            let index = dst_index.saturating_add(offset);
            stasis_jit_global_i32_array_store(dst, 0, index, i32::from(value));
        }
        return;
    }
    let mut values: Vec<i32> = Vec::with_capacity(count as usize);
    for offset in 0..count {
        let index = src_index.saturating_add(offset);
        values.push(stasis_jit_global_i32_array_load(src, 0, index));
    }
    for (offset, value) in values.into_iter().enumerate() {
        let index = dst_index.saturating_add(offset as i32);
        stasis_jit_global_i32_array_store(dst, 0, index, value);
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_sys_memcpy_i32(
    dst: i32,
    dst_index: i32,
    src: i32,
    src_index: i32,
    count: i32,
) {
    if count <= 0 {
        return;
    }
    let mut values: Vec<i32> = Vec::with_capacity(count as usize);
    for offset in 0..count {
        let index = src_index.saturating_add(offset);
        values.push(stasis_jit_global_i32_array_load(src, 0, index));
    }
    for (offset, value) in values.into_iter().enumerate() {
        let index = dst_index.saturating_add(offset as i32);
        stasis_jit_global_i32_array_store(dst, 0, index, value);
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_sys_memcpy_f32(
    dst: i32,
    dst_index: i32,
    src: i32,
    src_index: i32,
    count: i32,
) {
    if count <= 0 {
        return;
    }
    let mut values: Vec<f32> = Vec::with_capacity(count as usize);
    for offset in 0..count {
        let index = src_index.saturating_add(offset);
        values.push(stasis_jit_global_f32_array_load(src, 0, index));
    }
    for (offset, value) in values.into_iter().enumerate() {
        let index = dst_index.saturating_add(offset as i32);
        stasis_jit_global_f32_array_store(dst, 0, index, value);
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_sys_memmove_u8(
    dst: i32,
    dst_index: i32,
    src: i32,
    src_index: i32,
    count: i32,
) {
    stasis_jit_sys_memcpy_u8(dst, dst_index, src, src_index, count);
}

#[no_mangle]
pub extern "C" fn stasis_jit_sys_memmove_i32(
    dst: i32,
    dst_index: i32,
    src: i32,
    src_index: i32,
    count: i32,
) {
    stasis_jit_sys_memcpy_i32(dst, dst_index, src, src_index, count);
}

#[no_mangle]
pub extern "C" fn stasis_jit_sys_memmove_f32(
    dst: i32,
    dst_index: i32,
    src: i32,
    src_index: i32,
    count: i32,
) {
    stasis_jit_sys_memcpy_f32(dst, dst_index, src, src_index, count);
}

// ============================================================
// Audio host API (JIT extern call bridge)
// ============================================================

#[repr(C)]
#[derive(Clone, Copy)]
pub struct StasisAudioHostApi {
    pub init: Option<extern "C" fn(i32, i32, i32) -> i32>,
    pub shutdown: Option<extern "C" fn()>,
    pub is_available: Option<extern "C" fn() -> i32>,
    pub get_sample_rate: Option<extern "C" fn() -> i32>,
    pub get_channels: Option<extern "C" fn() -> i32>,
    pub get_queued_frames: Option<extern "C" fn() -> i32>,
    pub get_underruns: Option<extern "C" fn() -> i32>,
    pub push_f32_interleaved: Option<extern "C" fn(*const f32, i32) -> i32>,
    pub load_wav: Option<extern "C" fn(*const c_char) -> i32>,
    pub release: Option<extern "C" fn(i32)>,
    pub play: Option<extern "C" fn(i32, i32, f32, f32) -> i32>,
    pub stop: Option<extern "C" fn(i32)>,
    pub voice_is_playing: Option<extern "C" fn(i32) -> i32>,
    pub voice_set_paused: Option<extern "C" fn(i32, i32)>,
    pub voice_set_volume_pan: Option<extern "C" fn(i32, f32, f32)>,
    pub load_music: Option<extern "C" fn(*const c_char) -> i32>,
    pub load_effect: Option<extern "C" fn(*const c_char) -> i32>,
    pub play_music: Option<extern "C" fn(i32, i32, f32) -> i32>,
    pub stop_music: Option<extern "C" fn(i32)>,
    pub pause_music: Option<extern "C" fn(i32, i32)>,
    pub set_music_volume: Option<extern "C" fn(i32, f32)>,
    pub play_effect: Option<extern "C" fn(i32, f32) -> i32>,
}

static AUDIO_HOST_API: OnceLock<Mutex<Option<StasisAudioHostApi>>> = OnceLock::new();

fn current_audio_host_api() -> Option<StasisAudioHostApi> {
    *AUDIO_HOST_API
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("audio host API mutex poisoned")
}

pub fn install_audio_host_api(api: Option<StasisAudioHostApi>) {
    *AUDIO_HOST_API
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("audio host API mutex poisoned") = api;
}

fn invoke_audio_host_path(
    path_id: i32,
    callback: Option<extern "C" fn(*const c_char) -> i32>,
) -> Option<i32> {
    let callback = callback?;
    let Ok(path) = jit_text_arg_to_cstring(path_id) else {
        return Some(0);
    };
    Some(callback(path.as_ptr()))
}

#[no_mangle]
pub extern "C" fn stasis_jit_audio_init(
    sample_rate: i32,
    channels: i32,
    target_latency_frames: i32,
) -> i32 {
    if let Some(init) = current_audio_host_api().and_then(|api| api.init) {
        return init(sample_rate, channels, target_latency_frames);
    }
    let Ok(api) = stasis_graphics_assets_api() else {
        return 0;
    };
    let Some(address) = api.stasis_audio_init else {
        return 0;
    };
    #[cfg(windows)]
    let callback: extern "system" fn(i32, i32, i32) -> i32 =
        unsafe { std::mem::transmute(address) };
    #[cfg(not(windows))]
    let callback: extern "C" fn(i32, i32, i32) -> i32 = unsafe { std::mem::transmute(address) };
    callback(sample_rate, channels, target_latency_frames)
}

#[no_mangle]
pub extern "C" fn stasis_jit_audio_shutdown() {
    if let Some(shutdown) = current_audio_host_api().and_then(|api| api.shutdown) {
        shutdown();
        return;
    }
    let Ok(api) = stasis_graphics_assets_api() else {
        return;
    };
    let Some(address) = api.stasis_audio_shutdown else {
        return;
    };
    #[cfg(windows)]
    let callback: extern "system" fn() = unsafe { std::mem::transmute(address) };
    #[cfg(not(windows))]
    let callback: extern "C" fn() = unsafe { std::mem::transmute(address) };
    callback();
}

#[no_mangle]
pub extern "C" fn stasis_jit_audio_is_available() -> i32 {
    if let Some(available) = current_audio_host_api().and_then(|api| api.is_available) {
        return available();
    }
    let Ok(api) = stasis_graphics_assets_api() else {
        return 0;
    };
    let Some(address) = api.stasis_audio_is_available else {
        return 0;
    };
    #[cfg(windows)]
    let callback: extern "system" fn() -> i32 = unsafe { std::mem::transmute(address) };
    #[cfg(not(windows))]
    let callback: extern "C" fn() -> i32 = unsafe { std::mem::transmute(address) };
    callback()
}

#[no_mangle]
pub extern "C" fn stasis_jit_audio_get_sample_rate() -> i32 {
    if let Some(get) = current_audio_host_api().and_then(|api| api.get_sample_rate) {
        return get();
    }
    let Ok(api) = stasis_graphics_assets_api() else {
        return 0;
    };
    let Some(address) = api.stasis_audio_get_sample_rate else {
        return 0;
    };
    #[cfg(windows)]
    let callback: extern "system" fn() -> i32 = unsafe { std::mem::transmute(address) };
    #[cfg(not(windows))]
    let callback: extern "C" fn() -> i32 = unsafe { std::mem::transmute(address) };
    callback()
}

#[no_mangle]
pub extern "C" fn stasis_jit_audio_get_channels() -> i32 {
    if let Some(get) = current_audio_host_api().and_then(|api| api.get_channels) {
        return get();
    }
    let Ok(api) = stasis_graphics_assets_api() else {
        return 0;
    };
    let Some(address) = api.stasis_audio_get_channels else {
        return 0;
    };
    #[cfg(windows)]
    let callback: extern "system" fn() -> i32 = unsafe { std::mem::transmute(address) };
    #[cfg(not(windows))]
    let callback: extern "C" fn() -> i32 = unsafe { std::mem::transmute(address) };
    callback()
}

#[no_mangle]
pub extern "C" fn stasis_jit_audio_get_queued_frames() -> i32 {
    if let Some(get) = current_audio_host_api().and_then(|api| api.get_queued_frames) {
        return get();
    }
    let Ok(api) = stasis_graphics_assets_api() else {
        return 0;
    };
    let Some(address) = api.stasis_audio_get_queued_frames else {
        return 0;
    };
    #[cfg(windows)]
    let callback: extern "system" fn() -> i32 = unsafe { std::mem::transmute(address) };
    #[cfg(not(windows))]
    let callback: extern "C" fn() -> i32 = unsafe { std::mem::transmute(address) };
    callback()
}

#[no_mangle]
pub extern "C" fn stasis_jit_audio_get_underruns() -> i32 {
    if let Some(get) = current_audio_host_api().and_then(|api| api.get_underruns) {
        return get();
    }
    let Ok(api) = stasis_graphics_assets_api() else {
        return 0;
    };
    let Some(address) = api.stasis_audio_get_underruns else {
        return 0;
    };
    #[cfg(windows)]
    let callback: extern "system" fn() -> i32 = unsafe { std::mem::transmute(address) };
    #[cfg(not(windows))]
    let callback: extern "C" fn() -> i32 = unsafe { std::mem::transmute(address) };
    callback()
}

fn with_jit_audio_f32_interleaved(
    samples: i32,
    frame_count: i32,
    channels: i32,
    push: impl FnOnce(*const f32, i32) -> i32,
) -> i32 {
    if frame_count <= 0 || channels <= 0 {
        return 0;
    }
    let Some(sample_count) = frame_count.checked_mul(channels) else {
        return 0;
    };
    let values = stasis_jit_global_f32_array_ptr(samples, 0, sample_count);
    if values.is_null() {
        return 0;
    }
    push(values.cast_const(), frame_count)
}

#[no_mangle]
pub extern "C" fn stasis_jit_audio_push_f32_interleaved(samples: i32, frame_count: i32) -> i32 {
    if let Some(host) = current_audio_host_api() {
        let Some(push) = host.push_f32_interleaved else {
            return 0;
        };
        let channels = host.get_channels.map(|get| get()).unwrap_or(0);
        return with_jit_audio_f32_interleaved(samples, frame_count, channels, |values, frames| {
            push(values, frames)
        });
    }
    let Ok(api) = stasis_graphics_assets_api() else {
        return 0;
    };
    let Some(address) = api.stasis_audio_push_f32_interleaved else {
        return 0;
    };
    let channels = stasis_jit_audio_get_channels();
    #[cfg(windows)]
    let callback: extern "system" fn(*const f32, i32) -> i32 =
        unsafe { std::mem::transmute(address) };
    #[cfg(not(windows))]
    let callback: extern "C" fn(*const f32, i32) -> i32 = unsafe { std::mem::transmute(address) };
    with_jit_audio_f32_interleaved(samples, frame_count, channels, |values, frames| {
        callback(values, frames)
    })
}

#[no_mangle]
pub extern "C" fn stasis_jit_audio_load_wav(path_id: i32) -> i32 {
    if let Some(result) = invoke_audio_host_path(
        path_id,
        current_audio_host_api().and_then(|api| api.load_wav),
    ) {
        return result;
    }
    let Ok(path) = jit_text_arg_to_cstring(path_id) else {
        return 0;
    };
    let Ok(api) = stasis_graphics_assets_api() else {
        return 0;
    };
    let Some(address) = api.stasis_audio_load_wav else {
        return 0;
    };
    #[cfg(windows)]
    let callback: extern "system" fn(*const c_char) -> i32 =
        unsafe { std::mem::transmute(address) };
    #[cfg(not(windows))]
    let callback: extern "C" fn(*const c_char) -> i32 = unsafe { std::mem::transmute(address) };
    callback(path.as_ptr())
}

#[no_mangle]
pub extern "C" fn stasis_jit_audio_release(asset_handle: i32) {
    if let Some(release) = current_audio_host_api().and_then(|api| api.release) {
        release(asset_handle);
        return;
    }
    let Ok(api) = stasis_graphics_assets_api() else {
        return;
    };
    let Some(address) = api.stasis_audio_release else {
        return;
    };
    #[cfg(windows)]
    let callback: extern "system" fn(i32) = unsafe { std::mem::transmute(address) };
    #[cfg(not(windows))]
    let callback: extern "C" fn(i32) = unsafe { std::mem::transmute(address) };
    callback(asset_handle);
}

#[no_mangle]
pub extern "C" fn stasis_jit_audio_play(
    asset_handle: i32,
    looped: i32,
    volume: f32,
    pan: f32,
) -> i32 {
    if let Some(play) = current_audio_host_api().and_then(|api| api.play) {
        return play(asset_handle, looped, volume, pan);
    }
    let Ok(api) = stasis_graphics_assets_api() else {
        return 0;
    };
    let Some(address) = api.stasis_audio_play else {
        return 0;
    };
    #[cfg(windows)]
    let callback: extern "system" fn(i32, i32, f32, f32) -> i32 =
        unsafe { std::mem::transmute(address) };
    #[cfg(not(windows))]
    let callback: extern "C" fn(i32, i32, f32, f32) -> i32 =
        unsafe { std::mem::transmute(address) };
    callback(asset_handle, looped, volume, pan)
}

#[no_mangle]
pub extern "C" fn stasis_jit_audio_stop(voice_handle: i32) {
    if let Some(stop) = current_audio_host_api().and_then(|api| api.stop) {
        stop(voice_handle);
        return;
    }
    let Ok(api) = stasis_graphics_assets_api() else {
        return;
    };
    let Some(address) = api.stasis_audio_stop else {
        return;
    };
    #[cfg(windows)]
    let callback: extern "system" fn(i32) = unsafe { std::mem::transmute(address) };
    #[cfg(not(windows))]
    let callback: extern "C" fn(i32) = unsafe { std::mem::transmute(address) };
    callback(voice_handle);
}

#[no_mangle]
pub extern "C" fn stasis_jit_audio_voice_is_playing(voice_handle: i32) -> i32 {
    if let Some(is_playing) = current_audio_host_api().and_then(|api| api.voice_is_playing) {
        return is_playing(voice_handle);
    }
    let Ok(api) = stasis_graphics_assets_api() else {
        return 0;
    };
    let Some(address) = api.stasis_audio_voice_is_playing else {
        return 0;
    };
    #[cfg(windows)]
    let callback: extern "system" fn(i32) -> i32 = unsafe { std::mem::transmute(address) };
    #[cfg(not(windows))]
    let callback: extern "C" fn(i32) -> i32 = unsafe { std::mem::transmute(address) };
    callback(voice_handle)
}

#[no_mangle]
pub extern "C" fn stasis_jit_audio_voice_set_paused(voice_handle: i32, paused: i32) {
    if let Some(set_paused) = current_audio_host_api().and_then(|api| api.voice_set_paused) {
        set_paused(voice_handle, paused);
        return;
    }
    let Ok(api) = stasis_graphics_assets_api() else {
        return;
    };
    let Some(address) = api.stasis_audio_voice_set_paused else {
        return;
    };
    #[cfg(windows)]
    let callback: extern "system" fn(i32, i32) = unsafe { std::mem::transmute(address) };
    #[cfg(not(windows))]
    let callback: extern "C" fn(i32, i32) = unsafe { std::mem::transmute(address) };
    callback(voice_handle, paused);
}

#[no_mangle]
pub extern "C" fn stasis_jit_audio_voice_set_volume_pan(voice_handle: i32, volume: f32, pan: f32) {
    if let Some(set_volume_pan) = current_audio_host_api().and_then(|api| api.voice_set_volume_pan)
    {
        set_volume_pan(voice_handle, volume, pan);
        return;
    }
    let Ok(api) = stasis_graphics_assets_api() else {
        return;
    };
    let Some(address) = api.stasis_audio_voice_set_volume_pan else {
        return;
    };
    #[cfg(windows)]
    let callback: extern "system" fn(i32, f32, f32) = unsafe { std::mem::transmute(address) };
    #[cfg(not(windows))]
    let callback: extern "C" fn(i32, f32, f32) = unsafe { std::mem::transmute(address) };
    callback(voice_handle, volume, pan);
}

#[no_mangle]
pub extern "C" fn stasis_jit_audio_load_music(path_id: i32) -> i32 {
    if let Some(result) = invoke_audio_host_path(
        path_id,
        current_audio_host_api().and_then(|api| api.load_music),
    ) {
        return result;
    }
    let Ok(path) = jit_text_arg_to_cstring(path_id) else {
        return 0;
    };
    let Ok(api) = stasis_graphics_assets_api() else {
        return 0;
    };
    let Some(address) = api.stasis_audio_load_music else {
        return 0;
    };
    #[cfg(windows)]
    let callback: extern "system" fn(*const c_char) -> i32 =
        unsafe { std::mem::transmute(address) };
    #[cfg(not(windows))]
    let callback: extern "C" fn(*const c_char) -> i32 = unsafe { std::mem::transmute(address) };
    callback(path.as_ptr())
}

#[no_mangle]
pub extern "C" fn stasis_jit_audio_load_effect(path_id: i32) -> i32 {
    if let Some(result) = invoke_audio_host_path(
        path_id,
        current_audio_host_api().and_then(|api| api.load_effect),
    ) {
        return result;
    }
    let Ok(path) = jit_text_arg_to_cstring(path_id) else {
        return 0;
    };
    let Ok(api) = stasis_graphics_assets_api() else {
        return 0;
    };
    let Some(address) = api.stasis_audio_load_effect else {
        return 0;
    };
    #[cfg(windows)]
    let callback: extern "system" fn(*const c_char) -> i32 =
        unsafe { std::mem::transmute(address) };
    #[cfg(not(windows))]
    let callback: extern "C" fn(*const c_char) -> i32 = unsafe { std::mem::transmute(address) };
    callback(path.as_ptr())
}

#[no_mangle]
pub extern "C" fn stasis_jit_audio_play_music(asset_handle: i32, looped: i32, volume: f32) -> i32 {
    if let Some(play_music) = current_audio_host_api().and_then(|api| api.play_music) {
        return play_music(asset_handle, looped, volume);
    }
    let Ok(api) = stasis_graphics_assets_api() else {
        return 0;
    };
    let Some(address) = api.stasis_audio_play_music else {
        return 0;
    };
    #[cfg(windows)]
    let callback: extern "system" fn(i32, i32, f32) -> i32 =
        unsafe { std::mem::transmute(address) };
    #[cfg(not(windows))]
    let callback: extern "C" fn(i32, i32, f32) -> i32 = unsafe { std::mem::transmute(address) };
    callback(asset_handle, looped, volume)
}

#[no_mangle]
pub extern "C" fn stasis_jit_audio_stop_music(asset_handle: i32) {
    if let Some(stop_music) = current_audio_host_api().and_then(|api| api.stop_music) {
        stop_music(asset_handle);
        return;
    }
    let Ok(api) = stasis_graphics_assets_api() else {
        return;
    };
    let Some(address) = api.stasis_audio_stop_music else {
        return;
    };
    #[cfg(windows)]
    let callback: extern "system" fn(i32) = unsafe { std::mem::transmute(address) };
    #[cfg(not(windows))]
    let callback: extern "C" fn(i32) = unsafe { std::mem::transmute(address) };
    callback(asset_handle);
}

#[no_mangle]
pub extern "C" fn stasis_jit_audio_pause_music(asset_handle: i32, paused: i32) {
    if let Some(pause_music) = current_audio_host_api().and_then(|api| api.pause_music) {
        pause_music(asset_handle, paused);
        return;
    }
    let Ok(api) = stasis_graphics_assets_api() else {
        return;
    };
    let Some(address) = api.stasis_audio_pause_music else {
        return;
    };
    #[cfg(windows)]
    let callback: extern "system" fn(i32, i32) = unsafe { std::mem::transmute(address) };
    #[cfg(not(windows))]
    let callback: extern "C" fn(i32, i32) = unsafe { std::mem::transmute(address) };
    callback(asset_handle, paused);
}

#[no_mangle]
pub extern "C" fn stasis_jit_audio_set_music_volume(asset_handle: i32, volume: f32) {
    if let Some(set_music_volume) = current_audio_host_api().and_then(|api| api.set_music_volume) {
        set_music_volume(asset_handle, volume);
        return;
    }
    let Ok(api) = stasis_graphics_assets_api() else {
        return;
    };
    let Some(address) = api.stasis_audio_set_music_volume else {
        return;
    };
    #[cfg(windows)]
    let callback: extern "system" fn(i32, f32) = unsafe { std::mem::transmute(address) };
    #[cfg(not(windows))]
    let callback: extern "C" fn(i32, f32) = unsafe { std::mem::transmute(address) };
    callback(asset_handle, volume);
}

#[no_mangle]
pub extern "C" fn stasis_jit_audio_play_effect(asset_handle: i32, volume: f32) -> i32 {
    if let Some(play_effect) = current_audio_host_api().and_then(|api| api.play_effect) {
        return play_effect(asset_handle, volume);
    }
    let Ok(api) = stasis_graphics_assets_api() else {
        return 0;
    };
    let Some(address) = api.stasis_audio_play_effect else {
        return 0;
    };
    #[cfg(windows)]
    let callback: extern "system" fn(i32, f32) -> i32 = unsafe { std::mem::transmute(address) };
    #[cfg(not(windows))]
    let callback: extern "C" fn(i32, f32) -> i32 = unsafe { std::mem::transmute(address) };
    callback(asset_handle, volume)
}

type JitI32GlobalMap = std::collections::HashMap<i32, i32>;
type JitF32GlobalMap = std::collections::HashMap<i32, f32>;
type JitF64GlobalMap = std::collections::HashMap<i32, f64>;
type JitI32ArrayGlobalMap = std::collections::HashMap<(i32, i32, i32), i32>;
type JitF32ArrayGlobalMap = std::collections::HashMap<(i32, i32, i32), f32>;
type JitF64ArrayGlobalMap = std::collections::HashMap<(i32, i32, i32), f64>;
type JitStringLiteralMap = std::collections::HashMap<i32, String>;

#[derive(Default)]
struct JitStringLiteralStage {
    literals: JitStringLiteralMap,
    collision: Option<String>,
}

thread_local! {
    static JIT_STRING_LITERAL_STAGE: RefCell<Option<JitStringLiteralStage>> = const { RefCell::new(None) };
}

fn jit_i32_global_table() -> &'static Mutex<JitI32GlobalMap> {
    static TABLE: OnceLock<Mutex<JitI32GlobalMap>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

fn jit_f32_global_table() -> &'static Mutex<JitF32GlobalMap> {
    static TABLE: OnceLock<Mutex<JitF32GlobalMap>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

fn jit_f64_global_table() -> &'static Mutex<JitF64GlobalMap> {
    static TABLE: OnceLock<Mutex<JitF64GlobalMap>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

fn jit_i32_array_global_table() -> &'static Mutex<JitI32ArrayGlobalMap> {
    static TABLE: OnceLock<Mutex<JitI32ArrayGlobalMap>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

fn jit_f32_array_global_table() -> &'static Mutex<JitF32ArrayGlobalMap> {
    static TABLE: OnceLock<Mutex<JitF32ArrayGlobalMap>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

fn jit_f64_array_global_table() -> &'static Mutex<JitF64ArrayGlobalMap> {
    static TABLE: OnceLock<Mutex<JitF64ArrayGlobalMap>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

fn jit_string_literal_table() -> &'static Mutex<JitStringLiteralMap> {
    static TABLE: OnceLock<Mutex<JitStringLiteralMap>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::MutexGuard;

    fn hot_image(path: &str, count: Option<u64>, eligible: bool) -> HotRenderRuntimeImage {
        HotRenderRuntimeImage {
            logical_path: path.to_string(),
            logical_width: 32,
            logical_height: 24,
            max_renders_per_render: count,
            atlas_eligible: eligible,
            grouping_key: "batch-v3:test".to_string(),
            estimated_distinct_transitions: 8,
            group_member_count: 2,
            group_logical_pixel_area: 131_072,
            group_max_logical_width: 32,
            group_max_logical_height: 24,
            backend_constraints: "desktop-gl".to_string(),
        }
    }

    #[test]
    fn hot_render_policy_falls_back_for_missing_stale_and_cold_metadata() {
        let _guard = test_lock();
        replace_hot_render_metadata(
            HOT_RENDER_METADATA_VERSION,
            &[
                hot_image("assets/hot.png", Some(2), true),
                hot_image("assets/cold.png", Some(1), false),
                hot_image("assets/unknown.png", None, true),
            ],
        );
        assert!(hot_render_atlas_eligible("assets\\hot.png", 32, 24));
        let policy = hot_render_atlas_policy("assets/hot.png", 32, 24);
        assert!(policy.eligible);
        assert_ne!(policy.group_id, 0);
        assert_eq!(policy.member_count, 2);
        assert_eq!(policy.logical_pixel_area, 131_072);
        assert_eq!(
            (policy.max_logical_width, policy.max_logical_height),
            (32, 24)
        );
        assert!(!hot_render_atlas_eligible("assets/cold.png", 32, 24));
        assert!(!hot_render_atlas_eligible("assets/unknown.png", 32, 24));
        assert!(!hot_render_atlas_eligible("assets/missing.png", 32, 24));
        assert!(!hot_render_atlas_eligible("assets/hot.png", 33, 24));
        replace_hot_render_metadata(
            HOT_RENDER_METADATA_VERSION + 1,
            &[hot_image("assets/hot.png", Some(2), true)],
        );
        assert!(!hot_render_atlas_eligible("assets/hot.png", 32, 24));
    }

    #[test]
    fn hot_render_groups_are_deterministic_by_compiler_group_identity() {
        let images = vec![
            hot_image("z.png", Some(4), true),
            hot_image("a.png", Some(2), true),
            hot_image("cold.png", Some(1), false),
        ];
        let groups = plan_hot_render_groups(&images);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].1[0].0, "a.png");
        assert_eq!(groups[0].1[1].0, "z.png");
    }

    #[test]
    fn realized_hot_render_planner_allows_mixed_sizes_and_uses_limit_fallback() {
        let realized = vec![
            RealizedHotRenderImage {
                image: hot_image("b-2k.png", Some(8), true),
                realized_width: 2048,
                realized_height: 2048,
            },
            RealizedHotRenderImage {
                image: hot_image("a-2k.png", Some(8), true),
                realized_width: 2048,
                realized_height: 2048,
            },
            RealizedHotRenderImage {
                image: hot_image("4k.png", Some(8), true),
                realized_width: 4096,
                realized_height: 4096,
            },
        ];

        let plan_8k = plan_realized_hot_render_loads(&realized, 8192, 8192, 8192);
        assert_eq!(plan_8k.groups.len(), 1);
        assert!(plan_8k.standalone.is_empty());
        assert_eq!(plan_8k.groups[0].1[0].0, "4k.png");
        assert_eq!(plan_8k.groups[0].1[1].0, "a-2k.png");
        assert_eq!(plan_8k.groups[0].1[2].0, "b-2k.png");

        let plan_4k = plan_realized_hot_render_loads(&realized, 4096, 4096, 4096);
        assert_eq!(plan_4k.groups.len(), 1);
        assert_eq!(plan_4k.standalone, vec![("4k.png".to_string(), 4096, 4096)]);

        let plan_2k = plan_realized_hot_render_loads(&realized, 2048, 2048, 2048);
        assert!(plan_2k.groups.is_empty());
        assert_eq!(plan_2k.standalone.len(), 3);
    }

    #[test]
    #[ignore = "representative timing report; run explicitly with --ignored"]
    fn hot_render_planner_microbenchmark() {
        let realized = (0..8)
            .map(|index| RealizedHotRenderImage {
                image: hot_image(&format!("sprite{index}.png"), Some(64), true),
                realized_width: 512,
                realized_height: 512,
            })
            .collect::<Vec<_>>();
        let iterations = 10_000_u128;
        let start = std::time::Instant::now();
        let mut last = None;
        for _ in 0..iterations {
            last = Some(plan_realized_hot_render_loads(&realized, 2048, 2048, 4096));
        }
        let elapsed_ns = start.elapsed().as_nanos();
        let plan = last.expect("load plan");
        let standalone_bytes = 8_u64 * 512 * 512 * 4;
        let atlas_bytes = 2048_u64 * 2048 * 4;
        let live_bytes = standalone_bytes;
        eprintln!(
            "hot-render planner benchmark: iterations={iterations} mean_ns={} standalone_bytes={standalone_bytes} atlas_bytes={atlas_bytes} occupancy_percent={} standalone_binds=512 atlas_binds=1",
            elapsed_ns / iterations,
            live_bytes * 100 / atlas_bytes
        );
        assert_eq!(plan.groups.len(), 1);
        assert!(plan.standalone.is_empty());
    }

    #[test]
    fn recording_clock_uses_frame_index_without_accumulated_rounding() {
        let _guard = test_lock();
        set_recording_clock(60, 0);
        assert_eq!(stasis_get_time_ms(), 0);
        assert_eq!(stasis_get_time_us(), 0);
        set_recording_clock_frame(1);
        assert_eq!(stasis_get_time_us(), 16_666);
        set_recording_clock_frame(3);
        assert_eq!(stasis_get_time_us(), 50_000);
        set_recording_clock(59, 59);
        assert_eq!(stasis_get_time_us(), (59_u64 * 1_000_000 / 59) as i32);
        set_recording_clock(1, u64::MAX);
        assert_eq!(stasis_get_time_ms(), i32::MAX);
        assert_eq!(stasis_get_time_us(), i32::MAX);
        clear_recording_clock();
    }

    #[test]
    fn atomic_rename_no_replace_refuses_existing_destination() {
        let _guard = test_lock();
        let root = std::env::temp_dir().join(format!(
            "stasis-atomic-rename-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("create atomic rename test directory");
        let source = root.join("source");
        let destination = root.join("destination");
        std::fs::write(&source, b"source").expect("write source");
        std::fs::write(&destination, b"destination").expect("write destination");
        assert!(atomic_rename_no_replace(&source, &destination).is_err());
        assert_eq!(std::fs::read(&source).expect("read source"), b"source");
        assert_eq!(
            std::fs::read(&destination).expect("read destination"),
            b"destination"
        );
        std::fs::remove_dir_all(root).expect("remove atomic rename test directory");
    }

    static TEST_SPRITE_RELEASES: AtomicUsize = AtomicUsize::new(0);
    static TEST_AUDIO_CALLBACKS: AtomicUsize = AtomicUsize::new(0);
    static TEST_AUDIO_PATH_CALLS: AtomicUsize = AtomicUsize::new(0);

    fn test_sprite_load(_: &[u8], _: i32, _: i32) -> i32 {
        77
    }

    fn test_sprite_release(handle: i32) {
        if handle == 77 {
            TEST_SPRITE_RELEASES.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn test_font_load(_: &[u8], _: i32) -> i32 {
        1
    }

    fn test_measure_text(_: i32, _: &[u8]) -> f32 {
        1.0
    }

    fn test_cache_text(_: i32, _: &[u8]) -> i32 {
        1
    }

    fn test_replace_text(handle: i32, _: i32, _: &[u8]) -> i32 {
        if handle <= 1 {
            2
        } else {
            handle
        }
    }

    fn test_measure_cached(_: i32) -> f32 {
        1.0
    }

    fn test_poll_reload(_: i32) -> i32 {
        0
    }

    extern "C" fn test_audio_load(path: *const c_char) -> i32 {
        TEST_AUDIO_CALLBACKS.fetch_add(1, Ordering::SeqCst);
        if !path.is_null() {
            let path = unsafe { std::ffi::CStr::from_ptr(path) };
            if path.to_bytes().starts_with(b"assets/") {
                TEST_AUDIO_PATH_CALLS.fetch_add(1, Ordering::SeqCst);
            }
        }
        101
    }

    extern "C" fn test_audio_release(_: i32) {
        TEST_AUDIO_CALLBACKS.fetch_add(1, Ordering::SeqCst);
    }

    extern "C" fn test_audio_play(_: i32, _: i32, _: f32, _: f32) -> i32 {
        TEST_AUDIO_CALLBACKS.fetch_add(1, Ordering::SeqCst);
        202
    }

    extern "C" fn test_audio_stop(_: i32) {
        TEST_AUDIO_CALLBACKS.fetch_add(1, Ordering::SeqCst);
    }

    extern "C" fn test_audio_voice_is_playing(_: i32) -> i32 {
        TEST_AUDIO_CALLBACKS.fetch_add(1, Ordering::SeqCst);
        1
    }

    extern "C" fn test_audio_voice_set_paused(_: i32, _: i32) {
        TEST_AUDIO_CALLBACKS.fetch_add(1, Ordering::SeqCst);
    }

    extern "C" fn test_audio_voice_set_volume_pan(_: i32, _: f32, _: f32) {
        TEST_AUDIO_CALLBACKS.fetch_add(1, Ordering::SeqCst);
    }

    extern "C" fn test_audio_play_music(_: i32, _: i32, _: f32) -> i32 {
        TEST_AUDIO_CALLBACKS.fetch_add(1, Ordering::SeqCst);
        303
    }

    extern "C" fn test_audio_stop_music(_: i32) {
        TEST_AUDIO_CALLBACKS.fetch_add(1, Ordering::SeqCst);
    }

    extern "C" fn test_audio_pause_music(_: i32, _: i32) {
        TEST_AUDIO_CALLBACKS.fetch_add(1, Ordering::SeqCst);
    }

    extern "C" fn test_audio_set_music_volume(_: i32, _: f32) {
        TEST_AUDIO_CALLBACKS.fetch_add(1, Ordering::SeqCst);
    }

    extern "C" fn test_audio_play_effect(_: i32, _: f32) -> i32 {
        TEST_AUDIO_CALLBACKS.fetch_add(1, Ordering::SeqCst);
        404
    }

    struct AudioHostReset;

    impl Drop for AudioHostReset {
        fn drop(&mut self) {
            install_audio_host_api(None);
            clear_jit_string_literal_table();
        }
    }

    fn test_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .expect("dynload test lock mutex poisoned")
    }

    #[test]
    fn audio_asset_bridges_dispatch_through_installed_host_api() {
        let _guard = test_lock();
        let _host_reset = AudioHostReset;
        TEST_AUDIO_CALLBACKS.store(0, Ordering::SeqCst);
        TEST_AUDIO_PATH_CALLS.store(0, Ordering::SeqCst);
        clear_jit_string_literal_table();
        upsert_jit_string_literal(900, "assets/music.mp3");
        upsert_jit_string_literal(901, "assets/effect.wav");

        install_audio_host_api(Some(StasisAudioHostApi {
            init: None,
            shutdown: None,
            is_available: None,
            get_sample_rate: None,
            get_channels: None,
            get_queued_frames: None,
            get_underruns: None,
            push_f32_interleaved: None,
            load_wav: Some(test_audio_load),
            release: Some(test_audio_release),
            play: Some(test_audio_play),
            stop: Some(test_audio_stop),
            voice_is_playing: Some(test_audio_voice_is_playing),
            voice_set_paused: Some(test_audio_voice_set_paused),
            voice_set_volume_pan: Some(test_audio_voice_set_volume_pan),
            load_music: Some(test_audio_load),
            load_effect: Some(test_audio_load),
            play_music: Some(test_audio_play_music),
            stop_music: Some(test_audio_stop_music),
            pause_music: Some(test_audio_pause_music),
            set_music_volume: Some(test_audio_set_music_volume),
            play_effect: Some(test_audio_play_effect),
        }));

        assert_eq!(stasis_jit_audio_load_wav(901), 101);
        assert_eq!(stasis_jit_audio_load_music(900), 101);
        assert_eq!(stasis_jit_audio_load_effect(901), 101);
        stasis_jit_audio_release(101);
        assert_eq!(stasis_jit_audio_play(101, 1, 0.5, -0.25), 202);
        stasis_jit_audio_stop(202);
        assert_eq!(stasis_jit_audio_voice_is_playing(202), 1);
        stasis_jit_audio_voice_set_paused(202, 1);
        stasis_jit_audio_voice_set_volume_pan(202, 0.25, 0.5);
        assert_eq!(stasis_jit_audio_play_music(101, 1, 0.4), 303);
        stasis_jit_audio_stop_music(303);
        stasis_jit_audio_pause_music(101, 1);
        stasis_jit_audio_set_music_volume(101, 0.2);
        assert_eq!(stasis_jit_audio_play_effect(101, 0.3), 404);
        assert_eq!(TEST_AUDIO_PATH_CALLS.load(Ordering::SeqCst), 3);
        assert_eq!(TEST_AUDIO_CALLBACKS.load(Ordering::SeqCst), 14);
    }

    #[test]
    fn offline_web_network_bridges_are_inert_and_contractual() {
        assert_eq!(stasis_jit_offline_web_network_supported(), 0);
        assert_eq!(stasis_jit_offline_web_network_connect(), -4);
        assert_eq!(stasis_jit_offline_web_network_status(), -4);
        assert_eq!(stasis_jit_offline_web_network_poll(123, 0), -4);
        assert_eq!(stasis_jit_offline_web_network_send(456, 0), -4);
        assert_eq!(stasis_jit_offline_web_network_resume_seat(), -1);
        assert_eq!(stasis_jit_offline_web_network_last_sequence(), 0);
        assert_eq!(stasis_jit_offline_web_network_checkpoint(-1, 0), -4);

        assert_eq!(
            stasis_jit_offline_web_network_poll(123, -1),
            -1,
            "poll rejects negative capacity"
        );
        assert_eq!(
            stasis_jit_offline_web_network_poll(123, OFFLINE_WEB_NETWORK_MAX_MESSAGE_BYTES + 1),
            -1,
            "poll rejects oversized capacity"
        );
        assert_eq!(
            stasis_jit_offline_web_network_send(456, -1),
            -1,
            "send rejects negative length"
        );
        assert_eq!(
            stasis_jit_offline_web_network_send(456, OFFLINE_WEB_NETWORK_MAX_MESSAGE_BYTES + 1),
            -1,
            "send rejects oversized length"
        );
        assert_eq!(
            stasis_jit_offline_web_network_checkpoint(-2, 0),
            -1,
            "checkpoint rejects invalid seat"
        );
        assert_eq!(
            stasis_jit_offline_web_network_checkpoint(0, -1),
            -1,
            "checkpoint rejects invalid sequence"
        );
        assert_eq!(
            stasis_jit_offline_web_network_checkpoint(8, 0),
            -1,
            "checkpoint rejects out-of-range seat"
        );
    }

    #[test]
    fn network_seed_bridge_is_capability_gated() {
        #[cfg(feature = "network")]
        {
            assert_eq!(stasis_jit_network_supported(), 1);
            let seed = stasis_jit_network_host_random_seed();
            assert!(seed > 0 && seed <= i32::MAX);
        }
        #[cfg(not(feature = "network"))]
        {
            assert_eq!(stasis_jit_network_supported(), 0);
            assert_eq!(stasis_jit_network_host_random_seed(), 0);
        }
    }

    struct EmbeddedHostReset;

    impl Drop for EmbeddedHostReset {
        fn drop(&mut self) {
            set_embedded_graphics_host(None);
        }
    }

    #[test]
    fn jit_sprite_same_handle_replacement_releases_prior_acquisition() {
        let _guard = test_lock();
        clear_registered_global_memory();
        clear_jit_i32_global_table();
        clear_jit_i32_array_global_table();
        clear_jit_string_literal_table();
        TEST_SPRITE_RELEASES.store(0, Ordering::SeqCst);
        let _host_reset = EmbeddedHostReset;
        set_embedded_graphics_host(Some(EmbeddedGraphicsHost {
            load_sprite: test_sprite_load,
            release_sprite: test_sprite_release,
            load_font: test_font_load,
            measure_text: test_measure_text,
            cache_text: test_cache_text,
            replace_text: test_replace_text,
            measure_text_cached: test_measure_cached,
            measure_text_cached_height: test_measure_cached,
            poll_reload: test_poll_reload,
        }));
        let mut handles = [0_i32];
        let mut widths = [0_i32];
        let mut heights = [0_i32];
        register_global_i32_array(100, global_path_hash("handle"), handles.as_mut_ptr(), 1);
        register_global_i32_array(100, global_path_hash("width"), widths.as_mut_ptr(), 1);
        register_global_i32_array(100, global_path_hash("height"), heights.as_mut_ptr(), 1);
        upsert_jit_string_literal(55, "assets/sprite.svg");
        assert_eq!(stasis_jit_sprite_load_from(100, 0, 1, 55, 32, 24), 1);
        assert_eq!(stasis_jit_sprite_load_from(100, 0, 1, 55, 32, 24), 1);
        assert_eq!(handles[0], 77);
        assert_eq!(TEST_SPRITE_RELEASES.load(Ordering::SeqCst), 1);
        clear_registered_global_memory();
        clear_jit_i32_global_table();
        clear_jit_i32_array_global_table();
        clear_jit_string_literal_table();
    }

    #[test]
    fn jit_text_run_replacement_publishes_only_after_host_success() {
        let _guard = test_lock();
        clear_registered_global_memory();
        clear_jit_string_literal_table();
        let _host_reset = EmbeddedHostReset;
        set_embedded_graphics_host(Some(EmbeddedGraphicsHost {
            load_sprite: test_sprite_load,
            release_sprite: test_sprite_release,
            load_font: test_font_load,
            measure_text: test_measure_text,
            cache_text: test_cache_text,
            replace_text: test_replace_text,
            measure_text_cached: test_measure_cached,
            measure_text_cached_height: test_measure_cached,
            poll_reload: test_poll_reload,
        }));
        let mut fonts = [1_i32];
        let mut handles = [1_i32];
        let mut widths = [7.0_f32];
        let mut heights = [8.0_f32];
        register_global_i32_array(200, global_path_hash("font"), fonts.as_mut_ptr(), 1);
        register_global_i32_array(200, global_path_hash("handle"), handles.as_mut_ptr(), 1);
        register_global_f32_array(200, global_path_hash("width"), widths.as_mut_ptr(), 1);
        register_global_f32_array(200, global_path_hash("height"), heights.as_mut_ptr(), 1);
        upsert_jit_string_literal(88, "score 1");
        assert_eq!(stasis_jit_text_run_replace_from(200, 0, 1, 1, 88), 1);
        assert_eq!(
            (fonts[0], handles[0], widths[0], heights[0]),
            (1, 2, 1.0, 1.0)
        );
        upsert_jit_string_literal(88, "score 2");
        assert_eq!(stasis_jit_text_run_replace_from(200, 0, 1, 1, 88), 1);
        assert_eq!(handles[0], 2);
        let before = (fonts[0], handles[0], widths[0], heights[0]);
        assert_eq!(stasis_jit_text_run_replace_from(200, 0, 1, 0, 88), 0);
        assert_eq!((fonts[0], handles[0], widths[0], heights[0]), before);
        clear_registered_global_memory();
        clear_jit_string_literal_table();
    }

    #[test]
    fn bundled_graphics_runtime_is_default_candidate() {
        let executable = std::env::current_exe().expect("current test executable");
        let candidates = runtime_library_candidate_paths();
        let expected = executable
            .parent()
            .expect("test executable directory")
            .join(runtime_library_file_names()[0]);
        assert_eq!(candidates.first(), Some(&expected));
    }

    #[test]
    fn build_fingerprint_contract_accepts_only_sha256_hex() {
        let valid = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        assert!(is_verified_build_fingerprint(valid));
        assert!(is_verified_build_fingerprint(&valid.to_ascii_uppercase()));
        assert!(!is_verified_build_fingerprint("development"));
        assert!(!is_verified_build_fingerprint(""));
        assert!(!is_verified_build_fingerprint("not-a-sha256"));
    }

    #[test]
    fn configured_runtime_is_fallback_without_executable_directory() {
        let configured = PathBuf::from("configured/stasis_graphics.dll");
        let candidates =
            runtime_library_candidate_paths_for(None, std::slice::from_ref(&configured));
        assert_eq!(candidates.first(), Some(&configured));
    }

    #[cfg(not(windows))]
    #[test]
    fn portable_preferences_persist_and_fail_closed() {
        let _guard = test_lock();
        let root = std::env::temp_dir().join(format!(
            "stasis_dynload_preferences_{}_{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let _ = std::fs::remove_dir_all(&root);
        set_preference_storage_root(Some(root.clone()));

        assert_eq!(portable_storage_load_i32(b"game", b"tier", 1), 1);
        assert!(portable_storage_save_i32(b"game", b"tier", 4));
        assert_eq!(portable_storage_load_i32(b"game", b"tier", 1), 4);
        assert!(!portable_storage_save_i32(b"../game", b"tier", 5));
        clear_jit_string_literal_table();
        upsert_jit_string_literal(700, "game");
        upsert_jit_string_literal(701, "tier");
        assert_eq!(stasis_jit_storage_save_i32(700, 701, 6), 1);
        assert_eq!(stasis_jit_storage_load_i32(700, 701, 1), 6);
        std::fs::write(root.join("game/tier.i32"), "surprise\n").expect("write corrupt value");
        assert_eq!(portable_storage_load_i32(b"game", b"tier", 2), 2);

        clear_jit_string_literal_table();
        set_preference_storage_root(None);
        std::fs::remove_dir_all(root).expect("remove preference fixture");
    }

    #[cfg(windows)]
    #[test]
    fn can_load_kernel32_and_resolve_export() {
        let library = Library::load(Path::new("kernel32.dll")).expect("load kernel32");
        let address = library
            .symbol_address("GetTickCount")
            .expect("resolve GetTickCount");
        assert_ne!(address, 0);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn can_load_linux_libc_and_resolve_export() {
        let library = Library::load(Path::new("libc.so.6")).expect("load libc");
        let address = library.symbol_address("malloc").expect("resolve malloc");
        assert_ne!(address, 0);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn can_load_macos_libsystem_and_resolve_export() {
        let library =
            Library::load(Path::new("/usr/lib/libSystem.B.dylib")).expect("load libSystem");
        let address = library.symbol_address("malloc").expect("resolve malloc");
        assert_ne!(address, 0);
    }

    #[cfg(windows)]
    #[test]
    fn can_invoke_get_tick_count_export() {
        let library = Library::load(Path::new("kernel32.dll")).expect("load kernel32");
        let address = library
            .symbol_address("GetTickCount")
            .expect("resolve GetTickCount");
        let value = invoke_noarg_u64(address).expect("invoke GetTickCount");
        assert!(value <= u64::from(u32::MAX));
    }

    #[test]
    fn jit_debugger_blocks_on_breakpoints_and_preserves_real_nested_frames() {
        let _lock = test_lock();
        disable_jit_debugger();
        enable_jit_debugger([(2, 20)]);
        let worker = std::thread::spawn(|| {
            stasis_jit_debug_frame_enter(1);
            stasis_jit_debug_value_i64(0, 1, 7);
            stasis_jit_debug_statement(1, 10);
            stasis_jit_debug_frame_enter(2);
            stasis_jit_debug_value_f64(0, 2, 1.5);
            stasis_jit_debug_statement(2, 20);
            stasis_jit_debug_frame_enter(3);
            stasis_jit_debug_statement(3, 30);
            stasis_jit_debug_frame_leave(3);
            stasis_jit_debug_statement(2, 21);
            stasis_jit_debug_frame_leave(2);
            stasis_jit_debug_statement(1, 11);
            stasis_jit_debug_frame_leave(1);
        });

        let first = wait_for_jit_debug_stop(0, Duration::from_secs(2)).expect("breakpoint stop");
        assert_eq!((first.function_id, first.site_id), (2, 20));
        assert_eq!(first.frames.len(), 2);
        assert_eq!(
            first.frames[0].values.get(&0),
            Some(&JitDebugValue::I64 {
                type_tag: 1,
                value: 7
            })
        );
        assert_eq!(
            first.frames[1].values.get(&0),
            Some(&JitDebugValue::F64 {
                type_tag: 2,
                value: 1.5
            })
        );

        resume_jit_debugger(JitDebugResume::StepIn).expect("step in");
        let second =
            wait_for_jit_debug_stop(first.sequence, Duration::from_secs(2)).expect("step-in stop");
        assert_eq!((second.function_id, second.site_id), (3, 30));
        assert_eq!(second.frames.len(), 3);

        resume_jit_debugger(JitDebugResume::StepOut).expect("step out");
        let third = wait_for_jit_debug_stop(second.sequence, Duration::from_secs(2))
            .expect("step-out stop");
        assert_eq!((third.function_id, third.site_id), (2, 21));
        assert_eq!(third.frames.len(), 2);

        resume_jit_debugger(JitDebugResume::StepOut).expect("step out to caller");
        let fourth = wait_for_jit_debug_stop(third.sequence, Duration::from_secs(2))
            .expect("caller step-out stop");
        assert_eq!((fourth.function_id, fourth.site_id), (1, 11));
        assert_eq!(fourth.frames.len(), 1);

        resume_jit_debugger(JitDebugResume::Continue).expect("continue");
        worker.join().expect("debug worker");
        disable_jit_debugger();
        assert!(jit_debug_stop().is_none());
    }

    #[test]
    fn i32_array_ptr_migrates_fallback_values_and_supports_direct_access() {
        let _lock = test_lock();
        clear_registered_global_memory();
        clear_jit_i32_array_global_table();

        let collection_hash = 0x1357_2468i32;
        let field_hash = 0x2468_1357i32;
        stasis_jit_global_i32_array_store(collection_hash, field_hash, 0, 11);
        stasis_jit_global_i32_array_store(collection_hash, field_hash, 1, 22);
        stasis_jit_global_i32_array_store(collection_hash, field_hash, 2, 33);

        let ptr = stasis_jit_global_i32_array_ptr(collection_hash, field_hash, 4);
        assert!(!ptr.is_null());

        assert_eq!(
            stasis_jit_global_i32_array_load(collection_hash, field_hash, 0),
            11
        );
        assert_eq!(
            stasis_jit_global_i32_array_load(collection_hash, field_hash, 1),
            22
        );
        assert_eq!(
            stasis_jit_global_i32_array_load(collection_hash, field_hash, 2),
            33
        );

        // Safety: pointer comes from dynload-owned registration table for this test process.
        unsafe {
            *ptr.add(3) = 44;
        }
        assert_eq!(
            stasis_jit_global_i32_array_load(collection_hash, field_hash, 3),
            44
        );

        let ptr2 = stasis_jit_global_i32_array_ptr(collection_hash, field_hash, 4);
        assert_eq!(ptr, ptr2);
    }

    #[test]
    fn registered_i32_array_fill_is_bounded_and_does_not_partially_write() {
        let _lock = test_lock();
        clear_registered_global_memory();
        let key = 0x2468_1357i32;
        let mut values = [1, 2, 3, 4, 5, 6];
        register_global_i32_array(key, 0, values.as_mut_ptr(), values.len());

        fill_registered_global_i32_array(key, 0, 2, 3, 0).expect("fill registered range");
        assert_eq!(values, [1, 2, 0, 0, 0, 6]);

        let before_rejection = values;
        assert!(fill_registered_global_i32_array(key, 0, 5, 2, 9)
            .expect_err("reject out-of-bounds fill")
            .contains("exceeds length"));
        assert_eq!(values, before_rejection);
        clear_registered_global_memory();
    }

    #[test]
    fn render_trace_rejects_non_contract_lengths_before_allocating() {
        let _lock = test_lock();
        clear_registered_global_memory();

        // Safety: invalid lengths must be rejected before any pointer is resolved.
        let trace = unsafe {
            stasis_jit_render_trace(
                101,
                i32::MAX,
                102,
                STASIS_RENDER_F32_COUNT as i32,
                103,
                STASIS_RENDER_U8_COUNT as i32,
            )
        };

        assert_eq!(trace, 0);
        assert!(registered_i32_arrays()
            .lock()
            .expect("registered i32 array table mutex poisoned")
            .is_empty());
        assert!(registered_f32_arrays()
            .lock()
            .expect("registered f32 array table mutex poisoned")
            .is_empty());
        assert!(registered_u8_arrays()
            .lock()
            .expect("registered u8 array table mutex poisoned")
            .is_empty());
    }

    #[test]
    fn render_trace_preserves_cross_category_order() {
        let mut i32s = vec![0i32; STASIS_RENDER_I32_COUNT];
        let mut f32s = vec![0.0f32; STASIS_RENDER_F32_COUNT];
        let u8s = vec![0u8; STASIS_RENDER_U8_COUNT];
        i32s[0] = STASIS_RENDER_MAGIC;
        i32s[1] = STASIS_RENDER_VERSION;
        i32s[3] = 1;
        i32s[4] = 1;
        i32s[STASIS_RENDER_SPRITE_BASE] = 17;
        i32s[STASIS_RENDER_SPRITE_BASE + 1] = -1;
        f32s[4..12].copy_from_slice(&[1.0, 2.0, 3.0, 4.0, 0.5, 0.6, 0.7, 0.8]);
        f32s[STASIS_RENDER_SPRITE_BASE_F32..STASIS_RENDER_SPRITE_BASE_F32 + 13].copy_from_slice(&[
            10.0, 20.0, 30.0, 40.0, 0.0, 0.0, 0.0, 0.0, 15.0, 20.0, 1.0, 1.0, 0.0,
        ]);
        i32s[STASIS_RENDER_SPRITE_RUN_COUNT_INDEX] = 1;
        i32s[STASIS_RENDER_SPRITE_RUN_BASE] = 0;
        i32s[STASIS_RENDER_SPRITE_RUN_BASE + 1] = 1;
        i32s[STASIS_RENDER_SPRITE_RUN_BASE + 2] = -1;
        i32s[STASIS_RENDER_ORDER_COUNT_INDEX] = 2;
        i32s[STASIS_RENDER_ORDER_BASE] = 2 * 16_384;
        i32s[STASIS_RENDER_ORDER_BASE + 1] = 16_384;

        let sprite_then_line =
            unsafe { stasis_render_trace_native(i32s.as_ptr(), f32s.as_ptr(), u8s.as_ptr()) };
        i32s[STASIS_RENDER_ORDER_BASE] = 16_384;
        i32s[STASIS_RENDER_ORDER_BASE + 1] = 2 * 16_384;
        let line_then_sprite =
            unsafe { stasis_render_trace_native(i32s.as_ptr(), f32s.as_ptr(), u8s.as_ptr()) };
        i32s[STASIS_RENDER_ORDER_COUNT_INDEX] = 0;
        let fallback_without_order =
            unsafe { stasis_render_trace_native(i32s.as_ptr(), f32s.as_ptr(), u8s.as_ptr()) };

        assert_ne!(sprite_then_line, 0);
        assert_ne!(sprite_then_line, line_then_sprite);
        assert_eq!(line_then_sprite, fallback_without_order);
    }

    #[test]
    fn current_render_trace_accepts_only_the_current_canonical_buffers() {
        let mut i32s = vec![0i32; STASIS_RENDER_I32_COUNT];
        let mut f32s = vec![0.0f32; STASIS_RENDER_F32_COUNT];
        let u8s = vec![0u8; STASIS_RENDER_U8_COUNT];
        i32s[0] = STASIS_RENDER_MAGIC;
        i32s[1] = STASIS_RENDER_VERSION;
        i32s[3] = 1;
        f32s[4..12].copy_from_slice(&[1.0, 2.0, 3.0, 4.0, 0.5, 0.6, 0.7, 0.8]);

        assert_ne!(current_render_trace(&i32s, &f32s, &u8s), 0);
        assert_eq!(
            current_render_trace(&i32s[..i32s.len() - 1], &f32s, &u8s),
            0
        );
        assert_eq!(
            current_render_trace(&i32s, &f32s[..f32s.len() - 1], &u8s),
            0
        );
        assert_eq!(current_render_trace(&i32s, &f32s, &u8s[..u8s.len() - 1]), 0);

        i32s[1] = STASIS_RENDER_VERSION - 1;
        assert_eq!(current_render_trace(&i32s, &f32s, &u8s), 0);
        i32s[1] = STASIS_RENDER_VERSION;
        i32s[0] ^= 1;
        assert_eq!(current_render_trace(&i32s, &f32s, &u8s), 0);
    }

    #[test]
    fn render_trace_rejects_legacy_versions_and_non_current_lengths() {
        let _lock = test_lock();
        clear_registered_global_memory();

        let mut i32s = vec![0i32; STASIS_RENDER_I32_COUNT];
        let mut f32s = vec![0.0f32; STASIS_RENDER_F32_COUNT];
        let mut u8s = vec![0u8; STASIS_RENDER_U8_COUNT];
        i32s[0] = STASIS_RENDER_MAGIC;
        i32s[1] = STASIS_RENDER_VERSION;
        i32s[3] = 1;
        f32s[4..12].copy_from_slice(&[1.0, 2.0, 3.0, 4.0, 0.5, 0.6, 0.7, 0.8]);

        let i32_id = global_path_hash("gfx_cmd_i32");
        let f32_id = global_path_hash("gfx_cmd_f32");
        let u8_id = global_path_hash("gfx_cmd_u8");
        register_global_i32_array(i32_id, 0, i32s.as_mut_ptr(), i32s.len());
        register_global_f32_array(f32_id, 0, f32s.as_mut_ptr(), f32s.len());
        register_global_u8_array(u8_id, 0, u8s.as_mut_ptr(), u8s.len());

        let expected =
            unsafe { stasis_render_trace_native(i32s.as_ptr(), f32s.as_ptr(), u8s.as_ptr()) }
                as i32;
        assert_ne!(expected, 0);
        let current = unsafe {
            stasis_jit_render_trace(
                i32_id,
                STASIS_RENDER_I32_COUNT as i32,
                f32_id,
                STASIS_RENDER_F32_COUNT as i32,
                u8_id,
                STASIS_RENDER_U8_COUNT as i32,
            )
        };
        assert_eq!(current, expected);
        for legacy_version in 2..=5 {
            i32s[1] = legacy_version;
            let rejected = unsafe {
                stasis_jit_render_trace(
                    i32_id,
                    STASIS_RENDER_I32_COUNT as i32,
                    f32_id,
                    STASIS_RENDER_F32_COUNT as i32,
                    u8_id,
                    STASIS_RENDER_U8_COUNT as i32,
                )
            };
            assert_eq!(
                rejected, 0,
                "legacy render version {legacy_version} must be rejected"
            );
        }
        i32s[1] = STASIS_RENDER_VERSION;
        let wrong_i32_length = unsafe {
            stasis_jit_render_trace(
                i32_id,
                (STASIS_RENDER_I32_COUNT - 1) as i32,
                f32_id,
                STASIS_RENDER_F32_COUNT as i32,
                u8_id,
                STASIS_RENDER_U8_COUNT as i32,
            )
        };
        assert_eq!(wrong_i32_length, 0);
        let wrong_f32_length = unsafe {
            stasis_jit_render_trace(
                i32_id,
                STASIS_RENDER_I32_COUNT as i32,
                f32_id,
                (STASIS_RENDER_F32_COUNT - 1) as i32,
                u8_id,
                STASIS_RENDER_U8_COUNT as i32,
            )
        };
        assert_eq!(wrong_f32_length, 0);

        clear_registered_global_memory();
    }
    #[test]
    fn active_render_copy_preserves_reverse_rectangles() {
        let _lock = test_lock();
        clear_registered_global_memory();

        let mut i32s = vec![0i32; STASIS_RENDER_I32_COUNT];
        let mut f32s = vec![0.0f32; STASIS_RENDER_F32_COUNT];
        let mut u8s = vec![0u8; STASIS_RENDER_U8_COUNT];
        i32s[0] = STASIS_RENDER_MAGIC;
        i32s[1] = STASIS_RENDER_VERSION;
        i32s[3] = 1;
        i32s[STASIS_RENDER_RECT_COUNT_INDEX] = 2;
        i32s[STASIS_RENDER_ORDER_COUNT_INDEX] = 3;
        f32s[4..12].copy_from_slice(&[1.0, 2.0, 3.0, 4.0, 0.1, 0.2, 0.3, 0.4]);
        let rect_start = STASIS_RENDER_SPRITE_BASE_F32 - 2 * STASIS_RENDER_LINE_STRIDE;
        f32s[rect_start..STASIS_RENDER_SPRITE_BASE_F32].copy_from_slice(&[
            9.0, 10.0, 11.0, 12.0, 0.5, 0.6, 0.7, 0.8, 1.0, 2.0, 3.0, 4.0, 0.2, 0.3, 0.4, 0.5,
        ]);

        let i32_id = global_path_hash("gfx_cmd_i32");
        register_global_i32_array(i32_id, 0, i32s.as_mut_ptr(), i32s.len());
        register_global_f32_array(
            global_path_hash("gfx_cmd_f32"),
            0,
            f32s.as_mut_ptr(),
            f32s.len(),
        );
        register_global_u8_array(
            global_path_hash("gfx_cmd_u8"),
            0,
            u8s.as_mut_ptr(),
            u8s.len(),
        );

        let mut out_i32 = vec![0i32; STASIS_RENDER_I32_COUNT];
        let mut out_f32 = vec![0.0f32; STASIS_RENDER_F32_COUNT];
        let mut out_u8 = vec![0u8; STASIS_RENDER_U8_COUNT];
        let counts = copy_jit_render_active(&mut out_i32, &mut out_f32, &mut out_u8)
            .expect("copy current buffer");

        assert_eq!(counts.lines, 1);
        assert_eq!(counts.rects, 2);
        assert_eq!(counts.order, 3);
        assert_eq!(out_i32[STASIS_RENDER_RECT_COUNT_INDEX], 2);
        assert_eq!(
            &out_f32[rect_start..STASIS_RENDER_SPRITE_BASE_F32],
            &f32s[rect_start..STASIS_RENDER_SPRITE_BASE_F32]
        );

        clear_registered_global_memory();
    }

    #[test]
    fn audio_push_bridge_forwards_registered_interleaved_samples() {
        let _lock = test_lock();
        clear_registered_global_memory();

        let samples_id = global_path_hash("audio_samples");
        let mut samples = vec![0.25f32, -0.25, 0.5, -0.5];
        register_global_f32_array(samples_id, 0, samples.as_mut_ptr(), samples.len());

        let accepted = with_jit_audio_f32_interleaved(samples_id, 2, 2, |values, frames| {
            assert_eq!(frames, 2);
            assert_eq!(unsafe { std::slice::from_raw_parts(values, 4) }, samples);
            frames
        });
        assert_eq!(accepted, 2);
        assert_eq!(
            with_jit_audio_f32_interleaved(samples_id, i32::MAX, 2, |_, _| {
                panic!("overflowing sample count must not reach the runtime")
            }),
            0
        );

        clear_registered_global_memory();
    }

    #[test]
    fn runtime_state_snapshot_restores_owned_memory_without_reading_borrowed_memory() {
        let _lock = test_lock();
        clear_registered_global_memory();
        clear_jit_i32_global_table();
        clear_jit_f32_global_table();
        clear_jit_f64_global_table();
        clear_jit_i32_array_global_table();
        clear_jit_f32_array_global_table();
        clear_jit_f64_array_global_table();

        register_global_i32_ptr(1, std::ptr::null_mut());
        register_global_u8_array(2, 0, std::ptr::null_mut(), 4);
        let mut scalar = 7i32;
        let mut bytes = vec![1u8, 2, 3, 4];
        register_global_i32_ptr(10, &mut scalar);
        register_global_u8_array(20, 0, bytes.as_mut_ptr(), bytes.len());
        register_global_i32_array(21, 0, 1usize as *mut i32, 4);
        stasis_jit_global_i32_store(30, 9);
        stasis_jit_global_f32_array_store(40, 2, 0, 1.5);
        assert!(snapshot_jit_runtime_state_bounded(1)
            .expect_err("snapshot should be bounded")
            .contains("limit"));
        let snapshot = snapshot_jit_runtime_state();

        scalar = 70;
        bytes.copy_from_slice(&[9, 9, 9, 9]);
        stasis_jit_global_i32_store(30, 90);
        stasis_jit_global_i32_store(31, 100);
        stasis_jit_global_f32_array_store(40, 2, 0, 8.5);
        restore_jit_runtime_state(&snapshot);

        assert_eq!(scalar, 70);
        assert_eq!(bytes, vec![9, 9, 9, 9]);
        assert_eq!(stasis_jit_global_i32_load(30), 9);
        assert_eq!(stasis_jit_global_i32_load(31), 0);
        assert_eq!(stasis_jit_global_f32_array_load(40, 2, 0), 1.5);
    }

    #[test]
    fn runtime_state_snapshot_does_not_reclaim_scalar_replaced_by_host_memory() {
        let _lock = test_lock();
        clear_registered_global_memory();
        let key = 76;
        ensure_owned_i32_scalar(key).expect("provision owned scalar");
        stasis_jit_global_i32_store(key, 7);

        let mut borrowed = 11;
        register_global_i32_ptr(key, &mut borrowed);
        let snapshot = snapshot_jit_runtime_state();
        borrowed = 13;
        restore_jit_runtime_state(&snapshot);

        assert_eq!(borrowed, 13);
        assert_eq!(stasis_jit_global_i32_load(key), 13);
        clear_registered_global_memory();
    }

    #[test]
    fn runtime_state_rollback_leaves_borrowed_storage_descriptor_unchanged() {
        let _lock = test_lock();
        clear_registered_global_memory();
        let key = 77;
        let mut original = [3, 5];
        register_global_i32_array(key, 0, original.as_mut_ptr(), original.len());
        let slot_address = direct_array_storage_slot_address(JitStorageKind::I32, key, 0)
            .expect("reserve direct slot");
        let snapshot = snapshot_jit_runtime_state();

        let mut replacement = [9, 8, 7, 6];
        register_global_i32_array(key, 0, replacement.as_mut_ptr(), replacement.len());
        let slot = unsafe { &*(slot_address as *const JitStorageSlot) };
        assert_eq!(slot.data, replacement.as_ptr() as usize);
        assert_eq!(slot.len, replacement.len());

        restore_jit_runtime_state(&snapshot);
        let slot = unsafe { &*(slot_address as *const JitStorageSlot) };
        assert_eq!(slot.data, replacement.as_ptr() as usize);
        assert_eq!(slot.len, replacement.len());
        assert_eq!(unsafe { *(slot.data as *const i32).add(1) }, 8);
        clear_registered_global_memory();
    }

    #[test]
    fn direct_array_growth_is_rejected_during_guest_execution() {
        let _lock = test_lock();
        clear_registered_global_memory();
        let key = 78;
        let mut values = [3, 5];
        register_global_i32_array(key, 0, values.as_mut_ptr(), values.len());
        let slot_address = direct_array_storage_slot_address(JitStorageKind::I32, key, 0)
            .expect("reserve direct slot");

        {
            let _execution = JitExecutionGuard::enter();
            let error = ensure_jit_i32_array_capacity(key, 0, 4)
                .expect_err("growth must not rebind direct storage during guest execution");
            assert!(error.contains("between guest execution windows"));
            assert!(stasis_jit_global_i32_array_ptr(key, 0, 4).is_null());
        }

        let slot = unsafe { &*(slot_address as *const JitStorageSlot) };
        assert_eq!(slot.data, values.as_ptr() as usize);
        assert_eq!(slot.len, values.len());
        assert_eq!(values, [3, 5]);
        clear_registered_global_memory();
    }

    #[test]
    fn jit_text_arg_bytes_reads_string_literals() {
        let _lock = test_lock();
        clear_registered_global_memory();
        clear_jit_i32_global_table();
        clear_jit_i32_array_global_table();
        clear_jit_string_literal_table();

        upsert_jit_string_literal(1234, "hello");

        assert_eq!(jit_text_arg_bytes(1234), Some(b"hello".to_vec()));
        assert_eq!(stasis_jit_collection_i32_load(1234, 1), 5);
        assert_eq!(stasis_jit_collection_i32_load(1234, 2), 5);
        assert_eq!(stasis_jit_collection_i32_load(1234, 3), 5);
    }

    #[test]
    fn jit_global_i32_array_load_reads_string_literal_bytes_with_safe_bounds() {
        let _lock = test_lock();
        clear_registered_global_memory();
        clear_jit_i32_global_table();
        clear_jit_i32_array_global_table();
        clear_jit_string_literal_table();

        let literal_id = 0x1357_2468i32;
        upsert_jit_string_literal(literal_id, "52%");

        assert_eq!(
            stasis_jit_global_i32_array_load(literal_id, 0, 0),
            i32::from(b'5')
        );
        assert_eq!(
            stasis_jit_global_i32_array_load(literal_id, 0, 1),
            i32::from(b'2')
        );
        assert_eq!(
            stasis_jit_global_i32_array_load(literal_id, 0, 2),
            i32::from(b'%')
        );
        assert_eq!(stasis_jit_global_i32_array_load(literal_id, 0, 3), 0);
        assert_eq!(stasis_jit_global_i32_array_load(literal_id, 0, -1), 0);

        clear_jit_string_literal_table();
    }

    #[test]
    fn jit_global_i32_array_load_prefers_runtime_arrays_over_literal_hash_collisions() {
        let _lock = test_lock();
        clear_registered_global_memory();
        clear_jit_i32_global_table();
        clear_jit_i32_array_global_table();
        clear_jit_string_literal_table();

        let shared_id = 0x2468_1357i32;
        upsert_jit_string_literal(shared_id, "literal");

        stasis_jit_global_i32_array_store(shared_id, 0, 0, 81);
        let mut registered = [91i32, 92];
        register_global_i32_array(shared_id, 0, registered.as_mut_ptr(), registered.len());

        assert_eq!(stasis_jit_global_i32_array_load(shared_id, 0, 0), 91);
        assert_eq!(stasis_jit_global_i32_array_load(shared_id, 0, 1), 92);

        clear_registered_global_memory();
        assert_eq!(stasis_jit_global_i32_array_load(shared_id, 0, 0), 81);
        assert_eq!(stasis_jit_global_i32_array_load(shared_id, 0, 1), 0);

        clear_jit_i32_array_global_table();
        clear_jit_string_literal_table();
    }

    #[test]
    fn jit_memcpy_u8_copies_string_literal_bytes() {
        let _lock = test_lock();
        clear_registered_global_memory();
        clear_jit_i32_global_table();
        clear_jit_i32_array_global_table();
        clear_jit_string_literal_table();

        let source = 0x1234_5678i32;
        let destination = 0x2345_6789i32;
        let mut bytes = [0u8; 8];
        upsert_jit_string_literal(source, "field");
        register_global_u8_array(destination, 0, bytes.as_mut_ptr(), bytes.len());

        stasis_jit_sys_memcpy_u8(destination, 1, source, 1, 3);

        assert_eq!(&bytes, b"\0iel\0\0\0\0");
        clear_registered_global_memory();
        clear_jit_string_literal_table();
    }

    #[test]
    fn jit_memcpy_u8_preserves_raw_array_copying() {
        let _lock = test_lock();
        clear_registered_global_memory();
        clear_jit_string_literal_table();

        let source = 0x3456_789ai32;
        let destination = 0x4567_89abi32;
        let mut source_bytes = *b"raw-data";
        let mut destination_bytes = [0u8; 8];
        register_global_u8_array(source, 0, source_bytes.as_mut_ptr(), source_bytes.len());
        register_global_u8_array(
            destination,
            0,
            destination_bytes.as_mut_ptr(),
            destination_bytes.len(),
        );

        stasis_jit_sys_memcpy_u8(destination, 0, source, 0, 8);

        assert_eq!(&destination_bytes, b"raw-data");
        clear_registered_global_memory();
    }

    #[test]
    fn staged_string_literals_do_not_mutate_live_table_before_publish() {
        let _lock = test_lock();
        clear_jit_string_literal_table();
        upsert_jit_string_literal(1, "live");

        begin_jit_string_literal_staging().expect("begin staging");
        upsert_jit_string_literal(2, "candidate");
        assert_eq!(jit_string_literal_value(1).as_deref(), Some("live"));
        assert_eq!(jit_string_literal_value(2), None);
        let staged = finish_jit_string_literal_staging().expect("finish staging");

        replace_jit_string_literal_table(&staged);
        assert_eq!(jit_string_literal_value(1), None);
        assert_eq!(jit_string_literal_value(2).as_deref(), Some("candidate"));

        begin_jit_string_literal_staging().expect("begin collision staging");
        upsert_jit_string_literal(3, "first");
        upsert_jit_string_literal(3, "second");
        assert!(finish_jit_string_literal_staging()
            .expect_err("hash collision")
            .contains("collision"));
        assert_eq!(jit_string_literal_value(3), None);
    }

    #[test]
    fn jit_text_arg_bytes_reads_utf8_global_buffers() {
        let _lock = test_lock();
        clear_registered_global_memory();
        clear_jit_i32_global_table();
        clear_jit_i32_array_global_table();

        let collection_hash = 0x4455_6677i32;
        stasis_jit_collection_i32_store(collection_hash, 1, 4);
        stasis_jit_collection_i32_store(collection_hash, 2, 8);
        stasis_jit_collection_i32_store(collection_hash, 3, 4);
        stasis_jit_global_i32_array_store(collection_hash, 0, 0, i32::from(b'p'));
        stasis_jit_global_i32_array_store(collection_hash, 0, 1, i32::from(b'a'));
        stasis_jit_global_i32_array_store(collection_hash, 0, 2, i32::from(b't'));
        stasis_jit_global_i32_array_store(collection_hash, 0, 3, i32::from(b'h'));

        assert_eq!(jit_text_arg_bytes(collection_hash), Some(b"path".to_vec()));
    }

    #[test]
    fn jit_text_arg_bytes_prefers_runtime_buffer_over_literal_hash_collision() {
        let _lock = test_lock();
        clear_registered_global_memory();
        clear_jit_i32_global_table();
        clear_jit_i32_array_global_table();
        clear_jit_string_literal_table();

        let shared_id = 0x2244_6688i32;
        upsert_jit_string_literal(shared_id, "literal");
        stasis_jit_collection_i32_store(shared_id, 1, 6);
        stasis_jit_collection_i32_store(shared_id, 2, 8);
        stasis_jit_collection_i32_store(shared_id, 3, 6);
        stasis_jit_global_i32_array_store(shared_id, 0, 0, i32::from(b'b'));
        stasis_jit_global_i32_array_store(shared_id, 0, 1, i32::from(b'u'));
        stasis_jit_global_i32_array_store(shared_id, 0, 2, i32::from(b'f'));
        stasis_jit_global_i32_array_store(shared_id, 0, 3, i32::from(b'f'));
        stasis_jit_global_i32_array_store(shared_id, 0, 4, i32::from(b'e'));
        stasis_jit_global_i32_array_store(shared_id, 0, 5, i32::from(b'r'));

        assert_eq!(jit_text_arg_bytes(shared_id), Some(b"buffer".to_vec()));
        assert_eq!(stasis_jit_collection_i32_load(shared_id, 1), 6);
        assert_eq!(stasis_jit_collection_i32_load(shared_id, 2), 8);
        assert_eq!(stasis_jit_collection_i32_load(shared_id, 3), 6);
    }

    #[test]
    fn jit_profiler_aggregates_nested_inclusive_and_exclusive_time() {
        let _lock = test_lock();
        enable_jit_profiler();
        stasis_jit_profile_frame_enter(11);
        stasis_jit_profile_frame_enter(22);
        let mut value = 0_u64;
        for i in 0..10_000_u64 {
            value = std::hint::black_box(value.wrapping_add(i));
        }
        stasis_jit_profile_frame_leave(22);
        stasis_jit_profile_frame_leave(11);
        std::hint::black_box(value);
        disable_jit_profiler();

        let samples = jit_profile_snapshot();
        let parent = samples
            .iter()
            .find(|sample| sample.function_id == 11)
            .expect("parent sample");
        let child = samples
            .iter()
            .find(|sample| sample.function_id == 22)
            .expect("child sample");
        assert_eq!(parent.calls, 1);
        assert_eq!(child.calls, 1);
        assert!(parent.inclusive_ns >= child.inclusive_ns);
        assert!(parent.inclusive_ns >= parent.exclusive_ns);
        assert!(child.inclusive_ns >= child.exclusive_ns);
        assert_eq!(parent.max_inclusive_ns, parent.inclusive_ns);

        reset_jit_profile();
        assert!(jit_profile_snapshot().is_empty());
    }

    #[test]
    fn jit_profiler_merges_samples_from_multiple_threads() {
        let _lock = test_lock();
        enable_jit_profiler();
        let workers: Vec<_> = (0..4)
            .map(|_| {
                std::thread::spawn(|| {
                    for _ in 0..25 {
                        stasis_jit_profile_frame_enter(33);
                        std::hint::black_box(33);
                        stasis_jit_profile_frame_leave(33);
                    }
                })
            })
            .collect();
        for worker in workers {
            worker.join().expect("profile worker");
        }
        disable_jit_profiler();

        let samples = jit_profile_snapshot();
        let sample = samples
            .iter()
            .find(|sample| sample.function_id == 33)
            .expect("cross-thread sample");
        assert_eq!(sample.calls, 100);
        assert!(sample.inclusive_ns >= sample.exclusive_ns);
        assert!(sample.max_inclusive_ns > 0);
    }

    extern "C" fn host_entry_one() -> i32 {
        1
    }

    extern "C" fn host_entry_two() -> i32 {
        2
    }

    extern "C" fn add_frame(frame: i32) -> i32 {
        frame.saturating_add(7)
    }

    #[test]
    fn invokes_single_i32_to_i32_guest_abi() {
        let address = add_frame as *const () as usize;
        assert_eq!(invoke_i32_to_i32(address, 35), Ok(42));
        assert!(invoke_i32_to_i32(0, 0)
            .expect_err("null invocation must fail")
            .contains("null function pointer"));
    }

    #[test]
    fn host_entry_trampolines_publish_one_immutable_target_table() {
        let _lock = test_lock();
        let tick_trampoline = jit_host_tick_trampoline_ptr();
        let render_trampoline = jit_host_render_trampoline_ptr();
        begin_jit_host_entry_session(JitHostEntryTargets {
            revision: 41,
            main: host_entry_one as *const () as usize,
            tick: host_entry_one as *const () as usize,
            render: host_entry_one as *const () as usize,
            on_code_swap: None,
        })
        .expect("publish first host-entry table");
        assert_eq!(invoke_noarg_i32(tick_trampoline), Ok(1));
        assert_eq!(invoke_noarg_i32(render_trampoline), Ok(1));

        publish_jit_host_entry_targets(JitHostEntryTargets {
            revision: 42,
            main: host_entry_two as *const () as usize,
            tick: host_entry_two as *const () as usize,
            render: host_entry_two as *const () as usize,
            on_code_swap: None,
        })
        .expect("publish second host-entry table");
        assert_eq!(jit_host_tick_trampoline_ptr(), tick_trampoline);
        assert_eq!(jit_host_render_trampoline_ptr(), render_trampoline);
        assert_eq!(invoke_noarg_i32(tick_trampoline), Ok(2));
        assert_eq!(invoke_noarg_i32(render_trampoline), Ok(2));
        assert_eq!(jit_host_entry_targets().unwrap().revision, 42);
        assert!(publish_jit_host_entry_targets(JitHostEntryTargets {
            revision: 41,
            main: host_entry_one as *const () as usize,
            tick: host_entry_one as *const () as usize,
            render: host_entry_one as *const () as usize,
            on_code_swap: None,
        })
        .expect_err("stale publication")
        .contains("stale JIT host-entry revision"));
        assert_eq!(invoke_noarg_i32(tick_trampoline), Ok(2));
    }
}
