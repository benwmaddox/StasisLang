#[cfg(windows)]
use stasis_compiler::backend::aot::AotProcess;
use stasis_compiler::backend::jit::JitProcess;
use stasis_compiler::backend::wasm::WasmProcess;
use stasis_compiler::frontend::parser::rewrite_top_level_test_declarations;
#[cfg(windows)]
use stasis_jit::{AotLinkConfig, AotTarget};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const FIXTURE_PATH: &str = "tests/stasis/seams/rig2d_probe.stasis";
const FIXTURE: &str = include_str!("../../../tests/stasis/seams/rig2d_probe.stasis");
const ROOT: &str = "main";
const STASIS_TEST_PATH: &str = "tests/stasis/rig2d.test.stasis";
const STASIS_TESTS: &str = include_str!("../../../tests/stasis/rig2d.test.stasis");

#[cfg(windows)]
struct AotTree(PathBuf);

#[cfg(windows)]
impl Drop for AotTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct WasmModuleFile(PathBuf);

impl Drop for WasmModuleFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("canonical repository root")
}

#[cfg(windows)]
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

#[cfg(windows)]
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
fn rig2d_two_bone_ik_oracle_matches_jit_and_node_wasm() {
    let root = repository_root();
    let mut jit = JitProcess::new();
    jit.set_project_root(root.to_string_lossy())
        .expect("set JIT project root");
    jit.set_required_emit_roots(&[ROOT.to_string()]);
    jit.upsert_file(FIXTURE_PATH, FIXTURE);
    jit.compile().expect("compile rig2d JIT fixture");
    let jit_result = jit
        .execute_i32_noarg_by_name(ROOT)
        .expect("execute rig2d JIT fixture");

    let mut wasm = WasmProcess::new();
    wasm.set_project_root(root.to_string_lossy())
        .expect("set Web project root");
    wasm.set_required_emit_roots(&[ROOT.to_string()]);
    wasm.upsert_file(FIXTURE_PATH, FIXTURE);
    wasm.compile().expect("compile rig2d Web fixture");
    assert!(
        wasm.module_bytes().starts_with(b"\0asm\x01\0\0\0"),
        "rig2d Web fixture must produce a valid WebAssembly module"
    );
    let wasm_result = run_node_wasm_main(&wasm);

    assert_eq!(jit_result, 0, "JIT fixture failure code");
    assert_eq!(wasm_result, jit_result, "JIT/Node Wasm result parity");
}

#[cfg(windows)]
#[test]
fn rig2d_two_bone_ik_oracle_matches_jit_and_linked_aot() {
    let root = repository_root();
    let mut jit = JitProcess::new();
    jit.set_project_root(root.to_string_lossy())
        .expect("set JIT project root");
    jit.set_required_emit_roots(&[ROOT.to_string()]);
    jit.upsert_file(FIXTURE_PATH, FIXTURE);
    jit.compile().expect("compile rig2d JIT fixture");
    let jit_result = jit
        .execute_i32_noarg_by_name(ROOT)
        .expect("execute rig2d JIT fixture");

    let mut aot = AotProcess::new();
    aot.set_project_root(root.to_string_lossy())
        .expect("set AOT project root");
    aot.set_required_emit_roots(&[ROOT.to_string()]);
    aot.upsert_file(FIXTURE_PATH, FIXTURE);
    aot.compile().expect("compile rig2d AOT fixture");
    let output_dir = AotTree(
        std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| root.join("target"))
            .join(format!("rig2d-aot-{}", std::process::id())),
    );
    fs::create_dir_all(&output_dir.0).expect("create AOT evidence directory");
    let (import, runtime) = dynload_artifacts();
    fs::copy(runtime, output_dir.0.join("stasis_dynload.dll")).expect("copy AOT runtime");
    let linked = output_dir.0.join("rig2d_probe.exe");
    let config = AotLinkConfig {
        linker_path: Some(linker_path()),
        runtime_lib_paths: vec![import],
        target: AotTarget::Native,
    };
    aot.link_executable_for_i32_noarg_function(ROOT, &linked, &config)
        .expect("link rig2d AOT fixture");
    let status = Command::new(root.join(".cargo/stasis-sign-and-run.cmd"))
        .arg(linked.file_name().expect("linked AOT executable name"))
        .current_dir(&output_dir.0)
        .status()
        .expect("run linked rig2d AOT fixture");

    assert_eq!(jit_result, 0, "JIT fixture failure code");
    let aot_code = status.code().expect("linked AOT process exit code");
    assert!(
        status.success(),
        "linked AOT executable returned exit code {aot_code}"
    );
    assert_eq!(aot_code, jit_result, "JIT/AOT result parity");
}

fn run_node_wasm_main(wasm: &WasmProcess) -> i32 {
    let path = std::env::temp_dir().join(format!(
        "stasis_rig2d_{}_{}.wasm",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    let module_file = WasmModuleFile(path);
    fs::write(&module_file.0, wasm.module_bytes()).expect("write rig2d Wasm module");
    let node = std::env::var_os("STASIS_NODE").unwrap_or_else(|| "node".into());
    let output = Command::new(node)
        .args([
            "-e",
            "const fs=require('node:fs');const m=new WebAssembly.Module(fs.readFileSync(process.argv[1]));const imports=WebAssembly.Module.imports(m);if(imports.length!==0){throw new Error('unexpected imports: '+JSON.stringify(imports))}const i=new WebAssembly.Instance(m,{});process.stdout.write(String(i.exports.main()));",
        ])
        .arg(&module_file.0)
        .output()
        .expect("execute rig2d Wasm in Node");
    assert!(
        output.status.success(),
        "Node failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .parse()
        .expect("rig2d Wasm main result")
}

#[test]
fn rig2d_stasis_tests_pass_in_the_production_jit_test_shape() {
    let (rewritten, tests) =
        rewrite_top_level_test_declarations(STASIS_TESTS).expect("discover Stasis tests");
    assert_eq!(tests.len(), 21, "focused behavior test count");
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
