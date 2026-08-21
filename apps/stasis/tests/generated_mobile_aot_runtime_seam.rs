#![cfg(windows)]

use serde_json::json;
use stasis::{
    audit_mobile_aot_bindings, mobile_aot_function_for,
    write_mobile_aot_bindings_source_with_profile,
};
use stasis_compiler::backend::aot::AotProcess;
use stasis_compiler::backend::EngineEntrypoints;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const FIXTURE: &str =
    include_str!("../../../tests/stasis/seams/generated_mobile_aot_probe.stasis.fixture");
const STDLIB: &str = include_str!("../../../src/stdlib/stdlib.stasis");
const MEMORY: &str = include_str!("../../../src/stdlib/memory.stasis");
const GFX_CMD: &str = include_str!("../../../src/stdlib/internal/gfx_cmd.stasis");
const EXPECTED_TRACE: u32 = 2_880_741_754;

struct TestTree(PathBuf);

impl Drop for TestTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn repository_root() -> PathBuf {
    let canonical = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("canonical repository root");
    let display = canonical.to_string_lossy();
    PathBuf::from(display.strip_prefix(r"\\?\").unwrap_or(&display))
}

fn evidence_root() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| repository_root().join("target"))
        .join("seam-tests")
}

fn locate_cl() -> PathBuf {
    if let Some(path) = std::env::var_os("STASIS_C_COMPILER").map(PathBuf::from) {
        assert!(
            path.is_file(),
            "STASIS_C_COMPILER must name a compiler file"
        );
        return path;
    }
    let output = Command::new("where.exe")
        .arg("cl.exe")
        .output()
        .expect("locate MSVC compiler");
    assert!(
        output.status.success(),
        "MSVC cl.exe is required for IT-012"
    );
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(PathBuf::from)
        .expect("cl.exe path")
}

#[test]
fn generated_aot_objects_and_bindings_run_through_real_mobile_runtime() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let tree = TestTree(
        std::env::temp_dir().join(format!("stasis-it-012-{}-{stamp}", std::process::id())),
    );
    let project = tree.0.join("project");
    let bundle_dir = tree.0.join("bundle");
    fs::create_dir_all(project.join("src")).expect("create fixture source directory");
    fs::create_dir_all(project.join("assets")).expect("create fixture asset directory");
    fs::create_dir_all(project.join("vendor/stasis/stdlib/internal"))
        .expect("create fixture stdlib directory");
    fs::write(
        project.join("stasis.json"),
        "{\"manifest_version\":1,\"name\":\"it012\",\"entry\":\"src/main.stasis\",\"tests\":\"tests\",\"output\":\"build\"}\n",
    )
    .expect("write fixture manifest");
    fs::write(
        project.join("assets/manifest.json"),
        "{\"schema\":\"stasis-assets\",\"version\":1,\"assets\":[]}\n",
    )
    .expect("write empty asset manifest");
    fs::write(project.join("vendor/stasis/stdlib/stdlib.stasis"), STDLIB)
        .expect("write fixture stdlib");
    fs::write(project.join("vendor/stasis/stdlib/memory.stasis"), MEMORY)
        .expect("write fixture memory stdlib");
    fs::write(
        project.join("vendor/stasis/stdlib/internal/gfx_cmd.stasis"),
        GFX_CMD,
    )
    .expect("write fixture graphics command stdlib");
    fs::write(project.join("src/main.stasis"), FIXTURE).expect("write fixture source");

    let mut process = AotProcess::new();
    process
        .set_project_root(project.to_string_lossy())
        .expect("set native AOT project root");
    process
        .set_profile_functions(["render".to_string()])
        .expect("configure native AOT profiler");
    process.upsert_file("src/main.stasis", FIXTURE);
    process.compile().expect("compile native mobile fixture");
    let bundle = process
        .write_engine_bundle(&EngineEntrypoints::runtime_default(), &bundle_dir)
        .expect("write native engine bundle");
    let manifest: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(&bundle.manifest_path).expect("read engine manifest"),
    )
    .expect("parse engine manifest");
    let bindings_path = bundle_dir.join("published_aot_bindings.c");
    write_mobile_aot_bindings_source_with_profile(
        &manifest,
        &process.state_layout(),
        &project,
        &bindings_path,
        &["render".to_string()],
        1,
        2,
    )
    .expect("write generated mobile bindings");
    let bindings = fs::read_to_string(&bindings_path).expect("read generated bindings");
    audit_mobile_aot_bindings(&manifest, &bindings).expect("audit generated bindings");
    assert!(bindings.contains("stasis_jit_profile_register_function"));
    assert!(bindings.contains("stasis_jit_profile_configure(1, 2);"));

    let (tick_symbol, _) = mobile_aot_function_for(&manifest, "tick").expect("tick symbol");
    let declaration = format!("extern int32_t {tick_symbol}(void);");
    let wrong = bindings.replacen(
        &declaration,
        "extern int32_t stasis_it012_missing_tick(void);",
        1,
    );
    assert_ne!(wrong, bindings, "symbol mutation must apply");
    let audit_error = audit_mobile_aot_bindings(&manifest, &wrong)
        .expect_err("missing generated tick declaration must fail audit");
    assert_eq!(
        audit_error,
        format!("mobile AOT bindings missing declaration for generated symbol '{tick_symbol}'")
    );

    let root = repository_root();
    let runtime = root.join("runtime");
    let executable = tree.0.join("it012_generated_mobile.exe");
    let compiler = locate_cl();
    let compiler_is_clang = compiler
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(|stem| stem.eq_ignore_ascii_case("clang-cl"))
        .unwrap_or(false);
    let mut command = Command::new(compiler);
    command
        .current_dir(&tree.0)
        .args([
            "/nologo",
            "/W4",
            "/WX",
            "/wd4204",
            "/std:c11",
            "/D_CRT_SECURE_NO_WARNINGS",
        ])
        .arg(format!("/I{}", runtime.display()))
        .arg(runtime.join("tests/stasis_generated_mobile_integration.c"))
        .arg(runtime.join("stasis_mobile_runtime.c"))
        .arg(runtime.join("stasis_mobile_aot_runtime.c"))
        .arg(runtime.join("stasis_render_trace.c"))
        .arg(&bindings_path);
    if !compiler_is_clang {
        command.arg("/experimental:c11atomics");
    }
    for path in bundle.object_paths_by_function_id.values() {
        command.arg(path);
    }
    let output = command
        .arg(format!("/Fe:{}", executable.display()))
        .output()
        .expect("compile and link generated mobile runtime harness");
    assert!(
        output.status.success(),
        "generated mobile harness link failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let run = Command::new(&executable)
        .current_dir(&tree.0)
        .output()
        .expect("run generated mobile runtime harness");
    assert!(
        run.status.success(),
        "generated mobile harness failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8(run.stdout).expect("UTF-8 harness output");
    let trace = stdout
        .split_whitespace()
        .find_map(|field| field.strip_prefix("trace="))
        .and_then(|value| value.parse::<u32>().ok())
        .expect("harness trace");
    assert_eq!(trace, EXPECTED_TRACE, "first generated render trace");
    assert!(stdout.contains("state=15 frames=1 rects=1 texts=1 bytes=5 chars=4"));
    assert!(stdout.contains(
        "IT-013 order=123 paused_poll=1 reinit=1 main_stop=11 tick_stop=22 render_stop=33 frames_after_failures=0"
    ));
    assert!(
        stdout.contains("IT-014 order=123 marker=77 request=41:5:640:360 render_score=15 frames=1")
    );

    let evidence = json!({
        "schema": "stasis.seam_test.v1",
        "test_id": "IT-012",
        "status": "passed",
        "target": "windows-native-aot+c-mobile-runtime",
        "generated_objects": bundle.object_paths_by_function_id.len(),
        "bindings": bindings_path,
        "main_state": 10,
        "first_tick_state": 15,
        "first_render": {
            "frames": 1,
            "rects": 1,
            "texts": 1,
            "text_bytes_used": 6,
            "text_font": 7,
            "text_offset": 0,
            "text_byte_length": 5,
            "text_bytes": [67, 97, 102, 195, 169, 0],
            "forwarded_byte_length": 5,
            "forwarded_char_length": 4,
            "trace": trace
        },
        "oracle": {"symbol_audit_failure": audit_error}
    });
    let path = evidence_root().join("it-012-generated-mobile-aot-runtime.json");
    fs::create_dir_all(path.parent().expect("evidence parent")).expect("create evidence directory");
    fs::write(
        path,
        serde_json::to_vec_pretty(&evidence).expect("serialize evidence"),
    )
    .expect("write evidence");
    eprintln!("IT-012 evidence: {evidence}");

    let lifecycle_evidence = json!({
        "schema": "stasis.seam_test.v1",
        "test_id": "IT-013",
        "status": "passed",
        "target": "windows-native-aot+c-mobile-runtime",
        "entry_order": 123,
        "paused": {"polls": 1, "tick_or_render_calls": 0},
        "reinitialize": {"entry_order": 1, "score": 10},
        "stop_results": {"main": 11, "tick": 22, "render": 33},
        "frames_after_tick_or_render_failure": 0
    });
    let lifecycle_path = evidence_root().join("it-013-generated-mobile-aot-lifecycle.json");
    fs::write(
        lifecycle_path,
        serde_json::to_vec_pretty(&lifecycle_evidence).expect("serialize lifecycle evidence"),
    )
    .expect("write lifecycle evidence");
    eprintln!("IT-013 evidence: {lifecycle_evidence}");

    let ordering_evidence = json!({
        "schema": "stasis.seam_test.v1",
        "test_id": "IT-014",
        "status": "passed",
        "target": "windows-native-aot+c-mobile-runtime",
        "host_apply_submit_order": 123,
        "host_frame_marker_seen_by_tick": 77,
        "request_applied": {"sequence": 41, "flags": 5, "width": 640, "height": 360},
        "tick_score_seen_by_render": 15,
        "submitted_frames": 1
    });
    let ordering_path = evidence_root().join("it-014-mobile-host-frame-order.json");
    fs::write(
        ordering_path,
        serde_json::to_vec_pretty(&ordering_evidence).expect("serialize ordering evidence"),
    )
    .expect("write ordering evidence");
    eprintln!("IT-014 evidence: {ordering_evidence}");
}
