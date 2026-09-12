#![cfg(windows)]

use stasis_compiler::backend::aot::AotProcess;
use stasis_compiler::backend::jit::JitProcess;
use stasis_jit::{AotLinkConfig, AotTarget};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

const FIXTURE_PATH: &str = "tests/stasis/seams/receiver_struct_array_probe.stasis";
const FIXTURE: &str =
    include_str!("../../../tests/stasis/seams/receiver_struct_array_probe.stasis");
const PROBE_ROOT: &str = "receiver_struct_array_probe";
const DYNAMIC_ROOT: &str = "receiver_dynamic_parent";
const DYNAMIC_SET_ROOT: &str = "receiver_dynamic_set";
const TRAP_CHILD_INDEX: &str = "STASIS_RECEIVER_STRUCT_ARRAY_TRAP_INDEX";
const TRAP_CHILD_ROOT: &str = "STASIS_RECEIVER_STRUCT_ARRAY_TRAP_ROOT";
const TRAP_CHILD_EXE: &str = "STASIS_RECEIVER_STRUCT_ARRAY_TRAP_EXE";
const WINDOWS_ILLEGAL_INSTRUCTION: i32 = 0xC000001D_u32 as i32;

struct TestTree(PathBuf);

impl Drop for TestTree {
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

fn configured_jit(roots: &[&str], source: &str) -> JitProcess {
    let mut process = JitProcess::new();
    process
        .set_project_root(repository_root().to_string_lossy())
        .expect("set JIT project root");
    process.set_required_emit_roots(
        &roots
            .iter()
            .map(|root| (*root).to_string())
            .collect::<Vec<_>>(),
    );
    process.upsert_file(FIXTURE_PATH, source);
    process
        .compile()
        .expect("compile receiver struct-array JIT fixture");
    process
}

fn configured_aot() -> AotProcess {
    let mut process = AotProcess::new();
    process
        .set_project_root(repository_root().to_string_lossy())
        .expect("set AOT project root");
    process.set_required_emit_roots(&[PROBE_ROOT.to_string()]);
    process.upsert_file(FIXTURE_PATH, FIXTURE);
    process
        .compile()
        .expect("compile receiver struct-array AOT fixture");
    process
}

fn dynload_artifacts() -> (PathBuf, PathBuf) {
    let deps = std::env::current_exe()
        .expect("test executable")
        .parent()
        .expect("Cargo deps directory")
        .to_path_buf();
    let artifacts = [&deps, deps.parent().expect("Cargo profile directory")]
        .into_iter()
        .find_map(|directory| {
            let import = directory.join("stasis_dynload.dll.lib");
            let runtime = directory.join("stasis_dynload.dll");
            (import.is_file() && runtime.is_file()).then_some((import, runtime))
        })
        .expect("fresh stasis_dynload DLL and import library in the Cargo target");
    artifacts
}

fn linker_path() -> PathBuf {
    if let Some(explicit) = std::env::var_os("STASIS_AOT_LINKER") {
        let path = PathBuf::from(explicit);
        assert!(path.is_file(), "STASIS_AOT_LINKER must name a linker file");
        return path;
    }
    for candidate in ["link.exe", "lld-link.exe"] {
        if let Ok(output) = Command::new("where.exe").arg(candidate).output() {
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
                .map(PathBuf::from)
                .find(|path| path.is_file())
            {
                return path;
            }
        }
    }
    cc::windows_registry::find_tool("x86_64-pc-windows-msvc", "link.exe")
        .map(|tool| tool.path().to_path_buf())
        .filter(|path| path.is_file())
        .expect("MSVC link.exe or lld-link.exe is required for linked AOT execution")
}

#[test]
fn receiver_struct_arrays_match_native_jit_and_linked_aot() {
    let jit_result = configured_jit(&[PROBE_ROOT], FIXTURE)
        .execute_i32_noarg_by_name(PROBE_ROOT)
        .expect("execute receiver struct-array JIT fixture");
    assert_eq!(jit_result, 0, "JIT fixture failure code");

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| repository_root().join("target"));
    let tree = TestTree(target.join(format!(
        "receiver-struct-array-aot-{}-{stamp}",
        std::process::id()
    )));
    fs::create_dir_all(&tree.0).expect("create linked-AOT directory");
    let (import, runtime) = dynload_artifacts();
    fs::copy(runtime, tree.0.join("stasis_dynload.dll")).expect("copy linked-AOT runtime");
    let executable = tree.0.join("receiver_struct_array_probe.exe");
    configured_aot()
        .link_executable_for_i32_noarg_function(
            PROBE_ROOT,
            &executable,
            &AotLinkConfig {
                linker_path: Some(linker_path()),
                runtime_lib_paths: vec![import],
                target: AotTarget::Native,
            },
        )
        .expect("link receiver struct-array AOT fixture");
    let status = Command::new(repository_root().join(".cargo/stasis-sign-and-run.cmd"))
        .arg(executable.file_name().expect("linked executable name"))
        .current_dir(&tree.0)
        .status()
        .expect("run linked receiver struct-array AOT fixture");
    assert!(status.success(), "linked AOT fixture failed with {status}");
    assert_eq!(status.code(), Some(jit_result), "JIT/AOT result parity");
}

#[test]
fn dynamic_receiver_struct_array_bounds_trap_in_native_jit() {
    if let Some(index) = std::env::var_os(TRAP_CHILD_INDEX) {
        let index = index
            .to_string_lossy()
            .parse::<i32>()
            .expect("child trap index");
        let root = std::env::var(TRAP_CHILD_ROOT).expect("child trap root");
        let process = configured_jit(&[&root], FIXTURE);
        let _ = process
            .execute_i32_onearg_by_name(&root, index)
            .expect("out-of-range dynamic receiver access must trap before returning");
        return;
    }

    let process = configured_jit(&[DYNAMIC_ROOT, DYNAMIC_SET_ROOT], FIXTURE);
    assert_eq!(process.execute_i32_onearg_by_name(DYNAMIC_ROOT, 0), Ok(0));
    assert_eq!(process.execute_i32_onearg_by_name(DYNAMIC_ROOT, 23), Ok(0));
    assert_eq!(
        process.execute_i32_onearg_by_name(DYNAMIC_SET_ROOT, 0),
        Ok(0)
    );
    assert_eq!(
        process.execute_i32_onearg_by_name(DYNAMIC_SET_ROOT, 23),
        Ok(0)
    );
    let clif = process
        .clif_for_function_name("receiver_parent")
        .expect("receiver parent CLIF");
    assert!(
        clif.contains("trapz"),
        "dynamic receiver index omitted bounds trap:\n{clif}"
    );

    for root in [DYNAMIC_ROOT, DYNAMIC_SET_ROOT] {
        for index in [-1, 24] {
            let mut child = Command::new("powershell.exe")
                .args([
                    "-NoProfile",
                    "-Command",
                    "Add-Type 'using System; using System.Runtime.InteropServices; public static class StasisTrapMode { [DllImport(\"kernel32.dll\")] public static extern uint SetErrorMode(uint mode); }'; [StasisTrapMode]::SetErrorMode(3) | Out-Null; & $env:STASIS_RECEIVER_STRUCT_ARRAY_TRAP_EXE --exact dynamic_receiver_struct_array_bounds_trap_in_native_jit --nocapture; exit $LASTEXITCODE",
                ])
                .env(
                    TRAP_CHILD_EXE,
                    std::env::current_exe().expect("test executable"),
                )
                .env(TRAP_CHILD_ROOT, root)
                .env(TRAP_CHILD_INDEX, index.to_string())
                .spawn()
                .unwrap_or_else(|error| {
                    panic!("spawn isolated trap child for {root}({index}): {error}")
                });
            let deadline = std::time::Instant::now() + Duration::from_secs(120);
            let status = loop {
                if let Some(status) = child
                    .try_wait()
                    .unwrap_or_else(|error| panic!("wait for {root}({index}): {error}"))
                {
                    break status;
                }
                if std::time::Instant::now() >= deadline {
                    let _ = Command::new("taskkill.exe")
                        .args(["/PID", &child.id().to_string(), "/T", "/F"])
                        .status();
                    let _ = child.wait();
                    panic!("timed out waiting for isolated trap {root}({index})");
                }
                thread::sleep(Duration::from_millis(50));
            };
            assert_eq!(
                status.code(),
                Some(WINDOWS_ILLEGAL_INSTRUCTION),
                "{root}({index}) must terminate with the native Cranelift bounds trap"
            );
        }
    }
}

#[test]
fn receiver_struct_array_whole_element_and_type_mismatch_are_stable_errors() {
    let whole_element = FIXTURE.replace(
        "self.bones[index].parent = parent;",
        "self.bones[index] = receiver_right.bones[0];",
    );
    assert_ne!(whole_element, FIXTURE, "whole-element mutation must apply");
    let whole_error = configured_compile_error(&whole_element);
    assert_eq!(
        whole_error,
        "Backend(\"local indexed collection access requires field path for struct elements\")",
        "whole-element diagnostic contract"
    );

    let mismatched = FIXTURE.replace(
        "self.bones[index].parent = parent;",
        "self.bones[index].parent = local_x;",
    );
    assert_ne!(mismatched, FIXTURE, "type-mismatch mutation must apply");
    let mismatch_error = configured_compile_error(&mismatched);
    assert_eq!(
        mismatch_error, "Frontend(\"assignment expected i32 expression but found f32\")",
        "mismatched scalar diagnostic contract"
    );

    let nested_value_error = configured_compile_error(
        "struct Transform { x: f32; }\nstruct Bone { transform: Transform; }\nstruct Rig { bones: Bone[2]; }\nglobal rig: Rig;\nfunction consume(value: Transform): i32 { return 0; }\nfunction read_nested(self: Rig, index: i32): i32 { return consume(self.bones[index].transform); }\nfunction receiver_struct_array_probe(): i32 { return rig.read_nested(0); }\n",
    );
    assert_eq!(
        nested_value_error, "Backend(\"unknown local indexed collection field path 'transform'\")",
        "nested struct-valued field diagnostic contract"
    );

    let layout_error = configured_compile_error(
        "struct A { value: i32; }\nstruct B { value: i32; extra: f32; }\nglobal left: A[1];\nglobal right: B[1];\nfunction receiver_struct_array_probe(): i32 { left[0] = right[0]; return 0; }\n",
    );
    assert_eq!(
        layout_error,
        "Backend(\"struct indexed copy assignment requires matching field layout for 'left[...]' and 'right[...]'\")",
        "mismatched struct layout diagnostic contract"
    );
}

fn configured_compile_error(source: &str) -> String {
    let mut process = JitProcess::new();
    process
        .set_project_root(repository_root().to_string_lossy())
        .expect("set diagnostic project root");
    process.set_required_emit_roots(&[PROBE_ROOT.to_string()]);
    process.upsert_file(FIXTURE_PATH, source);
    format!(
        "{:?}",
        process
            .compile()
            .expect_err("unsupported receiver struct-array source must fail")
    )
}
