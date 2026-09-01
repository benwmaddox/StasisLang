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
const COMPILE_REJECTED_SWAP_REVISION: u64 = 2;

#[derive(Clone, Debug, Deserialize)]
struct FrameEvidence {
    frame: u64,
    entry_revision: u64,
    accepted: i32,
    rejected: i32,
    presented: i32,
    validation: i32,
    guest_trace: u32,
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

fn is_aborted_swap_status(status: &str) -> bool {
    let Some(details) = status.strip_prefix("aborted") else {
        return false;
    };
    if details.is_empty() {
        return true;
    }
    if let Some(reason) = details.strip_prefix(": ") {
        return !reason.is_empty();
    }
    details
        .strip_prefix(" (")
        .and_then(|details| details.split_once("): "))
        .is_some_and(|(metrics, reason)| !metrics.is_empty() && !reason.is_empty())
}

fn aborted_swap_revision_after(log: &str, minimum_revision: u64) -> Option<u64> {
    log.lines()
        .filter_map(|line| {
            let remainder = line.trim().strip_prefix("[swap] revision ")?;
            let (revision, status) = remainder.split_once(' ')?;
            if !is_aborted_swap_status(status) {
                return None;
            }
            revision.parse::<u64>().ok()
        })
        .filter(|revision| *revision > minimum_revision)
        .max()
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
        .map(|frame| frame.guest_trace)
        .collect::<BTreeSet<_>>();
    assert_eq!(traces.len(), 1, "revision {revision} emitted mixed traces");
    *traces.iter().next().expect("revision trace")
}

fn frame_generation_error(frame: &FrameEvidence, v1_trace: u32, v2_trace: u32) -> Option<String> {
    if frame.rejected != 0 || frame.validation != 0 || frame.accepted != frame.presented {
        return Some(format!(
            "submission counters invalid: accepted={} rejected={} presented={} validation={}",
            frame.accepted, frame.rejected, frame.presented, frame.validation
        ));
    }
    let expected_guest_trace = match frame.entry_revision {
        1 => v1_trace,
        2 => v2_trace,
        revision => return Some(format!("unexpected entry revision {revision}")),
    };
    (frame.guest_trace != expected_guest_trace).then(|| {
        format!(
            "guest trace {} does not match revision {} trace {}; submitted trace was {}",
            frame.guest_trace, frame.entry_revision, expected_guest_trace, frame.trace
        )
    })
}

#[test]
fn generation_oracle_ignores_host_overlay_trace_but_rejects_wrong_guest_trace() {
    let mut frame = FrameEvidence {
        frame: 7,
        entry_revision: 2,
        accepted: 3,
        rejected: 0,
        presented: 3,
        validation: 0,
        guest_trace: 222,
        trace: 999,
    };
    assert_eq!(frame_generation_error(&frame, 111, 222), None);

    frame.guest_trace = 111;
    assert!(frame_generation_error(&frame, 111, 222)
        .expect("wrong guest trace must be rejected")
        .contains("guest trace 111"));
}

#[test]
fn aborted_swap_revision_parser_is_strict_and_tolerates_superseded_events() {
    assert_eq!(
        aborted_swap_revision_after(
            "[swap] revision 2 rejected: compile failure\n[swap] revision 3 aborted: hook requested rejection\n",
            COMPILE_REJECTED_SWAP_REVISION,
        ),
        Some(3)
    );

    let superseded = "[swap] revision 3 queued\n\
[swap] revision 4 queued, superseding in-flight revision 3\n\
[swap] revision 3 discarded as superseded\n\
[swap] revision 4 aborted (compile=6ms package=0ms commit=251ms): hook requested rejection\n";
    assert_eq!(
        aborted_swap_revision_after(superseded, COMPILE_REJECTED_SWAP_REVISION),
        Some(4)
    );

    for malformed in [
        "aborted",
        "watch aborted revision 4",
        "[swap] revision 2 aborted: stale rejection",
        "[swap] revision three aborted",
        "[swap] revision 4 rejected: hook requested rejection",
        "[swap] revision 4 discarded as superseded",
        "[swap] revision 4 aborted:missing reason separator",
        "[swap] revision 4 aborted: ",
        "[swap] revision 4 aborted (): missing metrics",
        "[swap] revision 4 aborted (compile=6ms): ",
        "[swap] revision 4 aborted later",
    ] {
        assert_eq!(
            aborted_swap_revision_after(malformed, COMPILE_REJECTED_SWAP_REVISION),
            None,
            "malformed log must not satisfy hook abort: {malformed}"
        );
    }
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
                    .all(|frame| frame.entry_revision == 2 && frame.guest_trace == v2_trace)
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
            aborted_swap_revision_after(log, COMPILE_REJECTED_SWAP_REVISION).is_some()
                && frames
                    .last()
                    .is_some_and(|frame| frame.frame >= failure_frame + 8)
                && frames
                    .iter()
                    .rev()
                    .take(8)
                    .all(|frame| frame.entry_revision == 2 && frame.guest_trace == v2_trace)
        },
    );
    let hook_rejection_revision =
        aborted_swap_revision_after(&final_log, COMPILE_REJECTED_SWAP_REVISION)
            .expect("later hook-aborted swap revision");

    child.kill().expect("stop completed seam runner");
    let _ = child.wait().expect("reap seam runner");

    assert!(
        final_log.contains("rejection"),
        "hook failure must be explicit"
    );
    if let Some((frame, error)) = final_frames.iter().find_map(|frame| {
        frame_generation_error(frame, v1_trace, v2_trace).map(|error| (frame, error))
    }) {
        panic!(
            "frame {} violated generation evidence: {error}: {frame:?}",
            frame.frame
        );
    }
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
        "compile_failure_preserved_revision": COMPILE_REJECTED_SWAP_REVISION,
        "hook_rejection_revision": hook_rejection_revision,
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
