#![cfg(windows)]

use stasis_compiler::backend::aot::AotProcess;
use stasis_compiler::backend::jit::JitProcess;
use stasis_compiler::backend::state_layout::{aot_storage_symbol, AotStorageSymbolKind};
use stasis_dynload::{
    global_path_hash, register_global_f32_array, register_global_i32_array,
    register_global_u8_array, AotProbeSession, AotProbeStorageDescriptor, AotProbeStorageKind,
};
use stasis_jit::{link_objects_to_dynamic_library, AotLinkConfig, AotTarget};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const FIXTURE_PATH: &str = "tests/stasis/seams/sprite_run_writer_public_probe.stasis";
const FIXTURE: &str =
    include_str!("../../../tests/stasis/seams/sprite_run_writer_public_probe.stasis");
const GRAPHICS: &str = include_str!("../../../src/stdlib/graphics.stasis");
const BRICKOUT_PATH: &str = "samples/brickout_revenge/brickout_revenge.stasis";
const BRICKOUT: &str = include_str!("../../../samples/brickout_revenge/brickout_revenge.stasis");
const BRICKOUT_V1_PATH: &str = "samples/brickout_revenge/brickout_revenge_v1.stasis";
const BRICKOUT_V1: &str =
    include_str!("../../../samples/brickout_revenge/brickout_revenge_v1.stasis");
const BRICKOUT_V1_CMD_PATH: &str = "samples/brickout_revenge/brickout_revenge_v1_cmd.stasis";
const BRICKOUT_V1_CMD: &str =
    include_str!("../../../samples/brickout_revenge/brickout_revenge_v1_cmd.stasis");
const TYPED_SPRITE_PATH: &str = "samples/typed_sprite/main.stasis";
const TYPED_SPRITE: &str = include_str!("../../../samples/typed_sprite/main.stasis");
const ANDROID_RESOURCE_RESTORE_PATH: &str = "samples/android_resource_restore_seam/main.stasis";
const ANDROID_RESOURCE_RESTORE: &str =
    include_str!("../../../samples/android_resource_restore_seam/main.stasis");
const POINTER_PONG_PATH: &str = "samples/pointer_pong/main.stasis";
const POINTER_PONG: &str = include_str!("../../../samples/pointer_pong/main.stasis");
const LINE_ROOT: &str = "line_batch_public_probe";
const GEOMETRY_ROOT: &str = "geometry_order_public_probe";
const WRITER_ROOT: &str = "probe_writer_lifecycle";
const ROOT: &str = "sprite_run_writer_public_probe";
const RESET_ROOT: &str = "gfx_cmd_construction_reset";
const FINISH_ROOT: &str = "gfx_cmd_construction_finish";
const GFX_I32_COUNT: usize = 67_888;
const GFX_F32_COUNT: usize = 146_564;
const GFX_U8_COUNT: usize = 65_536;
const GFX_I_FLAGS: usize = 2;
const GFX_I_LINE_COUNT: usize = 3;
const GFX_I_SPRITE_COUNT: usize = 4;
const GFX_I_DROPPED_LINES: usize = 5;
const GFX_I_TEXT_COUNT: usize = 7;
const GFX_I_ORDER_COUNT: usize = 22;
const GFX_I_RECT_COUNT: usize = 24;
const GFX_I_SPRITE_RUN_COUNT: usize = 29;
const GFX_I_SPRITE_BASE: usize = 32;
const GFX_I_TEXT_BASE: usize = 12_320;
const GFX_I_SPRITE_RUN_BASE: usize = 18_464;
const GFX_I_ORDER_BASE: usize = 51_232;
const GFX_F_LINE_BASE: usize = 4;
const GFX_F_SPRITE_BASE: usize = 80_004;
const GFX_F_TEXT_BASE: usize = 133_252;

struct AotTree(PathBuf);

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

fn linker_path() -> PathBuf {
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

fn sign_aot_library(root: &Path, library: &Path) {
    let script = root.join("tools/windows/stasis-signing.ps1");
    let output = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(&script)
        .args(["sign", "-Artifact"])
        .arg(library)
        .output()
        .unwrap_or_else(|error| {
            panic!(
                "launch Windows signing policy for {}: {error}",
                library.display()
            )
        });
    assert!(
        output.status.success(),
        "Windows signing policy failed for {} with status {}\nstdout:\n{}\nstderr:\n{}",
        library.display(),
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn required_roots() -> Vec<String> {
    [
        LINE_ROOT,
        GEOMETRY_ROOT,
        WRITER_ROOT,
        ROOT,
        RESET_ROOT,
        FINISH_ROOT,
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

fn aot_storage_kind(storage_type: &str) -> AotProbeStorageKind {
    match storage_type {
        "i32" | "bool" | "u32" => AotProbeStorageKind::I32,
        "f32" => AotProbeStorageKind::F32,
        "f64" => AotProbeStorageKind::F64,
        "u8" => AotProbeStorageKind::U8,
        "u16" => AotProbeStorageKind::U16,
        other => panic!("unknown AOT storage lane '{other}'"),
    }
}

fn aot_storage_descriptors(process: &AotProcess) -> Vec<AotProbeStorageDescriptor> {
    let layout = process.state_layout();
    let mut descriptors = Vec::new();
    for scalar in &layout.scalars {
        descriptors.push(AotProbeStorageDescriptor::scalar(
            aot_storage_symbol(AotStorageSymbolKind::Scalar, &scalar.path, ""),
            global_path_hash(&scalar.path),
            aot_storage_kind(scalar.storage_type_name()),
        ));
    }
    for collection in &layout.collections {
        let len = usize::try_from(collection.capacity).unwrap_or_else(|_| {
            panic!("negative AOT collection capacity at '{}'", collection.path)
        });
        let collection_hash = global_path_hash(&collection.path);
        for field in &collection.fields {
            let field_hash = if field.field.is_empty() {
                0
            } else {
                global_path_hash(&field.field)
            };
            descriptors.push(AotProbeStorageDescriptor::array(
                aot_storage_symbol(AotStorageSymbolKind::Array, &collection.path, &field.field),
                collection_hash,
                field_hash,
                aot_storage_kind(field.storage_type_name()),
                len,
            ));
        }
    }
    descriptors
}

fn aot_storage_exports(process: &AotProcess) -> Vec<String> {
    let mut exports = BTreeSet::new();
    for root in required_roots() {
        exports.insert(aot_symbol(process, &root));
    }
    for descriptor in aot_storage_descriptors(process) {
        match descriptor {
            AotProbeStorageDescriptor::Scalar { symbol, .. }
            | AotProbeStorageDescriptor::Array { symbol, .. } => {
                exports.insert(symbol);
            }
        }
    }
    exports.into_iter().collect()
}

fn invoke_aot_frame(
    session: &mut AotProbeSession,
    reset: &str,
    root: &str,
    finish: &str,
    name: &str,
) {
    session
        .invoke_frame(reset, root, finish)
        .unwrap_or_else(|error| panic!("run linked AOT probe {name}: {error}"));
}

fn configured_jit(root: &Path) -> JitProcess {
    let mut process = JitProcess::new();
    process
        .set_project_root(root.to_string_lossy())
        .expect("set JIT project root");
    process.set_required_emit_roots(&required_roots());
    process.upsert_file(FIXTURE_PATH, FIXTURE);
    process
        .compile()
        .expect("compile public writer JIT fixture");
    process
}

fn run_jit_frame(process: &JitProcess, root: &str) {
    process
        .execute_void_noarg_by_name(RESET_ROOT)
        .unwrap_or_else(|error| panic!("start JIT frame for {root}: {error}"));
    let result = process
        .execute_i32_noarg_by_name(root)
        .unwrap_or_else(|error| panic!("execute JIT probe {root}: {error}"));
    assert_eq!(result, 0, "JIT probe result for {root}");
    let finish = process
        .execute_i32_onearg_by_name(FINISH_ROOT, result)
        .unwrap_or_else(|error| panic!("finish JIT frame for {root}: {error}"));
    assert_eq!(finish, 0, "JIT frame finish for {root}");
}

fn assert_f32(actual: f32, expected: f32, field: &str) {
    assert_eq!(actual.to_bits(), expected.to_bits(), "{field}");
}

fn assert_line_capacity(i32s: &[i32], f32s: &[f32]) {
    assert_eq!(i32s[GFX_I_FLAGS], 2, "line frame is published");
    assert_eq!(i32s[GFX_I_LINE_COUNT], 512, "line capacity");
    assert_eq!(
        i32s[GFX_I_DROPPED_LINES], 0,
        "line overflow is rejected by writer"
    );
    assert_eq!(i32s[GFX_I_ORDER_COUNT], 512, "line order count");
    assert_f32(f32s[GFX_F_LINE_BASE], 0.0, "first line x");
    assert_f32(
        f32s[GFX_F_LINE_BASE + 511 * 8],
        511.0,
        "last accepted line x",
    );
}

fn assert_geometry_order(i32s: &[i32]) {
    assert_eq!(i32s[GFX_I_FLAGS], 2, "geometry frame is published");
    assert_eq!(i32s[GFX_I_RECT_COUNT], 9_500, "geometry rectangle count");
    assert_eq!(i32s[GFX_I_LINE_COUNT], 500, "geometry line count");
    assert_eq!(i32s[GFX_I_DROPPED_LINES], 12, "geometry line overflow");
    assert_eq!(i32s[GFX_I_ORDER_COUNT], 10_000, "geometry order count");
    assert_eq!(i32s[GFX_I_ORDER_BASE], 4 * 16_384, "first rectangle order");
    assert_eq!(
        i32s[GFX_I_ORDER_BASE + 9_499],
        4 * 16_384 + 9_499,
        "last rectangle order"
    );
    assert_eq!(
        i32s[GFX_I_ORDER_BASE + 9_500],
        16_384,
        "first line order after geometry"
    );
    assert_eq!(
        i32s[GFX_I_ORDER_BASE + 9_999],
        16_384 + 499,
        "last line order after geometry"
    );
}

fn assert_final_packet(i32s: &[i32], f32s: &[f32], u8s: &[u8]) {
    assert_eq!(i32s[GFX_I_FLAGS], 2, "final frame is published");
    assert_eq!(i32s[GFX_I_SPRITE_COUNT], 4, "published sprite count");
    assert_eq!(i32s[GFX_I_TEXT_COUNT], 2, "published text count");
    assert_eq!(i32s[GFX_I_ORDER_COUNT], 6, "public drawable order count");
    assert_eq!(
        i32s[GFX_I_SPRITE_RUN_COUNT], 3,
        "published sprite run count"
    );
    assert_eq!(
        &i32s[GFX_I_SPRITE_BASE..GFX_I_SPRITE_BASE + 12],
        &[
            101,
            -1_430_532_899,
            0,
            202,
            287_454_020,
            0,
            303,
            -66,
            0,
            404,
            -239,
            0,
        ]
    );
    assert_eq!(
        &i32s[GFX_I_SPRITE_RUN_BASE..GFX_I_SPRITE_RUN_BASE + 24],
        &[0, 2, -1, 0, 0, 0, 0, 0, 2, 1, -1, 0, 0, 0, 0, 0, 3, 1, -1, 0, 0, 0, 0, 0,]
    );
    assert_eq!(
        &i32s[GFX_I_ORDER_BASE..GFX_I_ORDER_BASE + 6],
        &[32_768, 16_384, 32_769, 32_770, 49_152, 49_153]
    );
    assert_eq!(
        &i32s[GFX_I_TEXT_BASE..GFX_I_TEXT_BASE + 6],
        &[7, 0, 5, 5, -6, 0]
    );
    for (offset, expected) in [
        10.0, 20.0, 30.0, 40.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 1.5, 2.5, 15.0, 50.0, 60.0, 70.0,
        80.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, -1.0, 0.5, -30.0,
    ]
    .into_iter()
    .enumerate()
    {
        assert_f32(
            f32s[GFX_F_SPRITE_BASE + offset],
            expected,
            "semantic sprite descriptor",
        );
    }
    for (offset, expected) in [100.0, 101.0, 102.0, 103.0, 0.1, 0.2, 0.3, 0.4]
        .into_iter()
        .enumerate()
    {
        assert_f32(
            f32s[GFX_F_LINE_BASE + offset],
            expected,
            "line descriptor after sprite run",
        );
    }
    for (offset, expected) in [
        2.0, 3.0, 80.0, 48.0, 0.0, 0.0, 0.0, 0.0, 40.0, 24.0, 1.0, 1.0, 9.0, 12.0, 13.0, 14.0,
        15.0, 0.0, 0.0, 0.0, 0.0, 7.0, 7.5, 1.0, 1.0, 16.0,
    ]
    .into_iter()
    .enumerate()
    {
        assert_f32(
            f32s[GFX_F_SPRITE_BASE + 26 + offset],
            expected,
            "public immediate sprite descriptor",
        );
    }
    for (offset, expected) in [
        20.0, 21.0, 0.9, 0.8, 0.7, 0.6, 30.0, 31.0, 0.5, 0.4, 0.3, 0.2,
    ]
    .into_iter()
    .enumerate()
    {
        assert_f32(
            f32s[GFX_F_TEXT_BASE + offset],
            expected,
            "public text descriptor",
        );
    }
    assert_eq!(&u8s[..6], &[b'p', b'r', b'o', b'b', b'e', 0]);
}

fn aot_symbol(process: &AotProcess, name: &str) -> String {
    let function = process
        .program_snapshot()
        .expect("AOT program snapshot")
        .functions()
        .iter()
        .find(|function| function.name == name)
        .unwrap_or_else(|| panic!("AOT function metadata for {name}"));
    process
        .artifacts()
        .iter()
        .find(|artifact| artifact.function_id == function.id)
        .unwrap_or_else(|| panic!("AOT artifact for {name}"))
        .symbol_name
        .clone()
}

fn aot_array_symbol(path: &str) -> String {
    aot_storage_symbol(AotStorageSymbolKind::Array, path, "")
}

fn link_aot_probe_library(
    process: &mut AotProcess,
    output_dir: &Path,
    config: &AotLinkConfig,
) -> PathBuf {
    let final_symbol = aot_symbol(process, ROOT);
    let standalone_storage = process
        .compile_standalone_storage_object(&final_symbol)
        .expect("compile AOT command storage");
    let object_dir = output_dir.join("objects");
    let objects = process
        .write_object_files_by_id(&object_dir)
        .expect("write AOT probe objects");
    let mut object_paths: Vec<PathBuf> = objects.values().map(|(_, path)| path.clone()).collect();
    if let Some((storage_bytes, _wrapper_symbol)) = standalone_storage {
        let storage_path = object_dir.join("direct_storage.obj");
        fs::write(&storage_path, storage_bytes).expect("write AOT command storage");
        object_paths.push(storage_path);
    }

    let exports = aot_storage_exports(process);
    let library_path = output_dir.join("sprite_run_writer_public_probe.dll");
    link_objects_to_dynamic_library(&object_paths, &library_path, &exports, config)
        .expect("link AOT probe dynamic library");
    library_path
}

#[test]
fn public_sprite_run_writer_matches_jit_and_linked_aot() {
    let root = repository_root();
    let mut i32s = vec![0; GFX_I32_COUNT];
    let mut f32s = vec![0.0; GFX_F32_COUNT];
    let mut u8s = vec![0; GFX_U8_COUNT];
    register_global_i32_array(
        global_path_hash("gfx_cmd_i32"),
        0,
        i32s.as_mut_ptr(),
        i32s.len(),
    );
    register_global_f32_array(
        global_path_hash("gfx_cmd_f32"),
        0,
        f32s.as_mut_ptr(),
        f32s.len(),
    );
    register_global_u8_array(
        global_path_hash("gfx_cmd_u8"),
        0,
        u8s.as_mut_ptr(),
        u8s.len(),
    );

    let jit = configured_jit(&root);
    run_jit_frame(&jit, LINE_ROOT);
    assert_line_capacity(&i32s, &f32s);
    run_jit_frame(&jit, GEOMETRY_ROOT);
    assert_geometry_order(&i32s);
    run_jit_frame(&jit, WRITER_ROOT);
    assert_eq!(i32s[GFX_I_FLAGS], 2, "writer lifecycle frame is published");
    assert_eq!(
        i32s[GFX_I_SPRITE_COUNT], 0,
        "cancelled writer publishes no sprites"
    );
    assert_eq!(
        i32s[GFX_I_ORDER_COUNT], 0,
        "cancelled writer publishes no order"
    );
    run_jit_frame(&jit, ROOT);
    assert_final_packet(&i32s, &f32s, &u8s);

    let mut aot = AotProcess::new();
    aot.set_project_root(root.to_string_lossy())
        .expect("set AOT project root");
    aot.set_required_emit_roots(&required_roots());
    aot.upsert_file(FIXTURE_PATH, FIXTURE);
    aot.compile().expect("compile public writer AOT fixture");
    let output_dir = AotTree(
        std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| root.join("target"))
            .join(format!("sprite-run-writer-aot-{}", std::process::id())),
    );
    fs::create_dir_all(&output_dir.0).expect("create AOT output directory");
    let (import, runtime) = dynload_artifacts();
    let config = AotLinkConfig {
        linker_path: Some(linker_path()),
        // compile_standalone_storage_object emits an unused Windows entry
        // wrapper that references ExitProcess; the /NOENTRY DLL never calls
        // it, but the linker still needs the system import library.
        runtime_lib_paths: vec![import, PathBuf::from("kernel32.lib")],
        target: AotTarget::Native,
    };
    let library_path = link_aot_probe_library(&mut aot, &output_dir.0, &config);
    sign_aot_library(&root, &library_path);
    // The session owns the exact runtime DLL and probe DLL while their typed
    // compiler-owned storage descriptors are registered and copied.
    let mut session = AotProbeSession::load(&runtime, &library_path)
        .expect("open exclusive linked-AOT probe session");
    let invalid = [AotProbeStorageDescriptor::array(
        "invalid_storage_extent",
        0,
        0,
        AotProbeStorageKind::I32,
        i32::MAX as usize + 1,
    )];
    assert!(
        session.bootstrap(&[], &invalid).is_err(),
        "reject a malformed descriptor before registry mutation"
    );
    let literals = aot
        .string_literals()
        .iter()
        .map(|(&id, value)| (id, value.clone()))
        .collect::<Vec<_>>();
    let descriptors = aot_storage_descriptors(&aot);
    session
        .bootstrap(&literals, &descriptors)
        .expect("bootstrap linked AOT storage");
    let reset = aot_symbol(&aot, RESET_ROOT);
    let finish = aot_symbol(&aot, FINISH_ROOT);
    let line = aot_symbol(&aot, LINE_ROOT);
    let geometry = aot_symbol(&aot, GEOMETRY_ROOT);
    let writer = aot_symbol(&aot, WRITER_ROOT);
    let final_probe = aot_symbol(&aot, ROOT);
    let i32_symbol = aot_array_symbol("gfx_cmd_i32");
    let f32_symbol = aot_array_symbol("gfx_cmd_f32");
    let u8_symbol = aot_array_symbol("gfx_cmd_u8");
    assert!(
        session.read_f32(&i32_symbol).is_err(),
        "reject a typed snapshot requested with the wrong lane"
    );
    invoke_aot_frame(&mut session, &reset, &line, &finish, LINE_ROOT);
    let aot_i32s = session
        .read_i32(&i32_symbol)
        .expect("snapshot AOT i32 storage");
    let aot_f32s = session
        .read_f32(&f32_symbol)
        .expect("snapshot AOT f32 storage");
    assert_line_capacity(&aot_i32s, &aot_f32s);
    invoke_aot_frame(&mut session, &reset, &geometry, &finish, GEOMETRY_ROOT);
    let aot_i32s = session
        .read_i32(&i32_symbol)
        .expect("snapshot AOT i32 storage");
    assert_geometry_order(&aot_i32s);
    invoke_aot_frame(&mut session, &reset, &writer, &finish, WRITER_ROOT);
    let aot_i32s = session
        .read_i32(&i32_symbol)
        .expect("snapshot AOT i32 storage");
    assert_eq!(
        aot_i32s[GFX_I_FLAGS], 2,
        "AOT writer lifecycle frame is published"
    );
    assert_eq!(
        aot_i32s[GFX_I_SPRITE_COUNT], 0,
        "AOT cancelled writer publishes no sprites"
    );
    assert_eq!(
        aot_i32s[GFX_I_ORDER_COUNT], 0,
        "AOT cancelled writer publishes no order"
    );
    invoke_aot_frame(&mut session, &reset, &final_probe, &finish, ROOT);
    let aot_i32s = session
        .read_i32(&i32_symbol)
        .expect("snapshot AOT i32 storage");
    let aot_f32s = session
        .read_f32(&f32_symbol)
        .expect("snapshot AOT f32 storage");
    let aot_u8s = session
        .read_u8(&u8_symbol)
        .expect("snapshot AOT u8 storage");
    assert_final_packet(&aot_i32s, &aot_f32s, &aot_u8s);
}

#[test]
fn aot_probe_session_rejects_an_occupied_registry_and_releases_it() {
    let (_, runtime) = dynload_artifacts();
    let session = AotProbeSession::load(&runtime, &runtime)
        .expect("claim empty runtime registry for test session");
    let occupied = AotProbeSession::load(&runtime, &runtime)
        .err()
        .expect("reject a second session while storage is owned");
    assert!(occupied.contains("occupied"));
    drop(session);
    let reopened =
        AotProbeSession::load(&runtime, &runtime).expect("allow a session after registry teardown");
    drop(reopened);
}

#[test]
fn public_writer_contract_and_brickout_compile() {
    assert!(!GRAPHICS.contains("gfx_reserve_sprite_run"));
    assert!(!GRAPHICS.contains("let writer: SpriteRunWriter"));
    assert!(GRAPHICS.contains("function reserve(self: SpriteRunWriter, max_count: i32"));
    assert!(!BRICKOUT.contains("SpriteRunWriter"));
    assert!(BRICKOUT.contains("presentation: PresentationList"));
    assert!(BRICKOUT.contains("state.gfx.presentation.append_sprite("));
    assert!(BRICKOUT.contains("state.gfx.presentation.replay()"));
    assert!(GRAPHICS.contains("struct LineBatch"));
    assert!(!GRAPHICS.contains("function draw_lines("));
    assert!(BRICKOUT.contains("state.gfx.lines.draw()"));

    let mut jit = JitProcess::new();
    jit.set_project_root(repository_root().to_string_lossy())
        .expect("set Brickout JIT project root");
    jit.set_required_emit_roots(&["tick".to_string()]);
    jit.upsert_file(BRICKOUT_PATH, BRICKOUT);
    jit.compile().expect("compile migrated Brickout sample");

    for (path, source) in [
        (BRICKOUT_V1_PATH, BRICKOUT_V1),
        (BRICKOUT_V1_CMD_PATH, BRICKOUT_V1_CMD),
    ] {
        assert!(!source.contains(".handle"), "raw sprite handle in {path}");
        let mut jit = JitProcess::new();
        jit.set_project_root(repository_root().to_string_lossy())
            .expect("set historical Brickout JIT project root");
        jit.set_required_emit_roots(&["tick".to_string()]);
        jit.upsert_file(path, source);
        jit.compile()
            .unwrap_or_else(|error| panic!("compile migrated {path}: {error:?}"));
    }

    assert!(!TYPED_SPRITE.contains("sprite.handle"));
    let mut typed_sprite = JitProcess::new();
    typed_sprite
        .set_project_root(repository_root().to_string_lossy())
        .expect("set typed-sprite JIT project root");
    typed_sprite.set_required_emit_roots(&["main".to_string()]);
    typed_sprite.upsert_file(TYPED_SPRITE_PATH, TYPED_SPRITE);
    typed_sprite
        .compile()
        .expect("compile typed-sprite sample without raw sprite handles");

    assert!(!ANDROID_RESOURCE_RESTORE.contains("fallback.handle"));
    assert!(ANDROID_RESOURCE_RESTORE.contains("fallback_owner.reference()"));
    let resource_restore = ANDROID_RESOURCE_RESTORE.replace(
        "import \"/vendor/stasis/src/stdlib/graphics.stasis\";",
        "import \"../../src/stdlib/graphics.stasis\";",
    );
    let mut resource_restore_jit = JitProcess::new();
    resource_restore_jit
        .set_project_root(repository_root().to_string_lossy())
        .expect("set Android resource-restore JIT project root");
    resource_restore_jit.set_required_emit_roots(&[
        "main".to_string(),
        "tick".to_string(),
        "render".to_string(),
    ]);
    resource_restore_jit.upsert_file(ANDROID_RESOURCE_RESTORE_PATH, &resource_restore);
    resource_restore_jit
        .compile()
        .expect("compile Android resource-restore typed stale-reference path");
}

#[test]
fn dynamic_text_run_api_and_pointer_pong_compile_for_jit_and_aot() {
    assert!(GRAPHICS.contains(
        "function @effects(graphics)@extern(\"stasis_jit_text_run_replace_from\") replace_text_from(self: TextRun, font: i32, text: utf8[]): bool;"
    ));
    assert!(POINTER_PONG.contains("left_score_run.replace_text_from"));
    assert!(!POINTER_PONG.contains("struct ScoreDigits"));
    assert!(!POINTER_PONG.contains("ascii_push_i32(scratch"));
    assert!(POINTER_PONG.contains("48 + display / 10"));
    assert!(POINTER_PONG.contains("48 + display % 10"));
    let source = POINTER_PONG.replace(
        "import \".stasis_cache/toolchain/src/stdlib/graphics.stasis\";",
        "import \"../../src/stdlib/graphics.stasis\";",
    );
    let root = repository_root();
    let required = ["main".to_string(), "tick".to_string(), "render".to_string()];
    let mut jit = JitProcess::new();
    jit.set_project_root(root.to_string_lossy()).unwrap();
    jit.set_required_emit_roots(&required);
    jit.upsert_file(POINTER_PONG_PATH, &source);
    jit.compile().expect("compile dynamic Pointer Pong JIT");

    let mut aot = AotProcess::new();
    aot.set_project_root(root.to_string_lossy()).unwrap();
    aot.set_required_emit_roots(&required);
    aot.upsert_file(POINTER_PONG_PATH, &source);
    aot.compile().expect("compile dynamic Pointer Pong AOT");
}
