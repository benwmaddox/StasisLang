use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

fn temp_dir() -> PathBuf {
    std::env::temp_dir().join(format!(
        "stasis_generics_package_{}_{}",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::SeqCst)
    ))
}

fn sample_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("samples/generics_collections")
}

fn copy_web_fixture(project: &Path) {
    fs::create_dir_all(project.join("src")).expect("create generic package source directory");
    for relative in ["stasis.json", "src/wasm_entry.stasis"] {
        let source = sample_root().join(relative);
        let destination = project.join(relative);
        fs::copy(source, destination).expect("copy generic package fixture");
    }
}

fn runtime_config(source: &str) -> Value {
    let json = source
        .strip_prefix("window.STASIS_GAME = ")
        .and_then(|source| source.split_once(";\n").map(|(json, _)| json))
        .expect("runtime metadata prefix");
    serde_json::from_str(json).expect("parse runtime metadata")
}

fn write_zero_view_fixture(project: &Path) {
    fs::create_dir_all(project.join("src")).expect("create zero-view package source directory");
    fs::write(
        project.join("stasis.json"),
        r#"{"manifest_version":1,"name":"generic_zero_view_handles","entry":"src/main.stasis","tests":"tests","output":"build","web":{"entry":"src/main.stasis"}}"#,
    )
    .expect("write zero-view package manifest");
    fs::write(
        project.join("src/main.stasis"),
        r#"
struct Buffer<T: type, N: i32> {
    values: T[N];
}

function capacity(buffer: Buffer<T, N>): i32 {
    return N;
}

function set_first(buffer: Buffer<T, N>, value: T): void {
    if (N > 0) {
        buffer.values[0] = value;
    }
}

global empty_a: Buffer<i32, 0>;
global empty_b: Buffer<i32, 0>;
global source: Buffer<i32, 2>;
global destination: Buffer<i32, 2>;

extern function sys_memcpy_i32(
    destination: i32[],
    destination_index: i32,
    source: i32[],
    source_index: i32,
    count: i32
): void;

function main(): i32 {
    set_first(source, 40);
    set_first(destination, 1);
    sys_memcpy_i32(destination.values, 0, source.values, 0, 1);
    return capacity(empty_a) * 100
        + capacity(empty_b) * 10
        + capacity(source)
        + destination.values[0];
}

function tick(): i32 {
    return 0;
}

function render(): void {
    return;
}
"#,
    )
    .expect("write zero-view package source");
}

fn execute_zero_view_handle_fixture(package: &Path) -> std::process::Output {
    Command::new("node")
        .arg("-e")
        .arg(
            r#"const fs = require('node:fs');
const root = process.argv[1];
const source = fs.readFileSync(`${root}/game.js`, 'utf8');
const marker = 'window.STASIS_GAME = ';
const start = marker.length;
const end = source.indexOf(';\n', start);
const game = JSON.parse(source.slice(start, end));
if (game.collectionViewAbiVersion !== 2) throw new Error('expected collectionViewAbiVersion 2');
const byHandle = new Map(Object.values(game.memory || {})
  .filter(layout => Number.isSafeInteger(layout?.handle))
  .map(layout => [layout.handle | 0, layout]));
let instance;
const resolve = reference => {
  const layout = byHandle.get(reference | 0);
  if (!layout) throw new Error(`unknown collection handle ${reference}`);
  return layout;
};
function memcpy_i32(destinationHandle, destinationIndex, sourceHandle, sourceIndex, count) {
  if (!Number.isInteger(count) || count <= 0) return;
  const destination = resolve(destinationHandle);
  const sourceLayout = resolve(sourceHandle);
  if (destination.length < destinationIndex + count || sourceLayout.length < sourceIndex + count) {
    throw new Error('collection handle bounds check failed');
  }
  const view = new DataView(instance.exports.memory.buffer);
  const values = [];
  for (let index = 0; index < count; index += 1) {
    values.push(view.getInt32(sourceLayout.offset + (sourceIndex + index) * sourceLayout.stride, true));
  }
  values.forEach((value, index) => {
    view.setInt32(destination.offset + (destinationIndex + index) * destination.stride, value, true);
  });
}
WebAssembly.instantiate(fs.readFileSync(`${root}/game.wasm`), { env: { sys_memcpy_i32: memcpy_i32 } })
  .then(({ instance: value }) => { instance = value; process.stdout.write(String(instance.exports.main())); })
  .catch(error => { console.error(error); process.exit(1); });"#,
        )
        .arg(package)
        .output()
        .expect("execute zero-view WebAssembly fixture")
}

#[test]
fn generic_collections_web_package_executes_declared_entry() {
    let project = temp_dir();
    copy_web_fixture(&project);

    let output = Command::new(env!("CARGO_BIN_EXE_stasis"))
        .args([
            "package",
            "--target",
            "web",
            "--development-build",
            "--out",
            "dist",
        ])
        .current_dir(&project)
        .output()
        .expect("run generic Web package");
    assert!(
        output.status.success(),
        "package failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let package = project.join("dist");
    assert!(package.join("game.js").is_file());
    let wasm = package.join("game.wasm");
    assert!(wasm.is_file());
    let provenance: Value = serde_json::from_slice(
        &fs::read(package.join("stasis_provenance.json")).expect("read generic Web provenance"),
    )
    .expect("parse generic Web provenance");
    assert_eq!(provenance["schema"], "stasis.release_provenance.v1");
    assert_eq!(provenance["development_build"], true);

    let node = Command::new("node")
        .args([
            "-e",
            "const fs=require('node:fs'); WebAssembly.instantiate(fs.readFileSync(process.argv[1]), {}).then(({instance}) => { if (instance.exports.main() !== 0) process.exit(1); }).catch((error) => { console.error(error); process.exit(1); });",
        ])
        .arg(&wasm)
        .output()
        .expect("run packaged generic Web Wasm");
    assert!(
        node.status.success(),
        "Node failed: stdout={} stderr={}",
        String::from_utf8_lossy(&node.stdout),
        String::from_utf8_lossy(&node.stderr)
    );

    fs::remove_dir_all(project).expect("remove generic Web package fixture");
}

#[test]
fn generic_collections_web_package_emits_versioned_handles_for_zero_views() {
    let project = temp_dir();
    write_zero_view_fixture(&project);

    let output = Command::new(env!("CARGO_BIN_EXE_stasis"))
        .args([
            "package",
            "--target",
            "web",
            "--development-build",
            "--out",
            "dist",
        ])
        .current_dir(&project)
        .output()
        .expect("run zero-view generic Web package");
    assert!(
        output.status.success(),
        "package failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let package = project.join("dist");
    let runtime = fs::read_to_string(package.join("game.js")).expect("read zero-view runtime");
    let game = runtime_config(&runtime);
    assert_eq!(game["collectionViewAbiVersion"], serde_json::json!(2));

    let memory = game["memory"]
        .as_object()
        .expect("generated memory layouts");
    let paths = [
        "empty_a.values",
        "empty_b.values",
        "source.values",
        "destination.values",
    ];
    let mut handles = std::collections::BTreeSet::new();
    for path in paths {
        let layout = memory
            .get(path)
            .unwrap_or_else(|| panic!("missing retained memory layout {path}"));
        assert!(
            layout["handle"].as_i64().is_some(),
            "layout {path} omitted its opaque collection handle"
        );
        assert!(
            layout["offset"].as_u64().is_some(),
            "layout {path} omitted its physical memory offset"
        );
        assert!(
            handles.insert(layout["handle"].as_i64().expect("collection handle")),
            "duplicate collection handle in layout {path}"
        );
    }
    assert_eq!(handles.len(), paths.len());

    let empty_a = &memory["empty_a.values"];
    let empty_b = &memory["empty_b.values"];
    let source = &memory["source.values"];
    assert_eq!(empty_a["length"], serde_json::json!(0));
    assert_eq!(empty_b["length"], serde_json::json!(0));
    assert_eq!(
        empty_a["offset"], empty_b["offset"],
        "zero-capacity collections should share no storage bytes"
    );
    assert_eq!(
        empty_a["offset"], source["offset"],
        "zero-capacity collection storage must not reserve physical bytes"
    );
    assert_ne!(empty_a["handle"], empty_b["handle"]);
    assert_ne!(empty_a["handle"], source["handle"]);
    assert_ne!(empty_b["handle"], source["handle"]);

    let execution = execute_zero_view_handle_fixture(&package);
    assert!(
        execution.status.success(),
        "handle-based Wasm execution failed: stdout={} stderr={}",
        String::from_utf8_lossy(&execution.stdout),
        String::from_utf8_lossy(&execution.stderr)
    );
    assert_eq!(
        String::from_utf8(execution.stdout).expect("UTF-8 result"),
        "42"
    );

    fs::remove_dir_all(project).expect("remove zero-view generic Web fixture");
}
