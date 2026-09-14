use stasis_compiler::backend::aot::AotProcess;
use stasis_compiler::backend::jit::JitProcess;
use stasis_compiler::backend::wasm::WasmProcess;
use stasis_compiler::compiler::Compiler;
use stasis_compiler::frontend::parser::rewrite_top_level_test_declarations;
use std::path::{Path, PathBuf};
use std::process::Command;

const ENTRY_PATH: &str = "samples/generics_collections/src/main.stasis";
const ENTRY: &str = include_str!("../../../samples/generics_collections/src/main.stasis");
const WASM_ENTRY_PATH: &str = "samples/generics_collections/src/wasm_entry.stasis";
const WASM_ENTRY: &str =
    include_str!("../../../samples/generics_collections/src/wasm_entry.stasis");
const TEST_PATH: &str = "samples/generics_collections/tests/generics_collections.test.stasis";
const TESTS: &str =
    include_str!("../../../samples/generics_collections/tests/generics_collections.test.stasis");
const RIG2D_IMPORT: &str = "/vendor/stasis/stdlib/rig2d.stasis";
const GRAPHICS_IMPORT: &str = "/vendor/stasis/stdlib/graphics.stasis";
const WASM_ROOT: &str = "main";

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("canonical repository root")
}

fn repository_entry() -> String {
    ENTRY
        .replace(RIG2D_IMPORT, "../../../src/stdlib/rig2d.stasis")
        .replace(GRAPHICS_IMPORT, "../../../src/stdlib/graphics.stasis")
}

#[test]
fn generic_collection_scalar_fixture_executes_in_wasm() {
    let mut wasm = WasmProcess::new();
    wasm.set_project_root(repository_root().to_string_lossy())
        .expect("set sample Wasm project root");
    wasm.set_required_emit_roots(&[WASM_ROOT.to_string()]);
    wasm.upsert_file(WASM_ENTRY_PATH, WASM_ENTRY);
    wasm.compile()
        .expect("compile generic collection Wasm sample");
    assert!(
        wasm.module_bytes().starts_with(b"\0asm\x01\0\0\0"),
        "generic collection sample must produce a WebAssembly module"
    );

    let wasm_path = std::env::temp_dir().join(format!(
        "stasis_generic_collections_{}_{}.wasm",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    std::fs::write(&wasm_path, wasm.module_bytes()).expect("write generic collection Wasm");
    let output = Command::new("node")
        .args([
            "-e",
            "const fs=require('node:fs'); WebAssembly.instantiate(fs.readFileSync(process.argv[1]), {}).then(({instance}) => process.stdout.write(String(instance.exports.main()))).catch((error) => { console.error(error); process.exit(1); });",
        ])
        .arg(&wasm_path)
        .output()
        .expect("run Node for generic collection Wasm");
    let _ = std::fs::remove_file(&wasm_path);
    assert!(
        output.status.success(),
        "Node failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "0");
}

#[test]
fn generic_collection_sample_tests_pass_in_the_production_jit_shape() {
    let (rewritten, tests) =
        rewrite_top_level_test_declarations(TESTS).expect("discover generic sample tests");
    assert_eq!(tests.len(), 2);
    let mut process = JitProcess::new();
    process
        .set_project_root(repository_root().to_string_lossy())
        .expect("set generic sample test project root");
    process.set_required_emit_roots(
        &tests
            .iter()
            .map(|test| test.generated_function_name.clone())
            .collect::<Vec<_>>(),
    );
    process.upsert_file(ENTRY_PATH, repository_entry());
    process.upsert_file(TEST_PATH, rewritten);
    process.compile().expect("compile generic sample tests");
    for test in tests {
        assert!(
            process
                .execute_bool_noarg_by_name(&test.generated_function_name)
                .unwrap_or_else(|error| panic!("execute '{}': {error}", test.display_name)),
            "generic sample test returned false: {}",
            test.display_name
        );
    }
}

#[test]
fn generic_collection_aot_accepts_vendor_graphics_after_expansion() {
    let sample_root = repository_root().join("samples/generics_collections");
    let mut aot = AotProcess::new();
    aot.set_import_base_dir(&sample_root);
    aot.set_required_emit_roots(&["main".to_string(), "render".to_string()]);
    aot.upsert_file("src/main.stasis", ENTRY);
    aot.compile()
        .expect("generic collection AOT must preserve vendor graphics provenance");
}

#[test]
fn negative_generic_collection_fixtures_keep_expected_diagnostics() {
    let fixtures = [
        (
            "runtime_capacity",
            include_str!("../../../samples/generics_collections/negative/runtime_capacity.stasis"),
            "compile-time",
        ),
        (
            "unresolved_capacity",
            include_str!(
                "../../../samples/generics_collections/negative/unresolved_capacity.stasis"
            ),
            "first parameter",
        ),
        (
            "layout_overflow",
            include_str!("../../../samples/generics_collections/negative/layout_overflow.stasis"),
            "layout",
        ),
        (
            "ambiguous_receiver",
            include_str!(
                "../../../samples/generics_collections/negative/ambiguous_receiver.stasis"
            ),
            "ambiguous",
        ),
        (
            "legacy_function_generic",
            include_str!(
                "../../../samples/generics_collections/negative/legacy_function_generic.stasis"
            ),
            "no longer supported",
        ),
        (
            "explicit_turbofish_call",
            include_str!(
                "../../../samples/generics_collections/negative/explicit_turbofish_call.stasis"
            ),
            "explicit generic function call",
        ),
        (
            "explicit_angle_call",
            include_str!(
                "../../../samples/generics_collections/negative/explicit_angle_call.stasis.invalid"
            ),
            "explicit generic function call",
        ),
        (
            "later_parameter_binding",
            include_str!(
                "../../../samples/generics_collections/negative/later_parameter_binding.stasis"
            ),
            "first parameter",
        ),
    ];
    for (name, source, expected) in fixtures {
        let mut compiler = Compiler::new();
        compiler.upsert_file(format!("negative/{name}.stasis"), source);
        let result = compiler.check();
        assert!(
            result.is_err(),
            "negative fixture '{name}' unexpectedly compiled"
        );
        let error = result.expect_err("negative fixture unexpectedly compiled");
        assert!(
            format!("{error:?}").to_lowercase().contains(expected),
            "negative fixture '{name}' diagnostic did not contain '{expected}': {error:?}"
        );
    }

    let mut process = JitProcess::new();
    process.upsert_file(
        "negative/unsupported_element_copy.stasis",
        include_str!(
            "../../../samples/generics_collections/negative/unsupported_element_copy.stasis"
        ),
    );
    let error = process
        .compile()
        .expect_err("unsupported composite element copy unexpectedly compiled");
    assert!(
        format!("{error:?}")
            .to_lowercase()
            .contains("requires field path"),
        "unsupported composite element copy diagnostic was not specific: {error:?}"
    );
}
