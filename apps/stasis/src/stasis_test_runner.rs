use std::cmp::Ordering;
use std::fs;
use std::path::{Path, PathBuf};

use stasis_compiler::backend::jit::JitProcess;
use stasis_compiler::frontend::parser::{
    parse_top_level_test_declarations, rewrite_top_level_test_declarations, ParsedTestDeclaration,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StasisTestRunSummary {
    pub files_discovered: usize,
    pub files_with_tests: usize,
    pub tests_discovered: usize,
    pub tests_run: usize,
    pub tests_passed: usize,
    pub tests_failed: usize,
    pub failures: Vec<String>,
}

pub fn run_jit_tests_in_directory(root: &Path) -> Result<StasisTestRunSummary, String> {
    let mut files = Vec::new();
    collect_stasis_files_recursive(root, &mut files)?;
    files.sort_by(|left, right| {
        natural_path_cmp(
            &left.to_string_lossy().to_lowercase(),
            &right.to_string_lossy().to_lowercase(),
        )
    });

    let mut summary = StasisTestRunSummary {
        files_discovered: files.len(),
        files_with_tests: 0,
        tests_discovered: 0,
        tests_run: 0,
        tests_passed: 0,
        tests_failed: 0,
        failures: Vec::new(),
    };

    for file_path in files {
        let source = fs::read_to_string(&file_path).map_err(|error| {
            format!(
                "failed reading test source '{}': {error}",
                file_path.display()
            )
        })?;
        let (rewritten, tests) = rewrite_top_level_test_declarations(&source)?;
        if tests.is_empty() {
            continue;
        }

        summary.files_with_tests += 1;
        summary.tests_discovered += tests.len();

        let mut process = JitProcess::new();
        process.upsert_file(file_path.to_string_lossy().to_string(), rewritten);
        let required_roots: Vec<String> = tests
            .iter()
            .map(|test| test.generated_function_name.clone())
            .collect();
        process.set_required_emit_roots(&required_roots);
        if let Err(error) = process.compile() {
            for test in tests {
                summary.tests_failed += 1;
                summary.failures.push(format!(
                    "{} :: {} :: compile failed: {error:?}",
                    file_path.display(),
                    test.display_name
                ));
            }
            continue;
        }

        run_discovered_tests(&process, &tests, &file_path, &mut summary);
    }

    Ok(summary)
}

fn run_discovered_tests(
    process: &JitProcess,
    tests: &[ParsedTestDeclaration],
    file_path: &Path,
    summary: &mut StasisTestRunSummary,
) {
    for test in tests {
        summary.tests_run += 1;
        match process.execute_bool_noarg_by_name(&test.generated_function_name) {
            Ok(true) => {
                summary.tests_passed += 1;
            }
            Ok(false) => {
                summary.tests_failed += 1;
                summary.failures.push(format!(
                    "{} :: {} :: returned false",
                    file_path.display(),
                    test.display_name
                ));
            }
            Err(error) => {
                summary.tests_failed += 1;
                summary.failures.push(format!(
                    "{} :: {} :: execution failed: {error}",
                    file_path.display(),
                    test.display_name
                ));
            }
        }
    }
}

fn collect_stasis_files_recursive(root: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let metadata = fs::metadata(root)
        .map_err(|error| format!("failed to stat '{}': {error}", root.display()))?;
    if metadata.is_file() {
        if should_include_stasis_test_file(root)? {
            out.push(root.to_path_buf());
        }
        return Ok(());
    }
    let mut entries = Vec::new();
    for entry in fs::read_dir(root)
        .map_err(|error| format!("failed to read dir '{}': {error}", root.display()))?
    {
        let entry = entry.map_err(|error| {
            format!(
                "failed reading directory entry under '{}': {error}",
                root.display()
            )
        })?;
        entries.push(entry.path());
    }
    entries.sort_by(|left, right| {
        natural_path_cmp(
            &left.to_string_lossy().to_lowercase(),
            &right.to_string_lossy().to_lowercase(),
        )
    });
    for path in entries {
        let kind = fs::metadata(&path)
            .map_err(|error| format!("failed to stat '{}': {error}", path.display()))?;
        if kind.is_dir() {
            collect_stasis_files_recursive(&path, out)?;
        } else if should_include_stasis_test_file(&path)? {
            out.push(path);
        }
    }
    Ok(())
}

fn should_include_stasis_test_file(path: &Path) -> Result<bool, String> {
    let Some(name) = path.file_name() else {
        return Ok(false);
    };
    let lower = name.to_string_lossy().to_lowercase();
    if lower.ends_with(".test.stasis") {
        return Ok(true);
    }
    if !lower.ends_with(".stasis") {
        return Ok(false);
    }
    let source = fs::read_to_string(path)
        .map_err(|error| format!("failed reading '{}': {error}", path.display()))?;
    let tests = parse_top_level_test_declarations(&source)?;
    Ok(!tests.is_empty())
}

fn natural_path_cmp(left: &str, right: &str) -> Ordering {
    let left_bytes = left.as_bytes();
    let right_bytes = right.as_bytes();
    let mut i = 0usize;
    let mut j = 0usize;

    while i < left_bytes.len() && j < right_bytes.len() {
        let a = left_bytes[i];
        let b = right_bytes[j];
        if a.is_ascii_digit() && b.is_ascii_digit() {
            let (a_value, a_next) = parse_number(left_bytes, i);
            let (b_value, b_next) = parse_number(right_bytes, j);
            match a_value.cmp(&b_value) {
                Ordering::Equal => {
                    i = a_next;
                    j = b_next;
                    continue;
                }
                non_eq => return non_eq,
            }
        }
        match a.cmp(&b) {
            Ordering::Equal => {
                i += 1;
                j += 1;
            }
            non_eq => return non_eq,
        }
    }
    left_bytes.len().cmp(&right_bytes.len())
}

fn parse_number(bytes: &[u8], start: usize) -> (u64, usize) {
    let mut cursor = start;
    let mut value = 0u64;
    while cursor < bytes.len() && bytes[cursor].is_ascii_digit() {
        let digit = (bytes[cursor] - b'0') as u64;
        value = value.saturating_mul(10).saturating_add(digit);
        cursor += 1;
    }
    (value, cursor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn run_jit_tests_in_directory_executes_nested_stasis_tests() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("stasis_test_runner_{stamp}"));
        let nested = root.join("nested");
        fs::create_dir_all(&nested).expect("mkdir");
        let fixture = nested.join("sample.test.stasis");
        fs::write(
            &fixture,
            "test `passes`(): bool { return true; }\ntest `fails`(): bool { return false; }\n",
        )
        .expect("write");

        let summary = run_jit_tests_in_directory(&root).expect("run tests");
        assert_eq!(summary.files_discovered, 1);
        assert_eq!(summary.files_with_tests, 1);
        assert_eq!(summary.tests_discovered, 2);
        assert_eq!(summary.tests_run, 2);
        assert_eq!(summary.tests_passed, 1, "{summary:?}");
        assert_eq!(summary.tests_failed, 1, "{summary:?}");
        assert_eq!(summary.failures.len(), 1);

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn run_jit_tests_in_directory_accepts_test_keyword_without_test_extension() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("stasis_test_runner_keyword_{stamp}"));
        fs::create_dir_all(&root).expect("mkdir");
        let fixture = root.join("sample.stasis");
        fs::write(
            &fixture,
            "test `works without test suffix`(): bool { return true; }\n",
        )
        .expect("write");

        let summary = run_jit_tests_in_directory(&root).expect("run tests");
        assert_eq!(summary.files_discovered, 1);
        assert_eq!(summary.tests_discovered, 1);
        assert_eq!(summary.tests_passed, 1, "{summary:?}");
        assert_eq!(summary.tests_failed, 0, "{summary:?}");

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn run_jit_tests_in_directory_isolates_each_file_compile() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("stasis_test_runner_isolation_{stamp}"));
        fs::create_dir_all(&root).expect("mkdir");
        let left = root.join("left.test.stasis");
        let right = root.join("right.stasis");
        fs::write(
            &left,
            "global value: i32;\nfunction seed(): void { value = 1; }\ntest `left`(): bool { seed(); return value == 1; }\n",
        )
        .expect("write left");
        fs::write(
            &right,
            "global value: i32;\nfunction seed(): void { value = 2; }\ntest `right`(): bool { seed(); return value == 2; }\n",
        )
        .expect("write right");

        let summary = run_jit_tests_in_directory(&root).expect("run tests");
        assert_eq!(summary.files_discovered, 2, "{summary:?}");
        assert_eq!(summary.files_with_tests, 2, "{summary:?}");
        assert_eq!(summary.tests_discovered, 2, "{summary:?}");
        assert_eq!(summary.tests_run, 2, "{summary:?}");
        assert_eq!(summary.tests_passed, 2, "{summary:?}");
        assert_eq!(summary.tests_failed, 0, "{summary:?}");

        fs::remove_dir_all(&root).ok();
    }
}
