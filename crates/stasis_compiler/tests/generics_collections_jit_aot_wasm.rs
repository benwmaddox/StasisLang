use stasis_compiler::backend::aot::AotProcess;
use stasis_compiler::backend::jit::JitProcess;
use stasis_compiler::backend::wasm::WasmProcess;
use stasis_compiler::compiler::Compiler;
use stasis_compiler::frontend::parser::rewrite_top_level_test_declarations;
#[cfg(windows)]
use stasis_jit::{AotLinkConfig, AotTarget};
#[cfg(windows)]
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const ENTRY_PATH: &str = "samples/generics_collections/src/main.stasis";
const ENTRY: &str = include_str!("../../../samples/generics_collections/src/main.stasis");
const TEST_PATH: &str = "samples/generics_collections/tests/generics_collections.test.stasis";
const TESTS: &str =
    include_str!("../../../samples/generics_collections/tests/generics_collections.test.stasis");
const PARITY_ORACLE_PATH: &str = "tests/stasis/seams/generics_parity_oracle.stasis";
const PARITY_ORACLE: &str =
    include_str!("../../../tests/stasis/seams/generics_parity_oracle.stasis");
const PARITY_ORACLE_MODULE_PATH: &str = "tests/stasis/seams/generics_parity_module.stasis";
const PARITY_ORACLE_MODULE: &str =
    include_str!("../../../tests/stasis/seams/generics_parity_module.stasis");
const RIG2D_IMPORT: &str = "/vendor/stasis/stdlib/rig2d.stasis";
const GRAPHICS_IMPORT: &str = "/vendor/stasis/stdlib/graphics.stasis";
const WASM_ROOT: &str = "main";

#[cfg(windows)]
struct AotTree(PathBuf);

#[cfg(windows)]
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

#[cfg(windows)]
fn linker_path() -> PathBuf {
    if let Some(explicit) = std::env::var_os("STASIS_AOT_LINKER") {
        let path = PathBuf::from(explicit);
        assert!(path.is_file(), "STASIS_AOT_LINKER must name a linker file");
        return path;
    }
    if let Some(path) = cc::windows_registry::find_tool("x86_64-pc-windows-msvc", "link.exe")
        .map(|tool| tool.path().to_path_buf())
        .filter(|path| path.is_file())
    {
        return path;
    }
    for candidate in ["lld-link.exe", "link.exe"] {
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
                .find(|path| {
                    path.is_file()
                        && !path
                            .to_string_lossy()
                            .replace('/', "\\")
                            .to_ascii_lowercase()
                            .ends_with("\\git\\usr\\bin\\link.exe")
                })
            {
                return path;
            }
        }
    }
    panic!("MSVC link.exe or lld-link.exe is required for linked AOT execution")
}

#[cfg(windows)]
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

#[cfg(windows)]
fn run_linked_aot_oracle(aot: &AotProcess, expected_digest: i32) {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| repository_root().join("target"));
    let tree = AotTree(target.join(format!(
        "generics-parity-oracle-aot-{}-{stamp}",
        std::process::id()
    )));
    fs::create_dir_all(&tree.0).expect("create linked-AOT oracle directory");
    let (import, runtime) = dynload_artifacts();
    fs::copy(runtime, tree.0.join("stasis_dynload.dll")).expect("copy linked-AOT runtime");
    let executable = tree.0.join("generics_parity_oracle.exe");
    aot.link_executable_for_i32_noarg_function(
        "main",
        &executable,
        &AotLinkConfig {
            linker_path: Some(linker_path()),
            runtime_lib_paths: vec![import],
            target: AotTarget::Native,
        },
    )
    .expect("link shared generics oracle AOT executable");
    let status = Command::new(repository_root().join(".cargo/stasis-sign-and-run.cmd"))
        .arg(executable.file_name().expect("linked executable name"))
        .current_dir(&tree.0)
        .status()
        .expect("run linked shared generics oracle AOT executable");
    let aot_code = status.code().expect("linked AOT process exit code");
    let signed_execution_required =
        std::env::var_os("STASIS_REQUIRE_SIGNED_EXECUTION").is_some_and(|value| value == "1");
    if aot_code == 4551 && !signed_execution_required {
        eprintln!(
            "skipping linked AOT execution parity: Windows Application Control returned 4551 and signed execution is not required"
        );
    } else {
        assert_eq!(aot_code, expected_digest, "JIT/AOT result parity");
    }
}

fn repository_entry() -> String {
    ENTRY
        .replace(RIG2D_IMPORT, "../../../src/stdlib/rig2d.stasis")
        .replace(GRAPHICS_IMPORT, "../../../src/stdlib/graphics.stasis")
}

fn compile_wasm_fixture(
    path: &str,
    source: &str,
    roots: &[&str],
    imported_files: &[(&str, &str)],
) -> WasmProcess {
    let mut wasm = WasmProcess::new();
    wasm.set_project_root(repository_root().to_string_lossy())
        .expect("set focused Wasm fixture project root");
    wasm.set_required_emit_roots(
        &roots
            .iter()
            .map(|root| (*root).to_string())
            .collect::<Vec<_>>(),
    );
    wasm.upsert_file(path, source);
    for (import_path, import_source) in imported_files {
        wasm.upsert_file(*import_path, *import_source);
    }
    wasm.compile().expect("compile focused Wasm fixture");
    wasm
}

fn run_wasm_node(module: &WasmProcess, script: &str, label: &str) -> String {
    let wasm_path = std::env::temp_dir().join(format!(
        "stasis_generics_focused_{}_{}.wasm",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    std::fs::write(&wasm_path, module.module_bytes()).expect("write focused Wasm fixture");
    let output = Command::new("node")
        .args(["-e", script])
        .arg(&wasm_path)
        .output()
        .unwrap_or_else(|error| panic!("{label}: {error}"));
    let _ = std::fs::remove_file(&wasm_path);
    assert!(
        output.status.success(),
        "{label} failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("focused Wasm fixture output is UTF-8")
}

const STATIC_NAMED_STRUCT_VIEW_BOUNDS: &str = r#"
struct StaticItem {
    score: i32;
}

global static_items: StaticItem[3];

function item_score(item: StaticItem): i32 {
    return item.score;
}

function static_copy_source(source: i32): i32 {
    static_items[0] = static_items[source];
    return item_score(static_items[0]);
}

function main(): i32 {
    static_items[0].score = 10;
    static_items[1].score = 11;
    static_items[2].score = 12;
    return item_score(static_items[0]);
}

function render(index: i32): i32 {
    return static_copy_source(index);
}
"#;

const RECEIVER_FOREACH_AND_METADATA: &str = r#"
struct ReceiverItem {
    score: i32;
}

struct ReceiverBox {
    items: ReceiverItem[2];
}

struct ReceiverOuter {
    inner: ReceiverBox;
}

global direct_box: ReceiverBox;
global nested_box: ReceiverOuter;

function direct_total(self: ReceiverBox): i32 {
    let total: i32 = 0;
    foreach (let item, i in self.items) {
        total += item.score + i;
    }
    return total + self.items.max_length;
}

function nested_total(self: ReceiverOuter): i32 {
    let total: i32 = 0;
    foreach (let item, i in self.inner.items) {
        total += item.score + i;
    }
    return total + self.inner.items.max_length;
}

function main(): i32 {
    direct_box.items[0].score = 5;
    direct_box.items[1].score = 7;
    nested_box.inner.items[0].score = 10;
    nested_box.inner.items[1].score = 20;
    return direct_total(direct_box);
}

function render(index: i32): i32 {
    if (index == 0) {
        return direct_total(direct_box);
    }
    return nested_total(nested_box);
}
"#;

const SCRATCH_CLOBBER_GUARD: &str = r#"
struct ScratchItem {
    value: i32;
}

struct ScratchBox {
    items: ScratchItem[2];
}

global scratch_left: ScratchBox;
global scratch_right: ScratchBox;
global lhs_index_calls: i32;
global rhs_index_calls: i32;

function next_lhs_index(): i32 {
    lhs_index_calls += 1;
    return 1;
}

function next_rhs_index(): i32 {
    rhs_index_calls += 1;
    return 0;
}

function item_value(item: ScratchItem): i32 {
    return item.value;
}

function bump_from_other(self: ScratchBox): void {
    self.items[next_lhs_index()].value += item_value(scratch_right.items[next_rhs_index()]);
}

function main(): i32 {
    scratch_left.items[0].value = 10;
    scratch_left.items[1].value = 20;
    scratch_right.items[0].value = 100;
    scratch_right.items[1].value = 200;
    lhs_index_calls = 0;
    rhs_index_calls = 0;
    bump_from_other(scratch_left);
    return scratch_left.items[1].value
        + scratch_right.items[0].value
        + lhs_index_calls * 100
        + rhs_index_calls;
}
"#;

const INDEXED_VIEW_OVERLOADS: &str = r#"
import "generics_parity_module.stasis";

struct IndexedItem {
    score: i32;
}

struct IndexedOther {
    score: i32;
}

struct IndexedBox {
    items: IndexedItem[2];
}

global indexed_box: IndexedBox;
global qualified_value: generics_parity_module.OracleQualified<4>;

function inspect(item: IndexedItem): i32 {
    return item.score;
}

function inspect(item: IndexedOther): i32 {
    return 1000 + item.score;
}

function inspect_local(items: IndexedItem[]): i32 {
    return inspect(items[0]);
}

function inspect_receiver(self: IndexedBox): i32 {
    return inspect(self.items[0]);
}

function main(): i32 {
    indexed_box.items[0].score = 7;
    qualified_value.value = 6;
    return inspect_local(indexed_box.items)
        + inspect_receiver(indexed_box)
        + generics_parity_module.oracle_qualified_score(qualified_value);
}
"#;

#[test]
fn generic_collection_shared_fixture_executes_in_wasm() {
    let mut wasm = WasmProcess::new();
    wasm.set_project_root(repository_root().to_string_lossy())
        .expect("set sample Wasm project root");
    wasm.set_required_emit_roots(&[WASM_ROOT.to_string()]);
    wasm.upsert_file(ENTRY_PATH, repository_entry());
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
fn backend_neutral_generics_oracle_matches_jit_aot_and_wasm() {
    const EXPECTED_DIGEST: i32 = 685;
    const ROOTS: &[&str] = &["main", "tick", "render"];

    let mut jit = JitProcess::new();
    jit.set_required_emit_roots(
        &ROOTS
            .iter()
            .map(|root| (*root).to_string())
            .collect::<Vec<_>>(),
    );
    jit.upsert_file(PARITY_ORACLE_PATH, PARITY_ORACLE);
    jit.upsert_file(PARITY_ORACLE_MODULE_PATH, PARITY_ORACLE_MODULE);
    jit.compile()
        .expect("compile shared generics oracle for JIT");
    assert_eq!(
        jit.execute_i32_noarg_by_name("main")
            .expect("execute shared generics oracle in JIT"),
        EXPECTED_DIGEST
    );
    assert_eq!(
        jit.execute_i32_noarg_by_name("tick")
            .expect("read shared generics oracle digest in JIT"),
        EXPECTED_DIGEST
    );

    let mut aot = AotProcess::new();
    aot.set_required_emit_roots(
        &ROOTS
            .iter()
            .map(|root| (*root).to_string())
            .collect::<Vec<_>>(),
    );
    aot.upsert_file(PARITY_ORACLE_PATH, PARITY_ORACLE);
    aot.upsert_file(PARITY_ORACLE_MODULE_PATH, PARITY_ORACLE_MODULE);
    aot.compile()
        .expect("compile shared generics oracle to a native AOT object");
    #[cfg(windows)]
    run_linked_aot_oracle(&aot, EXPECTED_DIGEST);

    let mut wasm = WasmProcess::new();
    wasm.set_required_emit_roots(
        &ROOTS
            .iter()
            .map(|root| (*root).to_string())
            .collect::<Vec<_>>(),
    );
    wasm.upsert_file(PARITY_ORACLE_PATH, PARITY_ORACLE);
    wasm.upsert_file(PARITY_ORACLE_MODULE_PATH, PARITY_ORACLE_MODULE);
    wasm.compile()
        .expect("compile shared generics oracle for Wasm");
    let wasm_path = std::env::temp_dir().join(format!(
        "stasis_generics_parity_oracle_{}_{}.wasm",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    std::fs::write(&wasm_path, wasm.module_bytes()).expect("write shared generics oracle Wasm");
    let output = Command::new("node")
        .args([
            "-e",
            "const fs=require('node:fs'); WebAssembly.instantiate(fs.readFileSync(process.argv[1]), {}).then(({instance}) => { const e=instance.exports; const trapped=(index)=>{try{e.render(index);return false;}catch(error){return error instanceof WebAssembly.RuntimeError;}}; process.stdout.write([e.main(),e.tick(),e.render(0),e.render(2),trapped(-1),trapped(3)].join(',')); }).catch((error) => { console.error(error); process.exit(1); });",
        ])
        .arg(&wasm_path)
        .output()
        .expect("run shared generics oracle Wasm in Node");
    let _ = std::fs::remove_file(&wasm_path);
    assert!(
        output.status.success(),
        "Node failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("{EXPECTED_DIGEST},{EXPECTED_DIGEST},10,10,true,true")
    );
}

#[test]
fn static_named_struct_views_trap_at_bounds_including_copy_sources() {
    let wasm = compile_wasm_fixture(
        "focused/static_named_struct_view_bounds.stasis",
        STATIC_NAMED_STRUCT_VIEW_BOUNDS,
        &["main", "render"],
        &[],
    );
    let output = run_wasm_node(
        &wasm,
        "const fs=require('node:fs'); WebAssembly.instantiate(fs.readFileSync(process.argv[1]), {}).then(({instance}) => { const e=instance.exports; const trapped=(index)=>{try{e.render(index);return false;}catch(error){return error instanceof WebAssembly.RuntimeError;}}; process.stdout.write([e.main(),e.render(2),trapped(-1),trapped(3)].join(',')); }).catch((error) => { console.error(error); process.exit(1); });",
        "static named-struct view bounds",
    );
    assert_eq!(output, "10,12,true,true");
}

#[test]
fn receiver_named_struct_views_support_foreach_and_max_length() {
    let wasm = compile_wasm_fixture(
        "focused/receiver_foreach_and_metadata.stasis",
        RECEIVER_FOREACH_AND_METADATA,
        &["main", "render"],
        &[],
    );
    let output = run_wasm_node(
        &wasm,
        "const fs=require('node:fs'); WebAssembly.instantiate(fs.readFileSync(process.argv[1]), {}).then(({instance}) => { const e=instance.exports; process.stdout.write([e.main(),e.render(0),e.render(1)].join(',')); }).catch((error) => { console.error(error); process.exit(1); });",
        "receiver foreach and metadata",
    );
    assert_eq!(output, "15,15,33");
}

#[test]
fn receiver_compound_assignment_preserves_nested_view_indices_and_owners() {
    let wasm = compile_wasm_fixture(
        "focused/scratch_clobber_guard.stasis",
        SCRATCH_CLOBBER_GUARD,
        &["main"],
        &[],
    );
    let output = run_wasm_node(
        &wasm,
        "const fs=require('node:fs'); WebAssembly.instantiate(fs.readFileSync(process.argv[1]), {}).then(({instance}) => process.stdout.write(String(instance.exports.main()))).catch((error) => { console.error(error); process.exit(1); });",
        "receiver compound scratch guard",
    );
    assert_eq!(output, "321");
}

#[test]
fn indexed_named_struct_elements_resolve_local_receiver_and_qualified_calls() {
    let wasm = compile_wasm_fixture(
        "tests/stasis/seams/focused/indexed_view_overloads.stasis",
        INDEXED_VIEW_OVERLOADS,
        &["main"],
        &[(
            "tests/stasis/seams/focused/generics_parity_module.stasis",
            PARITY_ORACLE_MODULE,
        )],
    );
    let output = run_wasm_node(
        &wasm,
        "const fs=require('node:fs'); WebAssembly.instantiate(fs.readFileSync(process.argv[1]), {}).then(({instance}) => process.stdout.write(String(instance.exports.main()))).catch((error) => { console.error(error); process.exit(1); });",
        "indexed named-struct calls",
    );
    assert_eq!(output, "24");
}

#[test]
fn rejects_named_struct_array_returns_at_the_shared_frontend_boundary() {
    let mut wasm = WasmProcess::new();
    wasm.set_project_root(repository_root().to_string_lossy())
        .expect("set named-array return fixture project root");
    wasm.set_required_emit_roots(&["expose_items".into()]);
    wasm.upsert_file(
        "negative/named_array_return.stasis",
        include_str!("../../../samples/generics_collections/negative/named_array_return.stasis"),
    );
    let error = wasm
        .compile()
        .expect_err("named-struct array return must remain outside the shared value contract");
    assert!(
        format!("{error:?}").contains("named-struct result 'Item[]' is unsupported"),
        "unexpected named-array return diagnostic: {error:?}"
    );
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
            "wrong_generic_kind",
            include_str!(
                "../../../samples/generics_collections/negative/wrong_generic_kind.stasis"
            ),
            "expects a type argument",
        ),
        (
            "wrong_generic_arity",
            include_str!(
                "../../../samples/generics_collections/negative/wrong_generic_arity.stasis"
            ),
            "generic argument arity mismatch",
        ),
        (
            "expanding_recursion",
            include_str!(
                "../../../samples/generics_collections/negative/expanding_recursion.stasis"
            ),
            "generic instantiation depth exceeded",
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
