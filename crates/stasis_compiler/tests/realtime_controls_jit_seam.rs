#![cfg(windows)]

use object::{Object, ObjectSymbol};
use stasis_compiler::backend::aot::AotProcess;
use stasis_compiler::backend::jit::JitProcess;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

const FIXTURE_PATH: &str = "tests/stasis/seams/realtime_controls_jit_probe.stasis";
const FIXTURE: &str =
    include_str!("../../../tests/stasis/seams/realtime_controls_jit_probe.stasis");

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("canonical repository root")
}

#[test]
fn stasis_guest_drives_the_realtime_control_contract_through_jit() {
    let mut process = JitProcess::new();
    process
        .set_project_root(repository_root().to_string_lossy())
        .expect("set JIT project root");
    process.set_required_emit_roots(&["realtime_controls_jit_probe".to_string()]);
    process.upsert_file(FIXTURE_PATH, FIXTURE);
    process.compile().expect("compile realtime JIT fixture");

    let result = process
        .execute_i32_noarg_by_name("realtime_controls_jit_probe")
        .expect("execute realtime JIT fixture");
    assert_eq!(result, 0, "fixture failure code");
}

#[test]
fn realtime_guest_aot_objects_import_the_stable_native_contract() {
    let mut process = AotProcess::new();
    process
        .set_project_root(repository_root().to_string_lossy())
        .expect("set AOT project root");
    process.set_required_emit_roots(&["realtime_controls_jit_probe".to_string()]);
    process.upsert_file(FIXTURE_PATH, FIXTURE);
    process.compile().expect("compile realtime AOT fixture");

    let output_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| repository_root().join("target"))
        .join(format!("realtime-aot-symbols-{}", std::process::id()));
    let objects = process
        .write_object_files(&output_dir)
        .expect("write realtime AOT objects");
    let mut undefined = BTreeSet::new();
    for (_, path) in objects.values() {
        let bytes = fs::read(path).expect("read realtime AOT object");
        let object = object::File::parse(bytes.as_slice()).expect("parse realtime AOT object");
        undefined.extend(
            object
                .symbols()
                .filter(|symbol| symbol.is_undefined())
                .filter_map(|symbol| symbol.name().ok().map(str::to_string)),
        );
    }
    fs::remove_dir_all(&output_dir).expect("remove AOT symbol evidence");

    for symbol in [
        "stasis_realtime_start",
        "stasis_realtime_build_payload",
        "stasis_realtime_submit_payload",
        "stasis_realtime_current_epoch",
        "stasis_realtime_advance",
        "stasis_realtime_read_control",
        "stasis_realtime_disconnect",
        "stasis_realtime_reconnect",
        "stasis_realtime_stop",
    ] {
        assert!(
            undefined.contains(symbol),
            "missing native AOT import {symbol}"
        );
    }
}
