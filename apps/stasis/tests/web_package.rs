use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

const CONTINUE_LOOP_PARITY: &str =
    include_str!("../../../tests/stasis/seams/continue_loop_parity.stasis");

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
        String::from_utf8_lossy(&result.stdout).contains("\"development_build\":false"),
        "source-built package did not select local release provenance: {}",
        String::from_utf8_lossy(&result.stdout)
    );
    let provenance: serde_json::Value = serde_json::from_slice(
        &fs::read(output.join("stasis_provenance.json")).expect("read web provenance"),
    )
    .expect("parse web provenance");
    assert_eq!(provenance["build_class"], "local_release");
    assert_eq!(provenance["development_build"], false);
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

fn execute_web_main(wasm: &Path) -> Output {
    Command::new("node")
        .arg("-e")
        .arg(
            "const fs = require('node:fs'); WebAssembly.instantiate(fs.readFileSync(process.argv[1]), {}).then(({ instance }) => process.stdout.write(String(instance.exports.main()))).catch((error) => { console.error(error); process.exit(1); });",
        )
        .arg(wasm)
        .output()
        .expect("execute packaged Wasm in Node")
}

#[test]
fn web_continue_matches_native_for_and_foreach_loop_steps() {
    let root = repo_root();
    let workspace = root
        .join("build")
        .join(format!("web-continue-test-{}", stamp()));
    fs::create_dir_all(workspace.join("src")).expect("create web continue fixture");
    fs::write(
        workspace.join("stasis.json"),
        r#"{"manifest_version":1,"name":"web_continue_parity","entry":"src/main.stasis","tests":"tests","output":"build"}"#,
    )
    .expect("write web continue manifest");
    fs::write(workspace.join("src/main.stasis"), CONTINUE_LOOP_PARITY)
        .expect("write web continue source");

    let output = package(&workspace, Path::new("build/web-package"));
    let execution = execute_web_main(&output.join("game.wasm"));
    assert!(
        execution.status.success(),
        "packaged Wasm execution failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&execution.stdout),
        String::from_utf8_lossy(&execution.stderr)
    );
    assert_eq!(
        String::from_utf8(execution.stdout).expect("UTF-8 result"),
        "434"
    );

    fs::remove_dir_all(&workspace).expect("clean web continue fixture");
}

#[test]
fn web_package_contains_runnable_static_bundle_without_standalone_html() {
    let workspace = repo_root().join("samples/web_export_smoke");
    let stamp = stamp();
    let relative_output = PathBuf::from(format!("build/web-package-test-{stamp}"));
    let output = package(&workspace, &relative_output);
    fs::write(output.join("stale.txt"), "old package").expect("write stale package file");
    let output = package(&workspace, &relative_output);
    assert!(
        !output.join("stale.txt").exists(),
        "replacement retained a stale package file"
    );

    let wasm = fs::read(output.join("game.wasm")).expect("game.wasm");
    assert!(wasm.starts_with(b"\0asm\x01\0\0\0"));
    for export in ["main", "tick", "render"] {
        assert!(
            wasm.windows(export.len())
                .any(|window| window == export.as_bytes()),
            "missing Wasm export {export}"
        );
    }
    assert!(
        !wasm
            .windows("player_x".len())
            .any(|window| window == b"player_x"),
        "local release retained development-only Wasm symbols"
    );
    let runtime = fs::read_to_string(output.join("game.js")).expect("game.js");
    let index = fs::read_to_string(output.join("index.html")).expect("index.html");
    assert!(!index.contains("stasis-audio"));
    assert!(!index.contains("Enable sound"));
    for expected in [
        "requestAnimationFrame(frame)",
        "web_play_tone",
        "dataset.underBudget",
        "\"host_i32\"",
        "\"host_f32\"",
        "audio_push_f32_interleaved",
        "function writeHostFrame",
        "function applyWindowRequest",
        "const enableWebAudio = () =>",
        "addEventListener(\"pointerdown\", () => { void enableWebAudio(); }",
        "void enableWebAudio();",
        "function sdlScancode",
        "const spriteStride = version >= 5 ? 8 : 4;",
        "if (u0 === 0 && v0 === 0 && u1 === 1 && v1 === 1)",
        "context.drawImage(image, -width / 2, -height / 2, width, height);",
        "context.drawImage(image, sourceX, sourceY, sourceWidth, sourceHeight",
    ] {
        assert!(
            runtime.contains(expected),
            "missing web runtime data {expected}"
        );
    }
    assert!(!runtime.contains("STASIS_WASM_BASE64"));
    assert!(!runtime.contains("data:application/wasm;base64,"));
    assert!(output.join("index.html").is_file());
    let index = fs::read_to_string(output.join("index.html")).expect("index.html");
    assert!(!index.contains(r#"id="stasis-hud""#));
    assert!(!index.contains("__STASIS_"));
    assert!(!output.join("play").exists());
    assert!(output.join("stasis_provenance.json").is_file());
    assert!(!output.join("web_export_smoke.html").exists());

    fs::remove_dir_all(&output).expect("clean web package test output");
}

#[test]
fn minimal_pong_and_standard_reference_omit_audio_and_input() {
    let workspace = repo_root().join("samples/pong_web_minimal");
    let relative_output = PathBuf::from(format!("build/web-package-test-{}", stamp()));
    let output = package(&workspace, &relative_output);

    let wasm = fs::read(output.join("game.wasm")).expect("minimal Pong Wasm");
    let runtime = fs::read_to_string(output.join("game.js")).expect("minimal Pong runtime");
    let index = fs::read_to_string(output.join("index.html")).expect("minimal Pong index");
    for reachable in ["main", "tick", "render", "web_draw_rect"] {
        assert!(
            wasm.windows(reachable.len())
                .any(|window| window == reachable.as_bytes()),
            "missing reachable Pong dependency {reachable}"
        );
    }
    for omitted in [
        "unused_audio_feature",
        "unused_keyboard_feature",
        "web_play_tone",
        "web_input_axis",
    ] {
        assert!(
            !wasm
                .windows(omitted.len())
                .any(|window| window == omitted.as_bytes()),
            "Wasm retained unreachable dependency {omitted}"
        );
        assert!(!runtime.contains(omitted), "JS retained {omitted}");
    }
    assert!(!runtime.contains("AudioContext"));
    assert!(!runtime.contains("keydown"));
    assert!(!runtime.contains("pointerdown"));
    assert!(!index.contains("Enable sound"));
    assert!(
        runtime.len() < 10_000,
        "minimal runtime was {} bytes",
        runtime.len()
    );

    let standard = repo_root().join("samples/pong_web_standard");
    let standard_runtime =
        fs::read_to_string(standard.join("game.js")).expect("standard Pong runtime");
    let standard_index =
        fs::read_to_string(standard.join("index.html")).expect("standard Pong index");
    for omitted in ["AudioContext", "keydown", "pointerdown"] {
        assert!(
            !standard_runtime.contains(omitted),
            "standard JS retained {omitted}"
        );
        assert!(
            !standard_index.contains(omitted),
            "standard HTML retained {omitted}"
        );
    }
    for expected in [
        "requestAnimationFrame(frame)",
        "context.fillRect",
        "context.fillText",
        "SCREEN_WIDTH = 640",
        "SCREEN_HEIGHT = 360",
    ] {
        assert!(
            standard_runtime.contains(expected),
            "standard JS missing {expected}"
        );
    }

    fs::remove_dir_all(&output).expect("clean minimal Pong package");
}

#[test]
fn existing_windows_game_packages_command_buffers_sprites_and_font_for_web() {
    let workspace = repo_root().join("samples/windows_launch_smoke");
    let relative_output = PathBuf::from(format!("build/web-package-test-{}", stamp()));
    let output = package(&workspace, &relative_output);

    let wasm = fs::read(output.join("game.wasm")).expect("existing game Wasm");
    assert!(wasm.starts_with(b"\0asm\x01\0\0\0"));
    assert!(wasm.windows(6).any(|window| window == b"memory"));
    let runtime = fs::read_to_string(output.join("game.js")).expect("existing game runtime");
    for expected in ["gfx_cmd_i32", "gfx_cmd_f32", "stasis_jit_gfx_cache_text"] {
        assert!(
            runtime.contains(expected),
            "missing web runtime data {expected}"
        );
    }
    assert!(!runtime.contains("AudioContext"));
    assert!(!runtime.contains("audio_init"));
    let index = fs::read_to_string(output.join("index.html")).expect("existing game index");
    assert!(!index.contains("Enable sound"));
    assert!(runtime.contains(r#""assets":{}"#));
    assert!(!runtime.contains("../assets/"));
    for expected in [
        "data:image/png;base64,",
        "data:image/svg+xml;base64,",
        "data:font/ttf;base64,",
    ] {
        assert!(
            !runtime.contains(expected),
            "web runtime embedded {expected}"
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

    let runtime = fs::read_to_string(output.join("game.js")).expect("audio game runtime");
    for expected in [
        "stasis_jit_audio_load_music",
        "stasis_jit_audio_load_effect",
        "decodeAudioData",
        "document.addEventListener(\"visibilitychange\"",
        "addEventListener(\"pagehide\"",
        "addEventListener(\"pageshow\"",
        "if (event.persisted) suspendWebAudio()",
        "else shutdownWebAudio()",
        "pendingAudio.length = 0",
        "audioContext.suspend()",
        "resumingContext.resume()",
        "resumingContext.suspend()",
        "closingContext.close()",
    ] {
        assert!(
            runtime.contains(expected),
            "missing web audio data {expected}"
        );
    }
    for expected in ["data:audio/mpeg;base64,", "data:audio/wav;base64,"] {
        assert!(
            !runtime.contains(expected),
            "web runtime embedded {expected}"
        );
    }
    assert!(!runtime.contains("audioButton"));
    assert!(runtime.contains(r#""assets":{}"#));
    assert!(!runtime.contains("../assets/"));
    assert!(output.join("assets/tone.mp3").is_file());
    assert!(output.join("assets/tone.wav").is_file());

    fs::remove_dir_all(&workspace).expect("clean existing audio web fixture");
}
