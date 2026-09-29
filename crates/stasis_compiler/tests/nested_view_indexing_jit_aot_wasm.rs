use stasis_compiler::backend::jit::JitScalarValue;
use stasis_compiler::backend::{aot::AotProcess, jit::JitProcess, wasm::WasmProcess};
#[cfg(windows)]
use stasis_jit::{AotLinkConfig, AotTarget};
use std::fs;
#[cfg(windows)]
use std::path::{Path, PathBuf};
use std::process::Command;

const FIXTURE_PATH: &str = "tests/fixtures/nested_view_indexing/main.stasis";
const FIXTURE: &str = include_str!("../../../tests/fixtures/nested_view_indexing/main.stasis");
// The baseline parser rejected `app.a[row].b[column] = 42` with: `multiple index segments are unsupported in assignment target near '[column] = 42'`.
const ROOTS: &[&str] = &[
    "main",
    "trap_outer_negative",
    "trap_outer_upper",
    "trap_inner_negative",
    "trap_inner_upper",
];
const TRAP_CHILD: &str = "STASIS_NESTED_VIEW_TRAP_CHILD";
const EXPECTED: i32 = 43;
#[cfg(windows)]
const WINDOWS_ILLEGAL_INSTRUCTION: i32 = 0xC000001D_u32 as i32;

#[cfg(windows)]
fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("canonical repository root")
}

fn required_roots() -> Vec<String> {
    ROOTS.iter().map(|root| (*root).to_string()).collect()
}

#[cfg(windows)]
fn linker_path() -> PathBuf {
    cc::windows_registry::find_tool("x86_64-pc-windows-msvc", "link.exe")
        .map(|tool| tool.path().to_path_buf())
        .filter(|path| path.is_file())
        .expect("MSVC link.exe is required for linked nested-view AOT execution")
}

#[cfg(windows)]
fn run_linked_aot(aot: &AotProcess) {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| repository_root().join("target"));
    let directory = target.join(format!(
        "nested-view-indexing-aot-{}-{stamp}",
        std::process::id()
    ));
    fs::create_dir_all(&directory).expect("create nested-view AOT directory");
    let deps = std::env::current_exe()
        .expect("test executable")
        .parent()
        .expect("Cargo deps directory")
        .to_path_buf();
    let (import, runtime) = [&deps, deps.parent().expect("Cargo profile directory")]
        .into_iter()
        .find_map(|candidate| {
            let import = candidate.join("stasis_dynload.dll.lib");
            let runtime = candidate.join("stasis_dynload.dll");
            (import.is_file() && runtime.is_file()).then_some((import, runtime))
        })
        .expect("fresh dynload artifacts");
    fs::copy(runtime, directory.join("stasis_dynload.dll")).expect("copy dynload runtime");
    let config = AotLinkConfig {
        linker_path: Some(linker_path()),
        runtime_lib_paths: vec![import],
        target: AotTarget::Native,
    };
    for (function, expected) in [
        ("main", Some(EXPECTED)),
        ("trap_outer_negative", None),
        ("trap_outer_upper", None),
        ("trap_inner_negative", None),
        ("trap_inner_upper", None),
    ] {
        let executable = directory.join(format!("nested_view_{function}.exe"));
        aot.link_executable_for_i32_noarg_function(function, &executable, &config)
            .unwrap_or_else(|error| panic!("link nested-view AOT {function}: {error}"));
        let status = Command::new(repository_root().join(".cargo/stasis-sign-and-run.cmd"))
            .arg(executable.file_name().expect("linked executable name"))
            .current_dir(&directory)
            .status()
            .unwrap_or_else(|error| panic!("run linked nested-view AOT {function}: {error}"));
        if let Some(expected) = expected {
            assert_eq!(status.code(), Some(expected), "linked AOT result parity");
        } else {
            assert_eq!(
                status.code(),
                Some(WINDOWS_ILLEGAL_INSTRUCTION),
                "linked AOT {function} must preserve the fatal bounds trap"
            );
        }
    }
    let _ = fs::remove_dir_all(directory);
}

fn run_isolated_jit_trap(function: &str) {
    let executable = std::env::current_exe().expect("test executable");
    #[cfg(windows)]
    let output = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-Command",
            "Add-Type 'using System; using System.Runtime.InteropServices; public static class StasisNestedTrapMode { [DllImport(\"kernel32.dll\")] public static extern uint SetErrorMode(uint mode); }'; [StasisNestedTrapMode]::SetErrorMode(3) | Out-Null; & $env:STASIS_NESTED_VIEW_TEST_EXE --exact nested_view_indexing_matches_jit_linked_aot_and_executable_wasm --nocapture; exit $LASTEXITCODE",
        ])
        .env("STASIS_NESTED_VIEW_TEST_EXE", &executable)
        .env(TRAP_CHILD, function)
        .output()
        .expect("run isolated nested-view JIT trap child");
    #[cfg(not(windows))]
    let output = Command::new(executable)
        .args([
            "--exact",
            "nested_view_indexing_matches_jit_linked_aot_and_executable_wasm",
            "--nocapture",
        ])
        .env(TRAP_CHILD, function)
        .output()
        .expect("run isolated nested-view JIT trap child");
    #[cfg(windows)]
    assert_eq!(
        output.status.code(),
        Some(WINDOWS_ILLEGAL_INSTRUCTION),
        "JIT {function} must preserve the fatal bounds trap; stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(
            output.status.signal(),
            Some(4),
            "JIT {function} bounds trap"
        );
    }
}

#[test]
fn nested_view_indexing_matches_jit_linked_aot_and_executable_wasm() {
    let roots = required_roots();

    let mut jit = JitProcess::new();
    jit.set_required_emit_roots(&roots);
    jit.upsert_file(FIXTURE_PATH, FIXTURE);
    jit.compile().expect("compile nested-view JIT fixture");
    if let Some(function) = std::env::var_os(TRAP_CHILD) {
        let function = function.to_string_lossy();
        jit.execute_i32_noarg_by_name(&function)
            .unwrap_or_else(|error| {
                panic!("execute JIT nested-view trap child {function}: {error}")
            });
        panic!("JIT nested-view trap child {function} returned without trapping");
    }
    let layout = jit.state_layout();
    let nested_lane = layout
        .collections
        .iter()
        .find(|collection| collection.path == "app.a")
        .expect("outer struct collection state layout");
    assert_eq!(nested_lane.capacity, ROW_COUNT);
    let values = nested_lane
        .fields
        .iter()
        .find(|field| field.field == "b")
        .expect("flattened inner fixed-array lane");
    assert_eq!(values.storage_type_name(), "i32");
    assert_eq!(values.element_count, Some((ROW_COUNT * VALUE_COUNT) as u64));
    assert!(!layout
        .collections
        .iter()
        .any(|collection| collection.path == "app.a.b"));

    assert_eq!(jit.execute_i32_noarg_by_name("main"), Ok(EXPECTED));
    assert_eq!(
        jit.read_global_collection_scalar("app.a", "b", 5),
        Ok(JitScalarValue::I32(EXPECTED))
    );
    jit.write_global_collection_scalar("app.a", "b", 4, JitScalarValue::I32(44))
        .expect("write flattened nested-array lane through JIT runtime API");
    assert_eq!(
        jit.read_global_collection_scalar("app.a", "b", 4),
        Ok(JitScalarValue::I32(44))
    );

    for function in [
        "trap_outer_negative",
        "trap_outer_upper",
        "trap_inner_negative",
        "trap_inner_upper",
    ] {
        run_isolated_jit_trap(function);
    }

    let mut aot = AotProcess::new();
    aot.set_required_emit_roots(&roots);
    aot.upsert_file(FIXTURE_PATH, FIXTURE);
    aot.compile()
        .expect("compile nested-view native AOT fixture");
    #[cfg(windows)]
    run_linked_aot(&aot);

    let mut wasm = WasmProcess::new();
    wasm.set_required_emit_roots(&roots);
    wasm.upsert_file(FIXTURE_PATH, FIXTURE);
    wasm.compile().expect("compile nested-view Wasm fixture");
    let wasm_path = std::env::temp_dir().join(format!(
        "stasis_nested_view_{}_{}.wasm",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    fs::write(&wasm_path, wasm.module_bytes()).expect("write nested-view Wasm module");
    let output = Command::new("node")
        .args([
            "-e",
            "const fs=require('node:fs'); WebAssembly.instantiate(fs.readFileSync(process.argv[1]), {}).then(({instance})=>{const e=instance.exports;const trap=(name)=>{try{e['stasis_host_v1_'+name]();return false}catch(error){if(!(error instanceof WebAssembly.RuntimeError))throw error;return true}};process.stdout.write([e.main(),trap('trap_outer_negative'),trap('trap_outer_upper'),trap('trap_inner_negative'),trap('trap_inner_upper')].join(','))}).catch(error=>{console.error(error);process.exit(1)})",
        ])
        .arg(&wasm_path)
        .output()
        .expect("execute nested-view Wasm module");
    let _ = fs::remove_file(wasm_path);
    assert!(
        output.status.success(),
        "Node failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "43,true,true,true,true"
    );
}

#[test]
fn nested_index_helper_is_reachable_and_constant_edits_refresh_its_result() {
    let source = "const CHOSEN_COLUMN: i32 = 0; struct Row { b: i32[2]; } struct App { a: Row[1]; } global nested_incremental_app: App; function chosen_column(): i32 { return CHOSEN_COLUMN; } function main(): i32 { nested_incremental_app.a[0].b[0] = 10; nested_incremental_app.a[0].b[1] = 20; return nested_incremental_app.a[0].b[chosen_column()]; }";
    let mut jit = JitProcess::new();
    jit.set_required_emit_roots(&["main".to_string()]);
    jit.upsert_file(FIXTURE_PATH, source);
    jit.compile()
        .expect("compile nested-index helper reachability fixture");
    assert_eq!(jit.execute_i32_noarg_by_name("main"), Ok(10));

    let updated = source.replace(
        "const CHOSEN_COLUMN: i32 = 0;",
        "const CHOSEN_COLUMN: i32 = 1;",
    );
    jit.upsert_file(FIXTURE_PATH, &updated);
    jit.compile()
        .expect("recompile nested-index helper after constant edit");
    assert_eq!(jit.execute_i32_noarg_by_name("main"), Ok(20));
}

const ROW_COUNT: i32 = 2;
const VALUE_COUNT: i32 = 3;
