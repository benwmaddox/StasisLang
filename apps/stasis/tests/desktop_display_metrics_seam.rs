#![cfg(windows)]

use serde_json::json;
use stasis_compiler::backend::jit::JitProcess;
use stasis_dynload::{
    global_path_hash, register_global_f32_array, register_global_i32_array,
    register_global_u8_array, Library, StasisGraphicsApi,
};
use std::fs;
use std::path::{Path, PathBuf};

const FIXTURE: &str =
    include_str!("../../../tests/stasis/seams/desktop_display_metrics_probe.stasis");
const DISPLAY_CHANGED: i32 = 1;
const WINDOW_MINIMIZED: i32 = 2;
const WINDOW_RESTORED: i32 = 3;
const POINTER_DOWN: i32 = 3;
const POINTER_MOVE: i32 = 4;
const TOUCH_ID: i32 = 91;
const ODD_TRACE: i32 = -873_412_109;
const MINIMIZED_TRACE: i32 = 1_700_295_147;
const RESTORED_TRACE: i32 = -683_859_671;

#[derive(Clone, Copy)]
struct DisplaySample {
    logical: [i32; 2],
    native: [i32; 2],
    drawable: [i32; 2],
    safe_native: [i32; 4],
    safe_logical: [f32; 4],
    safe_rounded: [i32; 4],
    content_scale: f32,
    raster_scale: f32,
    display_generation: i32,
    density_generation: i32,
}

const ODD_FRACTIONAL: DisplaySample = DisplaySample {
    logical: [400, 300],
    native: [601, 451],
    drawable: [903, 677],
    safe_native: [50, 25, 501, 401],
    safe_logical: [33.27787, 16.62971, 333.44427, 266.74057],
    safe_rounded: [33, 16, 334, 267],
    content_scale: 2.2566667,
    raster_scale: 2.2566667,
    display_generation: 2,
    density_generation: 2,
};

const RESTORED_SCALE: DisplaySample = DisplaySample {
    logical: [400, 300],
    native: [801, 601],
    drawable: [1603, 1201],
    safe_native: [0, 0, 801, 601],
    safe_logical: [0.0, 0.0, 400.0, 300.0],
    safe_rounded: [0, 0, 400, 300],
    content_scale: 4.0025,
    raster_scale: 4.0025,
    display_generation: 3,
    density_generation: 3,
};

type PushDisplay = extern "system" fn(i32, i32, i32, i32, i32, i32, i32, i32, i32, i32, i32) -> i32;

struct NativeSurfaceHarness {
    _library: Library,
    push_display: PushDisplay,
    push_input: extern "system" fn(i32, i32, f32, f32) -> i32,
    get_lifecycle: extern "system" fn(*mut i32, i32) -> i32,
}

impl NativeSurfaceHarness {
    fn load(path: &Path) -> Self {
        let library = Library::load(path).expect("load graphics runtime for surface events");
        let push_display = unsafe {
            std::mem::transmute(
                library
                    .symbol_address("stasis_test_push_display_event")
                    .expect("resolve gated display-event seam"),
            )
        };
        let push_input = unsafe {
            std::mem::transmute(
                library
                    .symbol_address("stasis_test_push_input_event")
                    .expect("resolve gated input-event seam"),
            )
        };
        let get_lifecycle = unsafe {
            std::mem::transmute(
                library
                    .symbol_address("stasis_gfx_get_resource_lifecycle")
                    .expect("resolve renderer lifecycle snapshot"),
            )
        };
        Self {
            _library: library,
            push_display,
            push_input,
            get_lifecycle,
        }
    }

    fn display(&self, kind: i32, sample: DisplaySample) {
        assert_eq!(
            (self.push_display)(
                kind,
                sample.logical[0],
                sample.logical[1],
                sample.native[0],
                sample.native[1],
                sample.drawable[0],
                sample.drawable[1],
                sample.safe_native[0],
                sample.safe_native[1],
                sample.safe_native[2],
                sample.safe_native[3],
            ),
            1,
            "native display event injection failed"
        );
    }

    fn pointer(&self, kind: i32, x: f32, y: f32) {
        assert_eq!(
            (self.push_input)(kind, TOUCH_ID, x, y),
            1,
            "native pointer event injection failed"
        );
    }

    fn lifecycle(&self) -> [i32; 6] {
        let mut values = [0; 6];
        assert_eq!((self.get_lifecycle)(values.as_mut_ptr(), 6), 1);
        values
    }
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("canonical repository root")
}

fn scalar_i32(path: &str) -> i32 {
    stasis_dynload::stasis_jit_global_i32_load(global_path_hash(path))
}

fn scalar_f32(path: &str) -> f32 {
    stasis_dynload::stasis_jit_global_f32_load(global_path_hash(path))
}

fn assert_close(actual: f32, expected: f32, field: &str) {
    assert!(
        (actual - expected).abs() <= 0.002,
        "{field} mismatch: expected={expected} actual={actual}"
    );
}

fn assert_metrics(
    sample: DisplaySample,
    minimized: bool,
    resized: bool,
    host_i32: &[i32],
    host_f32: &[f32],
) {
    assert_eq!(host_i32[11], i32::from(resized), "HostFrame resized");
    assert_eq!(host_i32[18], i32::from(minimized), "HostFrame minimized");
    assert_eq!(
        &host_i32[22..26],
        &[
            sample.native[0],
            sample.native[1],
            sample.drawable[0],
            sample.drawable[1]
        ]
    );
    assert_eq!(host_i32[30], sample.display_generation);
    assert_eq!(host_i32[31], sample.density_generation);
    assert_close(host_f32[50], sample.logical[0] as f32, "logical width");
    assert_close(host_f32[51], sample.logical[1] as f32, "logical height");
    for (index, expected) in sample.safe_logical.iter().enumerate() {
        assert_close(host_f32[52 + index], *expected, "safe logical viewport");
    }
    assert_close(host_f32[48], sample.content_scale, "content scale");
    assert_close(host_f32[49], sample.raster_scale, "raster scale");

    assert_eq!(scalar_i32("metric_resized"), i32::from(resized));
    assert_eq!(scalar_i32("metric_minimized"), i32::from(minimized));
    assert_eq!(scalar_i32("metric_native_w"), sample.native[0]);
    assert_eq!(scalar_i32("metric_native_h"), sample.native[1]);
    assert_eq!(scalar_i32("metric_drawable_w"), sample.drawable[0]);
    assert_eq!(scalar_i32("metric_drawable_h"), sample.drawable[1]);
    assert_eq!(
        scalar_i32("metric_display_generation"),
        sample.display_generation
    );
    assert_eq!(
        scalar_i32("metric_density_generation"),
        sample.density_generation
    );
    assert_close(
        scalar_f32("metric_logical_w"),
        sample.logical[0] as f32,
        "guest logical width",
    );
    assert_close(
        scalar_f32("metric_logical_h"),
        sample.logical[1] as f32,
        "guest logical height",
    );
    for (field, expected) in [
        ("metric_safe_x", sample.safe_logical[0]),
        ("metric_safe_y", sample.safe_logical[1]),
        ("metric_safe_w", sample.safe_logical[2]),
        ("metric_safe_h", sample.safe_logical[3]),
        ("metric_content_scale", sample.content_scale),
        ("metric_raster_scale", sample.raster_scale),
    ] {
        assert_close(scalar_f32(field), expected, field);
    }
}

fn run_frame(
    gfx: &StasisGraphicsApi,
    jit: &mut JitProcess,
    host_i32: &mut [i32],
    host_f32: &mut [f32],
    gfx_i32: &mut [i32],
    gfx_f32: &[f32],
    gfx_u8: &[u8],
    sample: DisplaySample,
    minimized: bool,
    resized: bool,
) -> i32 {
    gfx.host_get_frame(host_i32, host_f32)
        .expect("snapshot native HostFrame");
    assert_eq!(jit.execute_i32_noarg_by_name("tick"), Ok(0));
    assert_metrics(sample, minimized, resized, host_i32, host_f32);
    assert_eq!(jit.execute_i32_noarg_by_name("render"), Ok(0));
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
    gfx.gfx_submit_u8(gfx_i32, gfx_f32, gfx_u8)
        .expect("submit guest frame through native renderer");
    assert_eq!(
        &gfx_i32[10..16],
        &[
            sample.logical[0],
            sample.logical[1],
            sample.native[0],
            sample.native[1],
            sample.drawable[0],
            sample.drawable[1]
        ]
    );
    assert_eq!(&gfx_i32[16..20], &sample.safe_rounded);
    assert_eq!(gfx_i32[20], sample.display_generation);
    assert_eq!(gfx_i32[21], sample.density_generation);
    trace
}

#[test]
fn desktop_surface_metrics_reach_stasis_and_renderer_in_one_generation() {
    let runtime_path = PathBuf::from(
        std::env::var_os("STASIS_RUNTIME_DLL_PATH")
            .expect("STASIS_RUNTIME_DLL_PATH must name the CI-built SDL runtime"),
    );
    std::env::set_var("STASIS_ENABLE_TEST_INPUT", "1");
    std::env::set_var("STASIS_USE_SDL", "1");

    let gfx = StasisGraphicsApi::load(&runtime_path).expect("load graphics runtime");
    assert!(gfx
        .init_window(400, 300, "Stasis IT-007 display metrics seam")
        .expect("initialize native window"));
    let native = NativeSurfaceHarness::load(&runtime_path);
    let initial_lifecycle = native.lifecycle();
    assert_eq!(&initial_lifecycle[0..3], &[1, 1, 1]);

    let mut host_i32 = vec![0; 768];
    let mut host_f32 = vec![0.0; 64];
    let mut gfx_i32 = vec![0; 34608];
    let mut gfx_f32 = vec![0.0; 108676];
    let mut gfx_u8 = vec![0; 65536];
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
        "tests/stasis/seams/desktop_display_metrics_probe.stasis",
        FIXTURE,
    );
    jit.compile().expect("compile display metrics fixture");
    assert_eq!(jit.execute_i32_noarg_by_name("main"), Ok(0));

    native.display(DISPLAY_CHANGED, ODD_FRACTIONAL);
    native.pointer(POINTER_DOWN, 200.0, 150.0);
    let odd_trace = run_frame(
        &gfx,
        &mut jit,
        &mut host_i32,
        &mut host_f32,
        &mut gfx_i32,
        &gfx_f32,
        &gfx_u8,
        ODD_FRACTIONAL,
        false,
        true,
    );
    assert_close(scalar_f32("metric_pointer_x"), 200.0, "odd pointer x");
    assert_close(scalar_f32("metric_pointer_y"), 150.0, "odd pointer y");
    assert_close(
        scalar_f32("metric_pointer_x_n"),
        0.5,
        "odd pointer normalized x",
    );
    assert_close(
        scalar_f32("metric_pointer_y_n"),
        134.0 / 267.0,
        "odd pointer normalized y",
    );

    native.display(WINDOW_MINIMIZED, ODD_FRACTIONAL);
    let minimized_trace = run_frame(
        &gfx,
        &mut jit,
        &mut host_i32,
        &mut host_f32,
        &mut gfx_i32,
        &gfx_f32,
        &gfx_u8,
        ODD_FRACTIONAL,
        true,
        false,
    );
    assert_eq!(
        native.lifecycle(),
        initial_lifecycle,
        "minimize must not invalidate a surviving SDL renderer"
    );

    native.display(WINDOW_RESTORED, RESTORED_SCALE);
    native.pointer(POINTER_MOVE, 100.0, 75.0);
    let restored_trace = run_frame(
        &gfx,
        &mut jit,
        &mut host_i32,
        &mut host_f32,
        &mut gfx_i32,
        &gfx_f32,
        &gfx_u8,
        RESTORED_SCALE,
        false,
        true,
    );
    assert_close(scalar_f32("metric_pointer_x"), 100.0, "restored pointer x");
    assert_close(scalar_f32("metric_pointer_y"), 75.0, "restored pointer y");
    assert_close(
        scalar_f32("metric_pointer_x_n"),
        0.25,
        "restored normalized x",
    );
    assert_close(
        scalar_f32("metric_pointer_y_n"),
        0.25,
        "restored normalized y",
    );

    native.display(WINDOW_RESTORED, RESTORED_SCALE);
    let duplicate_trace = run_frame(
        &gfx,
        &mut jit,
        &mut host_i32,
        &mut host_f32,
        &mut gfx_i32,
        &gfx_f32,
        &gfx_u8,
        RESTORED_SCALE,
        false,
        false,
    );
    assert_eq!(
        scalar_i32("metric_display_generation"),
        3,
        "duplicate restoration must not advance display generation"
    );
    assert_eq!(
        scalar_i32("metric_density_generation"),
        3,
        "duplicate restoration must not advance density generation"
    );
    assert_eq!(
        native.lifecycle(),
        initial_lifecycle,
        "display-only restoration must preserve renderer resources"
    );
    assert_eq!(odd_trace, ODD_TRACE);
    assert_eq!(minimized_trace, MINIMIZED_TRACE);
    assert_eq!(restored_trace, RESTORED_TRACE);
    assert_eq!(duplicate_trace, RESTORED_TRACE);

    let evidence = json!({
        "schema": "stasis.seam_test.v1",
        "test_id": "IT-007",
        "status": "passed",
        "target": "windows-sdl-jit",
        "samples": [
            {"stage": "odd_fractional", "logical": ODD_FRACTIONAL.logical, "native": ODD_FRACTIONAL.native, "drawable": ODD_FRACTIONAL.drawable, "safe": ODD_FRACTIONAL.safe_logical, "display_generation": 2, "density_generation": 2, "trace": odd_trace},
            {"stage": "minimized", "display_generation": 2, "density_generation": 2, "trace": minimized_trace},
            {"stage": "restored_scale", "logical": RESTORED_SCALE.logical, "native": RESTORED_SCALE.native, "drawable": RESTORED_SCALE.drawable, "display_generation": 3, "density_generation": 3, "trace": restored_trace},
            {"stage": "duplicate_restore", "display_generation": 3, "density_generation": 3, "trace": duplicate_trace}
        ],
        "oracle": {"renderer_lifecycle": initial_lifecycle, "restoration_generation_advances": 1, "pointer_round_trip_tolerance": 0.002}
    });
    let evidence_path = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| repository_root().join("target"))
        .join("seam-tests/it-007-desktop-display-metrics.json");
    fs::create_dir_all(evidence_path.parent().expect("evidence directory"))
        .expect("create evidence directory");
    fs::write(
        &evidence_path,
        serde_json::to_vec_pretty(&evidence).expect("serialize evidence"),
    )
    .expect("write evidence");
    eprintln!("IT-007 evidence: {evidence}");
}
