use stasis_dynload::runtime_library_candidate_paths;
use std::path::PathBuf;
use std::process::Command;

#[test]
fn runtime_environment_uses_canonical_path_then_legacy_alias() {
    const CHILD_CASE: &str = "STASIS_RUNTIME_ENVIRONMENT_TEST_CASE";
    const CANONICAL: &str = "canonical-runtime-probe";
    const ALIAS: &str = "alias-runtime-probe";
    let cases = [
        (Some(CANONICAL), None, vec![CANONICAL]),
        (None, Some(ALIAS), vec![ALIAS]),
        (Some(CANONICAL), Some(ALIAS), vec![CANONICAL, ALIAS]),
        (None, None, vec![]),
    ];

    if let Ok(case) = std::env::var(CHILD_CASE) {
        let index: usize = case.parse().expect("child case index");
        let candidates = runtime_library_candidate_paths();
        let configured: Vec<_> = candidates
            .into_iter()
            .filter(|path| path == &PathBuf::from(CANONICAL) || path == &PathBuf::from(ALIAS))
            .collect();
        let expected: Vec<_> = cases[index].2.iter().map(PathBuf::from).collect();
        assert_eq!(configured, expected, "environment case {index}");
        return;
    }

    // Separate processes exercise the real environment lookup without racing other tests.
    for (index, (canonical, alias, _)) in cases.into_iter().enumerate() {
        let mut child = Command::new(std::env::current_exe().expect("test executable"));
        child
            .args([
                "--exact",
                "runtime_environment_uses_canonical_path_then_legacy_alias",
            ])
            .env(CHILD_CASE, index.to_string())
            .env_remove("STASIS_RUNTIME_LIBRARY_PATH")
            .env_remove("STASIS_RUNTIME_DLL_PATH");
        if let Some(path) = canonical {
            child.env("STASIS_RUNTIME_LIBRARY_PATH", path);
        }
        if let Some(path) = alias {
            child.env("STASIS_RUNTIME_DLL_PATH", path);
        }
        let output = child.output().expect("run environment case");
        assert!(
            output.status.success(),
            "environment case {index} failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
