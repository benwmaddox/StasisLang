use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

const CONTINUE_LOOP_PARITY: &str =
    include_str!("../../../tests/stasis/seams/continue_loop_parity.stasis");
const ROOTED_WEB_ASSET: &str = "/assets/smoke.svg";

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

fn execute_web_main_with_measure_text(wasm: &Path) -> Output {
    Command::new("node")
        .arg("-e")
        .arg(
            "const fs = require('node:fs'); const bytes = fs.readFileSync(process.argv[1]); const imports = WebAssembly.Module.imports(new WebAssembly.Module(bytes)); if (!imports.some(({ module, name }) => module === 'env' && name === 'measure_text')) throw new Error('package did not retain env.measure_text'); WebAssembly.instantiate(bytes, { env: { measure_text: () => 12.5 } }).then(({ instance }) => process.stdout.write(String(instance.exports.main()))).catch((error) => { console.error(error); process.exit(1); });",
        )
        .arg(wasm)
        .output()
        .expect("execute packaged Wasm with measure_text import")
}

fn execute_web_main_with_byte_memcpy(package: &Path) -> Output {
    Command::new("node")
        .arg("-e")
        .arg(
            r#"const fs = require('node:fs');
const root = process.argv[1];
const source = fs.readFileSync(`${root}/game.js`, 'utf8');
const game = JSON.parse(source.slice('window.STASIS_GAME = '.length, source.indexOf(';', 'window.STASIS_GAME = '.length)));
const layoutsByHash = new Map(Object.values(game.memory || {})
  .filter(layout => layout?.byte_backed === true && Number.isSafeInteger(layout.hash))
  .map(layout => [layout.hash | 0, layout]));
const layoutsByOffset = new Map(Object.values(game.memory || {})
  .filter(layout => layout?.byte_backed === true && Number.isSafeInteger(layout.offset))
  .map(layout => [layout.offset | 0, layout]));
const layoutFor = reference => layoutsByHash.get(reference | 0) || layoutsByOffset.get(reference | 0);
let instance;
function copy_u8(destinationHash, destinationIndex, sourceHash, sourceIndex, count) {
  const destination = layoutFor(destinationHash);
  const sourceLayout = layoutFor(sourceHash);
  if (!destination || !sourceLayout || !instance?.exports?.memory || count <= 0) return;
  const bytes = new Uint8Array(instance.exports.memory.buffer);
  const values = [];
  for (let offset = 0; offset < count; offset += 1) {
    values.push(bytes[sourceLayout.offset + (sourceIndex + offset) * sourceLayout.stride]);
  }
  values.forEach((value, offset) => {
    bytes[destination.offset + (destinationIndex + offset) * destination.stride] = value;
  });
}
const bytes = fs.readFileSync(`${root}/game.wasm`);
WebAssembly.instantiate(bytes, { env: { sys_memcpy_u8: copy_u8 } })
  .then(({ instance: value }) => { instance = value; process.stdout.write(String(instance.exports.main())); })
  .catch(error => { console.error(error); process.exit(1); });"#,
        )
        .arg(package)
        .output()
        .expect("execute packaged Wasm with byte memcpy import")
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
fn web_nested_text_views_update_memory_backed_metadata() {
    let root = repo_root();
    let workspace = root
        .join("build")
        .join(format!("web-nested-text-metadata-test-{}", stamp()));
    fs::create_dir_all(workspace.join("src")).expect("create nested text fixture");
    fs::write(
        workspace.join("stasis.json"),
        r#"{"manifest_version":1,"name":"web_nested_text_metadata","entry":"src/main.stasis","tests":"tests","output":"build"}"#,
    )
    .expect("write nested text manifest");
    fs::write(
        workspace.join("src/main.stasis"),
        r#"
struct GameState {
    status_text: ascii[160];
    note_text: utf8[160];
}

global game: GameState;

function update_ascii(value: ascii[]): i32 {
    value.length = 3;
    return value.length;
}

function update_utf8(value: utf8[]): i32 {
    value.length = 4;
    value.char_length = 2;
    return value.length * 10 + value.char_length;
}

function literal_ascii_metadata(value: ascii[]): i32 {
    return value.length * 10 + value.max_length;
}

function literal_utf8_metadata(value: utf8[]): i32 {
    return value.length * 100 + value.max_length * 10 + value.char_length;
}

function literal_ascii_byte(value: ascii[], index: i32): u8 {
    return value[index];
}

function literal_utf8_byte(value: utf8[], index: i32): u8 {
    return value[index];
}

function literal_bytes_ok(): i32 {
    if (literal_ascii_byte("go", 0) != 103) {
        return 0;
    }
    if (literal_ascii_byte("go", 1) != 111) {
        return 0;
    }
    if (literal_utf8_byte("é", 0) != 195) {
        return 0;
    }
    if (literal_utf8_byte("é", 1) != 169) {
        return 0;
    }
    if (literal_utf8_byte("é", 2) != 0) {
        return 0;
    }
    if (literal_utf8_byte("é", -1) != 0) {
        return 0;
    }
    return 1;
}

function write_ascii_byte(value: ascii[], index: i32, byte: u8): u8 {
    value[index] = byte;
    return value[index];
}

function write_utf8_byte(value: utf8[], index: i32, byte: u8): u8 {
    value[index] = byte;
    return value[index];
}

function fixed_text_views_ok(): i32 {
    if (write_ascii_byte(game.status_text, 0, 65) != 65) {
        return 0;
    }
    if (write_utf8_byte(game.note_text, 0, 195) != 195) {
        return 0;
    }
    if (write_utf8_byte(game.note_text, 1, 169) != 169) {
        return 0;
    }
    return 1;
}

function main(): i32 {
    let metadata: i32 = update_ascii(game.status_text) * 100000
        + update_utf8(game.note_text) * 1000
        + literal_ascii_metadata("go") * 10
        + literal_utf8_metadata("é");
    return (metadata * 10 + literal_bytes_ok()) * 10 + fixed_text_views_ok();
}

function tick(): i32 {
    return 0;
}

function render(): i32 {
    return 0;
}
"#,
    )
    .expect("write nested text source");

    let output = package(&workspace, Path::new("build/web-package"));
    let execution = execute_web_main(&output.join("game.wasm"));
    assert!(
        execution.status.success(),
        "nested text Wasm execution failed: stdout={} stderr={}",
        String::from_utf8_lossy(&execution.stdout),
        String::from_utf8_lossy(&execution.stderr)
    );
    assert_eq!(
        String::from_utf8(execution.stdout).expect("UTF-8 result"),
        "34244111"
    );

    fs::remove_dir_all(&workspace).expect("clean nested text fixture");
}

#[test]
fn web_package_executes_scalar_struct_to_collection_copy() {
    let root = repo_root();
    let workspace = root
        .join("build")
        .join(format!("web-struct-copy-test-{}", stamp()));
    fs::create_dir_all(workspace.join("src")).expect("create struct copy fixture");
    fs::write(
        workspace.join("stasis.json"),
        r#"{"manifest_version":1,"name":"web_struct_copy","entry":"src/main.stasis","tests":"tests","output":"build"}"#,
    )
    .expect("write struct copy manifest");
    fs::write(
        workspace.join("src/main.stasis"),
        r#"
struct CopyState {
    count: i32;
    enabled: bool;
}

global source: CopyState;
global destination: CopyState[2];

function main(): i32 {
    source.count = 40;
    source.enabled = true;
    destination[0] = source;
    destination[1] = destination[0];
    if (destination[1].enabled) {
        return destination[1].count + 2;
    }
    return -1;
}

function tick(): i32 {
    return 0;
}

function render(): i32 {
    return 0;
}
"#,
    )
    .expect("write struct copy source");

    let output = package(&workspace, Path::new("build/web-package"));
    let execution = execute_web_main(&output.join("game.wasm"));
    assert!(
        execution.status.success(),
        "scalar struct copy Wasm execution failed: stdout={} stderr={}",
        String::from_utf8_lossy(&execution.stdout),
        String::from_utf8_lossy(&execution.stderr)
    );
    assert_eq!(
        String::from_utf8(execution.stdout).expect("UTF-8 result"),
        "42"
    );

    fs::remove_dir_all(&workspace).expect("clean struct copy fixture");
}

#[test]
fn web_package_instantiates_direct_measure_text_import() {
    let root = repo_root();
    let workspace = root
        .join("build")
        .join(format!("web-measure-text-test-{}", stamp()));
    fs::create_dir_all(workspace.join("src")).expect("create measure_text fixture");
    fs::write(
        workspace.join("stasis.json"),
        r#"{"manifest_version":1,"name":"web_measure_text","entry":"src/main.stasis","tests":"tests","output":"build"}"#,
    )
    .expect("write measure_text manifest");
    fs::write(
        workspace.join("src/main.stasis"),
        r#"
extern function measure_text(font: i32, text: string): f32;

function main(): i32 {
    let width: f32 = measure_text(2, "abc");
    if (width == 12.5) {
        return 0;
    }
    return 1;
}

function tick(): i32 {
    return 0;
}

function render(): i32 {
    return 0;
}
"#,
    )
    .expect("write measure_text source");

    let output = package(&workspace, Path::new("build/web-package"));
    let runtime = fs::read_to_string(output.join("game.js")).expect("measure_text web runtime");
    assert!(
        runtime.contains("measure_text:"),
        "web host omitted the direct measure_text import"
    );
    let execution = execute_web_main_with_measure_text(&output.join("game.wasm"));
    assert!(
        execution.status.success(),
        "measure_text Wasm instantiation failed: stdout={} stderr={}",
        String::from_utf8_lossy(&execution.stdout),
        String::from_utf8_lossy(&execution.stderr)
    );
    assert_eq!(
        String::from_utf8(execution.stdout).expect("UTF-8 result"),
        "0"
    );

    fs::remove_dir_all(&workspace).expect("clean measure_text fixture");
}

#[test]
fn web_package_retains_byte_backed_text_layouts_for_memcpy_import() {
    let root = repo_root();
    let workspace = root
        .join("build")
        .join(format!("web-byte-memcpy-test-{}", stamp()));
    fs::create_dir_all(workspace.join("src")).expect("create byte memcpy fixture");
    fs::write(
        workspace.join("stasis.json"),
        r#"{"manifest_version":1,"name":"web_byte_memcpy","entry":"src/main.stasis","tests":"tests","output":"build"}"#,
    )
    .expect("write byte memcpy manifest");
    fs::write(
        workspace.join("src/main.stasis"),
        r#"
extern function sys_memcpy_u8(dst: u8[], dst_index: i32, src: u8[], src_index: i32, count: i32): void;

global source: utf8[4];
global destination: ascii[4];

function seed_source(value: utf8[]): void {
    value[0] = 65;
    value[1] = 66;
}

function read_destination(value: ascii[]): i32 {
    return value[0] * 100 + value[1];
}

function main(): i32 {
    seed_source(source);
    sys_memcpy_u8(destination, 0, source, 0, 2);
    return read_destination(destination);
}

function tick(): i32 {
    return 0;
}

function render(): i32 {
    return 0;
}
"#,
    )
    .expect("write byte memcpy source");

    let output = package(&workspace, Path::new("build/web-package"));
    let runtime = fs::read_to_string(output.join("game.js")).expect("byte memcpy runtime");
    let game = runtime
        .strip_prefix("window.STASIS_GAME = ")
        .and_then(|source| source.split_once(';').map(|(json, _)| json))
        .map(|json| {
            serde_json::from_str::<serde_json::Value>(json).expect("parse runtime metadata")
        })
        .expect("runtime metadata prefix");
    for path in ["source", "destination"] {
        assert_eq!(
            game["memory"][path]["byte_backed"],
            serde_json::json!(true),
            "release metadata omitted byte-backed marker for {path}"
        );
    }
    assert!(runtime.contains("sys_memcpy_u8: sysMemcpyU8"));

    let execution = execute_web_main_with_byte_memcpy(&output);
    assert!(
        execution.status.success(),
        "byte memcpy Wasm execution failed: stdout={} stderr={}",
        String::from_utf8_lossy(&execution.stdout),
        String::from_utf8_lossy(&execution.stderr)
    );
    assert_eq!(
        String::from_utf8(execution.stdout).expect("UTF-8 result"),
        "6566"
    );

    fs::remove_dir_all(&workspace).expect("clean byte memcpy fixture");
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
    assert!(index.contains(r#"<title>web_export_smoke</title>"#));
    assert!(index.contains(r#"<h1 id="stasis-loading-title">web_export_smoke</h1>"#));
    assert!(index.contains(r#"id="stasis-loading-status">Preparing…</div>"#));
    assert!(!index.contains("stasis-audio"));
    assert!(!index.contains("Enable sound"));
    for expected in [
        "requestAnimationFrame(frame)",
        "web_play_tone",
        "dataset.underBudget",
        "dataset.wasmRenderMs",
        "dataset.browserReplayMs",
        "dataset.frameWorkMs",
        "dataset.worstFrameWorkMs",
        "dataset.backend",
        "dataset.hostReplayMs",
        "dataset.renderPrepMs",
        "dataset.gpuSubmitMs",
        "dataset.presentWaitMs",
        "dataset.instances",
        "dataset.uploadedBytes",
        "PERF_ROLLING_CAPACITY",
        "performanceWorstTimes",
        "if (hud) {\n      recordPerformanceWorst(",
        "RECT_BATCH_MIN",
        "drawArraysInstanced",
        "Canvas2D + WebGL2",
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
    assert!(!runtime.contains("render prep N/A"));
    assert!(!runtime.contains("GPU submit N/A"));
    assert!(!runtime.contains("present wait N/A"));
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
    for expected in [
        "dataset.wasmRenderMs",
        "dataset.browserReplayMs",
        "dataset.frameWorkMs",
        "dataset.worstFrameWorkMs",
        "dataset.backend",
        "dataset.hostReplayMs",
        "dataset.renderPrepMs",
        "dataset.gpuSubmitMs",
        "dataset.presentWaitMs",
        "PERF_ROLLING_CAPACITY",
        "if (hud)",
        "recordWorst(",
    ] {
        assert!(
            runtime.contains(expected),
            "minimal runtime missing {expected}"
        );
    }
    assert!(!runtime.contains("N/A"));
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
    for expected in [
        "gfx_cmd_i32",
        "gfx_cmd_f32",
        "stasis_jit_gfx_cache_text",
        "sys_memcpy_i32: sysMemcpyI32",
        "sys_memcpy_f32: sysMemcpyF32",
        "const pointerCount = pointer.hover || pointer.down || pointer.wentDown || pointer.wentUp ? 1 : 0;",
        "const inside = event.clientX >= bounds.left && event.clientX <= bounds.right",
        "&& event.clientY >= bounds.top && event.clientY <= bounds.bottom;",
        "pointer.hover = event.pointerType !== \"touch\" && inside;",
        "canvas.addEventListener(\"pointerleave\", () => { pointer.hover = false; });",
        "canvas.addEventListener(\"pointerup\", event => {\n    updatePointer(event);\n    pointer.down = false;\n    pointer.wentUp = true;\n  });",
        "canvas.addEventListener(\"pointercancel\", () => { pointer.hover = false; pointer.down = false; pointer.wentUp = true; });",
    ] {
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
fn configured_web_loading_font_is_staged_and_missing_font_fails_check() {
    let source = repo_root().join("samples/windows_launch_smoke");
    let workspace = repo_root()
        .join("build")
        .join(format!("web-loading-font-test-{}", stamp()));
    copy_tree(&source, &workspace);
    let manifest_path = workspace.join("stasis.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest_path).expect("read fixture manifest"))
            .expect("parse fixture manifest");
    manifest["web"] = serde_json::json!({"loading_font": "/assets/smoke.ttf"});
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).expect("serialize fixture manifest"),
    )
    .expect("write fixture manifest");

    let output = package(&workspace, Path::new("build/web-package"));
    let index = fs::read_to_string(output.join("index.html")).expect("read configured index");
    assert!(index.contains(
        r#"<link rel="preload" href="assets/smoke.ttf" as="font" type="font/ttf" crossorigin>"#
    ));
    assert!(index.contains(
        r#"@font-face { font-family: "StasisLoadingFont"; src: url("assets/smoke.ttf") format("truetype");"#
    ));
    assert!(output.join("assets/smoke.ttf").is_file());
    fs::remove_dir_all(&output).expect("clean configured package");

    manifest["web"]["loading_font"] = serde_json::json!("assets/smoke.ttf");
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).expect("serialize relative manifest"),
    )
    .expect("write relative manifest");
    let relative_output = package(&workspace, Path::new("build/web-package-relative"));
    let relative_index =
        fs::read_to_string(relative_output.join("index.html")).expect("read relative index");
    assert!(relative_index.contains(
        r#"<link rel="preload" href="assets/smoke.ttf" as="font" type="font/ttf" crossorigin>"#
    ));
    assert!(relative_output.join("assets/smoke.ttf").is_file());
    fs::remove_dir_all(&relative_output).expect("clean relative package");

    manifest["web"]["loading_font"] = serde_json::json!("/assets/missing.ttf");
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).expect("serialize missing manifest"),
    )
    .expect("write missing manifest");
    let check = Command::new(env!("CARGO_BIN_EXE_stasis"))
        .arg("check")
        .arg("--workspace")
        .arg(&workspace)
        .output()
        .expect("run missing loading font check");
    assert!(
        !check.status.success(),
        "missing loading font unexpectedly passed"
    );
    let diagnostics = format!(
        "{}{}",
        String::from_utf8_lossy(&check.stdout),
        String::from_utf8_lossy(&check.stderr)
    );
    assert!(
        diagnostics.contains("web.loading_font must name an existing file"),
        "missing font diagnostic was not clear: {diagnostics}"
    );
    fs::remove_dir_all(&workspace).expect("clean loading font fixture");
}

#[test]
fn rooted_web_asset_paths_emit_package_relative_assets() {
    let root = repo_root();
    let source_workspace = root.join("samples/windows_launch_smoke");
    let workspace = root
        .join("build")
        .join(format!("web-rooted-assets-test-{}", stamp()));
    fs::create_dir_all(workspace.join("assets")).expect("create rooted web fixture");
    for file in ["stasis.json", "main.stasis"] {
        fs::copy(source_workspace.join(file), workspace.join(file)).expect("copy web fixture");
    }
    for file in ["manifest.json", "smoke.png", "smoke.svg", "smoke.ttf"] {
        fs::copy(
            source_workspace.join("assets").join(file),
            workspace.join("assets").join(file),
        )
        .expect("copy web fixture asset");
    }
    let source = fs::read_to_string(workspace.join("main.stasis"))
        .expect("read rooted web fixture source")
        .replace(
            "extern function gfx_load_sprite",
            &format!(
                "const ROOTED_SMOKE_PATH: string = \"{ROOTED_WEB_ASSET}\";\n\nextern function gfx_load_sprite"
            ),
        )
        .replace("\"assets/smoke.svg\"", "ROOTED_SMOKE_PATH");
    fs::write(workspace.join("main.stasis"), source).expect("write rooted web fixture source");

    let output = package(&workspace, Path::new("build/web-package"));
    assert!(
        output.join("assets/smoke.svg").is_file(),
        "rooted asset was not emitted at its package-relative key"
    );
    let runtime = fs::read_to_string(output.join("game.js")).expect("rooted web runtime");
    assert!(
        runtime.contains(&format!("\"{ROOTED_WEB_ASSET}\"")),
        "rooted asset literal was not retained in runtime metadata"
    );
    assert!(
        runtime.contains("value.startsWith(\"/assets/\")")
            && runtime.contains("return value.slice(1)"),
        "web runtime is missing the rooted-to-package-relative asset mapping"
    );

    fs::remove_dir_all(&workspace).expect("clean rooted web fixture");
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
        "pendingAudioFrames = 0",
        "const queuedAudioFrames = () => scheduledAudioFrames() + pendingAudioFrames",
        "PENDING_AUDIO_SECONDS = 0.1",
        "PENDING_AUDIO_ENTRY_LIMIT = 32",
        "MAX_AUDIO_VOICES = 32",
        "const allocateAudioVoiceHandle",
        "const setAudioVoiceVolumePan",
        "audio_play: (handle, loop, volume, pan) => startAudio(handle, loop, volume, pan)",
        "createStereoPanner",
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
