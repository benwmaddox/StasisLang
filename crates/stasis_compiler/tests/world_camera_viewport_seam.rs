#![cfg(windows)]

use stasis_compiler::backend::aot::AotProcess;
use stasis_compiler::backend::jit::JitProcess;
use stasis_compiler::frontend::parser::rewrite_top_level_test_declarations;
use stasis_jit::{AotLinkConfig, AotTarget};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const FIXTURE_PATH: &str = "tests/stasis/seams/world_camera_viewport_probe.stasis";
const FIXTURE: &str =
    include_str!("../../../tests/stasis/seams/world_camera_viewport_probe.stasis");
const ROOT: &str = "world_camera_viewport_probe";
const STASIS_TEST_PATH: &str = "tests/stasis/world_camera.test.stasis";
const STASIS_TESTS: &str = include_str!("../../../tests/stasis/world_camera.test.stasis");
const SAMPLE_PATH: &str = "samples/world_camera_viewport/src/main.stasis";
const SAMPLE: &str = include_str!("../../../samples/world_camera_viewport/src/main.stasis");

struct AotTree(PathBuf);

impl Drop for AotTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("canonical repository root")
}

fn linker_path() -> PathBuf {
    if let Some(explicit) = std::env::var_os("STASIS_AOT_LINKER") {
        return PathBuf::from(explicit);
    }
    for candidate in ["link.exe", "lld-link.exe"] {
        let output = Command::new("where.exe")
            .arg(candidate)
            .output()
            .expect("locate Windows linker");
        if let Some(path) = output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout))
            .into_iter()
            .flat_map(|lines| {
                lines
                    .lines()
                    .map(str::trim)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .find(|line| !line.is_empty())
        {
            return PathBuf::from(path);
        }
    }
    panic!("MSVC link.exe or lld-link.exe is required");
}

fn dynload_artifacts() -> (PathBuf, PathBuf) {
    let deps = std::env::current_exe()
        .expect("test executable")
        .parent()
        .expect("deps directory")
        .to_path_buf();
    let artifacts = [&deps, deps.parent().expect("profile directory")]
        .into_iter()
        .find_map(|directory| {
            let import = directory.join("stasis_dynload.dll.lib");
            let runtime = directory.join("stasis_dynload.dll");
            (import.is_file() && runtime.is_file()).then_some((import, runtime))
        })
        .expect("stasis_dynload artifacts");
    artifacts
}

#[test]
fn world_camera_projection_and_clip_commands_match_jit_and_linked_aot() {
    let root = repository_root();
    let mut jit = JitProcess::new();
    jit.set_project_root(root.to_string_lossy())
        .expect("set JIT project root");
    jit.set_required_emit_roots(&[ROOT.to_string()]);
    jit.upsert_file(FIXTURE_PATH, FIXTURE);
    jit.compile().expect("compile world camera JIT fixture");
    let jit_result = jit
        .execute_i32_noarg_by_name(ROOT)
        .expect("execute world camera JIT fixture");

    let mut aot = AotProcess::new();
    aot.set_project_root(root.to_string_lossy())
        .expect("set AOT project root");
    aot.set_required_emit_roots(&[ROOT.to_string()]);
    aot.upsert_file(FIXTURE_PATH, FIXTURE);
    aot.compile().expect("compile world camera AOT fixture");
    let output_dir = AotTree(
        std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| root.join("target"))
            .join(format!("world-camera-aot-{}", std::process::id())),
    );
    fs::create_dir_all(&output_dir.0).expect("create AOT evidence directory");
    let (import, runtime) = dynload_artifacts();
    fs::copy(runtime, output_dir.0.join("stasis_dynload.dll")).expect("copy AOT runtime");
    let linked = output_dir.0.join("world_camera_viewport_probe.exe");
    let config = AotLinkConfig {
        linker_path: Some(linker_path()),
        runtime_lib_paths: vec![import],
        target: AotTarget::Native,
    };
    aot.link_executable_for_i32_noarg_function(ROOT, &linked, &config)
        .expect("link world camera AOT fixture");
    let status = Command::new(root.join(".cargo/stasis-sign-and-run.cmd"))
        .arg(&linked)
        .current_dir(&output_dir.0)
        .status()
        .expect("run linked world camera AOT fixture");

    assert_eq!(jit_result, 0, "JIT fixture failure code");
    let aot_code = status.code().expect("linked AOT process exit code");
    let signed_execution_required =
        std::env::var_os("STASIS_REQUIRE_SIGNED_EXECUTION").is_some_and(|value| value == "1");
    if aot_code == 4551 && !signed_execution_required {
        eprintln!(
            "skipping linked AOT execution parity: Windows Application Control returned 4551 and signed execution is not required"
        );
        return;
    }
    assert_eq!(aot_code, jit_result, "JIT/AOT result parity");
}

#[test]
fn world_camera_stasis_tests_pass_in_the_production_jit_test_shape() {
    let (rewritten, tests) =
        rewrite_top_level_test_declarations(STASIS_TESTS).expect("discover Stasis tests");
    assert_eq!(tests.len(), 7, "focused behavior test count");
    let mut process = JitProcess::new();
    process
        .set_project_root(repository_root().to_string_lossy())
        .expect("set Stasis test project root");
    process.set_required_emit_roots(
        &tests
            .iter()
            .map(|test| test.generated_function_name.clone())
            .collect::<Vec<_>>(),
    );
    process.upsert_file(STASIS_TEST_PATH, rewritten);
    process.compile().expect("compile focused Stasis tests");
    for test in tests {
        assert!(
            process
                .execute_bool_noarg_by_name(&test.generated_function_name)
                .unwrap_or_else(|error| panic!("execute '{}': {error}", test.display_name)),
            "Stasis test returned false: {}",
            test.display_name
        );
    }
}

#[test]
fn world_camera_sample_advances_sixty_ticks_without_input_gating() {
    let source = SAMPLE.replace(
        "/.stasis_cache/toolchain/src/stdlib/",
        "../../../src/stdlib/",
    );
    let mut process = JitProcess::new();
    process
        .set_project_root(repository_root().to_string_lossy())
        .expect("set sample project root");
    process.set_required_emit_roots(&[
        "sample_reset".to_string(),
        "sample_simulation_step".to_string(),
        "sample_cadence_probe".to_string(),
        "render".to_string(),
    ]);
    process.upsert_file(SAMPLE_PATH, source);
    process.compile().expect("compile world camera sample");
    assert_eq!(process.execute_i32_noarg_by_name("sample_reset"), Ok(0));
    for tick in 1..=60 {
        assert_eq!(
            process.execute_i32_noarg_by_name("sample_simulation_step"),
            Ok(0),
            "simulation tick {tick}"
        );
    }
    assert_eq!(process.read_i32_global_path("sample_simulation_ticks"), 60);
    assert_eq!(
        process.read_i32_global_path("sample_control_transitions"),
        3
    );
    assert_eq!(
        process.execute_i32_noarg_by_name("sample_cadence_probe"),
        Ok(0),
        "exact final position and control effect"
    );
}
