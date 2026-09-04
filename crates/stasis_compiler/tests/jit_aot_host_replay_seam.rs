#![cfg(windows)]

use serde::Serialize;
use serde_json::json;
use stasis_compiler::backend::aot::AotProcess;
use stasis_compiler::backend::jit::JitProcess;
use stasis_jit::{AotLinkConfig, AotTarget};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const FIXTURE_PATH: &str = "tests/stasis/seams/jit_aot_host_replay_probe.stasis";
const FIXTURE: &str = include_str!("../../../tests/stasis/seams/jit_aot_host_replay_probe.stasis");
const FIELDS: [&str; 6] = [
    "entry_results",
    "state_checksum",
    "lifecycle",
    "command_counts",
    "trace",
    "pointer_marks",
];
const EXPECTED_POINTER_MARKS: [i32; 3] = [11, 14, 37];

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct TickRecord {
    tick: usize,
    entry_results: i32,
    state_checksum: i32,
    lifecycle: i32,
    command_counts: i32,
    trace: i32,
    pointer_marks: i32,
}

impl TickRecord {
    fn values(&self) -> [i32; 6] {
        [
            self.entry_results,
            self.state_checksum,
            self.lifecycle,
            self.command_counts,
            self.trace,
            self.pointer_marks,
        ]
    }
}

struct TestTree(PathBuf);

impl Drop for TestTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("canonical repository root")
}

fn evidence_root() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| repository_root().join("target"))
        .join("seam-tests")
}

fn roots() -> Vec<String> {
    (1..=3)
        .flat_map(|tick| {
            [
                "entry",
                "state",
                "lifecycle",
                "counts",
                "trace",
                "pointer_marks",
            ]
            .map(move |field| format!("it011_t{tick}_{field}"))
        })
        .collect()
}

fn configured_jit(source: &str) -> JitProcess {
    let mut process = JitProcess::new();
    process
        .set_project_root(repository_root().to_string_lossy())
        .expect("set JIT project root");
    process.set_required_emit_roots(&roots());
    process.upsert_file(FIXTURE_PATH, source);
    process.compile().expect("compile JIT replay fixture");
    process
}

fn configured_aot() -> AotProcess {
    let mut process = AotProcess::new();
    process
        .set_project_root(repository_root().to_string_lossy())
        .expect("set AOT project root");
    process.set_required_emit_roots(&roots());
    process.upsert_file(FIXTURE_PATH, FIXTURE);
    process.compile().expect("compile AOT replay fixture");
    process
}

fn read_jit_records(process: &JitProcess) -> Vec<TickRecord> {
    (1..=3)
        .map(|tick| {
            let run = |field: &str| {
                process
                    .execute_i32_noarg_by_name(&format!("it011_t{tick}_{field}"))
                    .unwrap_or_else(|error| panic!("execute JIT tick {tick} {field}: {error}"))
            };
            TickRecord {
                tick,
                entry_results: run("entry"),
                state_checksum: run("state"),
                lifecycle: run("lifecycle"),
                command_counts: run("counts"),
                trace: run("trace"),
                pointer_marks: run("pointer_marks"),
            }
        })
        .collect()
}

fn ensure_dynload_artifacts() -> (PathBuf, PathBuf) {
    let deps_dir = std::env::current_exe()
        .expect("current test executable")
        .parent()
        .expect("Cargo deps directory")
        .to_path_buf();
    let find = || {
        [
            &deps_dir,
            deps_dir.parent().expect("Cargo profile directory"),
        ]
        .into_iter()
        .find_map(|directory| {
            let import_library = directory.join("stasis_dynload.dll.lib");
            let runtime = directory.join("stasis_dynload.dll");
            (import_library.is_file() && runtime.is_file()).then_some((import_library, runtime))
        })
    };
    if let Some(paths) = find() {
        return paths;
    }
    let profile = deps_dir.parent().expect("Cargo profile directory");
    let target = profile.parent().expect("Cargo target directory");
    let output = Command::new("cargo")
        .args(["build", "-p", "stasis_dynload"])
        .current_dir(repository_root())
        .env("CARGO_TARGET_DIR", target)
        .output()
        .expect("build stasis_dynload runtime");
    assert!(
        output.status.success(),
        "stasis_dynload build failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    find().expect("stasis_dynload build must produce its DLL and import library")
}

fn linker_path() -> PathBuf {
    if let Some(path) = std::env::var_os("STASIS_AOT_LINKER").map(PathBuf::from) {
        assert!(path.is_file(), "STASIS_AOT_LINKER must name a linker file");
        return path;
    }
    let output = Command::new("where.exe")
        .arg("link.exe")
        .output()
        .expect("locate MSVC linker");
    assert!(
        output.status.success(),
        "MSVC link.exe is required for IT-011"
    );
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(PathBuf::from)
        .expect("link.exe path")
}

fn read_aot_records(process: &AotProcess, tree: &Path) -> Vec<TickRecord> {
    let (import_library, runtime) = ensure_dynload_artifacts();
    fs::copy(&runtime, tree.join("stasis_dynload.dll")).expect("copy linked-AOT runtime");
    let config = AotLinkConfig {
        linker_path: Some(linker_path()),
        runtime_lib_paths: vec![import_library],
        target: AotTarget::Native,
    };
    let run = |tick: usize, field: &str| {
        let root = format!("it011_t{tick}_{field}");
        let executable = tree.join(format!("{root}.exe"));
        process
            .link_executable_for_i32_noarg_function(&root, &executable, &config)
            .unwrap_or_else(|error| panic!("link AOT tick {tick} {field}: {error}"));
        Command::new(repository_root().join(".cargo/stasis-sign-and-run.cmd"))
            .arg(executable.file_name().expect("linked AOT executable name"))
            .current_dir(tree)
            .status()
            .unwrap_or_else(|error| panic!("run {}: {error}", executable.display()))
            .code()
            .expect("linked AOT exit code")
    };
    (1..=3)
        .map(|tick| TickRecord {
            tick,
            entry_results: run(tick, "entry"),
            state_checksum: run(tick, "state"),
            lifecycle: run(tick, "lifecycle"),
            command_counts: run(tick, "counts"),
            trace: run(tick, "trace"),
            pointer_marks: run(tick, "pointer_marks"),
        })
        .collect()
}

fn compare_records(expected: &[TickRecord], actual: &[TickRecord]) -> Result<(), String> {
    for (expected_tick, actual_tick) in expected.iter().zip(actual) {
        for (index, (expected_value, actual_value)) in expected_tick
            .values()
            .into_iter()
            .zip(actual_tick.values())
            .enumerate()
        {
            if expected_value != actual_value {
                return Err(format!(
                    "JIT/AOT replay divergence: tick={} field={} jit={} aot={}",
                    expected_tick.tick, FIELDS[index], expected_value, actual_value
                ));
            }
        }
    }
    Ok(())
}

#[test]
fn identical_host_snapshots_match_jit_and_linked_aot_each_tick() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let tree = TestTree(
        std::env::temp_dir().join(format!("stasis-it-011-{}-{stamp}", std::process::id())),
    );
    fs::create_dir_all(&tree.0).expect("create linked-AOT directory");

    let jit = read_jit_records(&configured_jit(FIXTURE));
    let aot = read_aot_records(&configured_aot(), &tree.0);
    compare_records(&jit, &aot).expect("canonical per-tick JIT/AOT replay parity");
    let jit_pointer_marks: Vec<i32> = jit.iter().map(|record| record.pointer_marks).collect();
    let aot_pointer_marks: Vec<i32> = aot.iter().map(|record| record.pointer_marks).collect();
    assert_eq!(
        jit_pointer_marks, EXPECTED_POINTER_MARKS,
        "JIT must execute pointer down, move, and up branches"
    );
    assert_eq!(
        aot_pointer_marks, EXPECTED_POINTER_MARKS,
        "linked AOT must execute pointer down, move, and up branches"
    );

    let mutated = FIXTURE.replacen("score -= 2;", "score -= 3;", 1);
    assert_ne!(mutated, FIXTURE, "diagnostic mutation must apply");
    let mutation = read_jit_records(&configured_jit(&mutated));
    let diagnostic = compare_records(&mutation, &aot).expect_err("mutation must diverge");
    assert!(
        diagnostic.starts_with("JIT/AOT replay divergence: tick=2 field=entry_results"),
        "first divergence must name tick and field: {diagnostic}"
    );

    let evidence = json!({
        "schema": "stasis.seam_test.v1",
        "test_id": "IT-011",
        "status": "passed",
        "target": "windows-jit+linked-aot",
        "snapshots": [
            {"tick": 1, "keyboard": "down", "pointer": "down", "logical": [320, 180], "resized": false},
            {"tick": 2, "keyboard": "up", "pointer": "move", "logical": [641, 359], "resized": true},
            {"tick": 3, "keyboard": "down", "pointer": "up", "logical": [641, 359], "resized": false}
        ],
        "jit": jit,
        "linked_aot": aot,
        "pointer_branches": {
            "outcomes": ["down", "move", "up"],
            "expected_cumulative_marks": EXPECTED_POINTER_MARKS,
            "jit_cumulative_marks": jit_pointer_marks,
            "linked_aot_cumulative_marks": aot_pointer_marks
        },
        "checks": 25,
        "check_counts": {
            "jit_aot_field_parity": 18,
            "pointer_branch_outcomes": 6,
            "mutation_diagnostic": 1
        },
        "oracle": {"first_divergence_diagnostic": diagnostic}
    });
    let path = evidence_root().join("it-011-jit-aot-host-replay.json");
    fs::create_dir_all(path.parent().expect("evidence parent")).expect("create evidence directory");
    fs::write(
        path,
        serde_json::to_vec_pretty(&evidence).expect("serialize evidence"),
    )
    .expect("write evidence");
    eprintln!("IT-011 evidence: {evidence}");
}
