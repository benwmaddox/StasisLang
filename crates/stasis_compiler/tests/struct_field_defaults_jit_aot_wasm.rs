use stasis_compiler::backend::development_swap::{
    commit_development_swap, DevelopmentSwapDescriptor, DevelopmentSwapHost,
};
use stasis_compiler::backend::jit::JitScalarValue;
use stasis_compiler::backend::state_migration::state_layout_version;
use stasis_compiler::backend::{aot::AotProcess, jit::JitProcess, wasm::WasmProcess};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

const FIXTURE_PATH: &str = "tests/stasis/seams/struct_field_defaults_cross_backend.stasis";
const FIXTURE: &str =
    include_str!("../../../tests/stasis/seams/struct_field_defaults_cross_backend.stasis");
const ENUM_COLLECTION_FIXTURE_PATH: &str =
    "tests/stasis/seams/struct_field_enum_collection_defaults.stasis";
const ENUM_COLLECTION_FIXTURE: &str = r#"
enum Mode { Ready = 7, Active = 9 }
struct ModeActor { mode: Mode = Mode.Active; }
global enum_actors: ModeActor[2];
function main(): i32 {
    if (enum_actors[0].mode != Mode.Active || enum_actors[1].mode != Mode.Active) { return 1; }
    return 0;
}
"#;

fn compile_jit(source: &str) -> JitProcess {
    compile_jit_at(FIXTURE_PATH, source)
}

fn compile_jit_at(path: &str, source: &str) -> JitProcess {
    let mut process = JitProcess::new();
    process.set_required_emit_roots(&["main".to_string()]);
    process.upsert_file(path, source);
    process
        .compile()
        .expect("compile field-default fixture in JIT");
    process
}

fn assert_jit_default_bits(process: &JitProcess) {
    for (path, expected) in [
        ("world.tuning.health", JitScalarValue::I32(11)),
        ("world.tuning.minimum_i32", JitScalarValue::I32(i32::MIN)),
        ("world.tuning.enabled", JitScalarValue::Bool(true)),
        ("world.mode", JitScalarValue::I32(9)),
        ("world.tuning.no_default", JitScalarValue::I32(0)),
        ("world.tuning.length", JitScalarValue::I32(23)),
        ("world.tuning.max_length", JitScalarValue::I32(29)),
        ("world.tuning.byte", JitScalarValue::U8(17)),
        ("world.tuning.flags", JitScalarValue::U32(123456)),
    ] {
        assert_eq!(
            process.read_global_scalar(path),
            Ok(expected),
            "world scalar {path}"
        );
    }
    assert_f32_bits(
        process.read_global_scalar("world.tuning.rate_f32").unwrap(),
        0x3dcccccd,
    );
    assert_f32_bits(
        process
            .read_global_scalar("world.tuning.negative_zero_f32")
            .unwrap(),
        0x80000000,
    );
    assert_f32_bits(
        process
            .read_global_scalar("world.tuning.positive_zero_f32")
            .unwrap(),
        0x00000000,
    );
    assert_f64_bits(
        process.read_global_scalar("world.tuning.rate_f64").unwrap(),
        0x3fb999999999999a,
    );
    assert_f64_bits(
        process
            .read_global_scalar("world.tuning.negative_zero_f64")
            .unwrap(),
        0x8000000000000000,
    );

    let layout = process.state_layout();
    let actors = layout
        .collections
        .iter()
        .find(|collection| collection.path == "actors")
        .expect("actor SoA collection");
    for (field, expected) in [
        ("tuning.health", JitScalarValue::I32(7)),
        ("tuning.minimum_i32", JitScalarValue::I32(i32::MIN)),
        (
            "tuning.rate_f32",
            JitScalarValue::F32(f32::from_bits(0x3dcccccd)),
        ),
        ("tuning.negative_zero_f32", JitScalarValue::F32(-0.0)),
        ("tuning.positive_zero_f32", JitScalarValue::F32(0.0)),
        (
            "tuning.rate_f64",
            JitScalarValue::F64(f64::from_bits(0x3fb999999999999a)),
        ),
        ("tuning.negative_zero_f64", JitScalarValue::F64(-0.0)),
        ("tuning.enabled", JitScalarValue::Bool(true)),
        ("tuning.length", JitScalarValue::I32(23)),
        ("tuning.max_length", JitScalarValue::I32(29)),
        ("tuning.byte", JitScalarValue::U8(17)),
        ("tuning.flags", JitScalarValue::U32(123456)),
        ("tuning.no_default", JitScalarValue::I32(0)),
        ("armor", JitScalarValue::U16(513)),
        ("length", JitScalarValue::I32(31)),
        ("max_length", JitScalarValue::I32(37)),
    ] {
        assert!(
            actors.fields.iter().any(|layout| layout.field == field),
            "missing SoA lane {field}"
        );
        for index in 0..2 {
            assert_eq!(
                process.read_global_collection_scalar(&actors.path, field, index),
                Ok(expected),
                "actors[{index}].{field}"
            );
            if field == "tuning.rate_f32" {
                assert_collection_f32_bits(process, &actors.path, field, index, 0x3dcccccd);
            } else if field == "tuning.negative_zero_f32" {
                assert_collection_f32_bits(process, &actors.path, field, index, 0x80000000);
            } else if field == "tuning.positive_zero_f32" {
                assert_collection_f32_bits(process, &actors.path, field, index, 0x00000000);
            } else if field == "tuning.rate_f64" {
                assert_collection_f64_bits(process, &actors.path, field, index, 0x3fb999999999999a);
            } else if field == "tuning.negative_zero_f64" {
                assert_collection_f64_bits(process, &actors.path, field, index, 0x8000000000000000);
            }
        }
    }
}

fn assert_f32_bits(value: JitScalarValue, expected: u32) {
    match value {
        JitScalarValue::F32(actual) => assert_eq!(actual.to_bits(), expected),
        other => panic!("expected f32 storage, found {other:?}"),
    }
}

fn assert_f64_bits(value: JitScalarValue, expected: u64) {
    match value {
        JitScalarValue::F64(actual) => assert_eq!(actual.to_bits(), expected),
        other => panic!("expected f64 storage, found {other:?}"),
    }
}

fn assert_collection_f32_bits(
    process: &JitProcess,
    path: &str,
    field: &str,
    index: i32,
    expected: u32,
) {
    assert_f32_bits(
        process
            .read_global_collection_scalar(path, field, index)
            .unwrap(),
        expected,
    );
}

fn assert_collection_f64_bits(
    process: &JitProcess,
    path: &str,
    field: &str,
    index: i32,
    expected: u64,
) {
    assert_f64_bits(
        process
            .read_global_collection_scalar(path, field, index)
            .unwrap(),
        expected,
    );
}

#[derive(Default)]
struct NoopSwapHost;

impl DevelopmentSwapHost for NoopSwapHost {
    type Staged = ();

    fn stage(
        &mut self,
        _candidate: &JitProcess,
        _descriptor: &DevelopmentSwapDescriptor,
    ) -> Result<Self::Staged, String> {
        Ok(())
    }

    fn publish(&mut self, _staged: &mut Self::Staged) -> Result<(), String> {
        Ok(())
    }

    fn restore(&mut self, _staged: Self::Staged) -> Result<(), String> {
        Ok(())
    }
}

fn swap(active: &mut JitProcess, candidate: JitProcess) {
    commit_development_swap(
        active,
        candidate,
        DevelopmentSwapDescriptor::new(vec!["main".to_string()], true),
        &mut NoopSwapHost,
        |_| Ok::<(), String>(()),
    )
    .expect("commit field-default state migration");
}

#[test]
fn struct_field_defaults_match_jit_native_aot_and_wasm_and_migrate() {
    let mut active = compile_jit(FIXTURE);
    assert_jit_default_bits(&active);
    assert_eq!(active.execute_i32_noarg_by_name("main"), Ok(0));
    assert_eq!(
        active.read_global_scalar("world.tuning.health"),
        Ok(JitScalarValue::I32(90)),
        "an explicit write replaces the declared default"
    );
    assert_eq!(
        active.read_global_collection_scalar("actors", "tuning.health", 1),
        Ok(JitScalarValue::I32(91)),
        "an explicit SoA write replaces the lane default"
    );

    let mut aot = AotProcess::new();
    aot.set_required_emit_roots(&["main".to_string()]);
    aot.upsert_file(FIXTURE_PATH, FIXTURE);
    aot.compile().expect("compile field-default native AOT");
    run_linked_native_aot(&aot);

    let mut wasm = WasmProcess::new();
    wasm.set_required_emit_roots(&["main".to_string()]);
    wasm.upsert_file(FIXTURE_PATH, FIXTURE);
    wasm.compile().expect("compile field-default Wasm");
    run_node_wasm(&wasm);

    active
        .write_global_scalar("world.tuning.health", JitScalarValue::I32(123))
        .expect("seed live nested scalar");
    active
        .write_global_scalar("world.tuning.no_default", JitScalarValue::I32(456))
        .expect("seed live no-default scalar");
    active
        .write_global_scalar("world.mode", JitScalarValue::I32(7))
        .expect("seed live global enum scalar");
    active
        .write_global_collection_scalar("actors", "tuning.health", 0, JitScalarValue::I32(321))
        .expect("seed first actor lane");
    active
        .write_global_collection_scalar("actors", "tuning.health", 1, JitScalarValue::I32(322))
        .expect("seed second actor lane");
    active
        .write_global_collection_scalar("actors", "tuning.no_default", 0, JitScalarValue::I32(789))
        .expect("seed live no-default lane");

    let default_only = FIXTURE.replace("health: i32 = N;", "health: i32 = N + 10;");
    let mut candidate = active.staged_candidate();
    candidate.upsert_file(FIXTURE_PATH, &default_only);
    candidate
        .compile_staged()
        .expect("compile default-only candidate");
    assert_eq!(
        state_layout_version(&active.state_layout()).expect("active layout version"),
        state_layout_version(&candidate.state_layout()).expect("candidate layout version"),
        "field defaults are excluded from state layout identity"
    );
    swap(&mut active, candidate);
    assert_eq!(
        active.read_global_scalar("world.tuning.health"),
        Ok(JitScalarValue::I32(123)),
        "default-only hot swap preserves existing live state"
    );
    assert_eq!(
        active.read_global_scalar("world.mode"),
        Ok(JitScalarValue::I32(7)),
        "default-only hot swap preserves the global enum scalar"
    );
    assert_eq!(
        active.read_global_collection_scalar("actors", "tuning.health", 0),
        Ok(JitScalarValue::I32(321))
    );
    assert_eq!(
        active.read_global_collection_scalar("actors", "tuning.health", 1),
        Ok(JitScalarValue::I32(322))
    );

    let incoming = default_only
        .replace(
            "    no_default: i32;",
            "    no_default: i32;\n    rank: i32 = 73;",
        )
        .replace("global actors: Actor[2];", "global actors: Actor[3];");
    let mut candidate = active.staged_candidate();
    candidate.upsert_file(FIXTURE_PATH, &incoming);
    candidate
        .compile_staged()
        .expect("compile added-field and collection-growth candidate");
    swap(&mut active, candidate);

    assert_eq!(
        active.read_global_scalar("world.tuning.health"),
        Ok(JitScalarValue::I32(123)),
        "compatible nested scalar remains live"
    );
    assert_eq!(
        active.read_global_scalar("world.mode"),
        Ok(JitScalarValue::I32(7)),
        "explicit migration preserves the global enum scalar"
    );
    assert_eq!(
        active.read_global_collection_scalar("actors", "tuning.health", 0),
        Ok(JitScalarValue::I32(321)),
        "compatible SoA prefix is copied"
    );
    assert_eq!(
        active.read_global_collection_scalar("actors", "tuning.health", 1),
        Ok(JitScalarValue::I32(322)),
        "compatible SoA prefix is copied"
    );
    assert_eq!(
        active.read_global_collection_scalar("actors", "tuning.health", 2),
        Ok(JitScalarValue::I32(17)),
        "grown lane uses the refreshed incoming default"
    );
    assert_eq!(
        active.read_global_scalar("world.tuning.rank"),
        Ok(JitScalarValue::I32(73)),
        "new nested scalar uses its incoming default"
    );
    for index in 0..3 {
        assert_eq!(
            active.read_global_collection_scalar("actors", "tuning.rank", index),
            Ok(JitScalarValue::I32(73)),
            "new nested SoA lane actors[{index}] uses its incoming default"
        );
    }
    assert_eq!(
        active.read_global_collection_scalar("actors", "tuning.no_default", 0),
        Ok(JitScalarValue::I32(789)),
        "compatible no-default lane remains live"
    );
    assert_eq!(
        active.read_global_collection_scalar("actors", "tuning.no_default", 2),
        Ok(JitScalarValue::I32(0)),
        "grown no-default lane remains zero"
    );
    assert_eq!(
        active.read_global_collection_scalar("actors", "length", 2),
        Ok(JitScalarValue::I32(31)),
        "user length lane receives its default at the new index"
    );
    assert_eq!(
        active.read_global_collection_scalar("actors", "max_length", 2),
        Ok(JitScalarValue::I32(37)),
        "user max_length lane receives its default at the new index"
    );
}

#[test]
fn enum_struct_field_defaults_initialize_soa_in_jit_native_aot_and_wasm() {
    let jit = compile_jit_at(ENUM_COLLECTION_FIXTURE_PATH, ENUM_COLLECTION_FIXTURE);
    let enum_actors = jit
        .state_layout()
        .collections
        .into_iter()
        .find(|collection| collection.path == "enum_actors")
        .expect("enum SoA collection");
    assert!(
        enum_actors.fields.iter().any(|field| field.field == "mode"),
        "missing enum SoA lane"
    );
    for index in 0..2 {
        assert_eq!(
            jit.read_global_collection_scalar("enum_actors", "mode", index),
            Ok(JitScalarValue::I32(9)),
            "enum_actors[{index}].mode default"
        );
    }
    assert_eq!(jit.execute_i32_noarg_by_name("main"), Ok(0));

    let mut aot = AotProcess::new();
    aot.set_required_emit_roots(&["main".to_string()]);
    aot.upsert_file(ENUM_COLLECTION_FIXTURE_PATH, ENUM_COLLECTION_FIXTURE);
    aot.compile().expect("compile enum-default native AOT");
    run_linked_native_aot(&aot);

    let mut wasm = WasmProcess::new();
    wasm.set_required_emit_roots(&["main".to_string()]);
    wasm.upsert_file(ENUM_COLLECTION_FIXTURE_PATH, ENUM_COLLECTION_FIXTURE);
    wasm.compile().expect("compile enum-default Wasm");
    run_node_wasm(&wasm);
}

fn run_node_wasm(wasm: &WasmProcess) {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "stasis_struct_field_defaults_{}_{}.wasm",
        std::process::id(),
        stamp
    ));
    fs::write(&path, wasm.module_bytes()).expect("write field-default Wasm module");
    let node = std::env::var_os("STASIS_NODE").unwrap_or_else(|| "node".into());
    let output = Command::new(node)
        .args([
            "-e",
            "const fs=require('node:fs');const m=new WebAssembly.Module(fs.readFileSync(process.argv[1]));const imports=WebAssembly.Module.imports(m);if(imports.length!==0){throw new Error('unexpected imports: '+JSON.stringify(imports))}const i=new WebAssembly.Instance(m,{});process.stdout.write(String(i.exports.main()));",
        ])
        .arg(&path)
        .output()
        .expect("run Node field-default Wasm fixture");
    let _ = fs::remove_file(path);
    assert!(
        output.status.success(),
        "Node failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "0");
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("canonical repository root")
}

fn run_linked_native_aot(aot: &AotProcess) {
    use stasis_jit::{AotLinkConfig, AotTarget};

    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| repository_root().join("target"));
    let directory = target.join(format!(
        "struct-field-defaults-aot-{}-{stamp}",
        std::process::id()
    ));
    fs::create_dir_all(&directory).expect("create linked field-default AOT directory");
    let deps = std::env::current_exe()
        .expect("test executable")
        .parent()
        .expect("Cargo deps directory")
        .to_path_buf();
    let (import, runtime) = [&deps, deps.parent().expect("Cargo profile directory")]
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
    .expect("copy linked field-default dynload runtime");
    let executable = directory.join(format!(
        "struct_field_defaults_aot{}",
        std::env::consts::EXE_SUFFIX
    ));

    #[cfg(windows)]
    let linker_path = Some(
        std::env::var_os("STASIS_AOT_LINKER")
            .map(PathBuf::from)
            .or_else(|| {
                cc::windows_registry::find_tool("x86_64-pc-windows-msvc", "link.exe")
                    .map(|tool| tool.path().to_path_buf())
            })
            .expect("MSVC link.exe is required for linked field-default AOT execution"),
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
    .expect("link field-default native AOT executable");

    #[cfg(windows)]
    let output = Command::new(repository_root().join(".cargo/stasis-sign-and-run.cmd"))
        .arg(executable.file_name().expect("linked executable name"))
        .current_dir(&directory)
        .output()
        .expect("run signed field-default AOT executable");
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
            .expect("run linked field-default AOT executable")
    };
    let _ = fs::remove_dir_all(directory);
    assert!(
        output.status.success(),
        "linked field-default AOT failed: status={} stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
