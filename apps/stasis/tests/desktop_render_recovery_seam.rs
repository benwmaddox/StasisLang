#![cfg(windows)]

use image::RgbaImage;
use serde_json::json;
use stasis_dynload::{
    Library, StasisGraphicsApi, StasisPerformanceMetricsV1, STASIS_PERF_UNAVAILABLE,
    STASIS_RENDER_F32_COUNT, STASIS_RENDER_I32_COUNT, STASIS_RENDER_MAGIC,
    STASIS_RENDER_MAX_SPRITES, STASIS_RENDER_ORDER_BASE, STASIS_RENDER_RECT_REVERSE_BASE_F32,
    STASIS_RENDER_TEXT_BASE_I32, STASIS_RENDER_U8_COUNT, STASIS_RENDER_VERSION,
};
use std::ffi::CString;
use std::fs;
use std::path::{Path, PathBuf};

const FLAGS_CLEAR_PRESENT: i32 = 3;
const FLAGS_PRESENT: i32 = 2;
const ORDER_KIND_SCALE: i32 = 16_384;
const ORDER_TEXT: i32 = 3 * ORDER_KIND_SCALE;
const ORDER_RECT: i32 = 4 * ORDER_KIND_SCALE;

type ScheduleScreenshot = extern "system" fn(*const std::ffi::c_char) -> i32;
type GetSubmissionState = extern "system" fn(*mut i32, i32) -> i32;
type GetPresentationBaselineState = extern "system" fn(*mut i32, i32) -> i32;
type PoisonPhysicalTarget = extern "system" fn(*mut i32, i32) -> i32;
type FailNextTextPreparation = extern "system" fn() -> i32;
type ReadPhysicalTargetPixel = extern "system" fn(*mut i32, i32) -> i32;
type PresentationPoisonTargetKind = extern "system" fn() -> i32;
type SetMetricsEnabled = extern "system" fn(i32);
type MetricsEnabled = extern "system" fn() -> i32;
type SetGuestMetrics = extern "system" fn(u64, u64);
type GetLatestMetrics = extern "system" fn(*mut StasisPerformanceMetricsV1, usize) -> i32;
type PushInputEvent = extern "system" fn(i32, i32, f32, f32) -> i32;
type Shutdown = extern "system" fn();

const SDL_SCANCODE_F3: i32 = 60;

struct NativeRecoveryHarness {
    _library: Library,
    schedule_screenshot: ScheduleScreenshot,
    get_submission_state: GetSubmissionState,
    get_presentation_baseline_state: Option<GetPresentationBaselineState>,
    poison_physical_target: Option<PoisonPhysicalTarget>,
    fail_next_text_preparation: Option<FailNextTextPreparation>,
    read_physical_target_pixel: Option<ReadPhysicalTargetPixel>,
    presentation_poison_target_kind: Option<PresentationPoisonTargetKind>,
    set_metrics_enabled: SetMetricsEnabled,
    metrics_enabled: MetricsEnabled,
    set_guest_metrics: SetGuestMetrics,
    get_latest_metrics: GetLatestMetrics,
    push_input_event: PushInputEvent,
    shutdown: Shutdown,
}

impl NativeRecoveryHarness {
    fn load(path: &Path) -> Self {
        let library = Library::load(path).expect("load graphics runtime for recovery seam");
        let schedule_screenshot = unsafe {
            std::mem::transmute(
                library
                    .symbol_address("stasis_host_schedule_screenshot")
                    .expect("resolve screenshot scheduler"),
            )
        };
        let get_submission_state = unsafe {
            std::mem::transmute(
                library
                    .symbol_address("stasis_test_get_render_submission_state")
                    .expect("resolve gated submission state"),
            )
        };
        let poison_physical_target = library
            .symbol_address("stasis_test_poison_physical_target")
            .ok()
            .map(|address| unsafe { std::mem::transmute(address) });
        let get_presentation_baseline_state = library
            .symbol_address("stasis_test_get_presentation_baseline_state")
            .ok()
            .map(|address| unsafe { std::mem::transmute(address) });
        let fail_next_text_preparation = library
            .symbol_address("stasis_test_fail_next_text_preparation")
            .ok()
            .map(|address| unsafe { std::mem::transmute(address) });
        let read_physical_target_pixel = library
            .symbol_address("stasis_test_read_physical_target_pixel")
            .ok()
            .map(|address| unsafe { std::mem::transmute(address) });
        let presentation_poison_target_kind = library
            .symbol_address("stasis_test_presentation_poison_target_kind")
            .ok()
            .map(|address| unsafe { std::mem::transmute(address) });
        let set_metrics_enabled = unsafe {
            std::mem::transmute(
                library
                    .symbol_address("stasis_host_set_performance_metrics_enabled")
                    .expect("resolve native performance-metrics request toggle"),
            )
        };
        let metrics_enabled = unsafe {
            std::mem::transmute(
                library
                    .symbol_address("stasis_host_performance_metrics_enabled")
                    .expect("resolve combined performance-metrics state"),
            )
        };
        let set_guest_metrics = unsafe {
            std::mem::transmute(
                library
                    .symbol_address("stasis_host_set_performance_metrics")
                    .expect("resolve guest timing handoff"),
            )
        };
        let get_latest_metrics = unsafe {
            std::mem::transmute(
                library
                    .symbol_address("stasis_host_get_latest_performance_metrics_v1")
                    .expect("resolve v1 performance-metrics snapshot"),
            )
        };
        let push_input_event = unsafe {
            std::mem::transmute(
                library
                    .symbol_address("stasis_test_push_input_event")
                    .expect("resolve gated native input event seam"),
            )
        };
        let shutdown = unsafe {
            std::mem::transmute(
                library
                    .symbol_address("stasis_shutdown")
                    .expect("resolve runtime shutdown"),
            )
        };
        Self {
            _library: library,
            schedule_screenshot,
            get_submission_state,
            get_presentation_baseline_state,
            poison_physical_target,
            fail_next_text_preparation,
            read_physical_target_pixel,
            presentation_poison_target_kind,
            set_metrics_enabled,
            metrics_enabled,
            set_guest_metrics,
            get_latest_metrics,
            push_input_event,
            shutdown,
        }
    }

    fn state(&self) -> [i32; 5] {
        let mut state = [0; 5];
        assert_eq!((self.get_submission_state)(state.as_mut_ptr(), 5), 1);
        state
    }

    fn screenshot(&self, path: &Path) {
        let path = CString::new(path.to_string_lossy().as_bytes()).expect("screenshot CString");
        assert_eq!((self.schedule_screenshot)(path.as_ptr()), 1);
    }

    fn poison_physical_target(&self) -> [i32; 40] {
        let poison = self
            .poison_physical_target
            .expect("runtime must be built with STASIS_TEST_PRESENTATION_POISON");
        let mut state = [0; 40];
        assert_eq!(poison(state.as_mut_ptr(), state.len() as i32), 1);
        state
    }

    fn presentation_baseline_state(&self) -> [i32; 40] {
        let read_state = self
            .get_presentation_baseline_state
            .expect("runtime must be built with STASIS_TEST_PRESENTATION_POISON");
        let mut state = [0; 40];
        assert_eq!(read_state(state.as_mut_ptr(), state.len() as i32), 1);
        state
    }

    fn fail_next_text_preparation(&self) {
        let fail = self
            .fail_next_text_preparation
            .expect("runtime must provide the test-only text preflight seam");
        assert_eq!(fail(), 1, "text preflight fault must be armed");
    }

    fn physical_target_pixel(&self) -> [i32; 4] {
        let read = self
            .read_physical_target_pixel
            .expect("runtime must provide the test-only physical pixel readback seam");
        let mut pixel = [0; 4];
        assert_eq!(read(pixel.as_mut_ptr(), pixel.len() as i32), 1);
        pixel
    }

    fn presentation_poison_target_kind(&self) -> i32 {
        let query = self
            .presentation_poison_target_kind
            .expect("runtime must provide the test-only poison target query");
        query()
    }

    fn metrics(&self) -> StasisPerformanceMetricsV1 {
        let mut metrics = std::mem::MaybeUninit::<StasisPerformanceMetricsV1>::zeroed();
        assert_eq!(
            (self.get_latest_metrics)(
                metrics.as_mut_ptr(),
                std::mem::size_of::<StasisPerformanceMetricsV1>(),
            ),
            1
        );
        unsafe { metrics.assume_init() }
    }

    fn push_f3(&self, down: bool) {
        assert_eq!(
            (self.push_input_event)(if down { 1 } else { 2 }, SDL_SCANCODE_F3, 0.0, 0.0,),
            1,
            "native F3 event must be accepted by the gated test seam"
        );
    }
}

impl Drop for NativeRecoveryHarness {
    fn drop(&mut self) {
        (self.shutdown)();
    }
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("canonical repository root")
}

fn evidence_root() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| repository_root().join("target"))
        .join("seam-tests")
}

fn presentation_poison_runtime_path() -> PathBuf {
    // The loader verifies shared build compatibility; this path selects the test-only binary variant.
    PathBuf::from(
        std::env::var_os("STASIS_PRESENTATION_POISON_RUNTIME_DLL_PATH")
            .expect("STASIS_PRESENTATION_POISON_RUNTIME_DLL_PATH must name the test-only poisoned runtime DLL"),
    )
}

fn valid_frame() -> (Vec<i32>, Vec<f32>, Vec<u8>) {
    let mut i32s = vec![0; STASIS_RENDER_I32_COUNT];
    let mut f32s = vec![0.0; STASIS_RENDER_F32_COUNT];
    let u8s = vec![0; STASIS_RENDER_U8_COUNT];
    i32s[0] = STASIS_RENDER_MAGIC;
    i32s[1] = STASIS_RENDER_VERSION;
    i32s[2] = FLAGS_CLEAR_PRESENT;
    i32s[22] = 1;
    i32s[24] = 1;
    i32s[STASIS_RENDER_ORDER_BASE] = ORDER_RECT;
    f32s[0..4].copy_from_slice(&[0.03, 0.06, 0.12, 1.0]);
    f32s[STASIS_RENDER_RECT_REVERSE_BASE_F32..STASIS_RENDER_RECT_REVERSE_BASE_F32 + 8]
        .copy_from_slice(&[80.0, 45.0, 160.0, 90.0, 0.9, 0.15, 0.08, 1.0]);
    (i32s, f32s, u8s)
}

fn red_pixels(image: &RgbaImage) -> usize {
    image
        .pixels()
        .filter(|pixel| pixel[0] > 170 && pixel[1] < 90 && pixel[2] < 80)
        .count()
}

fn assert_submission_delta(
    actual: [i32; 5],
    baseline: [i32; 5],
    accepted_delta: i32,
    rejected_delta: i32,
    presented_delta: i32,
    expected_validation: i32,
    label: &str,
) {
    assert_eq!(
        actual[0] - baseline[0],
        accepted_delta,
        "{label} accepted count"
    );
    assert_eq!(
        actual[1] - baseline[1],
        rejected_delta,
        "{label} rejected count"
    );
    assert_eq!(
        actual[2] - baseline[2],
        presented_delta,
        "{label} present count"
    );
    assert_eq!(actual[3], expected_validation, "{label} validation result");
}

#[test]
fn present_only_frames_capture_through_the_installed_runtime_abi() {
    let runtime_path = PathBuf::from(
        std::env::var_os("STASIS_RUNTIME_DLL_PATH")
            .expect("STASIS_RUNTIME_DLL_PATH must name the runtime DLL under test"),
    );
    std::env::set_var("STASIS_ENABLE_TEST_INPUT", "1");
    let gfx = StasisGraphicsApi::load(&runtime_path).expect("load graphics runtime");
    assert!(gfx
        .init_window(320, 180, "Stasis present-only release baseline")
        .expect("initialize native window"));
    let native = NativeRecoveryHarness::load(&runtime_path);
    let initial_state = native.state();

    let first_screenshot = evidence_root().join("present-only-first.png");
    fs::create_dir_all(first_screenshot.parent().expect("screenshot parent"))
        .expect("create evidence directory");
    let (mut painted_i32, painted_f32, painted_u8) = valid_frame();
    painted_i32[2] = FLAGS_PRESENT;
    native.screenshot(&first_screenshot);
    gfx.gfx_submit_u8(&mut painted_i32, &painted_f32, &painted_u8)
        .expect("submit present-only painted frame");
    let first_state = native.state();
    assert_submission_delta(
        first_state,
        initial_state,
        1,
        0,
        1,
        0,
        "first present-only frame",
    );
    assert_ne!(first_state[4], 0, "first valid frame must produce a trace");
    let first_image = image::open(&first_screenshot)
        .expect("open first present-only screenshot")
        .to_rgba8();
    let first_red_pixels = red_pixels(&first_image);
    assert!(
        first_red_pixels > (first_image.width() * first_image.height()) as usize / 8,
        "present-only frame must contain its red scene"
    );

    let second_screenshot = evidence_root().join("present-only-empty.png");
    let (mut empty_i32, empty_f32, empty_u8) = valid_frame();
    empty_i32[2] = FLAGS_PRESENT;
    empty_i32[22] = 0;
    empty_i32[24] = 0;
    native.screenshot(&second_screenshot);
    gfx.gfx_submit_u8(&mut empty_i32, &empty_f32, &empty_u8)
        .expect("submit empty present-only frame");
    let second_state = native.state();
    assert_submission_delta(
        second_state,
        first_state,
        1,
        0,
        1,
        0,
        "second present-only frame",
    );
    let second_image = image::open(&second_screenshot)
        .expect("open second present-only screenshot")
        .to_rgba8();

    let (mut unpublished_i32, unpublished_f32, unpublished_u8) = valid_frame();
    unpublished_i32[2] = 0;
    gfx.gfx_submit_u8(&mut unpublished_i32, &unpublished_f32, &unpublished_u8)
        .expect("submit valid construction without PRESENT");
    let unpublished_state = native.state();
    assert_submission_delta(
        unpublished_state,
        second_state,
        0,
        0,
        0,
        0,
        "valid construction without PRESENT",
    );
    assert_eq!(
        unpublished_state[4], second_state[4],
        "no-present must retain the last trace"
    );

    let evidence = json!({
        "schema": "stasis.present_only_release_baseline.v1",
        "runtime_path": runtime_path,
        "first_frame_flags": FLAGS_PRESENT,
        "first_frame_scene": "red rectangle, no CLEAR flag",
        "first_frame_submission": first_state,
        "first_frame": {
            "width": first_image.width(),
            "height": first_image.height(),
            "red_pixels": first_red_pixels,
            "upper_left_rgba": first_image.get_pixel(0, 0).0,
            "screenshot": first_screenshot,
        },
        "second_frame_flags": FLAGS_PRESENT,
        "second_frame_scene": "no draw commands, no CLEAR flag",
        "second_frame_submission": second_state,
        "second_frame": {
            "width": second_image.width(),
            "height": second_image.height(),
            "upper_left_rgba": second_image.get_pixel(0, 0).0,
            "screenshot": second_screenshot,
        },
        "no_present_submission_delta": [
            unpublished_state[0] - second_state[0],
            unpublished_state[1] - second_state[1],
            unpublished_state[2] - second_state[2],
        ],
        "physical_target_poisoned": false,
        "note": "This uses only the installed runtime ABI; output pixels are observational, not a physical-target poison assertion."
    });
    let evidence_path = evidence_root().join("present-only-release-baseline.json");
    fs::write(
        &evidence_path,
        serde_json::to_vec_pretty(&evidence).expect("serialize present-only baseline evidence"),
    )
    .expect("write present-only baseline evidence");
    eprintln!("Present-only release baseline evidence: {evidence}");
}

#[test]
fn present_only_frame_initializes_a_poisoned_physical_target() {
    let runtime_path = presentation_poison_runtime_path();
    std::env::set_var("STASIS_ENABLE_TEST_INPUT", "1");
    let gfx = StasisGraphicsApi::load(&runtime_path).expect("load graphics runtime");
    assert!(gfx
        .init_window(320, 180, "Stasis poisoned presentation target")
        .expect("initialize native window"));
    let native = NativeRecoveryHarness::load(&runtime_path);
    let initial_state = native.state();
    let poison_state = native.poison_physical_target();
    assert_eq!(
        &poison_state[..20],
        &poison_state[20..],
        "physical poison hook must restore logical presentation, viewport, clip, blend, and color"
    );

    let screenshot = evidence_root().join("present-only-physical-poison.png");
    fs::create_dir_all(screenshot.parent().expect("screenshot parent"))
        .expect("create evidence directory");
    native.screenshot(&screenshot);
    let (mut frame_i32, frame_f32, frame_u8) = valid_frame();
    frame_i32[2] = FLAGS_PRESENT;
    gfx.gfx_submit_u8(&mut frame_i32, &frame_f32, &frame_u8)
        .expect("submit no-CLEAR scene over poisoned target");
    let submission = native.state();
    assert_submission_delta(
        submission,
        initial_state,
        1,
        0,
        1,
        0,
        "poisoned target present",
    );
    let baseline_state = native.presentation_baseline_state();
    assert_eq!(
        &baseline_state[..20],
        &baseline_state[20..],
        "frame background clear must restore logical presentation, viewport, clip, blend, and color"
    );

    let image = image::open(&screenshot)
        .expect("open complete physical target capture")
        .to_rgba8();
    assert_eq!(image.width(), poison_state[0] as u32);
    assert_eq!(image.height(), poison_state[1] as u32);
    let scale = (image.width() as f32 / 320.0).min(image.height() as f32 / 180.0);
    let viewport_x = ((image.width() as f32 - 320.0 * scale) * 0.5).round() as u32;
    let viewport_y = ((image.height() as f32 - 180.0 * scale) * 0.5).round() as u32;
    let sample = |x: f32, y: f32| {
        let px = (viewport_x as f32 + x * scale).floor() as u32;
        let py = (viewport_y as f32 + y * scale).floor() as u32;
        image
            .get_pixel(px.min(image.width() - 1), py.min(image.height() - 1))
            .0
    };
    let physical_corner = image.get_pixel(0, 0).0;
    let logical_background = sample(20.0, 20.0);
    let scene_pixel = sample(100.0, 70.0);
    let red_pixel_count = red_pixels(&image);
    let evidence = json!({
        "schema": "stasis.presentation_physical_poison.v1",
        "runtime_path": runtime_path,
        "physical_target_poison": "opaque magenta SDL_RenderClear with logical presentation disabled",
        "poison_state_before_and_after": poison_state.to_vec(),
        "background_state_before_and_after": baseline_state.to_vec(),
        "logical_size": [320, 180],
        "scene": {"kind": "opaque red rectangle", "rect": [80, 45, 160, 90], "clear_flag": false},
        "submission": submission,
        "capture": {
            "width": image.width(),
            "height": image.height(),
            "physical_corner_rgba": physical_corner,
            "logical_background_rgba": logical_background,
            "scene_rgba": scene_pixel,
            "red_pixel_count": red_pixel_count,
            "screenshot": screenshot,
        }
    });
    let evidence_path = evidence_root().join("present-only-physical-poison.json");
    fs::write(
        &evidence_path,
        serde_json::to_vec_pretty(&evidence).expect("serialize poisoned target evidence"),
    )
    .expect("write poisoned target evidence");
    eprintln!("Present-only physical poison evidence: {evidence}");

    assert_eq!(
        physical_corner,
        [0, 0, 0, 255],
        "physical margins must be initialized to opaque black"
    );
    assert_eq!(
        logical_background,
        [0, 0, 0, 255],
        "untouched logical canvas pixels must be initialized to opaque black"
    );
    assert!(scene_pixel[0] > 170 && scene_pixel[1] < 90 && scene_pixel[2] < 80);
    assert!(red_pixel_count > (image.width() * image.height()) as usize / 80);
}

#[test]
fn failed_text_preparation_keeps_the_poisoned_target_unpublished() {
    let runtime_path = presentation_poison_runtime_path();
    std::env::set_var("STASIS_ENABLE_TEST_INPUT", "1");
    let gfx = StasisGraphicsApi::load(&runtime_path).expect("load graphics runtime");
    assert!(gfx
        .init_window(320, 180, "Stasis withheld text preparation")
        .expect("initialize native window"));
    let native = NativeRecoveryHarness::load(&runtime_path);
    let initial_state = native.state();
    let (mut baseline_i32, baseline_f32, baseline_u8) = valid_frame();
    baseline_i32[2] = FLAGS_PRESENT;
    gfx.gfx_submit_u8(&mut baseline_i32, &baseline_f32, &baseline_u8)
        .expect("submit accepted baseline frame");
    let baseline_state = native.state();
    assert_submission_delta(
        baseline_state,
        initial_state,
        1,
        0,
        1,
        0,
        "accepted baseline",
    );

    let font_path = repository_root().join("samples/render_parity/assets/parity.ttf");
    let font_handle = gfx
        .load_font(&font_path, 18)
        .expect("load text-preflight font");
    assert!(
        font_handle > 0,
        "font asset must produce a native font handle"
    );
    let poison_state = native.poison_physical_target();
    assert_eq!(
        native.presentation_poison_target_kind(),
        1,
        "poison must target the window"
    );
    let magenta_before = native.physical_target_pixel();
    assert_eq!(magenta_before, [255, 0, 255, 255]);

    native.fail_next_text_preparation();
    let (mut text_i32, text_f32, mut text_u8) = valid_frame();
    text_i32[2] = FLAGS_PRESENT;
    text_i32[7] = 1;
    text_i32[9] = 2;
    text_i32[22] = 2;
    text_i32[STASIS_RENDER_ORDER_BASE + 1] = ORDER_TEXT;
    text_i32[STASIS_RENDER_TEXT_BASE_I32] = font_handle;
    text_i32[STASIS_RENDER_TEXT_BASE_I32 + 1] = 0;
    text_i32[STASIS_RENDER_TEXT_BASE_I32 + 2] = 1;
    text_u8[0] = b'A';
    text_u8[1] = 0;
    gfx.gfx_submit_u8(&mut text_i32, &text_f32, &text_u8)
        .expect("submit frame whose text preflight is rejected");

    let failed_state = native.state();
    assert_submission_delta(
        failed_state,
        baseline_state,
        1,
        0,
        0,
        0,
        "text preparation failure",
    );
    let magenta_after = native.physical_target_pixel();
    assert_eq!(
        magenta_after, magenta_before,
        "text preflight failure must return before clearing or publishing the physical target"
    );

    let evidence = json!({
        "schema": "stasis.presentation_text_preflight_withheld.v1",
        "runtime_path": runtime_path,
        "window_poison_kind": native.presentation_poison_target_kind(),
        "poison_state_before_and_after": poison_state.to_vec(),
        "font_path": font_path,
        "font_handle": font_handle,
        "fault": "test-only failure injected after successful font/resource preparation",
        "baseline_submission": baseline_state,
        "withheld_submission": failed_state,
        "presented_count_delta": failed_state[2] - baseline_state[2],
        "physical_target_pixel_before": magenta_before,
        "physical_target_pixel_after": magenta_after,
    });
    let evidence_path = evidence_root().join("text-preparation-withheld-present.json");
    fs::create_dir_all(evidence_path.parent().expect("evidence parent"))
        .expect("create evidence directory");
    fs::write(
        &evidence_path,
        serde_json::to_vec_pretty(&evidence).expect("serialize text-preflight evidence"),
    )
    .expect("write text-preflight evidence");
    eprintln!("Text-preflight withholding evidence: {evidence}");
}

#[test]
fn malformed_frames_are_rejected_without_poisoning_the_next_valid_frame() {
    let runtime_path = PathBuf::from(
        std::env::var_os("STASIS_RUNTIME_DLL_PATH")
            .expect("STASIS_RUNTIME_DLL_PATH must name the CI-built SDL runtime"),
    );
    std::env::set_var("STASIS_ENABLE_TEST_INPUT", "1");
    let gfx = StasisGraphicsApi::load(&runtime_path).expect("load graphics runtime");
    assert!(gfx
        .init_window(320, 180, "Stasis IT-009 render recovery seam")
        .expect("initialize native window"));
    let native = NativeRecoveryHarness::load(&runtime_path);
    let initial_state = native.state();
    let (mut valid_i32, valid_f32, valid_u8) = valid_frame();

    gfx.gfx_submit_u8(&mut valid_i32, &valid_f32, &valid_u8)
        .expect("submit initial valid frame");
    let initial = native.state();
    assert_submission_delta(initial, initial_state, 1, 0, 1, 0, "initial recovery frame");
    assert_ne!(
        initial[4], 0,
        "valid current frame must produce a native trace"
    );

    let mut rejection_evidence = Vec::new();
    for (name, expected_validation, mutate) in [
        ("bad_magic", 3, 0),
        ("bad_version", 4, 1),
        ("negative_count", 5, 2),
        ("excessive_count", 6, 3),
        ("bad_text_span", 7, 4),
        ("bad_order_reference", 8, 5),
    ] {
        let (mut bad_i32, bad_f32, bad_u8) = valid_frame();
        match mutate {
            0 => bad_i32[0] = 0,
            1 => bad_i32[1] = 99,
            2 => bad_i32[3] = -1,
            3 => bad_i32[4] = STASIS_RENDER_MAX_SPRITES as i32 + 1,
            4 => {
                bad_i32[7] = 1;
                bad_i32[9] = 2;
                bad_i32[STASIS_RENDER_TEXT_BASE_I32] = 1;
                bad_i32[STASIS_RENDER_TEXT_BASE_I32 + 1] = 1;
                bad_i32[STASIS_RENDER_TEXT_BASE_I32 + 2] = 1;
            }
            5 => bad_i32[STASIS_RENDER_ORDER_BASE] = 3 * 16_384,
            _ => unreachable!(),
        }
        gfx.gfx_submit_u8(&mut bad_i32, &bad_f32, &bad_u8)
            .expect("submit malformed frame");
        let state = native.state();
        assert_submission_delta(
            state,
            initial,
            0,
            rejection_evidence.len() as i32 + 1,
            0,
            expected_validation,
            name,
        );
        assert_eq!(
            state[4], initial[4],
            "{name} must preserve last valid trace"
        );
        rejection_evidence.push(json!({"case": name, "validation": expected_validation}));
    }

    let screenshot = evidence_root().join("it-009-recovered-valid-frame.png");
    fs::create_dir_all(screenshot.parent().expect("screenshot parent"))
        .expect("create evidence directory");
    native.screenshot(&screenshot);
    let (mut final_i32, final_f32, final_u8) = valid_frame();
    gfx.gfx_submit_u8(&mut final_i32, &final_f32, &final_u8)
        .expect("submit final valid frame");
    let final_state = native.state();
    assert_submission_delta(
        final_state,
        initial,
        1,
        rejection_evidence.len() as i32,
        1,
        0,
        "recovered valid frame",
    );
    assert_eq!(final_state[4], initial[4], "final valid trace recovery");
    let image = image::open(&screenshot)
        .expect("open recovered frame screenshot")
        .to_rgba8();
    let recovered_red_pixels = red_pixels(&image);
    assert!(
        recovered_red_pixels > (image.width() * image.height()) as usize / 8,
        "recovered frame must contain its red center region"
    );

    (native.set_metrics_enabled)(1);
    (native.set_guest_metrics)(111, 222);
    let (mut seeded_i32, seeded_f32, seeded_u8) = valid_frame();
    gfx.gfx_submit_u8(&mut seeded_i32, &seeded_f32, &seeded_u8)
        .expect("submit seeded metrics frame");
    let seeded_metrics = native.metrics();
    assert_eq!(seeded_metrics.tick_us, 111);
    assert_eq!(seeded_metrics.guest_render_us, 222);

    (native.set_metrics_enabled)(0);
    (native.set_guest_metrics)(999, 888);
    (native.set_metrics_enabled)(1);
    let rearmed_metrics = native.metrics();
    assert_eq!(rearmed_metrics.frame_work_us, STASIS_PERF_UNAVAILABLE);
    let (mut stale_i32, stale_f32, stale_u8) = valid_frame();
    gfx.gfx_submit_u8(&mut stale_i32, &stale_f32, &stale_u8)
        .expect("submit without a current guest timing handoff");
    let stale_metrics = native.metrics();
    assert_eq!(
        stale_metrics.tick_us, 111,
        "late re-arm exposes prior tick data"
    );
    assert_eq!(
        stale_metrics.guest_render_us, 222,
        "late re-arm exposes prior guest-render data"
    );

    (native.set_metrics_enabled)(0);
    (native.set_guest_metrics)(777, 666);
    (native.set_metrics_enabled)(1);
    let current_rearm_metrics = native.metrics();
    assert_eq!(current_rearm_metrics.frame_work_us, STASIS_PERF_UNAVAILABLE);
    (native.set_guest_metrics)(333, 444);
    let (mut current_i32, current_f32, current_u8) = valid_frame();
    gfx.gfx_submit_u8(&mut current_i32, &current_f32, &current_u8)
        .expect("submit current metrics frame after re-arm");
    let current_metrics = native.metrics();
    assert_eq!(current_metrics.tick_us, 333);
    assert_eq!(current_metrics.guest_render_us, 444);

    let mut host_i32 = vec![0; 768];
    let mut host_f32 = vec![0.0; 64];
    native.push_f3(true);
    gfx.host_get_frame(&mut host_i32, &mut host_f32)
        .expect("turn performance HUD on");
    let (mut hud_on_i32, hud_on_f32, hud_on_u8) = valid_frame();
    gfx.gfx_submit_u8(&mut hud_on_i32, &hud_on_f32, &hud_on_u8)
        .expect("complete the HUD-on input frame");
    assert_eq!(
        (native.metrics_enabled)(),
        1,
        "visible HUD keeps metrics active"
    );
    (native.set_metrics_enabled)(0);
    assert_eq!(
        (native.metrics_enabled)(),
        1,
        "clearing the profile request leaves the HUD's independent request active"
    );
    (native.set_metrics_enabled)(1);
    native.push_f3(false);
    gfx.host_get_frame(&mut host_i32, &mut host_f32)
        .expect("release F3 key");
    let (mut hud_keyup_i32, hud_keyup_f32, hud_keyup_u8) = valid_frame();
    gfx.gfx_submit_u8(&mut hud_keyup_i32, &hud_keyup_f32, &hud_keyup_u8)
        .expect("complete the F3 key-up input frame");
    native.push_f3(true);
    gfx.host_get_frame(&mut host_i32, &mut host_f32)
        .expect("turn performance HUD off while profile guard is active");
    assert_eq!((native.metrics_enabled)(), 0);
    (native.set_metrics_enabled)(0);
    assert_eq!(
        (native.metrics_enabled)(),
        0,
        "clearing the profile request after HUD-off must stop hidden collection"
    );

    let evidence = json!({
        "schema": "stasis.seam_test.v1",
        "test_id": "IT-009",
        "status": "passed",
        "target": "windows-sdl-native",
        "initial_valid": initial,
        "rejections": rejection_evidence,
        "final_valid": final_state,
        "performance_metrics": {
            "seeded_tick_us": seeded_metrics.tick_us,
            "seeded_guest_render_us": seeded_metrics.guest_render_us,
            "late_rearm_stale_tick_us": stale_metrics.tick_us,
            "late_rearm_stale_guest_render_us": stale_metrics.guest_render_us,
            "current_tick_us": current_metrics.tick_us,
            "current_guest_render_us": current_metrics.guest_render_us,
            "rearm_reset_frame_work_unavailable": rearmed_metrics.frame_work_us == STASIS_PERF_UNAVAILABLE
                && current_rearm_metrics.frame_work_us == STASIS_PERF_UNAVAILABLE,
            "hud_on_keeps_collection_after_profile_clear": true,
            "hud_off_profile_clear_stops_hidden_collection": true
        },
        "oracle": {"recovered_red_pixels": recovered_red_pixels, "screenshot": screenshot}
    });
    let evidence_path = evidence_root().join("it-009-render-recovery.json");
    fs::write(
        evidence_path,
        serde_json::to_vec_pretty(&evidence).expect("serialize seam evidence"),
    )
    .expect("write seam evidence");
    eprintln!("IT-009 evidence: {evidence}");
}
