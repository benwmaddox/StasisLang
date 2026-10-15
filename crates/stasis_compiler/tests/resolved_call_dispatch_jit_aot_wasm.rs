use object::{Object, ObjectSymbol};
use stasis_compiler::backend::{aot::AotProcess, jit::JitProcess, wasm::WasmProcess};
use stasis_jit::{AotLinkConfig, AotTarget};
use std::{fs, path::PathBuf, process::Command};

const MAIN_SOURCE: &str = r#"
import "left.stasis";
import "right.stasis";

global left_item: LeftItem;
global right_item: RightItem;

function main(): i32 {
    let message: string = "right";
    left_item.value = 2;
    right_item.value = 3;
    let receiver_score: i32 = left_item.score();
    let function_score: i32 = score(right_item);
    let chosen_integer: i32 = left.choose(1);
    let chosen_boolean: bool = right.choose(1);
    left.poll(17);
    right.poll(23);
    if (!chosen_boolean) { return 1000; }
    return receiver_score + function_score - 25 + chosen_integer - 101;
}
"#;

const LEFT_SOURCE: &str = r#"
struct LeftItem { value: i32; }
function score(self: LeftItem): i32 { return self.value + 10; }
function choose(value: i32): i32 { return value + 100; }
function @extern("stasis_jit_print_i32") poll(value: i32): void;
"#;

const RIGHT_SOURCE: &str = r#"
struct RightItem { value: i32; }
function score(self: RightItem): i32 { return self.value + 10; }
function choose(value: i32): bool { return value == 1; }
function @extern("stasis_jit_print_string") poll(value: i32): void;
"#;

fn roots() -> Vec<String> {
    vec!["main".to_string()]
}

fn probe_string_literal_id() -> i32 {
    let mut probe = AotProcess::new();
    probe.set_required_emit_roots(&roots());
    probe.upsert_file("main.stasis", MAIN_SOURCE);
    probe.upsert_file("left.stasis", LEFT_SOURCE);
    probe.upsert_file("right.stasis", RIGHT_SOURCE);
    probe.compile().expect("compile literal-ID probe");
    let (id, value) = probe
        .string_literals()
        .iter()
        .find(|(_, value)| value.as_str() == "right")
        .expect("literal-ID probe contains the right string");
    assert_eq!(value, "right");
    *id
}

fn main_source(literal_id: i32) -> String {
    MAIN_SOURCE.replace("right.poll(23)", &format!("right.poll({literal_id})"))
}

fn configure_jit(process: &mut JitProcess, main: &str) {
    process.set_required_emit_roots(&roots());
    process.upsert_file("main.stasis", main);
    process.upsert_file("left.stasis", LEFT_SOURCE);
    process.upsert_file("right.stasis", RIGHT_SOURCE);
}

fn configure_aot(process: &mut AotProcess, main: &str) {
    process.set_required_emit_roots(&roots());
    process.upsert_file("main.stasis", main);
    process.upsert_file("left.stasis", LEFT_SOURCE);
    process.upsert_file("right.stasis", RIGHT_SOURCE);
}

fn configure_wasm(process: &mut WasmProcess, main: &str) {
    process.set_required_emit_roots(&roots());
    process.upsert_file("main.stasis", main);
    process.upsert_file("left.stasis", LEFT_SOURCE);
    process.upsert_file("right.stasis", RIGHT_SOURCE);
}

struct JitOutputCapture;

impl JitOutputCapture {
    fn start() -> Self {
        stasis_dynload::set_jit_output_capture(true);
        Self
    }
}

impl Drop for JitOutputCapture {
    fn drop(&mut self) {
        stasis_dynload::set_jit_output_capture(false);
    }
}

fn run_wasm(wasm: &WasmProcess) -> String {
    let module_path = std::env::temp_dir().join(format!(
        "stasis_resolved_calls_{}_{}.wasm",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    fs::write(&module_path, wasm.module_bytes()).expect("write resolved-call Wasm module");
    let node = std::env::var_os("STASIS_NODE").unwrap_or_else(|| "node".into());
    let output = Command::new(node)
        .args([
            "-e",
            "const fs=require('node:fs');const m=new WebAssembly.Module(fs.readFileSync(process.argv[1]));const seen=[];const i=new WebAssembly.Instance(m,{env:{stasis_jit_print_i32:x=>seen.push('i32:'+x),stasis_jit_print_string:x=>seen.push('string:'+x)}});process.stdout.write(String(i.exports.main())+'|'+seen.join(','));",
        ])
        .arg(&module_path)
        .output()
        .expect("run Node Wasm resolved-call fixture");
    let _ = fs::remove_file(&module_path);
    assert!(
        output.status.success(),
        "Node failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("Wasm output is UTF-8")
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("canonical repository root")
}

fn run_linked_aot(aot: &AotProcess, expected_output: &str) {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| repository_root().join("target"));
    let directory = target.join(format!("resolved-call-aot-{}-{stamp}", std::process::id()));
    fs::create_dir_all(&directory).expect("create linked AOT directory");
    let deps = std::env::current_exe()
        .expect("test executable")
        .parent()
        .expect("Cargo deps directory")
        .to_path_buf();
    let profile = deps.parent().expect("Cargo profile directory");
    let (import, runtime) = [&deps, profile]
        .into_iter()
        .find_map(|candidate| {
            let (import_name, runtime_name) = if cfg!(windows) {
                ("stasis_dynload.dll.lib", "stasis_dynload.dll")
            } else if cfg!(target_os = "macos") {
                ("libstasis_dynload.dylib", "libstasis_dynload.dylib")
            } else {
                ("libstasis_dynload.so", "libstasis_dynload.so")
            };
            let import = candidate.join(import_name);
            let runtime = candidate.join(runtime_name);
            (import.is_file() && runtime.is_file()).then_some((import, runtime))
        })
        .expect("fresh dynload runtime and link library");
    fs::copy(
        &runtime,
        directory.join(runtime.file_name().expect("runtime library name")),
    )
    .expect("copy dynload runtime");
    let executable = directory.join(format!("resolved_call_aot{}", std::env::consts::EXE_SUFFIX));
    #[cfg(windows)]
    let linker_path = Some(
        std::env::var_os("STASIS_AOT_LINKER")
            .map(PathBuf::from)
            .or_else(|| {
                cc::windows_registry::find_tool("x86_64-pc-windows-msvc", "link.exe")
                    .map(|tool| tool.path().to_path_buf())
            })
            .expect("MSVC link.exe is required for linked AOT execution"),
    );
    #[cfg(not(windows))]
    let linker_path = std::env::var_os("STASIS_AOT_LINKER").map(PathBuf::from);
    aot.link_executable_for_i32_noarg_function(
        "main",
        &executable,
        &AotLinkConfig {
            linker_path,
            runtime_lib_paths: vec![import],
            target: AotTarget::Native,
        },
    )
    .expect("link resolved-call AOT executable");
    #[cfg(windows)]
    let output = Command::new(repository_root().join(".cargo/stasis-sign-and-run.cmd"))
        .arg(executable.file_name().expect("linked executable name"))
        .current_dir(&directory)
        .output()
        .expect("run linked resolved-call AOT executable");
    #[cfg(not(windows))]
    let output = {
        let variable = if cfg!(target_os = "macos") {
            "DYLD_LIBRARY_PATH"
        } else {
            "LD_LIBRARY_PATH"
        };
        let mut paths = vec![directory.clone()];
        if let Some(existing) = std::env::var_os(variable) {
            paths.extend(std::env::split_paths(&existing));
        }
        Command::new(&executable)
            .current_dir(&directory)
            .env(
                variable,
                std::env::join_paths(paths).expect("join runtime search path"),
            )
            .output()
            .expect("run linked resolved-call AOT executable")
    };
    let _ = fs::remove_dir_all(&directory);
    assert!(
        output.status.success(),
        "linked AOT failed: status={} stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.ends_with(expected_output),
        "linked AOT extern dispatch output should end with {expected_output:?}; got {stdout:?}"
    );
}

#[test]
fn resolved_identity_selects_exact_extern_and_internal_calls() {
    let literal_id = probe_string_literal_id();
    let main_source = main_source(literal_id);

    let mut jit = JitProcess::new();
    configure_jit(&mut jit, &main_source);
    jit.compile().expect("compile resolved-call JIT fixture");
    assert_eq!(
        jit.program_snapshot()
            .unwrap()
            .literal_table()
            .get(&literal_id)
            .map(String::as_str),
        Some("right")
    );
    let capture = JitOutputCapture::start();
    let jit_result = jit
        .execute_i32_noarg_by_name("main")
        .expect("execute resolved-call JIT fixture");
    let jit_output = stasis_dynload::drain_jit_output();
    drop(capture);
    assert_eq!(jit_result, 0, "receiver and function-form internal calls");
    assert_eq!(jit_output, "17right", "exact extern symbols in JIT");

    let mut aot = AotProcess::new();
    configure_aot(&mut aot, &main_source);
    aot.compile().expect("compile resolved-call AOT fixture");
    assert_eq!(
        aot.string_literals().get(&literal_id).map(String::as_str),
        Some("right")
    );
    let object_dir = std::env::temp_dir().join(format!(
        "stasis_resolved_calls_aot_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    let objects = aot
        .write_object_files_by_id(&object_dir)
        .expect("write resolved-call AOT objects");
    let mut undefined = std::collections::BTreeSet::new();
    for (_, path) in objects.values() {
        let bytes = fs::read(path).expect("read resolved-call AOT object");
        let object = object::File::parse(bytes.as_slice()).expect("parse resolved-call AOT object");
        undefined.extend(
            object
                .symbols()
                .filter(|symbol| symbol.is_undefined())
                .filter_map(|symbol| symbol.name().ok().map(str::to_string)),
        );
    }
    let _ = fs::remove_dir_all(&object_dir);
    for expected in ["stasis_jit_print_i32", "stasis_jit_print_string"] {
        assert!(
            undefined
                .iter()
                .any(|symbol| symbol.trim_start_matches('_').ends_with(expected)),
            "AOT object imports exact extern symbol {expected}; got {undefined:?}"
        );
    }
    run_linked_aot(&aot, "17right");

    let mut wasm = WasmProcess::new();
    configure_wasm(&mut wasm, &main_source);
    wasm.compile().expect("compile resolved-call Wasm fixture");
    assert_eq!(run_wasm(&wasm), format!("0|i32:17,string:{literal_id}"));
}
