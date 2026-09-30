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
const ORDER_RECT: i32 = 4 * 16_384;

type ScheduleScreenshot = extern "system" fn(*const std::ffi::c_char) -> i32;
type GetSubmissionState = extern "system" fn(*mut i32, i32) -> i32;
type SetMetricsEnabled = extern "system" fn(i32);
type MetricsEnabled = extern "system" fn() -> i32;
type SetGuestMetrics = extern "system" fn(u64, u64);
type GetLatestMetrics = extern "system" fn(*mut StasisPerformanceMetricsV1, usize) -> i32;
type PushInputEvent = extern "system" fn(i32, i32, f32, f32) -> i32;

const SDL_SCANCODE_F3: i32 = 60;

struct NativeRecoveryHarness {
    _library: Library,
    schedule_screenshot: ScheduleScreenshot,
    get_submission_state: GetSubmissionState,
    set_metrics_enabled: SetMetricsEnabled,
    metrics_enabled: MetricsEnabled,
    set_guest_metrics: SetGuestMetrics,
    get_latest_metrics: GetLatestMetrics,
    push_input_event: PushInputEvent,
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
        Self {
            _library: library,
            schedule_screenshot,
            get_submission_state,
            set_metrics_enabled,
            metrics_enabled,
            set_guest_metrics,
            get_latest_metrics,
            push_input_event,
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
    let (mut valid_i32, valid_f32, valid_u8) = valid_frame();

    gfx.gfx_submit_u8(&mut valid_i32, &valid_f32, &valid_u8)
        .expect("submit initial valid frame");
    let initial = native.state();
    assert_eq!(&initial[0..4], &[1, 0, 1, 0]);
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
        assert_eq!(state[0], 1, "{name} must not be accepted");
        assert_eq!(state[1], rejection_evidence.len() as i32 + 1);
        assert_eq!(state[2], 1, "{name} must not present");
        assert_eq!(state[3], expected_validation, "{name} diagnostic");
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
    assert_eq!(&final_state[0..4], &[2, 6, 2, 0]);
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
