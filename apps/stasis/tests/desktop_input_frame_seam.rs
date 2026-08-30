#![cfg(windows)]

use serde_json::json;
use stasis_compiler::backend::jit::JitProcess;
use stasis_dynload::{
    global_path_hash, register_global_f32_array, register_global_i32_array,
    register_global_u8_array, Library, StasisGraphicsApi, STASIS_RENDER_F32_COUNT,
    STASIS_RENDER_I32_COUNT, STASIS_RENDER_U8_COUNT,
};
use std::fs;
use std::path::{Path, PathBuf};

const FIXTURE: &str = include_str!("../../../tests/stasis/seams/desktop_input_frame_probe.stasis");
const KEY_SPACE: i32 = 44;
const EVENT_KEY_DOWN: i32 = 1;
const EVENT_KEY_UP: i32 = 2;
const EVENT_POINTER_DOWN: i32 = 3;
const EVENT_POINTER_MOVE: i32 = 4;
const EVENT_POINTER_UP: i32 = 5;
const TOUCH_ID: i32 = 73;

struct NativeInputInjector {
    _library: Library,
    push: extern "system" fn(i32, i32, f32, f32) -> i32,
}

impl NativeInputInjector {
    fn load(path: &Path) -> Self {
        let library = Library::load(path).expect("load graphics runtime for input injection");
        let address = library
            .symbol_address("stasis_test_push_input_event")
            .expect("graphics runtime must expose the gated SDL input test seam");
        let push = unsafe { std::mem::transmute(address) };
        Self {
            _library: library,
            push,
        }
    }

    fn event(&self, kind: i32, code: i32, x: f32, y: f32) {
        assert_eq!(
            (self.push)(kind, code, x, y),
            1,
            "native SDL input injection failed: kind={kind} code={code} x={x} y={y}"
        );
    }
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("canonical repository root")
}

fn evidence_path() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| repository_root().join("target"))
        .join("seam-tests/it-006-desktop-input-frame.json")
}

fn scalar_i32(path: &str) -> i32 {
    stasis_dynload::stasis_jit_global_i32_load(global_path_hash(path))
}

fn scalar_f32(path: &str) -> f32 {
    stasis_dynload::stasis_jit_global_f32_load(global_path_hash(path))
}

fn assert_close(actual: f32, expected: f32, field: &str) {
    assert!(
        (actual - expected).abs() <= 0.01,
        "{field} mismatch: expected={expected} actual={actual}"
    );
}

fn run_guest_frame(
    gfx: &StasisGraphicsApi,
    jit: &mut JitProcess,
    host_i32: &mut [i32],
    host_f32: &mut [f32],
    gfx_i32: &mut [i32],
    gfx_f32: &[f32],
    gfx_u8: &[u8],
    phase: i32,
    checksum: i32,
    x: f32,
    y: f32,
    clear: [f32; 4],
) -> i32 {
    gfx.host_get_frame(host_i32, host_f32)
        .expect("snapshot native HostFrame");
    eprintln!(
        "IT-006 snapshot phase={phase} key={} pointers={} slot1={:?} xy=({},{}) dxy=({},{})",
        host_i32[32 + KEY_SPACE as usize],
        host_i32[7],
        &host_i32[548..552],
        host_f32[6],
        host_f32[7],
        host_f32[8],
        host_f32[9]
    );
    assert_eq!(
        jit.execute_i32_noarg_by_name("tick"),
        Ok(0),
        "execute guest tick"
    );
    assert_eq!(
        jit.execute_i32_noarg_by_name("render"),
        Ok(0),
        "execute guest render"
    );

    assert_eq!(scalar_i32("seam_phase"), phase, "guest phase");
    assert_eq!(
        scalar_i32("seam_transition_count"),
        phase,
        "each native edge must mutate guest state exactly once"
    );
    assert_eq!(scalar_i32("seam_state_checksum"), checksum);
    assert_close(scalar_f32("seam_pointer_x"), x, "guest pointer x");
    assert_close(scalar_f32("seam_pointer_y"), y, "guest pointer y");

    assert_eq!(&gfx_i32[0..4], &[1196967473, 6, 3, 0]);
    assert_eq!(gfx_i32[22], 1, "render order count");
    assert_eq!(gfx_i32[24], 1, "render rectangle count");
    for (index, expected) in clear.iter().enumerate() {
        assert_close(gfx_f32[index], *expected, "clear color");
    }
    assert_close(gfx_f32[79996], x, "render marker x");
    assert_close(gfx_f32[79997], y, "render marker y");

    let trace = unsafe {
        stasis_dynload::stasis_jit_render_v2_trace(
            global_path_hash("gfx_cmd_i32"),
            gfx_i32.len() as i32,
            global_path_hash("gfx_cmd_f32"),
            gfx_f32.len() as i32,
            global_path_hash("gfx_cmd_u8"),
            gfx_u8.len() as i32,
        )
    };
    assert_ne!(trace, 0, "native render trace must accept the guest frame");
    gfx.gfx_submit_u8(gfx_i32, gfx_f32, gfx_u8)
        .expect("submit guest command buffers to native graphics host");
    trace
}

#[test]
fn desktop_sdl_input_changes_jit_state_and_submitted_frame_on_the_intended_tick() {
    let runtime_path = PathBuf::from(
        std::env::var_os("STASIS_RUNTIME_DLL_PATH")
            .expect("STASIS_RUNTIME_DLL_PATH must name the CI-built SDL runtime"),
    );
    std::env::set_var("STASIS_ENABLE_TEST_INPUT", "1");
    std::env::set_var("STASIS_USE_SDL", "1");

    let gfx = StasisGraphicsApi::load(&runtime_path).expect("load graphics runtime");
    assert!(gfx
        .init_window(320, 180, "Stasis IT-006 desktop input seam")
        .expect("initialize native window"));
    let injector = NativeInputInjector::load(&runtime_path);

    let mut host_i32 = vec![0; 768];
    let mut host_f32 = vec![0.0; 64];
    // These host-owned buffers must stay exactly aligned with the canonical
    // renderer ABI.  In particular, V6 adds the clip arena to both command
    // lanes; using a pre-V6 capacity makes registration fail before the seam
    // can exercise input behavior.
    let mut gfx_i32 = vec![0; STASIS_RENDER_I32_COUNT];
    let mut gfx_f32 = vec![0.0; STASIS_RENDER_F32_COUNT];
    let mut gfx_u8 = vec![0; STASIS_RENDER_U8_COUNT];
    assert_eq!(gfx_i32.len(), STASIS_RENDER_I32_COUNT);
    assert_eq!(gfx_f32.len(), STASIS_RENDER_F32_COUNT);
    assert_eq!(gfx_u8.len(), STASIS_RENDER_U8_COUNT);
    register_global_i32_array(
        global_path_hash("host_i32"),
        0,
        host_i32.as_mut_ptr(),
        host_i32.len(),
    );
    register_global_f32_array(
        global_path_hash("host_f32"),
        0,
        host_f32.as_mut_ptr(),
        host_f32.len(),
    );
    register_global_i32_array(
        global_path_hash("gfx_cmd_i32"),
        0,
        gfx_i32.as_mut_ptr(),
        gfx_i32.len(),
    );
    register_global_f32_array(
        global_path_hash("gfx_cmd_f32"),
        0,
        gfx_f32.as_mut_ptr(),
        gfx_f32.len(),
    );
    register_global_u8_array(
        global_path_hash("gfx_cmd_u8"),
        0,
        gfx_u8.as_mut_ptr(),
        gfx_u8.len(),
    );

    let mut jit = JitProcess::new();
    jit.set_project_root(repository_root().to_string_lossy())
        .expect("set fixture project root");
    jit.upsert_file(
        "tests/stasis/seams/desktop_input_frame_probe.stasis",
        FIXTURE,
    );
    jit.compile().expect("compile desktop input fixture");
    assert_eq!(jit.execute_i32_noarg_by_name("main"), Ok(0));

    injector.event(EVENT_KEY_DOWN, KEY_SPACE, 0.0, 0.0);
    injector.event(EVENT_POINTER_DOWN, TOUCH_ID, 80.0, 45.0);
    let down_trace = run_guest_frame(
        &gfx,
        &mut jit,
        &mut host_i32,
        &mut host_f32,
        &mut gfx_i32,
        &gfx_f32,
        &gfx_u8,
        1,
        108045,
        80.0,
        45.0,
        [0.8, 0.1, 0.1, 1.0],
    );
    assert_eq!(host_i32[32 + KEY_SPACE as usize], 1);
    assert_eq!(host_i32[7], 2);
    assert_eq!(&host_i32[548..552], &[1, 1, 1, 0]);
    assert_close(host_f32[6], 80.0, "HostFrame down x");
    assert_close(host_f32[7], 45.0, "HostFrame down y");
    assert_close(host_f32[10], 0.25, "HostFrame down normalized x");
    assert_close(host_f32[11], 0.25, "HostFrame down normalized y");

    injector.event(EVENT_KEY_DOWN, KEY_SPACE, 0.0, 0.0);
    injector.event(EVENT_POINTER_MOVE, TOUCH_ID, 120.0, 70.0);
    let move_trace = run_guest_frame(
        &gfx,
        &mut jit,
        &mut host_i32,
        &mut host_f32,
        &mut gfx_i32,
        &gfx_f32,
        &gfx_u8,
        2,
        212070,
        120.0,
        70.0,
        [0.1, 0.8, 0.1, 1.0],
    );
    assert_eq!(host_i32[32 + KEY_SPACE as usize], 1);
    assert_eq!(&host_i32[548..552], &[1, 1, 0, 0]);
    assert_close(host_f32[8], 40.0, "HostFrame move dx");
    assert_close(host_f32[9], 25.0, "HostFrame move dy");
    assert_close(host_f32[10], 0.375, "HostFrame move normalized x");
    assert_close(host_f32[11], 70.0 / 180.0, "HostFrame move normalized y");

    injector.event(EVENT_KEY_UP, KEY_SPACE, 0.0, 0.0);
    injector.event(EVENT_POINTER_UP, TOUCH_ID, 120.0, 70.0);
    let up_trace = run_guest_frame(
        &gfx,
        &mut jit,
        &mut host_i32,
        &mut host_f32,
        &mut gfx_i32,
        &gfx_f32,
        &gfx_u8,
        3,
        312070,
        120.0,
        70.0,
        [0.1, 0.1, 0.8, 1.0],
    );
    assert_eq!(host_i32[32 + KEY_SPACE as usize], 0);
    assert_eq!(&host_i32[548..552], &[1, 0, 0, 1]);
    assert_eq!(
        [down_trace, move_trace, up_trace],
        [1845463013, -947354335, -119375539],
        "guest state markers must produce the locked native frame traces"
    );

    gfx.host_get_frame(&mut host_i32, &mut host_f32)
        .expect("snapshot quiet frame");
    assert_eq!(
        host_i32[7], 1,
        "released touch must leave the next snapshot"
    );
    assert_eq!(scalar_i32("seam_transition_count"), 3);

    let evidence = json!({
        "schema": "stasis.seam_test.v1",
        "test_id": "IT-006",
        "status": "passed",
        "target": "windows-sdl-jit",
        "input": {
            "keyboard_scancode": KEY_SPACE,
            "pointer_slot": 1,
            "ticks": [
                {"tick": 1, "event": "down", "x": 80.0, "y": 45.0},
                {"tick": 2, "event": "move", "x": 120.0, "y": 70.0, "dx": 40.0, "dy": 25.0},
                {"tick": 3, "event": "up", "x": 120.0, "y": 70.0}
            ]
        },
        "oracle": {
            "state_checksums": [108045, 212070, 312070],
            "transition_count": 3,
            "render_traces": [down_trace, move_trace, up_trace],
            "quiet_pointer_count": 1
        }
    });
    let path = evidence_path();
    fs::create_dir_all(path.parent().expect("evidence directory"))
        .expect("create evidence directory");
    fs::write(
        &path,
        serde_json::to_vec_pretty(&evidence).expect("serialize evidence"),
    )
    .expect("write evidence");
    eprintln!("IT-006 evidence: {}", evidence);
}
