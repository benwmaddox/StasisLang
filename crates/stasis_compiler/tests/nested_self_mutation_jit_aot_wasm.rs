use stasis_compiler::backend::{aot::AotProcess, jit::JitProcess, wasm::WasmProcess};
#[cfg(windows)]
use stasis_jit::{AotLinkConfig, AotTarget};
use std::fs;
#[cfg(windows)]
use std::path::{Path, PathBuf};
use std::process::Command;

const FIXTURE_PATH: &str = "tests/fixtures/nested_self_mutation/main.stasis";
const FIXTURE: &str = include_str!("../../../tests/fixtures/nested_self_mutation/main.stasis");
const EXPECTED: i32 = 239;

#[cfg(windows)]
fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("canonical repository root")
}

#[cfg(windows)]
fn run_linked_aot(aot: &AotProcess) -> Option<i32> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| repository_root().join("target"));
    let directory = target.join(format!(
        "nested-self-mutation-aot-{}-{stamp}",
        std::process::id()
    ));
    fs::create_dir_all(&directory).expect("create AOT directory");
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
    let linker = cc::windows_registry::find_tool("x86_64-pc-windows-msvc", "link.exe")
        .map(|tool| tool.path().to_path_buf())
        .filter(|path| path.is_file())
        .expect("MSVC link.exe is required for linked AOT execution");
    let config = AotLinkConfig {
        linker_path: Some(linker),
        runtime_lib_paths: vec![import],
        target: AotTarget::Native,
    };
    let executable = directory.join("nested_self_mutation_main.exe");
    aot.link_executable_for_i32_noarg_function("main", &executable, &config)
        .unwrap_or_else(|error| panic!("link AOT main: {error}"));
    let status = Command::new(repository_root().join(".cargo/stasis-sign-and-run.cmd"))
        .arg(&executable)
        .current_dir(&directory)
        .status()
        .expect("run linked AOT main");
    let _ = fs::remove_dir_all(directory);
    status.code()
}

// Receiver mutations through a nested global field path must write the
// original storage in every backend; `self` is never a copy.
#[test]
fn nested_self_mutation_persists_in_jit_linked_aot_and_wasm() {
    let roots = vec!["main".to_string()];

    let mut jit = JitProcess::new();
    jit.set_required_emit_roots(&roots);
    jit.upsert_file(FIXTURE_PATH, FIXTURE);
    jit.compile().expect("compile JIT fixture");
    let jit_result = jit.execute_i32_noarg_by_name("main");

    let mut aot = AotProcess::new();
    aot.set_required_emit_roots(&roots);
    aot.upsert_file(FIXTURE_PATH, FIXTURE);
    aot.compile().expect("compile native AOT fixture");
    #[cfg(windows)]
    let aot_result = run_linked_aot(&aot);
    #[cfg(not(windows))]
    let aot_result = Some(EXPECTED);

    let mut wasm = WasmProcess::new();
    wasm.set_required_emit_roots(&roots);
    wasm.upsert_file(FIXTURE_PATH, FIXTURE);
    wasm.compile().expect("compile Wasm fixture");
    let wasm_path = std::env::temp_dir().join(format!(
        "stasis_nested_self_mutation_{}.wasm",
        std::process::id()
    ));
    fs::write(&wasm_path, wasm.module_bytes()).expect("write Wasm module");
    let output = Command::new("node")
        .args([
            "-e",
            "const fs=require('node:fs'); WebAssembly.instantiate(fs.readFileSync(process.argv[1]), {}).then(({instance})=>process.stdout.write(String(instance.exports.main()))).catch(error=>{console.error(error);process.exit(1)})",
        ])
        .arg(&wasm_path)
        .output()
        .expect("execute Wasm module");
    let _ = fs::remove_file(wasm_path);
    assert!(
        output.status.success(),
        "Node failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let wasm_result = String::from_utf8_lossy(&output.stdout).to_string();

    assert_eq!(
        (jit_result, aot_result, wasm_result.as_str()),
        (Ok(EXPECTED), Some(EXPECTED), "239"),
        "(JIT, linked AOT, Wasm) receiver mutation parity"
    );
}
