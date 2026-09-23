use stasis_compiler::backend::jit::JitProcess;

const SOURCE: &str = "global observed: i32; function main(): void { observed = 10; } function @host_export(set_access) set_access(a: i32, b: i32, enabled: bool): void { if (enabled) { observed = a + b; } } function tick(): i32 { return observed; }";

#[test]
fn declared_host_export_is_retained_with_a_stable_symbol() {
    let mut process = JitProcess::new();
    process.upsert_file("host_exports.stasis", SOURCE);
    process.compile().expect("compile host exports");
    assert!(process
        .symbol_code_ptrs()
        .contains_key("stasis_host_v1_set_access"));
    assert!(!process.symbol_code_ptrs().contains_key("set_access"));
    use stasis_compiler::host_exports::HostValue::{Bool, I32};
    process.execute_void_noarg_by_name("main").unwrap();
    process
        .invoke_host_export("set_access", &[I32(20), I32(22), Bool(false)])
        .unwrap();
    assert_eq!(process.execute_i32_noarg_by_name("tick").unwrap(), 10);
    assert!(process
        .invoke_host_export("set_access", &[I32(20), I32(22), I32(1)])
        .is_err());
    assert_eq!(process.execute_i32_noarg_by_name("tick").unwrap(), 10);
    process
        .invoke_host_export("set_access", &[I32(20), I32(22), Bool(true)])
        .unwrap();
    assert_eq!(process.execute_i32_noarg_by_name("tick").unwrap(), 42);
    assert!(process.invoke_host_export("main", &[]).is_err());
}

#[test]
fn bundle_declares_typed_host_exports_and_header() {
    use stasis_compiler::backend::{aot::AotProcess, EngineEntrypoints};
    let mut process = AotProcess::new();
    process.upsert_file(
        "host_exports.stasis",
        format!("{SOURCE} function render(): void {{}}"),
    );
    process.compile().expect("compile bundle");
    let root = std::env::temp_dir().join(format!("stasis_host_exports_{}", std::process::id()));
    let bundle = process
        .write_engine_bundle(
            &EngineEntrypoints {
                tick: "tick".into(),
                render: "render".into(),
                on_code_swap: None,
            },
            &root,
        )
        .expect("write bundle");
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(bundle.manifest_path).unwrap()).unwrap();
    assert_eq!(manifest["host_exports"]["abi_version"], 1);
    assert_eq!(
        manifest["host_exports"]["functions"][0]["signature"]["parameters"],
        serde_json::json!(["i32", "i32", "bool"])
    );
    assert!(std::fs::read_to_string(root.join("stasis_host_exports.h"))
        .unwrap()
        .contains("void stasis_host_v1_set_access(int32_t arg0, int32_t arg1, int32_t arg2);"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn wasm_host_export_executes_before_first_tick() {
    let mut process = stasis_compiler::backend::wasm::WasmProcess::new();
    process.upsert_file("host_exports.stasis", SOURCE);
    process.compile().unwrap();
    let path =
        std::env::temp_dir().join(format!("stasis_host_exports_{}.wasm", std::process::id()));
    std::fs::write(&path, process.module_bytes()).unwrap();
    let output = std::process::Command::new("node")
        .args([
            "-e",
            r#"
const fs = require('fs');
(async () => {
 const {instance} = await WebAssembly.instantiate(fs.readFileSync(process.argv[1]), {});
 instance.exports.main();
 instance.exports.stasis_host_v1_set_access(20, 22, 1);
 if (instance.exports.tick() !== 42) throw Error('host state not observed by first tick');
 if (instance.exports.set_access) throw Error('internal alias exposed');
})().catch(e => { console.error(e); process.exit(1); });
"#,
        ])
        .arg(&path)
        .output()
        .unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn unsupported_and_duplicate_exports_fail_before_backend_selection() {
    for declaration in [
        "function @host_export(foo) a(v: f32): void {}",
        "function @host_export(tick) a(): void {}",
        "function @host_export(foo) main(): void {}",
        "extern function @host_export(foo) a(): void;",
        "function @host_export(foo) a(values: i32[]): void {}",
        "function @host_export(foo) a(a: i32, b: i32, c: i32, d: i32): void {}",
        "function @host_export(foo) @host_export(bar) a(): void {}",
        "function @host_export(foo) a(): void {} function @host_export(foo) b(): void {}",
    ] {
        let mut compiler = stasis_compiler::compiler::Compiler::new();
        compiler.upsert_file("invalid.stasis", declaration);
        assert!(compiler.index_pass().is_err(), "accepted {declaration}");
    }
}

#[test]
fn export_signature_change_preserves_the_accepted_generation() {
    use stasis_compiler::host_exports::HostValue::{Bool, I32};
    let mut process = JitProcess::new();
    process.upsert_file("host_exports.stasis", SOURCE);
    process.compile().unwrap();
    let before = process.symbol_code_ptrs();
    process.upsert_file(
        "host_exports.stasis",
        SOURCE
            .replace("enabled: bool", "enabled: i32")
            .replace("if (enabled)", "if (enabled != 0)"),
    );
    assert!(process.compile().is_err());
    assert_eq!(process.symbol_code_ptrs(), before);
    process
        .invoke_host_export("set_access", &[I32(20), I32(22), Bool(true)])
        .unwrap();
    assert_eq!(process.execute_i32_noarg_by_name("tick").unwrap(), 42);
}

#[test]
fn manifest_version_and_target_mismatch_are_rejected() {
    use stasis_compiler::host_exports::{HostExport, HostExportRecord, HostExports};
    let mut exports = HostExports {
        abi_version: 1,
        functions: vec![HostExportRecord {
            signature: HostExport {
                name: "set_access".into(),
                parameters: vec!["i32".into(), "i32".into(), "bool".into()],
                return_type: "void".into(),
            },
            symbol: "stasis_host_v1_set_access".into(),
            target_symbol: "aot_fn_1".into(),
            source_symbol_id: "source".into(),
            source_path: "game.stasis".into(),
        }],
    };
    assert!(exports.validate().is_ok());
    exports.abi_version = 2;
    assert!(exports.validate().is_err());
    exports.abi_version = 1;
    let manifest = serde_json::json!({"host_exports": exports, "functions": []});
    assert!(HostExports::from_manifest(&manifest).is_err());
}

#[test]
fn stable_header_ignores_source_function_name_and_order() {
    let header = |source: String| {
        let mut compiler = stasis_compiler::compiler::Compiler::new();
        compiler.upsert_file("exports.stasis", source);
        compiler.index_pass().unwrap();
        stasis_compiler::host_exports::HostExports::from_compiler(&compiler)
            .header()
            .unwrap()
    };
    assert_eq!(
        header(SOURCE.into()),
        header(format!(
            "function unrelated(): i32 {{ return 9; }} {}",
            SOURCE.replace("set_access(a:", "renamed(a:")
        ))
    );
}
