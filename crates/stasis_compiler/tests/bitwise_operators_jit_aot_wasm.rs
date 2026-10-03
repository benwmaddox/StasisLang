use stasis_compiler::backend::{aot::AotProcess, jit::JitProcess, wasm::WasmProcess};
use std::fs;
use std::process::Command;

const ORACLE_SOURCE: &str =
    include_str!("../../../samples/generics_collections/src/bitwise_oracle.stasis");
const MAIN_SOURCE: &str = r#"
import "bitwise_oracle.stasis";

function main(): i32 {
    return bitwise_operator_oracle();
}
"#;

#[test]
fn shifted_value_arguments_specialize_through_the_public_jit_pipeline() {
    const SOURCE: &str = r#"
struct Bits<N: i32> { values: i32[N]; }
global left: Bits<(1 << 3)>;
global right: Bits<(8 >> 1)>;

function capacity(buffer: Bits<N>): i32 { return N; }

function main(): i32 {
    left.values[7] = 8;
    right.values[3] = 4;
    return left.values[7] + right.values[3] + capacity(left) + capacity(right) - 24;
}
"#;
    let mut jit = JitProcess::new();
    jit.set_required_emit_roots(&["main".to_string()]);
    jit.upsert_file("main.stasis", SOURCE);
    jit.compile()
        .expect("compile shifted generic value arguments in JIT");
    assert_eq!(
        jit.execute_i32_noarg_by_name("main"),
        Ok(0),
        "shifted generic array extents and value arguments"
    );
}

fn compile_oracle() -> (JitProcess, AotProcess, WasmProcess) {
    let roots = vec!["main".to_string()];

    let mut jit = JitProcess::new();
    jit.set_required_emit_roots(&roots);
    jit.upsert_file("bitwise_oracle.stasis", ORACLE_SOURCE);
    jit.upsert_file("main.stasis", MAIN_SOURCE);
    jit.compile().expect("compile bitwise oracle JIT");

    let mut aot = AotProcess::new();
    aot.set_required_emit_roots(&roots);
    aot.upsert_file("bitwise_oracle.stasis", ORACLE_SOURCE);
    aot.upsert_file("main.stasis", MAIN_SOURCE);
    aot.compile().expect("compile bitwise oracle native AOT");

    let mut wasm = WasmProcess::new();
    wasm.set_required_emit_roots(&roots);
    wasm.upsert_file("bitwise_oracle.stasis", ORACLE_SOURCE);
    wasm.upsert_file("main.stasis", MAIN_SOURCE);
    wasm.compile().expect("compile bitwise oracle WebAssembly");

    (jit, aot, wasm)
}

#[test]
fn bitwise_oracle_matches_jit_linked_native_aot_and_node_wasm() {
    let (jit, aot, wasm) = compile_oracle();
    assert_eq!(
        jit.execute_i32_noarg_by_name("main"),
        Ok(0),
        "bitwise oracle case id"
    );

    #[cfg(windows)]
    run_linked_native_aot(&aot);
    #[cfg(not(windows))]
    let _ = aot;

    run_node_wasm(&wasm);
}

fn compile_error(source: &str) -> String {
    let mut process = JitProcess::new();
    process.set_required_emit_roots(&["main".to_string()]);
    process.upsert_file("main.stasis", source);
    let error = process.compile().expect_err(&format!(
        "invalid bitwise fixture must fail compilation: {source}"
    ));
    format!("{error:?}")
}

fn frontend_error(source: &str) -> String {
    let diagnostic = compile_error(source);
    assert!(
        diagnostic.starts_with("Frontend("),
        "expected a frontend diagnostic for {source:?}, got {diagnostic}"
    );
    diagnostic
}

#[test]
fn bitwise_rejections_report_the_operand_or_lane_error() {
    for (source, expected) in [
        (
            "function main(): i32 { let value: i32 = 1; return value & true; }",
            "operator '&' requires integer operands",
        ),
        (
            "function main(): i32 { let value: f32 = 1.0; return value | 1; }",
            "operator '|' requires integer operands",
        ),
        (
            "function main(): i32 { let left: u8 = 1; let right: u16 = 1; return left ^ right; }",
            "operator '^' requires operands in the same integer lane",
        ),
        (
            "function main(): i32 { let left: u8 = 1; return left | 256; }",
            "integer literal 256 is not representable in contextual lane u8",
        ),
        (
            "function main(): i32 { let value: u8 = 1 | 256 | 0; return value; }",
            "integer literal 256 is not representable in contextual lane u8",
        ),
        (
            "function main(): i32 { let value: u8 = 1 | -1; return value; }",
            "integer literal -1 is not representable in contextual lane u8",
        ),
        (
            "function main(): i32 { let value: u8 = -1 | 1; return value; }",
            "integer literal -1 is not representable in contextual lane u8",
        ),
        (
            "function main(): i32 { let value: u8 = ~(-1); return value; }",
            "integer literal -1 is not representable in contextual lane u8",
        ),
        (
            "struct Token { value: i32; } global token: Token; function main(): i32 { return token & 1; }",
            "operator '&' requires integer operands",
        ),
        (
            "function invalid(a: i32, b: i32, c: i32): bool { return a & b == c; } function main(): i32 { return 0; }",
            "operator '&' requires integer operands",
        ),
        (
            "function main(): i32 { let value: u8 = 1; return value << true; }",
            "operator '<<' requires integer operands",
        ),
    ] {
        let diagnostic = frontend_error(source);
        assert!(
            diagnostic.contains(expected),
            "expected diagnostic fragment {expected:?}, got {diagnostic}"
        );
    }
}

#[test]
fn bitwise_compound_assignments_remain_unsupported() {
    for operator in ["&=", "|=", "^=", "<<=", ">>="] {
        let source = format!(
            "global value: i32; function main(): i32 {{ value {operator} 1; return value; }}"
        );
        let diagnostic = compile_error(&source);
        assert!(
            diagnostic.contains("unsupported assignment operator"),
            "compound form {operator} should remain unsupported: {diagnostic}"
        );
    }
}

fn run_node_wasm(wasm: &WasmProcess) {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "stasis_bitwise_{}_{}.wasm",
        std::process::id(),
        stamp
    ));
    fs::write(&path, wasm.module_bytes()).expect("write bitwise Wasm module");
    let output = Command::new("node")
        .args([
            "-e",
            "const fs=require('node:fs');const m=new WebAssembly.Module(fs.readFileSync(process.argv[1]));const imports=WebAssembly.Module.imports(m);if(imports.length!==0){throw new Error('unexpected imports: '+JSON.stringify(imports))}const i=new WebAssembly.Instance(m,{});process.stdout.write(String(i.exports.main()));",
        ])
        .arg(&path)
        .output()
        .expect("run Node Wasm bitwise oracle");
    let _ = fs::remove_file(path);
    assert!(
        output.status.success(),
        "Node failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "0",
        "bitwise oracle case id"
    );
}

#[cfg(windows)]
fn repository_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("canonical repository root")
}

#[cfg(windows)]
fn linker_path() -> std::path::PathBuf {
    cc::windows_registry::find_tool("x86_64-pc-windows-msvc", "link.exe")
        .map(|tool| tool.path().to_path_buf())
        .filter(|path| path.is_file())
        .expect("MSVC link.exe is required for linked bitwise AOT execution")
}

#[cfg(windows)]
fn run_linked_native_aot(aot: &AotProcess) {
    use stasis_jit::{AotLinkConfig, AotTarget};
    use std::path::PathBuf;

    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| repository_root().join("target"));
    let directory = target.join(format!("bitwise-aot-{}-{stamp}", std::process::id()));
    fs::create_dir_all(&directory).expect("create bitwise AOT directory");
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
    let executable = directory.join("bitwise_aot.exe");
    aot.link_executable_for_i32_noarg_function(
        "main",
        &executable,
        &AotLinkConfig {
            linker_path: Some(linker_path()),
            runtime_lib_paths: vec![import],
            target: AotTarget::Native,
        },
    )
    .expect("link bitwise AOT executable");
    let status = Command::new(repository_root().join(".cargo/stasis-sign-and-run.cmd"))
        .arg(executable.file_name().expect("linked executable name"))
        .current_dir(&directory)
        .status()
        .expect("run linked bitwise AOT executable");
    let _ = fs::remove_dir_all(directory);
    assert_eq!(status.code(), Some(0), "linked AOT bitwise oracle case id");
}
