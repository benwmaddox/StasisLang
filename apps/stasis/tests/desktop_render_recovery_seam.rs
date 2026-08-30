#![cfg(windows)]

use image::RgbaImage;
use serde_json::json;
use stasis_dynload::{
    Library, StasisGraphicsApi, STASIS_RENDER_F32_COUNT, STASIS_RENDER_I32_COUNT,
    STASIS_RENDER_MAGIC, STASIS_RENDER_MAX_SPRITES, STASIS_RENDER_ORDER_BASE,
    STASIS_RENDER_RECT_REVERSE_BASE_F32, STASIS_RENDER_TEXT_BASE_I32, STASIS_RENDER_U8_COUNT,
    STASIS_RENDER_VERSION,
};
use std::ffi::CString;
use std::fs;
use std::path::{Path, PathBuf};

const FLAGS_CLEAR_PRESENT: i32 = 3;
const ORDER_RECT: i32 = 4 * 16_384;

type ScheduleScreenshot = extern "system" fn(*const std::ffi::c_char) -> i32;
type GetSubmissionState = extern "system" fn(*mut i32, i32) -> i32;

struct NativeRecoveryHarness {
    _library: Library,
    schedule_screenshot: ScheduleScreenshot,
    get_submission_state: GetSubmissionState,
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
        Self {
            _library: library,
            schedule_screenshot,
            get_submission_state,
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
    std::env::set_var("STASIS_USE_SDL", "1");
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

    let evidence = json!({
        "schema": "stasis.seam_test.v1",
        "test_id": "IT-009",
        "status": "passed",
        "target": "windows-sdl-native",
        "initial_valid": initial,
        "rejections": rejection_evidence,
        "final_valid": final_state,
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
