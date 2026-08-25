#![cfg(windows)]

use serde_json::json;
use stasis::{
    audit_mobile_aot_bindings, sign_output_artifact_if_configured, write_mobile_aot_bindings_source,
};
use stasis_assets::{load_project_asset_manifest, prepare_asset_bundle, sha256_bytes, AssetLimits};
use stasis_compiler::backend::aot::AotProcess;
use stasis_compiler::backend::EngineEntrypoints;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const FIXTURE: &str =
    include_str!("../../../tests/stasis/seams/mobile_packaged_assets_probe.stasis");
const EXPECTED_RENDER_TRACE: u32 = 4_249_029_299;

struct TestTree(PathBuf);

impl Drop for TestTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn repository_root() -> PathBuf {
    let canonical = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("canonical repository root");
    let display = canonical.to_string_lossy();
    PathBuf::from(display.strip_prefix(r"\\?\").unwrap_or(&display))
}

fn evidence_root() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| repository_root().join("target"))
        .join("seam-tests")
}

fn locate_cl() -> PathBuf {
    if let Some(path) = std::env::var_os("STASIS_C_COMPILER").map(PathBuf::from) {
        assert!(
            path.is_file(),
            "STASIS_C_COMPILER must name a compiler file"
        );
        return path;
    }
    let output = Command::new("where.exe")
        .arg("cl.exe")
        .output()
        .expect("locate MSVC compiler");
    assert!(
        output.status.success(),
        "MSVC cl.exe is required for IT-015"
    );
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(PathBuf::from)
        .expect("cl.exe path")
}

fn locate_runtime_import_library(runtime_dll: &Path) -> PathBuf {
    if let Some(path) = std::env::var_os("STASIS_RUNTIME_IMPORT_LIBRARY_PATH").map(PathBuf::from) {
        assert!(
            path.is_file(),
            "STASIS_RUNTIME_IMPORT_LIBRARY_PATH must name an import library file"
        );
        return path;
    }
    let candidates = [
        runtime_dll.with_file_name("stasis_graphics.lib"),
        repository_root().join("runtime/build/bin/stasis_graphics.lib"),
    ];
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .expect("graphics runtime import library is missing")
}

fn wav_fixture() -> Vec<u8> {
    let samples = [0_i16, 16_384, -16_384, 8_192];
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36_u32 + (samples.len() * 2) as u32).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&24_000_u32.to_le_bytes());
    bytes.extend_from_slice(&48_000_u32.to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&((samples.len() * 2) as u32).to_le_bytes());
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

#[test]
fn packaged_mobile_assets_reach_real_native_hosts_from_linked_aot() {
    let runtime_dll = PathBuf::from(
        std::env::var_os("STASIS_RUNTIME_DLL_PATH")
            .expect("STASIS_RUNTIME_DLL_PATH must name the CI-built SDL runtime"),
    );
    let runtime_import = locate_runtime_import_library(&runtime_dll);
    assert!(runtime_dll.is_file(), "graphics runtime DLL is missing");

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let tree = TestTree(
        std::env::temp_dir().join(format!("stasis-it-015-{}-{stamp}", std::process::id())),
    );
    let project = tree.0.join("project");
    let bundle_root = tree.0.join("package/stasis_game");
    let engine_root = tree.0.join("engine");
    fs::create_dir_all(project.join("src")).expect("create source directory");
    fs::create_dir_all(project.join("assets")).expect("create asset directory");
    fs::write(project.join("src/main.stasis"), FIXTURE).expect("write AOT fixture");
    fs::copy(
        repository_root().join("samples/windows_launch_smoke/assets/smoke.png"),
        project.join("assets/sprite.png"),
    )
    .expect("copy sprite fixture");
    fs::copy(
        repository_root().join("samples/windows_launch_smoke/assets/smoke.ttf"),
        project.join("assets/ui.ttf"),
    )
    .expect("copy font fixture");
    let wav = wav_fixture();
    fs::write(project.join("assets/tone.wav"), &wav).expect("write audio fixture");
    fs::copy(project.join("assets/ui.ttf"), tree.0.join("outside.ttf"))
        .expect("write traversal sentinel");

    let sprite = fs::read(project.join("assets/sprite.png")).expect("read sprite fixture");
    let font = fs::read(project.join("assets/ui.ttf")).expect("read font fixture");
    let source_hashes = [
        sha256_bytes(&sprite),
        sha256_bytes(&font),
        sha256_bytes(&wav),
    ];
    let manifest = json!({
        "schema": "stasis-assets",
        "version": 1,
        "assets": [
            {"id":"sprite","path":"assets/sprite.png","content_sha256":source_hashes[0],"format":{"kind":"sprite","encoding":"png","width":64,"height":64},"dependencies":[]},
            {"id":"ui","path":"assets/ui.ttf","content_sha256":source_hashes[1],"format":{"kind":"font","encoding":"ttf"},"dependencies":[]},
            {"id":"tone","path":"assets/tone.wav","content_sha256":source_hashes[2],"format":{"kind":"audio","encoding":"wav","sample_rate":24000,"channels":1,"duration_frames":4},"dependencies":[]}
        ]
    });
    fs::write(
        project.join("assets/manifest.json"),
        serde_json::to_vec_pretty(&manifest).expect("encode manifest"),
    )
    .expect("write manifest");
    fs::write(
        project.join("stasis.json"),
        "{\"manifest_version\":1,\"name\":\"it015\",\"entry\":\"src/main.stasis\",\"tests\":\"tests\",\"output\":\"build\"}\n",
    )
    .expect("write project manifest");

    let resolved = load_project_asset_manifest(&project, AssetLimits::default())
        .expect("validate source asset identities and hashes");
    assert_eq!(resolved.assets.len(), 3);
    let prepared = prepare_asset_bundle(&resolved, &bundle_root, tree.0.join("asset-cache"))
        .expect("prepare mobile stasis_game bundle");
    assert_eq!(prepared.copied_assets, 3);
    let packaged = load_project_asset_manifest(&bundle_root, AssetLimits::default())
        .expect("validate packaged asset identities and hashes");
    assert_eq!(
        packaged
            .by_id("sprite")
            .expect("packaged sprite")
            .entry
            .content_sha256,
        source_hashes[0]
    );
    assert_eq!(
        packaged
            .by_id("ui")
            .expect("packaged font")
            .entry
            .content_sha256,
        source_hashes[1]
    );
    assert_eq!(
        packaged
            .by_id("tone")
            .expect("packaged audio")
            .entry
            .content_sha256,
        source_hashes[2]
    );

    let mut process = AotProcess::new();
    process
        .set_project_root(project.to_string_lossy())
        .expect("set AOT project root");
    process.upsert_file("src/main.stasis", FIXTURE);
    process
        .compile()
        .expect("compile packaged asset AOT fixture");
    let engine = process
        .write_engine_bundle(&EngineEntrypoints::runtime_default(), &engine_root)
        .expect("write engine bundle");
    let engine_manifest: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(&engine.manifest_path).expect("read engine manifest"),
    )
    .expect("decode engine manifest");
    let bindings = engine_root.join("published_aot_bindings.c");
    write_mobile_aot_bindings_source(
        &engine_manifest,
        &process.state_layout(),
        &project,
        &bindings,
    )
    .expect("write mobile AOT bindings");
    audit_mobile_aot_bindings(
        &engine_manifest,
        &fs::read_to_string(&bindings).expect("read bindings"),
    )
    .expect("audit mobile AOT bindings");

    let root = repository_root();
    let runtime = root.join("runtime");
    let executable = tree.0.join("it015_mobile_packaged_assets.exe");
    let mut command = Command::new(locate_cl());
    command
        .current_dir(&tree.0)
        .args([
            "/nologo",
            "/W4",
            "/WX",
            "/std:c11",
            "/experimental:c11atomics",
            "/wd4244",
            "/wd4267",
            "/wd4456",
            "/D_CRT_SECURE_NO_WARNINGS",
        ])
        .arg(format!("/I{}", runtime.display()))
        .arg(runtime.join("tests/stasis_mobile_packaged_assets_integration.c"))
        .arg(runtime.join("stasis_mobile_aot_runtime.c"))
        .arg(runtime.join("stasis_audio_assets.c"))
        .arg(&bindings);
    for path in engine.object_paths_by_function_id.values() {
        command.arg(path);
    }
    let link = command
        .arg(&runtime_import)
        .arg(format!("/Fe:{}", executable.display()))
        .output()
        .expect("compile linked mobile asset harness");
    assert!(
        link.status.success(),
        "mobile asset harness link failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&link.stdout),
        String::from_utf8_lossy(&link.stderr)
    );
    sign_output_artifact_if_configured(&executable).expect("sign linked mobile asset harness");

    let run = Command::new(&executable)
        .arg(&bundle_root)
        .current_dir(runtime_dll.parent().expect("runtime DLL directory"))
        .env("SDL_VIDEODRIVER", "dummy")
        .env("SDL_RENDER_DRIVER", "software")
        .env("SDL_AUDIODRIVER", "dummy")
        .output()
        .expect("run linked mobile asset harness");
    assert!(
        run.status.success(),
        "mobile asset harness failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8(run.stdout).expect("UTF-8 harness output");
    assert!(stdout.contains("stasis.seam_test.v1 IT-015"));
    assert!(stdout.contains("samples=0.125:0.125 offline_active=1"));
    assert!(stdout.contains("../../missing.wav"));
    let trace = stdout
        .split_whitespace()
        .find_map(|field| field.strip_prefix("trace="))
        .and_then(|value| value.parse::<u32>().ok())
        .expect("render trace");
    assert_eq!(
        trace, EXPECTED_RENDER_TRACE,
        "exact packaged asset frame trace"
    );

    let evidence = json!({
        "schema": "stasis.seam_test.v1",
        "test_id": "IT-015",
        "status": "passed",
        "target": "windows-native-aot+mobile-package+shared-sdl-runtime",
        "bundle_root": bundle_root,
        "manifest": packaged.assets.iter().map(|asset| json!({
            "id": asset.entry.id,
            "path": asset.entry.path,
            "sha256": asset.entry.content_sha256
        })).collect::<Vec<_>>(),
        "native_handles": {"sprite": "positive", "font": "positive", "cached_text": "positive", "audio": "positive", "voice": "positive"},
        "render_trace": trace,
        "offline_audio": {"output_lr_frame_1": [0.125, 0.125], "voice_active_after_4_output_frames": true},
        "containment": {
            "font_request": "../../outside.ttf",
            "outside_valid_font_loaded": false,
            "audio_request": "../../missing.wav",
            "diagnostics_preserved_requests": true
        }
    });
    let evidence_path = evidence_root().join("it-015-mobile-packaged-assets.json");
    fs::create_dir_all(evidence_path.parent().expect("evidence parent"))
        .expect("create evidence directory");
    fs::write(
        &evidence_path,
        serde_json::to_vec_pretty(&evidence).expect("encode evidence"),
    )
    .expect("write evidence");
    eprintln!("IT-015 evidence: {evidence}");
}
