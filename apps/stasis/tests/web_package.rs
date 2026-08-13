use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repository root")
        .to_path_buf()
}

fn stamp() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos()
}

fn package(workspace: &Path, relative_output: &Path) -> PathBuf {
    let output = workspace.join(relative_output);
    let result = Command::new(env!("CARGO_BIN_EXE_stasis"))
        .arg("package")
        .arg("--workspace")
        .arg(workspace)
        .arg("--target")
        .arg("web")
        .arg("--development-build")
        .arg("--out")
        .arg(relative_output)
        .arg("--json")
        .output()
        .expect("run web package");
    assert!(
        result.status.success(),
        "web package failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        String::from_utf8_lossy(&result.stdout).contains("\"wasm_optimized\":false"),
        "development package unexpectedly optimized Wasm: {}",
        String::from_utf8_lossy(&result.stdout)
    );
    output
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("create copied fixture directory");
    let mut entries = fs::read_dir(source)
        .expect("read copied fixture")
        .collect::<Result<Vec<_>, _>>()
        .expect("enumerate copied fixture");
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let file_type = entry.file_type().expect("copied fixture file type");
        assert!(!file_type.is_symlink(), "fixture symlinks are unsupported");
        let target = destination.join(entry.file_name());
        if file_type.is_dir() {
            copy_tree(&entry.path(), &target);
        } else if file_type.is_file() {
            fs::copy(entry.path(), target).expect("copy fixture file");
        }
    }
}

#[test]
fn web_package_contains_runnable_wasm_static_bundle_and_standalone_file() {
    let workspace = repo_root().join("samples/web_export_smoke");
    let stamp = stamp();
    let relative_output = PathBuf::from(format!("build/web-package-test-{stamp}"));
    let output = package(&workspace, &relative_output);

    let wasm = fs::read(output.join("play/game.wasm")).expect("play/game.wasm");
    assert!(wasm.starts_with(b"\0asm\x01\0\0\0"));
    for export in ["main", "tick", "render", "player_x"] {
        assert!(
            wasm.windows(export.len())
                .any(|window| window == export.as_bytes()),
            "missing Wasm export {export}"
        );
    }
    let runtime = fs::read_to_string(output.join("play/game.js")).expect("play/game.js");
    for expected in [
        "requestAnimationFrame(frame)",
        "web_play_tone",
        "dataset.underBudget",
        "\"host_i32\"",
        "\"host_f32\"",
        "audio_push_f32_interleaved",
        "function writeHostFrame",
        "function applyWindowRequest",
        "function sdlScancode",
    ] {
        assert!(
            runtime.contains(expected),
            "missing web runtime data {expected}"
        );
    }
    let standalone =
        fs::read_to_string(output.join("web_export_smoke.html")).expect("standalone web package");
    assert!(standalone.contains("window.STASIS_WASM_BASE64"));
    assert!(!standalone.contains("__STASIS_"));
    assert!(output.join("play/index.html").is_file());
    assert!(output.join("stasis_provenance.json").is_file());

    fs::remove_dir_all(&output).expect("clean web package test output");
}

#[test]
fn existing_windows_game_packages_command_buffers_sprites_and_font_for_web() {
    let workspace = repo_root().join("samples/windows_launch_smoke");
    let relative_output = PathBuf::from(format!("build/web-package-test-{}", stamp()));
    let output = package(&workspace, &relative_output);

    let wasm = fs::read(output.join("play/game.wasm")).expect("existing game Wasm");
    assert!(wasm.starts_with(b"\0asm\x01\0\0\0"));
    assert!(wasm.windows(6).any(|window| window == b"memory"));
    let runtime = fs::read_to_string(output.join("play/game.js")).expect("existing game runtime");
    let standalone = fs::read_to_string(output.join("windows_launch_smoke.html"))
        .expect("existing game standalone runtime");
    for expected in ["gfx_cmd_i32", "gfx_cmd_f32", "stasis_jit_gfx_cache_text"] {
        assert!(
            runtime.contains(expected),
            "missing web runtime data {expected}"
        );
    }
    for expected in [
        "data:image/png;base64,",
        "data:image/svg+xml;base64,",
        "data:font/ttf;base64,",
    ] {
        assert!(
            standalone.contains(expected),
            "missing embedded asset {expected}"
        );
        assert!(
            !runtime.contains(expected),
            "static runtime embedded {expected}"
        );
    }
    assert!(output.join("assets/smoke.png").is_file());
    assert!(output.join("assets/smoke.svg").is_file());
    assert!(output.join("assets/smoke.ttf").is_file());

    fs::remove_dir_all(&output).expect("clean existing game web package");
}

#[test]
fn existing_audio_game_packages_wav_and_mp3_for_web_audio() {
    let root = repo_root();
    let workspace = root
        .join("build")
        .join(format!("web-audio-test-{}", stamp()));
    copy_tree(&root.join("samples/audio_asset_playback"), &workspace);
    copy_tree(&root.join("src"), &workspace.join("vendor/stasis/src"));
    let output = package(&workspace, Path::new("build/web-package"));

    let runtime = fs::read_to_string(output.join("play/game.js")).expect("audio game runtime");
    let standalone = fs::read_to_string(output.join("audio_asset_playback.html"))
        .expect("audio standalone runtime");
    for expected in [
        "stasis_jit_audio_load_music",
        "stasis_jit_audio_load_effect",
        "decodeAudioData",
    ] {
        assert!(
            runtime.contains(expected),
            "missing web audio data {expected}"
        );
    }
    for expected in ["data:audio/mpeg;base64,", "data:audio/wav;base64,"] {
        assert!(
            standalone.contains(expected),
            "missing embedded audio {expected}"
        );
        assert!(
            !runtime.contains(expected),
            "static runtime embedded {expected}"
        );
    }
    assert!(output.join("assets/tone.mp3").is_file());
    assert!(output.join("assets/tone.wav").is_file());

    fs::remove_dir_all(&workspace).expect("clean existing audio web fixture");
}
