use serde_json::json;
use sha2::{Digest, Sha256};
use stasis_compiler::backend::jit::JitProcess;
use stasis_dynload::{global_path_hash, register_global_f32_array, register_global_i32_array};
use std::path::PathBuf;

const HOST_FRAME: &str = include_str!("../../../src/stdlib/internal/host_frame.stasis");
const PROBE: &str = include_str!("../../../tests/stasis/seams/host_frame_jit_probe.stasis");

const I32_FIELDS: [&str; 31] = [
    "host_tick_index",
    "host_time_ms",
    "host_pointer_count",
    "host_dropped_pointers",
    "host_quit_requested",
    "host_resized",
    "host_screen_w_px",
    "host_screen_h_px",
    "host_version",
    "host_flags",
    "host_tick_hz",
    "host_window_focused",
    "host_window_minimized",
    "host_time_us",
    "host_native_w_px",
    "host_native_h_px",
    "host_drawable_w_px",
    "host_drawable_h_px",
    "host_display_generation",
    "host_density_generation",
    "host_key_down(41)",
    "host_key_down(-1)",
    "host_key_down(512)",
    "host_pointer_id(0)",
    "host_pointer_is_down(0)",
    "host_pointer_went_down(0)",
    "host_pointer_went_up(0)",
    "host_pointer_id(7)",
    "host_pointer_is_down(7)",
    "host_pointer_went_down(7)",
    "host_pointer_went_up(7)",
];

const F32_FIELDS: [&str; 20] = [
    "host_logical_width",
    "host_logical_height",
    "host_safe_x",
    "host_safe_y",
    "host_safe_width",
    "host_safe_height",
    "host_content_scale",
    "host_raster_scale",
    "host_pointer_x_logical(0)",
    "host_pointer_y_logical(0)",
    "host_pointer_dx_logical(0)",
    "host_pointer_dy_logical(0)",
    "host_pointer_x_n(0)",
    "host_pointer_y_n(0)",
    "host_pointer_x_logical(7)",
    "host_pointer_y_logical(7)",
    "host_pointer_dx_logical(7)",
    "host_pointer_dy_logical(7)",
    "host_pointer_x_n(7)",
    "host_pointer_y_n(7)",
];

struct ProbeResult {
    actual_i32: Vec<i32>,
    actual_f32: Vec<f32>,
    checksum: i32,
}

fn representative_host_frame() -> (Vec<i32>, Vec<f32>) {
    let mut i32s = vec![0; 768];
    let mut f32s = vec![0.0; 64];
    for (index, value) in [
        (0, 101),
        (7, 8),
        (8, 2),
        (9, 1),
        (10, 17),
        (11, 1),
        (12, 640),
        (13, 360),
        (14, 3),
        (15, 11),
        (16, 60),
        (17, 1),
        (18, 0),
        (19, 1001),
        (22, 1280),
        (23, 720),
        (24, 1920),
        (25, 1080),
        (30, 7),
        (31, 9),
        (32 + 41, 1),
        (544, 100),
        (545, 1),
        (546, 1),
        (547, 0),
        (572, 107),
        (573, 0),
        (574, 0),
        (575, 1),
    ] {
        i32s[index] = value;
    }
    for (index, value) in [
        (0, 12.5),
        (1, 20.25),
        (2, 1.5),
        (3, -2.25),
        (4, 0.25),
        (5, 0.5),
        (42, 70.5),
        (43, 71.25),
        (44, 3.5),
        (45, -4.25),
        (46, 0.75),
        (47, 0.875),
        (48, 1.5),
        (49, 2.0),
        (50, 320.0),
        (51, 180.0),
        (52, 10.0),
        (53, 5.0),
        (54, 300.0),
        (55, 170.0),
    ] {
        f32s[index] = value;
    }
    (i32s, f32s)
}

fn expected_outputs() -> (Vec<i32>, Vec<f32>) {
    (
        vec![
            17, 101, 8, 2, 1, 1, 640, 360, 3, 11, 60, 1, 0, 1001, 1280, 720, 1920, 1080, 7, 9, 1,
            0, 0, 100, 1, 1, 0, 107, 0, 0, 1,
        ],
        vec![
            320.0, 180.0, 10.0, 5.0, 300.0, 170.0, 1.5, 2.0, 12.5, 20.25, 1.5, -2.25, 0.25, 0.5,
            70.5, 71.25, 3.5, -4.25, 0.75, 0.875,
        ],
    )
}

fn expected_checksum(i32s: &[i32], f32s: &[f32]) -> i32 {
    let ints = i32s
        .iter()
        .enumerate()
        .map(|(i, value)| value * (i as i32 + 1))
        .sum::<i32>();
    ints + f32s
        .iter()
        .enumerate()
        .map(|(i, value)| (*value * 8.0) as i32 * (i as i32 + 32))
        .sum::<i32>()
}

fn run_probe(host_source: &str) -> ProbeResult {
    let mut process = JitProcess::new();
    process.set_required_emit_roots(&["probe_host_frame".to_string()]);
    process.upsert_file("host_frame.stasis", host_source);
    process.upsert_file("host_frame_jit_probe.stasis", PROBE);
    process.compile().expect("compile HostFrame JIT probe");

    let (host_i32, host_f32) = representative_host_frame();
    let host_i32 = Box::leak(host_i32.into_boxed_slice());
    let host_f32 = Box::leak(host_f32.into_boxed_slice());
    let probe_i32 = Box::leak(vec![0; I32_FIELDS.len()].into_boxed_slice());
    let probe_f32 = Box::leak(vec![0.0; F32_FIELDS.len()].into_boxed_slice());
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
        global_path_hash("probe_i32"),
        0,
        probe_i32.as_mut_ptr(),
        probe_i32.len(),
    );
    register_global_f32_array(
        global_path_hash("probe_f32"),
        0,
        probe_f32.as_mut_ptr(),
        probe_f32.len(),
    );
    let checksum = process
        .execute_i32_noarg_by_name("probe_host_frame")
        .expect("execute HostFrame JIT probe");
    ProbeResult {
        actual_i32: probe_i32.to_vec(),
        actual_f32: probe_f32.to_vec(),
        checksum,
    }
}

fn verify(result: &ProbeResult) -> Result<(), String> {
    let (expected_i32, expected_f32) = expected_outputs();
    for (index, (expected, actual)) in expected_i32.iter().zip(&result.actual_i32).enumerate() {
        if expected != actual {
            return Err(format!("HostFrame mismatch: producer=desktop_host_frame consumer=compiled_stasis field={} expected={} actual={}", I32_FIELDS[index], expected, actual));
        }
    }
    for (index, (expected, actual)) in expected_f32.iter().zip(&result.actual_f32).enumerate() {
        if expected.to_bits() != actual.to_bits() {
            return Err(format!("HostFrame mismatch: producer=desktop_host_frame consumer=compiled_stasis field={} expected={} actual={}", F32_FIELDS[index], expected, actual));
        }
    }
    let expected = expected_checksum(&expected_i32, &expected_f32);
    if expected != result.checksum {
        return Err(format!("HostFrame mismatch: producer=desktop_host_frame consumer=compiled_stasis field=checksum expected={expected} actual={}", result.checksum));
    }
    Ok(())
}

fn evidence_path() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join("target")
        })
        .join("seam-tests/it-002-host-frame-jit.json")
}

#[test]
fn every_host_frame_field_round_trips_through_desktop_jit() {
    let result = run_probe(HOST_FRAME);
    verify(&result).expect("canonical HostFrame probe");

    let mutated = HOST_FRAME.replacen(
        "const HOST_I_TICK_INDEX: i32 = 10;",
        "const HOST_I_TICK_INDEX: i32 = 11;",
        1,
    );
    assert_ne!(mutated, HOST_FRAME, "offset mutation fixture must apply");
    let mutation_error = verify(&run_probe(&mutated)).expect_err("offset drift must fail");
    assert_eq!(mutation_error, "HostFrame mismatch: producer=desktop_host_frame consumer=compiled_stasis field=host_tick_index expected=17 actual=1");

    let (expected_i32, expected_f32) = expected_outputs();
    let fixture_revision = format!("{:x}", Sha256::digest([HOST_FRAME, PROBE].concat()));
    let evidence = json!({
        "schema": "stasis.seam_test.v1",
        "test_id": "IT-002",
        "status": "passed",
        "target": "desktop-jit",
        "fixture_revision": fixture_revision,
        "checks": I32_FIELDS.len() + F32_FIELDS.len() + 2,
        "oracle": {
            "checksum": expected_checksum(&expected_i32, &expected_f32),
            "pointer_indices": [0, 7],
            "rejected_key_indices": [-1, 512],
            "mutation_diagnostic": mutation_error,
        }
    });
    let path = evidence_path();
    std::fs::create_dir_all(path.parent().expect("evidence directory"))
        .expect("create evidence directory");
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&evidence).expect("serialize evidence"),
    )
    .expect("write evidence");
}
