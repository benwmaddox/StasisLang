#![cfg(windows)]

use stasis_compiler::backend::aot::AotProcess;
use stasis_compiler::backend::jit::JitProcess;
use stasis_dynload::{
    global_path_hash, register_global_f32_array, register_global_i32_array,
    register_global_u8_array,
};
use stasis_jit::{AotLinkConfig, AotTarget};
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
const ROOT: &str = "sprite_run_writer_public_probe";
const GFX_I32_COUNT: usize = 67_888;
const GFX_F32_COUNT: usize = 146_564;
const GFX_U8_COUNT: usize = 65_536;

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

fn assert_f32(actual: f32, expected: f32, field: &str) {
    assert_eq!(actual.to_bits(), expected.to_bits(), "{field}");
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

    let mut jit = JitProcess::new();
    jit.set_project_root(root.to_string_lossy())
        .expect("set JIT project root");
    jit.set_required_emit_roots(&[ROOT.to_string()]);
    jit.upsert_file(FIXTURE_PATH, FIXTURE);
    jit.compile().expect("compile public writer JIT fixture");
    assert_eq!(
        jit.execute_i32_noarg_by_name(ROOT),
        Ok(0),
        "public writer JIT result"
    );

    assert_eq!(i32s[4], 4, "published sprite count");
    assert_eq!(i32s[7], 2, "published text count");
    assert_eq!(i32s[22], 6, "public drawable order count");
    assert_eq!(i32s[29], 3, "published sprite run count");
    assert_eq!(
        &i32s[32..44],
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
        &i32s[18_464..18_488],
        &[0, 2, -1, 0, 0, 0, 0, 0, 2, 1, -1, 0, 0, 0, 0, 0, 3, 1, -1, 0, 0, 0, 0, 0,]
    );
    assert_eq!(
        &i32s[51_232..51_238],
        &[32_768, 16_384, 32_769, 32_770, 49_152, 49_153]
    );
    assert_eq!(&i32s[12_320..12_326], &[7, 0, 5, 5, -6, 0]);
    for (offset, expected) in [
        10.0, 20.0, 30.0, 40.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 1.5, 2.5, 15.0, 50.0, 60.0, 70.0,
        80.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, -1.0, 0.5, -30.0,
    ]
    .into_iter()
    .enumerate()
    {
        assert_f32(
            f32s[80_004 + offset],
            expected,
            "semantic sprite descriptor",
        );
    }
    for (offset, expected) in [100.0, 101.0, 102.0, 103.0, 0.1, 0.2, 0.3, 0.4]
        .into_iter()
        .enumerate()
    {
        assert_f32(
            f32s[4 + offset],
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
            f32s[80_030 + offset],
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
        assert_f32(f32s[133_252 + offset], expected, "public text descriptor");
    }
    assert_eq!(&u8s[..6], &[b'p', b'r', b'o', b'b', b'e', 0]);

    let mut aot = AotProcess::new();
    aot.set_project_root(root.to_string_lossy())
        .expect("set AOT project root");
    aot.set_required_emit_roots(&[ROOT.to_string()]);
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
    fs::copy(runtime, output_dir.0.join("stasis_dynload.dll")).expect("copy AOT runtime");
    let executable = output_dir.0.join("sprite_run_writer_public_probe.exe");
    let config = AotLinkConfig {
        linker_path: Some(linker_path()),
        runtime_lib_paths: vec![import],
        target: AotTarget::Native,
    };
    aot.link_executable_for_i32_noarg_function(ROOT, &executable, &config)
        .expect("link public writer AOT fixture");
    let status = Command::new(root.join(".cargo/stasis-sign-and-run.cmd"))
        .arg(executable.file_name().expect("AOT executable name"))
        .current_dir(&output_dir.0)
        .status()
        .expect("run linked public writer AOT fixture");
    let aot_code = status.code().expect("linked AOT process exit code");
    let signed_execution_required =
        std::env::var_os("STASIS_REQUIRE_SIGNED_EXECUTION").is_some_and(|value| value == "1");
    if aot_code == 4551 && !signed_execution_required {
        eprintln!(
            "skipping linked AOT execution parity: Windows Application Control returned 4551 and signed execution is not required"
        );
        return;
    }
    assert_eq!(aot_code, 0, "linked AOT public writer result");
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
