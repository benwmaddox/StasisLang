use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use stasis_compiler::backend::jit::JitProcess;
use stasis_compiler::frontend::indexer::hash_text;
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
    pub timing_discovery_us: u64,
    pub timing_prepare_us: u64,
    pub timing_compile_us: u64,
    pub timing_execute_us: u64,
    pub timing_total_us: u64,
}

pub fn run_jit_tests_in_directory(root: &Path) -> Result<StasisTestRunSummary, String> {
    let mut session = StasisTestRunSession::new();
    run_jit_tests_in_directory_with_session(root, &mut session)
}

pub struct StasisTestRunSession {
    by_path: BTreeMap<PathBuf, CachedTestProcess>,
    last_active_path: Option<PathBuf>,
}

struct CachedTestProcess {
    source_hash: u64,
    process: JitProcess,
}

impl StasisTestRunSession {
    pub fn new() -> Self {
        Self {
            by_path: BTreeMap::new(),
            last_active_path: None,
        }
    }
}

impl Default for StasisTestRunSession {
    fn default() -> Self {
        Self::new()
    }
}

pub fn run_jit_tests_in_directory_with_session(
    root: &Path,
    session: &mut StasisTestRunSession,
) -> Result<StasisTestRunSummary, String> {
    run_jit_tests_in_directory_with_project_root_and_session(root, root, session)
}

pub fn run_jit_tests_in_directory_with_project_root_and_session(
    root: &Path,
    project_root: &Path,
    session: &mut StasisTestRunSession,
) -> Result<StasisTestRunSummary, String> {
    let total_started = Instant::now();
    let discovery_started = Instant::now();
    let mut files = Vec::new();
    collect_stasis_files_recursive(root, &mut files)?;
    files.sort_by(|left, right| {
        natural_path_cmp(
            &left.to_string_lossy().to_lowercase(),
            &right.to_string_lossy().to_lowercase(),
        )
    });
    let timing_discovery_us = elapsed_us_u64(discovery_started.elapsed().as_micros());
    let mut summary = StasisTestRunSummary {
        files_discovered: files.len(),
        files_with_tests: 0,
        tests_discovered: 0,
        tests_run: 0,
        tests_passed: 0,
        tests_failed: 0,
        failures: Vec::new(),
        timing_discovery_us,
        timing_prepare_us: 0,
        timing_compile_us: 0,
        timing_execute_us: 0,
        timing_total_us: 0,
    };
    let mut seen_paths: BTreeSet<PathBuf> = BTreeSet::new();

    for file_path in files {
        seen_paths.insert(file_path.clone());
        let prepare_started = Instant::now();
        let source = fs::read_to_string(&file_path).map_err(|error| {
            format!(
                "failed reading test source '{}': {error}",
                file_path.display()
            )
        })?;
        let (rewritten, tests) = rewrite_top_level_test_declarations(&source)?;
        summary.timing_prepare_us = summary
            .timing_prepare_us
            .saturating_add(elapsed_us_u64(prepare_started.elapsed().as_micros()));
        if tests.is_empty() {
            continue;
        }

        summary.files_with_tests += 1;
        summary.tests_discovered += tests.len();

        let source_hash = hash_text(&source);
        let entry = session.by_path.entry(file_path.clone()).or_insert_with(|| {
            let mut process = JitProcess::new();
            process
                .set_project_root(project_root.to_string_lossy())
                .expect("test project root is an absolute path");
            CachedTestProcess {
                source_hash: 0,
                process,
            }
        });
        let dependency_changed = entry
            .process
            .refresh_imported_sources_from_disk(&file_path.to_string_lossy());
        let compile_required = entry.source_hash != source_hash || dependency_changed;
        let runtime_rebind_required = !compile_required
            && session
                .last_active_path
                .as_ref()
                .is_some_and(|active| active != &file_path);
        if compile_required || runtime_rebind_required {
            entry
                .process
                .upsert_file(file_path.to_string_lossy().to_string(), rewritten);
            let required_roots: Vec<String> = tests
                .iter()
                .map(|test| test.generated_function_name.clone())
                .collect();
            entry.process.set_required_emit_roots(&required_roots);
            let compile_started = Instant::now();
            if let Err(error) = entry.process.compile() {
                summary.timing_compile_us = summary
                    .timing_compile_us
                    .saturating_add(elapsed_us_u64(compile_started.elapsed().as_micros()));
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
            summary.timing_compile_us = summary
                .timing_compile_us
                .saturating_add(elapsed_us_u64(compile_started.elapsed().as_micros()));
            entry.source_hash = source_hash;
            session.last_active_path = Some(file_path.clone());
        }

        entry.process.activate_runtime_state();
        let execute_started = Instant::now();
        run_discovered_tests(&entry.process, &tests, &file_path, &mut summary);
        session.last_active_path = Some(file_path.clone());
        summary.timing_execute_us = summary
            .timing_execute_us
            .saturating_add(elapsed_us_u64(execute_started.elapsed().as_micros()));
    }
    session.by_path.retain(|path, _| seen_paths.contains(path));
    if let Some(active_path) = session.last_active_path.as_ref() {
        if !seen_paths.contains(active_path) {
            session.last_active_path = None;
        }
    }

    summary.timing_total_us = elapsed_us_u64(total_started.elapsed().as_micros());

    Ok(summary)
}

fn elapsed_us_u64(value: u128) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
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
            out.push(normalize_runner_path(root));
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
            if should_skip_discovery_directory(&path) {
                continue;
            }
            collect_stasis_files_recursive(&path, out)?;
        } else if should_include_stasis_test_file(&path)? {
            out.push(normalize_runner_path(&path));
        }
    }
    Ok(())
}

fn normalize_runner_path(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn should_skip_discovery_directory(path: &Path) -> bool {
    let Some(name) = path.file_name() else {
        return false;
    };
    matches!(
        name.to_string_lossy().as_ref(),
        ".git" | "target" | ".stasis_cache"
    )
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
    if !looks_like_test_declaration_source(&source) {
        return Ok(false);
    }
    let tests = parse_top_level_test_declarations(&source)?;
    Ok(!tests.is_empty())
}

fn looks_like_test_declaration_source(source: &str) -> bool {
    source.contains("test") && source.contains('`')
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
    fn looks_like_test_declaration_source_prefilter() {
        assert!(!looks_like_test_declaration_source(
            "function tick(): i32 { return 0; }\n"
        ));
        assert!(!looks_like_test_declaration_source(
            "// Stress test fixture only; no declarations.\n"
        ));
        assert!(looks_like_test_declaration_source(
            "test `smoke`(): bool { return true; }\n"
        ));
    }

    #[test]
    fn run_jit_tests_in_directory_skips_target_git_and_stasis_cache_dirs() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("stasis_test_runner_skipdirs_{stamp}"));
        let target_dir = root.join("target");
        let git_dir = root.join(".git");
        let stasis_cache_dir = root.join(".stasis_cache");
        let suite_dir = root.join("suite");
        fs::create_dir_all(&target_dir).expect("mkdir target");
        fs::create_dir_all(&git_dir).expect("mkdir git");
        fs::create_dir_all(&stasis_cache_dir).expect("mkdir stasis cache");
        fs::create_dir_all(&suite_dir).expect("mkdir suite");
        fs::write(
            target_dir.join("bad.test.stasis"),
            "test `bad`(): bool { return false; }\n",
        )
        .expect("write target test");
        fs::write(
            git_dir.join("bad2.test.stasis"),
            "test `bad2`(): bool { return false; }\n",
        )
        .expect("write git test");
        fs::write(
            stasis_cache_dir.join("bad3.test.stasis"),
            "test `bad3`(): bool { return false; }\n",
        )
        .expect("write stasis cache test");
        fs::write(
            suite_dir.join("good.test.stasis"),
            "test `good`(): bool { return true; }\n",
        )
        .expect("write suite test");

        let summary = run_jit_tests_in_directory(&root).expect("run tests");
        assert_eq!(summary.files_discovered, 1, "{summary:?}");
        assert_eq!(summary.tests_discovered, 1, "{summary:?}");
        assert_eq!(summary.tests_passed, 1, "{summary:?}");
        assert_eq!(summary.tests_failed, 0, "{summary:?}");

        fs::remove_dir_all(&root).ok();
    }

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

    #[test]
    fn run_jit_tests_in_directory_resolves_imported_helper_functions() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("stasis_test_runner_imports_{stamp}"));
        fs::create_dir_all(&root).expect("mkdir");
        let helper = root.join("helper.stasis");
        let fixture = root.join("sample.test.stasis");
        fs::write(&helper, "function helper(): i32 { return 7; }\n").expect("write helper");
        fs::write(
            &fixture,
            "import \"helper.stasis\";\ntest `imports helper`(): bool { return helper() == 7; }\n",
        )
        .expect("write fixture");

        let summary = run_jit_tests_in_directory(&root).expect("run tests");
        assert_eq!(summary.files_discovered, 1, "{summary:?}");
        assert_eq!(summary.tests_discovered, 1, "{summary:?}");
        assert_eq!(summary.tests_run, 1, "{summary:?}");
        assert_eq!(summary.tests_passed, 1, "{summary:?}");
        assert_eq!(summary.tests_failed, 0, "{summary:?}");

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn run_jit_tests_in_directory_resolves_import_from_utf8_bom_source() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("stasis_test_runner_import_bom_{stamp}"));
        fs::create_dir_all(&root).expect("mkdir");
        let helper = root.join("helper.stasis");
        let fixture = root.join("sample.test.stasis");
        fs::write(&helper, "function helper(): i32 { return 7; }\n").expect("write helper");
        fs::write(
            &fixture,
            "\u{feff}import \"helper.stasis\";\ntest `imports helper from bom`(): bool { return helper() == 7; }\n",
        )
        .expect("write fixture");

        let summary = run_jit_tests_in_directory(&root).expect("run tests");
        assert_eq!(summary.files_discovered, 1, "{summary:?}");
        assert_eq!(summary.tests_discovered, 1, "{summary:?}");
        assert_eq!(summary.tests_run, 1, "{summary:?}");
        assert_eq!(summary.tests_passed, 1, "{summary:?}");
        assert_eq!(summary.tests_failed, 0, "{summary:?}");

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn session_skips_compile_for_unchanged_files() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("stasis_test_runner_session_{stamp}"));
        fs::create_dir_all(&root).expect("mkdir");
        let fixture = root.join("sample.test.stasis");
        fs::write(&fixture, "test `ok`(): bool { return true; }\n").expect("write");

        let mut session = StasisTestRunSession::new();
        let first = run_jit_tests_in_directory_with_session(&root, &mut session).expect("first");
        assert_eq!(first.tests_passed, 1, "{first:?}");
        assert!(first.timing_compile_us > 0, "{first:?}");
        let second = run_jit_tests_in_directory_with_session(&root, &mut session).expect("second");
        assert_eq!(second.tests_passed, 1, "{second:?}");
        assert_eq!(second.timing_compile_us, 0, "{second:?}");

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn session_restores_dispatch_table_for_cached_process_before_execute() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("stasis_test_runner_dispatch_{stamp}"));
        fs::create_dir_all(&root).expect("mkdir");
        let left = root.join("left.test.stasis");
        let right = root.join("right.test.stasis");
        fs::write(
            &left,
            "function left_helper(): bool { return true; }\ntest `left`(): bool { return left_helper(); }\n",
        )
        .expect("write left");
        fs::write(
            &right,
            "function right_helper(): bool { return false; }\ntest `right`(): bool { return !right_helper(); }\n",
        )
        .expect("write right");

        let mut session = StasisTestRunSession::new();
        let first = run_jit_tests_in_directory_with_session(&root, &mut session).expect("first");
        assert_eq!(first.tests_passed, 2, "{first:?}");
        assert_eq!(first.tests_failed, 0, "{first:?}");

        fs::write(
            &left,
            "function left_helper(): bool { return true; }\ntest `left`(): bool { if (1 == 2) { return false; } return left_helper(); }\n",
        )
        .expect("rewrite left");

        let second =
            run_jit_tests_in_directory_with_session(&root, &mut session).expect("second run");
        assert_eq!(second.tests_passed, 2, "{second:?}");
        assert_eq!(second.tests_failed, 0, "{second:?}");

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn session_recompiles_when_imported_dependency_changes() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("stasis_test_runner_dependency_change_{stamp}"));
        fs::create_dir_all(&root).expect("mkdir");
        let helper = root.join("helper.stasis");
        let fixture = root.join("sample.test.stasis");
        fs::write(&helper, "function helper(): i32 { return 7; }\n").expect("write helper");
        fs::write(
            &fixture,
            "import \"helper.stasis\";\ntest `dependency-sensitive`(): bool { return helper() == 7; }\n",
        )
        .expect("write fixture");

        let mut session = StasisTestRunSession::new();
        let first = run_jit_tests_in_directory_with_session(&root, &mut session).expect("first");
        assert_eq!(first.tests_passed, 1, "{first:?}");
        assert_eq!(first.tests_failed, 0, "{first:?}");

        fs::write(&helper, "function helper(): i32 { return 8; }\n").expect("rewrite helper");
        let second = run_jit_tests_in_directory_with_session(&root, &mut session).expect("second");
        assert_eq!(second.tests_run, 1, "{second:?}");
        assert_eq!(second.tests_passed, 0, "{second:?}");
        assert_eq!(second.tests_failed, 1, "{second:?}");

        fs::remove_dir_all(&root).ok();
    }
}
