use serde_json::json;
use sha2::{Digest, Sha256};
use stasis_compiler::backend::jit::JitProcess;
use stasis_dynload::{global_path_hash, register_global_f32_array, register_global_i32_array};
use std::path::PathBuf;

const RAW_HOST_FRAME: &str = include_str!("../../../src/stdlib/internal/host_frame_raw.stasis");
const PUBLIC_HOST_FRAME: &str = include_str!("../../../src/stdlib/host_frame.stasis");
const PROBE: &str = include_str!("../../../tests/stasis/seams/host_frame_jit_probe.stasis");

const I32_FIELDS: [&str; 34] = [
    "time_ms",
    "time_us",
    "tick_index",
    "version",
    "flags",
    "tick_hz",
    "window_focused",
    "window_minimized",
    "quit_requested",
    "pointer_count",
    "dropped_pointer_count",
    "display.resized",
    "display.screen_width_px",
    "display.screen_height_px",
    "display.native_width_px",
    "display.native_height_px",
    "display.drawable_width_px",
    "display.drawable_height_px",
    "display.generation",
    "display.density_generation",
    "keys[0]",
    "keys[41]",
    "keys[511]",
    "pointers[0].id",
    "pointers[0].is_down",
    "pointers[0].went_down",
    "pointers[0].went_up",
    "pointers[7].id",
    "pointers[7].is_down",
    "pointers[7].went_down",
    "pointers[7].went_up",
    "all_keys_checksum",
    "all_pointer_i32_checksum",
    "checksum",
];

const F32_FIELDS: [&str; 23] = [
    "display.logical_width",
    "display.logical_height",
    "display.safe_x",
    "display.safe_y",
    "display.safe_width",
    "display.safe_height",
    "display.available_width",
    "display.available_height",
    "display.content_scale",
    "display.raster_scale",
    "pointers[0].x_logical",
    "pointers[0].y_logical",
    "pointers[0].dx_logical",
    "pointers[0].dy_logical",
    "pointers[0].x_normalized",
    "pointers[0].y_normalized",
    "pointers[7].x_logical",
    "pointers[7].y_logical",
    "pointers[7].dx_logical",
    "pointers[7].dy_logical",
    "pointers[7].x_normalized",
    "pointers[7].y_normalized",
    "all_pointer_f32_checksum",
];

struct ProbeResult {
    actual_i32: Vec<i32>,
    actual_f32: Vec<f32>,
    checksum: i32,
}

fn expected_host_frame(pointer_count: i32) -> (Vec<i32>, Vec<f32>) {
    let mut i32s = vec![0; 768];
    let mut f32s = vec![0.0; 64];
    for (index, value) in [
        (0, 101),
        (7, pointer_count),
        (8, 2),
        (9, 1),
        (10, 0),
        (11, 1),
        (12, 640),
        (13, 360),
        (14, 4),
        (15, 11),
        (16, 0),
        (17, 1),
        (18, 0),
        (19, 1001),
        (22, 1280),
        (23, 720),
        (24, 1920),
        (25, 1080),
        (30, 7),
        (31, 9),
    ] {
        i32s[index] = value;
    }
    for index in 0..512 {
        i32s[32 + index] = ((index * 3 + 1) % 11) as i32;
    }
    for index in 0..8 {
        let i32_base = 544 + index * 4;
        i32s[i32_base] = 100 + index as i32;
        i32s[i32_base + 1] = (index % 2) as i32;
        i32s[i32_base + 2] = i32::from(index == 0 || index == 7);
        i32s[i32_base + 3] = i32::from(index == 7);

        let f32_base = index * 6;
        let coordinate = index as f32 * 10.0;
        f32s[f32_base] = coordinate + 0.25;
        f32s[f32_base + 1] = coordinate + 0.5;
        f32s[f32_base + 2] = coordinate + 0.75;
        f32s[f32_base + 3] = coordinate + 1.0;
        f32s[f32_base + 4] = index as f32 / 8.0;
        f32s[f32_base + 5] = (8 - index) as f32 / 8.0;
    }
    for (index, value) in [
        (48, 1.5),
        (49, 2.0),
        (50, 320.0),
        (51, 180.0),
        (52, 10.0),
        (53, 5.0),
        (54, 300.0),
        (55, 170.0),
        (56, 640.0),
        (57, 360.0),
    ] {
        f32s[index] = value;
    }
    (i32s, f32s)
}

fn expected_outputs(pointer_count: i32) -> (Vec<i32>, Vec<f32>) {
    let (host_i32, host_f32) = expected_host_frame(pointer_count);
    let key_checksum = (0..512)
        .map(|index| host_i32[32 + index] * (index as i32 + 1))
        .sum();
    let pointer_i32_checksum = (0..8)
        .map(|index| {
            let base = 544 + index * 4;
            host_i32[base] * (index as i32 + 1)
                + host_i32[base + 1] * (index as i32 + 9)
                + host_i32[base + 2] * (index as i32 + 17)
                + host_i32[base + 3] * (index as i32 + 25)
        })
        .sum();
    let pointer_f32_checksum = host_f32[..48].iter().sum();
    let mut expected_i32 = vec![
        101,
        1001,
        0,
        4,
        11,
        0,
        1,
        0,
        1,
        pointer_count.clamp(0, 8),
        2,
        1,
        640,
        360,
        1280,
        720,
        1920,
        1080,
        7,
        9,
        host_i32[32],
        host_i32[32 + 41],
        host_i32[32 + 511],
        100,
        0,
        1,
        0,
        107,
        1,
        1,
        1,
        key_checksum,
        pointer_i32_checksum,
    ];
    let expected_f32 = vec![
        320.0,
        180.0,
        10.0,
        5.0,
        300.0,
        170.0,
        640.0,
        360.0,
        1.5,
        2.0,
        0.25,
        0.5,
        0.75,
        1.0,
        0.0,
        1.0,
        70.25,
        70.5,
        70.75,
        71.0,
        0.875,
        0.125,
        pointer_f32_checksum,
    ];
    let checksum = expected_checksum(&expected_i32, &expected_f32);
    expected_i32.push(checksum);
    (expected_i32, expected_f32)
}

fn expected_checksum(i32s: &[i32], f32s: &[f32]) -> i32 {
    let ints = i32s
        .iter()
        .enumerate()
        .map(|(index, value)| value * (index as i32 + 1))
        .sum::<i32>();
    ints + f32s
        .iter()
        .enumerate()
        .map(|(index, value)| (*value * 8.0) as i32 * (index as i32 + 34))
        .sum::<i32>()
}

const NATIVE_SOURCE: &str = include_str!("../../../runtime/stasis_graphics.c");
const NATIVE_HARNESS: &str = include_str!("../../../runtime/tests/host_frame_fixture.c");

// Compile the production function verbatim; only its external inputs are stubbed.
fn native_host_frame(pointer_count: i32, mutate_writer: bool) -> (Vec<i32>, Vec<f32>) {
    let signature = "STASIS_EXPORT void stasis_host_get_frame(int32_t* out_i32, float* out_f32) {";
    let native_source = NATIVE_SOURCE.replace("\r\n", "\n");
    let start = native_source
        .find(signature)
        .expect("native writer definition");
    let end = start
        + native_source[start..]
            .find("\n}\n")
            .expect("native writer end")
        + 3;
    let mut writer = native_source[start..end].to_string();
    if mutate_writer {
        let mutated = writer.replacen(
            "out_i32[0] = stasis_get_time_ms();",
            "out_i32[1] = stasis_get_time_ms();",
            1,
        );
        assert_ne!(writer, mutated, "writer mutation must apply");
        writer = mutated;
    }
    let dir = evidence_path().parent().unwrap().join(format!(
        "native-{}-{}",
        std::process::id(),
        mutate_writer
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let source = dir.join("fixture.c");
    std::fs::write(
        &source,
        NATIVE_HARNESS.replace("/* NATIVE_WRITER */", &writer),
    )
    .unwrap();
    let executable = dir.join(if cfg!(windows) {
        "fixture.exe"
    } else {
        "fixture"
    });
    let target = target_lexicon::HOST.to_string();
    let compiler = cc::Build::new()
        .cargo_metadata(false)
        .out_dir(&dir)
        .opt_level(0)
        .host(&target)
        .target(&target)
        .get_compiler();
    let mut command = compiler.to_command();
    command.current_dir(&dir).arg(&source);
    if compiler.is_like_msvc() {
        command
            .arg("/std:c11")
            .arg(format!("/Fe{}", executable.display()));
    } else {
        command.args(["-std=c11", "-o"]).arg(&executable);
    }
    let output = command.output().expect("compile native HostFrame writer");
    assert!(
        output.status.success(),
        "native compiler: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let output = std::process::Command::new(&executable)
        .arg(pointer_count.to_string())
        .output()
        .expect("run native writer");
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    let values: Vec<&str> = text.split_whitespace().collect();
    assert_eq!(values.len(), 768 + 64);
    (
        values[..768].iter().map(|v| v.parse().unwrap()).collect(),
        values[768..].iter().map(|v| v.parse().unwrap()).collect(),
    )
}

fn run_probe(raw_host_source: &str, pointer_count: i32, mutate_writer: bool) -> ProbeResult {
    let mut process = JitProcess::new();
    process.set_required_emit_roots(&["probe_host_frame".to_string()]);
    process.upsert_file("internal/host_frame_raw.stasis", raw_host_source);
    process.upsert_file("host_frame.stasis", PUBLIC_HOST_FRAME);
    process.upsert_file("host_frame_jit_probe.stasis", PROBE);
    process.compile().expect("compile HostFrame JIT probe");

    let (host_i32, host_f32) = native_host_frame(pointer_count, mutate_writer);
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

fn verify(result: &ProbeResult, pointer_count: i32) -> Result<(), String> {
    let (expected_i32, expected_f32) = expected_outputs(pointer_count);
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
    let expected = *expected_i32.last().expect("checksum output");
    if expected != result.checksum {
        return Err(format!("HostFrame mismatch: producer=desktop_host_frame consumer=compiled_stasis field=return_checksum expected={expected} actual={}", result.checksum));
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
fn caller_owned_host_frame_round_trips_through_desktop_jit() {
    let high = run_probe(RAW_HOST_FRAME, 99, false);
    verify(&high, 99).expect("canonical HostFrame probe with high clamp");
    verify(&run_probe(RAW_HOST_FRAME, -3, false), -3).expect("HostFrame low clamp");

    let mutated = RAW_HOST_FRAME.replacen(
        "const HOST_I_TICK_INDEX: i32 = 10;",
        "const HOST_I_TICK_INDEX: i32 = 11;",
        1,
    );
    assert_ne!(
        mutated, RAW_HOST_FRAME,
        "offset mutation fixture must apply"
    );
    let mutation_error =
        verify(&run_probe(&mutated, 99, false), 99).expect_err("offset drift must fail");
    assert_eq!(mutation_error, "HostFrame mismatch: producer=desktop_host_frame consumer=compiled_stasis field=tick_index expected=0 actual=1");

    let writer_error = verify(&run_probe(RAW_HOST_FRAME, 99, true), 99)
        .expect_err("native writer offset drift must fail");
    assert_eq!(writer_error, "HostFrame mismatch: producer=desktop_host_frame consumer=compiled_stasis field=time_ms expected=101 actual=0");

    let fixture_revision = format!(
        "{:x}",
        Sha256::digest(
            [
                RAW_HOST_FRAME,
                PUBLIC_HOST_FRAME,
                PROBE,
                NATIVE_SOURCE,
                NATIVE_HARNESS
            ]
            .concat()
        )
    );
    let evidence = json!({
        "schema": "stasis.seam_test.v1",
        "test_id": "IT-002",
        "status": "passed",
        "target": "desktop-jit",
        "fixture_revision": fixture_revision,
        "checks": I32_FIELDS.len() + F32_FIELDS.len() + 4,
        "oracle": {
            "checksum": high.checksum,
            "pointer_indices": [0, 7],
            "pointer_count_clamps": { "low": 0, "high": 8 },
            "key_indices": [0, 41, 511],
            "mutation_diagnostic": mutation_error,
            "writer_mutation_diagnostic": writer_error,
            "producer": "compiled runtime/stasis_graphics.c::stasis_host_get_frame",
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
