use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};
use stasis_assets::{AssetPackageIdentity, ASSET_PACKAGE_IDENTITY_PATH};
use stasis_network::StaticBundle;

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

fn package_with_mode(workspace: &Path, relative_output: &Path, development: bool) -> PathBuf {
    let output = workspace.join(relative_output);
    let mut command = Command::new(env!("CARGO_BIN_EXE_stasis"));
    command
        .arg("package")
        .arg("--workspace")
        .arg(workspace)
        .arg("--target")
        .arg("web")
        .arg("--out")
        .arg(relative_output);
    if development {
        command.arg("--development-build");
    }
    let result = command.arg("--json").output().expect("run web package");
    assert!(
        result.status.success(),
        "web package failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        String::from_utf8_lossy(&result.stdout).contains(&format!(
            "\"development_build\":{}",
            if development { "true" } else { "false" }
        )),
        "package receipt did not report the selected development mode: {}",
        String::from_utf8_lossy(&result.stdout)
    );
    let provenance: serde_json::Value = serde_json::from_slice(
        &fs::read(output.join("stasis_provenance.json")).expect("read web provenance"),
    )
    .expect("parse web provenance");
    let receipt: serde_json::Value =
        serde_json::from_slice(&result.stdout).expect("parse Web package command receipt");
    assert_eq!(
        provenance["build_class"],
        if development {
            "development"
        } else {
            "local_release"
        }
    );
    assert_eq!(provenance["development_build"], development);
    assert_eq!(
        receipt["result"]["web_size_metrics"],
        provenance["web_package"]["size_metrics"]
    );
    output
}

fn package(workspace: &Path, relative_output: &Path) -> PathBuf {
    package_with_mode(workspace, relative_output, false)
}

fn package_development(workspace: &Path, relative_output: &Path) -> PathBuf {
    package_with_mode(workspace, relative_output, true)
}

fn runtime_config(source: &str) -> serde_json::Value {
    let json = source
        .strip_prefix("window.STASIS_GAME = ")
        .and_then(|source| source.split_once(";\n").map(|(json, _)| json))
        .expect("runtime metadata prefix");
    serde_json::from_str(json).expect("parse runtime metadata")
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
    assert!(runtime.contains("sysMemcpyU8"));

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
    let first_runtime = fs::read(output.join("game.js")).expect("first deterministic game.js");
    let first_provenance: serde_json::Value = serde_json::from_slice(
        &fs::read(output.join("stasis_provenance.json")).expect("first provenance"),
    )
    .expect("parse first provenance");
    fs::write(output.join("stale.txt"), "old package").expect("write stale package file");
    let output = package(&workspace, &relative_output);
    assert!(
        !output.join("stale.txt").exists(),
        "replacement retained a stale package file"
    );
    assert_eq!(
        fs::read(output.join("game.js")).expect("second deterministic game.js"),
        first_runtime
    );
    let provenance: serde_json::Value = serde_json::from_slice(
        &fs::read(output.join("stasis_provenance.json")).expect("second provenance"),
    )
    .expect("parse second provenance");
    assert_eq!(
        provenance["web_package"]["size_metrics"],
        first_provenance["web_package"]["size_metrics"]
    );
    let metrics = &provenance["web_package"]["size_metrics"];
    assert_eq!(metrics["javascript_minified"], true);
    assert!(
        metrics["javascript"]["after"]["raw_bytes"].as_u64()
            < metrics["javascript"]["before"]["raw_bytes"].as_u64()
    );
    assert!(
        metrics["javascript"]["after"]["gzip_bytes"].as_u64()
            < metrics["javascript"]["before"]["gzip_bytes"].as_u64()
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
    let runtime = fs::read_to_string(output.join("game.js"))
        .expect("game.js")
        .replace("\r\n", "\n");
    let index = fs::read_to_string(output.join("index.html"))
        .expect("index.html")
        .replace("\r\n", "\n");
    assert!(index.contains(r#"<title>web_export_smoke</title>"#));
    assert!(index.contains(r#"<h1 id="stasis-loading-title">web_export_smoke</h1>"#));
    assert!(index.contains(r#"id="stasis-loading-status">Preparing…</div>"#));
    assert_eq!(index.matches("viewport-fit=cover").count(), 1);
    assert_eq!(index.matches("safe-area-inset-").count(), 8);
    assert_eq!(index.matches("100svh").count(), 1);
    assert_eq!(index.matches("100dvh").count(), 1);
    assert_eq!(index.matches("<script>\n    (() => {").count(), 1);
    assert_eq!(
        index
            .matches("      addEventListener(\"resize\", fit)")
            .count(),
        1
    );
    assert_eq!(
        index
            .matches("      addEventListener(\"orientationchange\", fit)")
            .count(),
        1
    );
    assert_eq!(index.matches("visualViewport.addEventListener").count(), 2);
    assert_eq!(index.matches("new MutationObserver(fit)").count(), 1);
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
        "dataset.composites",
        "dataset.renderSubmissions",
        "dataset.atlasPages",
        "dataset.atlasLiveEntries",
        "dataset.atlasAllocatedBytes",
        "dataset.atlasUploadCount",
        "dataset.atlasUploadBytes",
        "dataset.uploadedBytes",
        "PERF_ROLLING_CAPACITY",
        "performanceWorstTimes",
        "recordPerformanceWorst",
        "getGpuBatcher",
        "drawArraysInstanced",
        "drawSprites",
        "atlasByResource",
        "atlasPages",
        "atlasUploadBytes",
        "performanceBackend",
        "getContext",
        "WebGL2 is required by the Stasis Web renderer",
        "host_i32",
        "host_f32",
        "audio_push_f32_interleaved",
        "writeHostFrame",
        "applyWindowRequest",
        "enableWebAudio",
        "pointerdown",
        "sdlScancode",
        ".setClip(",
        "drawPreparedText",
        "deterministicMissingSprite",
    ] {
        assert!(
            runtime.contains(expected),
            "missing web runtime data {expected}"
        );
    }
    assert!(!runtime.contains("GFX_CMD_V2_VERSION"));
    assert!(!runtime.contains("GFX_CMD_V3_VERSION"));
    assert!(!runtime.contains("GFX_CMD_V4_VERSION"));
    assert!(!runtime.contains("GFX_CMD_V5_VERSION"));
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
    assert!(!output.join(ASSET_PACKAGE_IDENTITY_PATH).exists());
    assert!(!runtime.contains("\"schema\":\"stasis.asset_package\""));
    assert!(!output.join("web_export_smoke.html").exists());

    fs::remove_dir_all(&output).expect("clean web package test output");
}

#[test]
fn network_web_package_embeds_retained_nested_assets_only() {
    let root = repo_root();
    let source = root.join("samples/windows_launch_smoke");
    let workspace = root.join(format!("build/network-bundle-fixture-{}", stamp()));
    copy_tree(&source, &workspace);

    fs::create_dir_all(workspace.join("assets/fonts")).expect("create nested font directory");
    fs::copy(
        workspace.join("assets/smoke.ttf"),
        workspace.join("assets/fonts/ui.ttf"),
    )
    .expect("copy retained font fixture");
    let source_path = workspace.join("main.stasis");
    let source_text = fs::read_to_string(&source_path)
        .expect("read network fixture source")
        .replace("/assets/smoke.ttf", "/assets/fonts/ui.ttf");
    assert!(source_text.contains("load_font(\"/assets/fonts/ui.ttf\""));
    fs::write(&source_path, source_text).expect("rewrite retained font path");

    let unused_path = workspace.join("assets/fonts/unused.ttf");
    fs::copy(workspace.join("assets/smoke.ttf"), &unused_path).expect("copy unused font fixture");
    let unused = fs::read(&unused_path).expect("read unused font fixture");
    let manifest_path = workspace.join("assets/manifest.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest_path).expect("read fixture asset manifest"))
            .expect("parse fixture asset manifest");
    let assets = manifest["assets"].as_array_mut().expect("asset entries");
    assets
        .iter_mut()
        .find(|asset| asset["id"] == "smoke_font")
        .expect("font entry")["path"] = serde_json::Value::String("assets/fonts/ui.ttf".into());
    assets.push(serde_json::json!({
        "id": "unused_asset",
        "path": "assets/fonts/unused.ttf",
        "content_sha256": format!("{:x}", Sha256::digest(&unused)),
        "format": {"kind": "font", "encoding": "ttf"},
        "dependencies": []
    }));
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).expect("encode fixture asset manifest"),
    )
    .expect("write fixture asset manifest");

    let mut project: serde_json::Value = serde_json::from_slice(
        &fs::read(workspace.join("stasis.json")).expect("read fixture project manifest"),
    )
    .expect("parse fixture project manifest");
    project["capabilities"] = serde_json::json!({"network": true});
    project["web"] = serde_json::json!({"entry": "main.stasis"});
    fs::write(
        workspace.join("stasis.json"),
        serde_json::to_vec_pretty(&project).expect("encode fixture project manifest"),
    )
    .expect("write fixture project manifest");

    let relative_output = PathBuf::from(format!("build/network-bundle-output-{}", stamp()));
    let output = package(&workspace, &relative_output);
    let asset_identity: AssetPackageIdentity = serde_json::from_slice(
        &fs::read(output.join(ASSET_PACKAGE_IDENTITY_PATH)).expect("asset package identity"),
    )
    .expect("parse asset package identity");
    asset_identity.validate().expect("valid asset identity");
    assert_eq!(
        asset_identity.manifest_sha256,
        format!(
            "{:x}",
            Sha256::digest(fs::read(output.join("assets/manifest.json")).unwrap())
        )
    );
    let runtime = fs::read_to_string(output.join("game.js")).expect("packaged Web runtime");
    assert!(!runtime.contains("\"schema\":\"stasis.asset_package\""));
    assert!(!runtime.contains(&asset_identity.manifest_sha256));
    let bundle = StaticBundle::decode(
        &fs::read(output.join("network_guest.bundle")).expect("read network guest bundle"),
    )
    .expect("decode network guest bundle");
    for core in ["index.html", "game.js", "game.wasm"] {
        assert!(
            bundle.get(core).is_some(),
            "missing core bundle file {core}"
        );
    }
    assert_eq!(
        bundle.get("game.js").expect("bundled game.js").bytes,
        fs::read(output.join("game.js")).expect("final game.js")
    );
    let expected_font = fs::read(output.join("assets/fonts/ui.ttf")).expect("read staged font");
    let font = bundle
        .get("assets/fonts/ui.ttf")
        .expect("retained nested font");
    assert_eq!(font.mime, "font/ttf");
    assert_eq!(font.bytes, expected_font);
    assert!(bundle.get("assets/fonts/unused.ttf").is_none());

    fs::remove_dir_all(&workspace).expect("clean network fixture");
}

#[test]
fn pong_packages_the_single_webgl2_runtime_with_unused_features_stripped() {
    let workspace = repo_root().join("samples/pong_web_minimal");
    let relative_output = PathBuf::from(format!("build/web-package-test-{}", stamp()));
    let output = package(&workspace, &relative_output);

    let wasm = fs::read(output.join("game.wasm")).expect("minimal Pong Wasm");
    let runtime = fs::read_to_string(output.join("game.js"))
        .expect("Pong runtime")
        .replace("\r\n", "\n");
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
    assert!(runtime.contains("keydown"));
    assert!(runtime.contains("pointerdown"));
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
        "recordPerformanceWorst",
        "recordPerformanceWorst(",
        "getContext",
    ] {
        assert!(
            runtime.contains(expected),
            "single runtime missing {expected}"
        );
    }
    assert!(!runtime.contains("N/A"));
    assert!(!runtime.contains("game_minimal"));

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

    fs::remove_dir_all(&output).expect("clean Pong package");
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
        "sysMemcpyI32",
        "sysMemcpyF32",
        "pointerCount",
        "pointerleave",
        "pointerup",
        "pointercancel",
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
    let config = runtime_config(&runtime);
    assert_eq!(config["assets"], serde_json::json!({}));
    let smoke = config["asset_metadata"]["assets/smoke.png"]
        .as_object()
        .expect("release smoke metadata");
    assert_eq!(smoke["prepared_width"], 64);
    assert!(smoke.keys().all(|field| [
        "encoding",
        "prepared_width",
        "prepared_height",
        "logical_width",
        "logical_height"
    ]
    .contains(&field.as_str())));
    for removed in [
        "path",
        "prepared_bytes",
        "source_bytes",
        "source_sha256",
        "prepared_sha256",
    ] {
        assert!(!smoke.contains_key(removed), "release retained {removed}");
    }
    assert!(config.get("asset_package").is_none());
    let provenance: serde_json::Value = serde_json::from_slice(
        &fs::read(output.join("stasis_provenance.json")).expect("asset provenance"),
    )
    .expect("parse asset provenance");
    let audit = &provenance["web_package"]["asset_metadata_audit"]["assets/smoke.png"];
    assert_eq!(audit["path"], "assets/smoke.png");
    assert_eq!(audit["prepared_bytes"], 455);
    assert_eq!(audit["source_bytes"], 455);
    assert_eq!(
        audit["source_sha256"],
        "98d61197c8db539121336207a1cc722093a0d3e0acd5ef5196c1eda3e9b92d72"
    );
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
fn brickout_line_batch_packages_with_canonical_repo_stdlib_for_web() {
    let repository = repo_root();
    let workspace = repository
        .join("build")
        .join(format!("web-brickout-line-batch-test-{}", stamp()));
    let sample = repository.join("samples/brickout_revenge");
    copy_tree(&sample, &workspace.join("samples/brickout_revenge"));
    copy_tree(&repository.join("src"), &workspace.join("src"));
    copy_tree(&sample.join("assets"), &workspace.join("assets"));
    fs::write(
        workspace.join("main.stasis"),
        r#"import "samples/brickout_revenge/brickout_revenge.stasis";

function render(): i32 { return 0; }
"#,
    )
    .expect("write Brickout web entry adapter");
    fs::write(
        workspace.join("stasis.json"),
        r#"{"manifest_version":1,"name":"brickout_line_batch_web","entry":"main.stasis","tests":"tests","output":"build"}"#,
    )
    .expect("write Brickout web manifest");

    let asset_shapes = [
        ("paddle.svg", 128, 24),
        ("ball.svg", 32, 32),
        ("brick_basic.svg", 160, 64),
        ("brick_basic_turret.svg", 160, 64),
        ("brick_basic_fx.svg", 160, 64),
        ("brick_armored.svg", 160, 64),
        ("brick_armored_turret.svg", 160, 64),
        ("brick_armored_fx.svg", 160, 64),
        ("brick_reflector.svg", 160, 64),
        ("brick_reflector_fx.svg", 160, 64),
    ];
    let assets = asset_shapes
        .iter()
        .map(|(name, width, height)| {
            let bytes =
                fs::read(workspace.join("assets").join(name)).expect("read Brickout sprite asset");
            serde_json::json!({
                "id": name.trim_end_matches(".svg"),
                "path": format!("assets/{name}"),
                "content_sha256": format!("{:x}", Sha256::digest(bytes)),
                "format": {
                    "kind": "sprite",
                    "encoding": "svg",
                    "width": width,
                    "height": height
                },
                "dependencies": []
            })
        })
        .collect::<Vec<_>>();
    fs::write(
        workspace.join("assets/manifest.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema": "stasis-assets",
            "version": 1,
            "assets": assets
        }))
        .expect("serialize Brickout asset manifest"),
    )
    .expect("write Brickout asset manifest");

    let output = package(&workspace, Path::new("build/web-package"));
    assert!(output.join("game.wasm").is_file());
    assert!(output.join("game.js").is_file());
    assert!(output.join("index.html").is_file());

    fs::remove_dir_all(&workspace).expect("clean Brickout web fixture");
}

#[test]
fn development_web_package_remains_readable_and_retains_asset_diagnostics() {
    let workspace = repo_root().join("samples/windows_launch_smoke");
    let relative_output = PathBuf::from(format!("build/web-development-test-{}", stamp()));
    let output = package_development(&workspace, &relative_output);
    let runtime = fs::read_to_string(output.join("game.js")).expect("development game.js");
    assert!(runtime.contains("const canvas = document.getElementById"));
    assert!(runtime.contains("const rasterSprite = async request =>"));
    assert!(runtime.lines().count() > 1000);

    let config = runtime_config(&runtime);
    let smoke = config["asset_metadata"]["assets/smoke.png"]
        .as_object()
        .expect("development smoke metadata");
    for diagnostic in [
        "path",
        "prepared_bytes",
        "source_bytes",
        "source_sha256",
        "prepared_sha256",
    ] {
        assert!(
            smoke.contains_key(diagnostic),
            "development omitted {diagnostic}"
        );
    }
    assert_eq!(config["asset_package"]["schema"], "stasis.asset_package");

    let provenance: serde_json::Value = serde_json::from_slice(
        &fs::read(output.join("stasis_provenance.json")).expect("development provenance"),
    )
    .expect("parse development provenance");
    let metrics = &provenance["web_package"]["size_metrics"];
    assert_eq!(metrics["javascript_minified"], false);
    assert_eq!(
        metrics["javascript"]["before"],
        metrics["javascript"]["after"]
    );
    assert_eq!(
        metrics["asset_metadata"]["before"],
        metrics["asset_metadata"]["after"]
    );

    fs::remove_dir_all(&output).expect("clean development Web package");
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
            "import \".stasis_cache/toolchain/src/stdlib/graphics.stasis\";",
            &format!(
                "import \".stasis_cache/toolchain/src/stdlib/graphics.stasis\";\n\nconst ROOTED_SMOKE_PATH: string = \"{ROOTED_WEB_ASSET}\";"
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
        runtime.contains("startsWith")
            && runtime.contains("/assets/")
            && runtime.contains("slice(1)"),
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
        "stasis_jit_asset_request_audio",
        "stasis_jit_asset_task_poll",
        "stasis_jit_asset_task_take_handle",
        "audio_play:",
        "decodeAudioData",
        "visibilitychange",
        "pagehide",
        "pageshow",
        "suspendWebAudio",
        "shutdownWebAudio",
        "pendingAudio",
        "pendingAudioFrames",
        "queuedAudioFrames",
        "allocateAudioVoiceHandle",
        "setAudioVoiceVolumePan",
        "startAudio",
        "createStereoPanner",
        ".suspend()",
        ".resume()",
        ".close()",
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
