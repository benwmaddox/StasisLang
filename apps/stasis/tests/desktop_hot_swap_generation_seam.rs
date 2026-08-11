#![cfg(windows)]

use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const V1: &str = include_str!("../../../tests/stasis/seams/desktop_hot_swap_generation_v1.stasis");
const V2: &str = include_str!("../../../tests/stasis/seams/desktop_hot_swap_generation_v2.stasis");
const INVALID: &str =
    include_str!("../../../tests/stasis/seams/desktop_hot_swap_generation_invalid.stasis");
const REJECT: &str =
    include_str!("../../../tests/stasis/seams/desktop_hot_swap_generation_reject.stasis");

#[derive(Clone, Debug, Deserialize)]
struct FrameEvidence {
    frame: u64,
    entry_revision: u64,
    accepted: i32,
    rejected: i32,
    presented: i32,
    validation: i32,
    trace: u32,
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

fn read_frames(path: &Path) -> Vec<FrameEvidence> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

fn read_log(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

fn wait_for(
    description: &str,
    child: &mut Child,
    log_path: &Path,
    frames_path: &Path,
    predicate: impl Fn(&str, &[FrameEvidence]) -> bool,
) -> (String, Vec<FrameEvidence>) {
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        let log = read_log(log_path);
        let frames = read_frames(frames_path);
        if predicate(&log, &frames) {
            return (log, frames);
        }
        if let Some(status) = child.try_wait().expect("poll stasis play") {
            panic!(
                "stasis play exited before {description}: status={status} log={log} frames={frames:?}"
            );
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {description}: log={log} frames={frames:?}"
        );
        thread::sleep(Duration::from_millis(50));
    }
}

fn sole_trace(frames: &[FrameEvidence], revision: u64) -> u32 {
    let traces = frames
        .iter()
        .filter(|frame| frame.entry_revision == revision)
        .map(|frame| frame.trace)
        .collect::<BTreeSet<_>>();
    assert_eq!(traces.len(), 1, "revision {revision} emitted mixed traces");
    *traces.iter().next().expect("revision trace")
}

#[test]
fn desktop_watch_frames_never_mix_tick_and_render_generations() {
    let runtime_path = PathBuf::from(
        std::env::var_os("STASIS_RUNTIME_DLL_PATH")
            .expect("STASIS_RUNTIME_DLL_PATH must name the CI-built SDL runtime"),
    );
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let tree = TestTree(
        std::env::temp_dir().join(format!("stasis-it-010-{}-{stamp}", std::process::id())),
    );
    fs::create_dir_all(&tree.0).expect("create seam project");
    let source = tree.0.join("main.stasis");
    let frames_path = tree.0.join("frames.jsonl");
    let log_path = tree.0.join("play.log");
    fs::write(&source, V1).expect("write v1 source");

    let log_file = fs::File::create(&log_path).expect("create play log");
    let mut child = Command::new(env!("CARGO_BIN_EXE_stasis"))
        .args([
            "play",
            source.to_str().expect("source path"),
            "--watch-dir",
            tree.0.to_str().expect("project path"),
            "--ticks",
            "6000",
            "--tick-sleep-us",
            "5000",
        ])
        .current_dir(&tree.0)
        .env("STASIS_RUNTIME_LIBRARY_PATH", &runtime_path)
        .env("STASIS_RUNTIME_DLL_PATH", &runtime_path)
        .env("STASIS_USE_SDL", "1")
        .env("STASIS_ENABLE_TEST_INPUT", "1")
        .env("STASIS_DESKTOP_FRAME_EVIDENCE", &frames_path)
        .stdout(Stdio::from(log_file.try_clone().expect("clone stdout log")))
        .stderr(Stdio::from(log_file))
        .spawn()
        .expect("launch stasis play");

    let (_, initial_frames) = wait_for(
        "eight initial frames",
        &mut child,
        &log_path,
        &frames_path,
        |_, frames| frames.len() >= 8,
    );
    let v1_trace = sole_trace(&initial_frames, 1);
    assert_ne!(v1_trace, 0);

    fs::write(&source, V2).expect("write v2 source");
    let (_, v2_frames) = wait_for(
        "published v2 and eight v2 frames",
        &mut child,
        &log_path,
        &frames_path,
        |log, frames| {
            log.contains("[swap] revision 1 published")
                && frames
                    .iter()
                    .filter(|frame| frame.entry_revision == 2)
                    .count()
                    >= 8
        },
    );
    let v2_trace = sole_trace(&v2_frames, 2);
    assert_ne!(
        v2_trace, v1_trace,
        "the accepted edit must change the frame"
    );

    fs::write(&source, INVALID).expect("write malformed source");
    let (_, after_compile_failure) = wait_for(
        "compile rejection and continued v2 frames",
        &mut child,
        &log_path,
        &frames_path,
        |log, frames| {
            log.contains("[swap] revision 2 rejected")
                && frames
                    .iter()
                    .rev()
                    .take(8)
                    .all(|frame| frame.entry_revision == 2 && frame.trace == v2_trace)
        },
    );
    let failure_frame = after_compile_failure.last().expect("failure frame").frame;

    fs::write(&source, REJECT).expect("write hook-rejecting source");
    let (final_log, final_frames) = wait_for(
        "on_code_swap abort and continued v2 frames",
        &mut child,
        &log_path,
        &frames_path,
        |log, frames| {
            log.contains("[swap] revision 3 aborted")
                && frames
                    .last()
                    .is_some_and(|frame| frame.frame >= failure_frame + 8)
                && frames
                    .iter()
                    .rev()
                    .take(8)
                    .all(|frame| frame.entry_revision == 2 && frame.trace == v2_trace)
        },
    );

    child.kill().expect("stop completed seam runner");
    let _ = child.wait().expect("reap seam runner");

    assert!(
        final_log.contains("rejection"),
        "hook failure must be explicit"
    );
    assert!(final_frames.iter().all(|frame| {
        frame.rejected == 0
            && frame.validation == 0
            && frame.accepted == frame.presented
            && match frame.entry_revision {
                1 => frame.trace == v1_trace,
                2 => frame.trace == v2_trace,
                _ => false,
            }
    }));
    assert!(
        final_frames
            .windows(2)
            .all(|pair| pair[0].entry_revision <= pair[1].entry_revision),
        "entry-table revisions must publish monotonically"
    );

    let evidence_dir = evidence_root();
    fs::create_dir_all(&evidence_dir).expect("create evidence directory");
    let frame_evidence_path = evidence_dir.join("it-010-desktop-hot-swap-frames.jsonl");
    fs::copy(&frames_path, &frame_evidence_path).expect("retain per-frame evidence");
    let evidence = json!({
        "schema": "stasis.seam_test.v1",
        "test_id": "IT-010",
        "status": "passed",
        "target": "windows-sdl-native-jit-watch",
        "frames": final_frames.len(),
        "entry_trace_pairs": [
            {"entry_revision": 1, "trace": v1_trace},
            {"entry_revision": 2, "trace": v2_trace}
        ],
        "compile_failure_preserved_revision": 2,
        "hook_rejection_preserved_revision": 2,
        "oracle": {"mixed_generation_frames": 0, "source_frames": frame_evidence_path}
    });
    let evidence_path = evidence_dir.join("it-010-desktop-hot-swap-generations.json");
    fs::write(
        evidence_path,
        serde_json::to_vec_pretty(&evidence).expect("serialize evidence"),
    )
    .expect("write evidence");
    eprintln!("IT-010 evidence: {evidence}");
}
