use std::path::Path;

#[test]
fn checked_in_stasis_behavior_suite_passes() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    let suite_root = repo_root.join("tests").join("stasis");
    let summary = stasis::run_jit_tests_in_directory_with_project_root_and_session(
        &suite_root,
        &repo_root,
        &mut stasis::StasisTestRunSession::new(),
    )
    .expect("run checked-in Stasis behavior suite");

    assert!(
        summary.tests_discovered > 0,
        "suite must contain real tests"
    );
    assert_eq!(
        summary.tests_failed, 0,
        "Stasis behavior failures: {:?}",
        summary.failures
    );
    assert_eq!(summary.tests_run, summary.tests_discovered);
}
