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
