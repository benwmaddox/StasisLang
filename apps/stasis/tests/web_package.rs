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

#[test]
fn web_package_contains_runnable_wasm_static_bundle_and_standalone_file() {
    let workspace = repo_root().join("samples/web_export_smoke");
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let relative_output = PathBuf::from(format!("build/web-package-test-{stamp}"));
    let output = workspace.join(&relative_output);

    let result = Command::new(env!("CARGO_BIN_EXE_stasis"))
        .arg("package")
        .arg("--workspace")
        .arg(&workspace)
        .arg("--target")
        .arg("web")
        .arg("--development-build")
        .arg("--out")
        .arg(&relative_output)
        .arg("--json")
        .output()
        .expect("run web package");
    assert!(
        result.status.success(),
        "web package failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );

    let wasm = fs::read(output.join("game.wasm")).expect("game.wasm");
    assert!(wasm.starts_with(b"\0asm\x01\0\0\0"));
    for export in ["main", "tick", "render", "player_x"] {
        assert!(
            wasm.windows(export.len())
                .any(|window| window == export.as_bytes()),
            "missing Wasm export {export}"
        );
    }
    let runtime = fs::read_to_string(output.join("game.js")).expect("game.js");
    assert!(runtime.contains("requestAnimationFrame(frame)"));
    assert!(runtime.contains("web_play_tone"));
    assert!(runtime.contains("dataset.underBudget"));
    let standalone =
        fs::read_to_string(output.join("web_export_smoke.html")).expect("standalone web package");
    assert!(standalone.contains("window.STASIS_WASM_BASE64"));
    assert!(!standalone.contains("__STASIS_"));
    assert!(output.join("index.html").is_file());
    assert!(output.join("stasis_provenance.json").is_file());

    fs::remove_dir_all(&output).expect("clean web package test output");
}
