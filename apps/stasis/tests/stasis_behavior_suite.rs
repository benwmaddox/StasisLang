use std::path::Path;

struct JitOutputCapture;

impl JitOutputCapture {
    fn start() -> Self {
        stasis_dynload::set_jit_output_capture(true);
        Self
    }

    fn finish(self) -> String {
        let output = stasis_dynload::drain_jit_output();
        drop(self);
        output
    }
}

impl Drop for JitOutputCapture {
    fn drop(&mut self) {
        stasis_dynload::set_jit_output_capture(false);
    }
}

#[test]
fn checked_in_stasis_behavior_suite_passes() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    let suite_root = repo_root.join("tests").join("stasis");
    let capture = JitOutputCapture::start();
    let summary = stasis::run_jit_tests_in_directory_with_project_root_and_session(
        &suite_root,
        &repo_root,
        &mut stasis::StasisTestRunSession::new(),
    )
    .expect("run checked-in Stasis behavior suite");
    let output = capture.finish();

    assert!(
        summary.tests_discovered > 0,
        "suite must contain real tests"
    );
    assert!(
        output.contains("42stringasciiutf8"),
        "print overloads must preserve i32, string, ASCII, and UTF-8 output; captured {output:?}"
    );
    assert_eq!(
        summary.tests_failed, 0,
        "Stasis behavior failures: {:?}",
        summary.failures
    );
    assert_eq!(summary.tests_run, summary.tests_discovered);
}
