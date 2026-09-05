#[allow(dead_code)]
#[path = "../build.rs"]
mod build_script;

use std::fs;
use std::path::{Path, PathBuf};

fn write_fixture(path: &Path) {
    fs::create_dir_all(path.parent().expect("fixture parent"))
        .expect("create runtime fixture parent");
    fs::write(path, b"runtime").expect("write runtime fixture");
}

#[test]
fn fresh_runtime_build_precedes_legacy_copies_but_explicit_override_wins() {
    let root = std::env::temp_dir().join(format!(
        "stasis_runtime_library_candidates_{}_{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let _ = fs::remove_dir_all(&root);

    let fresh = root
        .join("runtime")
        .join("build")
        .join("bin")
        .join("Release")
        .join("stasis_graphics.dll");
    let stale_root = root.join("stasis_graphics.dll");
    let stale_build = root.join("build").join("stasis_graphics.dll");
    write_fixture(&fresh);
    write_fixture(&stale_root);
    write_fixture(&stale_build);

    let candidates = build_script::runtime_library_candidate_paths_for(
        &root,
        std::iter::empty::<PathBuf>(),
        &["stasis_graphics.dll"],
    );
    let selected = candidates
        .iter()
        .find(|candidate| candidate.is_file())
        .expect("select a runtime candidate");
    assert_eq!(selected, &fresh);

    let explicit = root.join("explicit").join("stasis_graphics.dll");
    write_fixture(&explicit);
    let candidates = build_script::runtime_library_candidate_paths_for(
        &root,
        [explicit.clone()],
        &["stasis_graphics.dll"],
    );
    let selected = candidates
        .iter()
        .find(|candidate| candidate.is_file())
        .expect("select an explicitly configured runtime candidate");
    assert_eq!(selected, &explicit);

    fs::remove_dir_all(root).expect("remove runtime candidate fixture");
}
