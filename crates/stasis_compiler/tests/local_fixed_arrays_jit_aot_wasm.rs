use stasis_compiler::backend::{aot::AotProcess, jit::JitProcess, wasm::WasmProcess};
#[cfg(windows)]
use stasis_jit::{AotLinkConfig, AotTarget};
use std::fs;
#[cfg(windows)]
use std::path::{Path, PathBuf};
use std::process::Command;

const FIXTURE_PATH: &str = "tests/fixtures/local_fixed_arrays/main.stasis";
const FIXTURE: &str = include_str!("../../../tests/fixtures/local_fixed_arrays/main.stasis");
const ROOTS: &[&str] = &[
    "main",
    "tick",
    "render",
    "trap_negative_entry",
    "trap_upper_entry",
];
const EXPECTED: i32 = 146;
const TRAP_CHILD: &str = "STASIS_LOCAL_FIXED_ARRAY_TRAP_CHILD";
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

fn assert_shared_compile_rejection(source: &str, expected: &str) {
    let roots = vec!["main".to_string()];

    let mut jit = JitProcess::new();
    jit.set_required_emit_roots(&roots);
    jit.upsert_file("shared_rejection.stasis", source);
    let jit_error = jit
        .compile()
        .expect_err("JIT must reject invalid local array contract");
    let jit_diagnostic = format!("{jit_error:?}");
    assert!(
        jit_diagnostic.contains(expected),
        "JIT diagnostic: {jit_diagnostic}"
    );

    let mut aot = AotProcess::new();
    aot.set_required_emit_roots(&roots);
    aot.upsert_file("shared_rejection.stasis", source);
    let aot_error = aot
        .compile()
        .expect_err("AOT must reject invalid local array contract");
    let aot_diagnostic = format!("{aot_error:?}");
    assert!(
        aot_diagnostic.contains(expected),
        "AOT diagnostic: {aot_diagnostic}"
    );

    let mut wasm = WasmProcess::new();
    wasm.set_required_emit_roots(&roots);
    wasm.upsert_file("shared_rejection.stasis", source);
    let wasm_error = wasm
        .compile()
        .expect_err("Wasm must reject invalid local array contract");
    let wasm_diagnostic = format!("{wasm_error:?}");
    assert!(
        wasm_diagnostic.contains(expected),
        "Wasm diagnostic: {wasm_diagnostic}"
    );
}

#[cfg(windows)]
fn linker_path() -> PathBuf {
    if let Some(path) = cc::windows_registry::find_tool("x86_64-pc-windows-msvc", "link.exe")
        .map(|tool| tool.path().to_path_buf())
        .filter(|path| path.is_file())
    {
        return path;
    }
    panic!("MSVC link.exe is required for linked local-array AOT execution");
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
        "local-fixed-arrays-aot-{}-{stamp}",
        std::process::id()
    ));
    fs::create_dir_all(&directory).expect("create local-array AOT directory");
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
        ("trap_negative_entry", None),
        ("trap_upper_entry", None),
    ] {
        let executable = directory.join(format!("local_fixed_arrays_{function}.exe"));
        aot.link_executable_for_i32_noarg_function(function, &executable, &config)
            .unwrap_or_else(|error| panic!("link local-array AOT {function}: {error}"));
        let status = Command::new(repository_root().join(".cargo/stasis-sign-and-run.cmd"))
            .arg(executable.file_name().expect("linked executable name"))
            .current_dir(&directory)
            .status()
            .unwrap_or_else(|error| panic!("run linked local-array AOT {function}: {error}"));
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
            "Add-Type 'using System; using System.Runtime.InteropServices; public static class StasisTrapMode { [DllImport(\"kernel32.dll\")] public static extern uint SetErrorMode(uint mode); }'; [StasisTrapMode]::SetErrorMode(3) | Out-Null; & $env:STASIS_LOCAL_ARRAY_TEST_EXE --exact local_fixed_arrays_match_jit_linked_aot_and_executable_wasm --nocapture; exit $LASTEXITCODE",
        ])
        .env("STASIS_LOCAL_ARRAY_TEST_EXE", &executable)
        .env(TRAP_CHILD, function)
        .output()
        .expect("run isolated local-array JIT trap child");
    #[cfg(not(windows))]
    let output = Command::new(executable)
        .args([
            "--exact",
            "local_fixed_arrays_match_jit_linked_aot_and_executable_wasm",
            "--nocapture",
        ])
        .env(TRAP_CHILD, function)
        .output()
        .expect("run isolated local-array JIT trap child");
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
fn local_fixed_arrays_match_jit_linked_aot_and_executable_wasm() {
    let roots = required_roots();

    let mut jit = JitProcess::new();
    jit.set_required_emit_roots(&roots);
    jit.upsert_file(FIXTURE_PATH, FIXTURE);
    jit.compile().expect("compile local-array JIT fixture");
    if let Some(function) = std::env::var_os(TRAP_CHILD) {
        let function = function.to_string_lossy();
        jit.execute_i32_noarg_by_name(&function)
            .unwrap_or_else(|error| panic!("execute JIT trap child {function}: {error}"));
        panic!("JIT trap child {function} returned without a fatal bounds trap");
    }
    assert_eq!(jit.execute_i32_noarg_by_name("main"), Ok(EXPECTED));
    assert_eq!(jit.execute_i32_noarg_by_name("tick"), Ok(EXPECTED));
    run_isolated_jit_trap("trap_negative_entry");
    run_isolated_jit_trap("trap_upper_entry");

    let mut aot = AotProcess::new();
    aot.set_required_emit_roots(&roots);
    aot.upsert_file(FIXTURE_PATH, FIXTURE);
    aot.compile()
        .expect("compile local-array native AOT fixture");
    #[cfg(windows)]
    run_linked_aot(&aot);

    let mut wasm = WasmProcess::new();
    wasm.set_required_emit_roots(&roots);
    wasm.upsert_file(FIXTURE_PATH, FIXTURE);
    wasm.compile().expect("compile local-array Wasm fixture");
    let wasm_path = std::env::temp_dir().join(format!(
        "stasis_local_fixed_arrays_{}_{}.wasm",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    fs::write(&wasm_path, wasm.module_bytes()).expect("write local-array Wasm module");
    let output = Command::new("node")
        .args([
            "-e",
            "const fs=require('node:fs'); WebAssembly.instantiate(fs.readFileSync(process.argv[1]), {}).then(({instance})=>{const e=instance.exports;const traps=(index)=>{try{e.render(index);return false}catch(error){if(!(error instanceof WebAssembly.RuntimeError))throw error;return true}};process.stdout.write([e.main(),e.tick(),traps(-1),traps(9)].join(','))}).catch(error=>{console.error(error);process.exit(1)})",
        ])
        .arg(&wasm_path)
        .output()
        .expect("execute local-array Wasm module");
    let _ = fs::remove_file(wasm_path);
    assert!(
        output.status.success(),
        "Node failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "146,146,true,true");
}

#[test]
fn owned_local_fixed_arrays_cannot_escape_into_array_views() {
    let cases = [
        (
            r#"
function consume(values: i32[]): i32 {
    return 0;
}

function main(): i32 {
    let values: i32[2];
    values[0] = 7;
    return consume(values);
}
"#,
            "owned local fixed array 'values' is non-escaping",
        ),
        (
            r#"
function consume(values: i32[2]): i32 {
    return 0;
}

function main(): i32 {
    let values: i32[2];
    return consume(values);
}
"#,
            "owned local fixed array 'values' is non-escaping",
        ),
        (
            r#"
function main(): i32 {
    let values: i32[2];
    let alias: i32[2] = values;
    return 0;
}
"#,
            "owned local fixed array 'values' is non-escaping",
        ),
        (
            r#"
global backing: i32[2];

function main(): i32 {
    let values: i32[2];
    values = backing;
    return 0;
}
"#,
            "owned local fixed array 'values' is non-escaping and cannot be rebound",
        ),
        (
            r#"
function main(): i32[2] {
    let values: i32[2];
    return values;
}
"#,
            "owned local fixed array 'values' is non-escaping",
        ),
    ];
    for (source, expected) in cases {
        assert_shared_compile_rejection(source, expected);
    }
}

#[test]
fn owned_local_named_struct_arrays_are_rejected_uniformly() {
    assert_shared_compile_rejection(
        r#"
struct Entity {
    score: i32;
}

function main(): i32 {
    let items: Entity[2];
    return 0;
}
"#,
        "owned local fixed array 'items' requires a primitive scalar element type",
    );
}

#[test]
fn same_named_owned_and_borrowed_local_declarations_are_rejected_uniformly() {
    assert_shared_compile_rejection(
        r#"
global backing: i32[2];

function main(flag: bool): i32 {
    if (flag) {
        let values: i32[2];
        values[0] = 1;
    } else {
        let values: i32[2] = backing;
        return values[0];
    }
    return 0;
}
"#,
        "local 'values' cannot mix owned fixed-array and borrowed declarations",
    );
}
