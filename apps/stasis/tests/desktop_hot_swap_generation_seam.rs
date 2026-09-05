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
const PUBLIC_GRAPHICS_IMPORT: &str =
    "import \"/.stasis_cache/toolchain/src/stdlib/graphics.stasis\";";

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

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("create fixture destination");
    for entry in fs::read_dir(source).expect("read fixture directory") {
        let entry = entry.expect("read fixture entry");
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if source_path.is_dir() {
            copy_tree(&source_path, &destination_path);
        } else {
            fs::copy(&source_path, &destination_path).expect("copy fixture file");
        }
    }
}

fn materialize_toolchain_stdlib(project: &Path) {
    let destination = project.join(".stasis_cache/toolchain/src/stdlib");
    copy_tree(&repository_root().join("src/stdlib"), &destination);
    assert!(
        destination.join("graphics.stasis").is_file(),
        "toolchain stdlib staging omitted graphics.stasis"
    );
}

fn assert_public_graphics_fixture(name: &str, source: &str) {
    assert!(
        source.contains(PUBLIC_GRAPHICS_IMPORT),
        "{name} must use the rooted public graphics import"
    );
    assert!(
        !source.contains("gfx_cmd_"),
        "{name} must not name private graphics command storage"
    );
}

fn assert_generation_fixtures_use_public_graphics() {
    for (name, source) in [
        ("v1", V1),
        ("v2", V2),
        ("invalid", INVALID),
        ("reject", REJECT),
    ] {
        assert_public_graphics_fixture(name, source);
    }
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

fn rejection_revision(log: &str, hook_abort: bool) -> Option<u64> {
    if hook_abort {
        return aborted_swap_revision_after(log, 0);
    }
    log.lines()
        .filter_map(|line| {
            let remainder = line.trim().strip_prefix("[swap] revision ")?;
            let (revision, reason) = remainder.split_once(" rejected: ")?;
            if reason.is_empty() {
                return None;
            }
            revision.parse::<u64>().ok()
        })
        .max()
}

fn published_edit_frames(frames: &[FrameEvidence], old_trace: u32) -> bool {
    frames.len() >= 8
        && frames
            .iter()
            .rev()
            .take(8)
            .all(|frame| frame.entry_revision > 1 && frame.guest_trace != old_trace)
}

fn preserved_edit_frames(
    frames: &[FrameEvidence],
    after_frame: u64,
    revision: u64,
    trace: u32,
) -> bool {
    frames
        .iter()
        .filter(|frame| frame.frame > after_frame)
        .count()
        >= 8
        && frames
            .iter()
            .filter(|frame| frame.frame > after_frame)
            .all(|frame| frame.entry_revision == revision && frame.guest_trace == trace)
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
        2.. => v2_trace,
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
            2,
        ),
        Some(3)
    );

    let superseded = "[swap] revision 3 queued\n\
[swap] revision 4 queued, superseding in-flight revision 3\n\
[swap] revision 3 discarded as superseded\n\
[swap] revision 4 aborted (compile=6ms package=0ms commit=251ms): hook requested rejection\n";
    assert_eq!(aborted_swap_revision_after(superseded, 2), Some(4));

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
            aborted_swap_revision_after(malformed, 2),
            None,
            "malformed log must not satisfy hook abort: {malformed}"
        );
    }
}

#[test]
fn duplicate_watch_events_do_not_control_generation_waits() {
    let log = "[watch] queued revision 1\n[watch] queued revision 2\n\
[watch] discarded superseded revision 1 (latest 2)\n\
[swap] revision 2 published re_jit=2 reused=0\n";
    let mut frames = (1..=8)
        .map(|frame| FrameEvidence {
            frame,
            entry_revision: if frame < 5 { 2 } else { 3 },
            accepted: 3,
            rejected: 0,
            presented: 3,
            validation: 0,
            guest_trace: 222,
            trace: 999,
        })
        .collect::<Vec<_>>();
    assert!(!log.contains("[swap] revision 1 published"));
    assert!(published_edit_frames(&frames, 111));
    assert!(frames
        .iter()
        .all(|frame| frame_generation_error(frame, 111, 222).is_none()));
    assert!(!preserved_edit_frames(&frames, 8, 3, 222));
    for frame in 9..=16 {
        let mut next = frames.last().unwrap().clone();
        next.frame = frame;
        frames.push(next);
    }
    assert!(preserved_edit_frames(&frames, 8, 3, 222));
    frames.last_mut().unwrap().entry_revision = 4;
    assert!(!preserved_edit_frames(&frames, 8, 3, 222));
    frames.last_mut().unwrap().entry_revision = 3;
    frames.last_mut().unwrap().guest_trace = 333;
    assert!(!preserved_edit_frames(&frames, 8, 3, 222));
    let rejected = "[watch] discarded superseded revision 3 (latest 5)\n\
[swap] revision 5 rejected: parse error\n";
    assert_eq!(rejection_revision(log, false), None);
    assert_eq!(rejection_revision(rejected, false), Some(5));
    assert_eq!(rejection_revision(rejected, true), None);
    assert_eq!(
        rejection_revision(
            "[swap] revision 8 aborted (compile=6ms): hook requested rejection\n",
            true
        ),
        Some(8)
    );
}

#[test]
fn generation_fixtures_use_only_public_graphics() {
    assert_generation_fixtures_use_public_graphics();
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
    materialize_toolchain_stdlib(&tree.0);
    assert_generation_fixtures_use_public_graphics();
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
        |_, frames| published_edit_frames(frames, v1_trace),
    );
    let v2_trace = v2_frames.last().expect("v2 frame").guest_trace;
    assert_ne!(
        v2_trace, v1_trace,
        "the accepted edit must change the frame"
    );

    let mut rejection_revisions = Vec::new();
    let mut preserved_revisions = Vec::new();
    let mut final_log = String::new();
    let mut final_frames = Vec::new();
    for (fixture, hook_abort) in [(INVALID, false), (REJECT, true)] {
        // A stage-local log suffix excludes earlier failures without assuming event counts.
        let log_start = read_log(&log_path).len();
        fs::write(&source, fixture).expect("write rejecting source");
        let (log, frames) = wait_for(
            "edit rejection",
            &mut child,
            &log_path,
            &frames_path,
            |log, _| {
                rejection_revision(log.get(log_start..).unwrap_or_default(), hook_abort).is_some()
            },
        );
        rejection_revisions.push(
            rejection_revision(log.get(log_start..).unwrap_or_default(), hook_abort)
                .expect("observed rejection"),
        );
        let rejection_frame = frames.last().expect("frame at rejection");
        let preserved_revision = rejection_frame.entry_revision;
        preserved_revisions.push(preserved_revision);
        let rejection_frame = rejection_frame.frame;
        (final_log, final_frames) = wait_for(
            "eight preserved v2 frames after rejection",
            &mut child,
            &log_path,
            &frames_path,
            |_, frames| {
                preserved_edit_frames(frames, rejection_frame, preserved_revision, v2_trace)
            },
        );
    }

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
        "entry_trace_pairs": final_frames.iter()
            .map(|frame| (frame.entry_revision, frame.guest_trace))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|(revision, trace)| json!({"entry_revision": revision, "trace": trace}))
            .collect::<Vec<_>>(),
        "compile_rejection_watch_revision": rejection_revisions[0],
        "compile_failure_preserved_trace": v2_trace,
        "compile_failure_preserved_revision": preserved_revisions[0],
        "hook_rejection_watch_revision": rejection_revisions[1],
        "hook_rejection_preserved_trace": v2_trace,
        "hook_rejection_preserved_revision": preserved_revisions[1],
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
