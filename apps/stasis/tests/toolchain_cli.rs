use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

fn temp_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "stasis_cli_integration_{name}_{}_{}",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::SeqCst)
    ))
}

fn stasis(args: &[&str], cwd: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_stasis"))
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("run stasis CLI")
}

fn json_stdout(output: &Output) -> Value {
    assert!(
        output.stderr.is_empty(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("single JSON stdout object")
}

fn json_stderr(output: &Output) -> Value {
    assert!(
        output.stdout.is_empty(),
        "stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    serde_json::from_slice(&output.stderr).expect("single JSON stderr object")
}

#[test]
fn project_commands_emit_stable_json_from_nested_directories() {
    let parent = temp_dir("success");
    fs::create_dir_all(&parent).expect("create temp parent");
    let project = parent.join("demo");

    let created = stasis(&["--json", "new", "demo", "--dir", "demo"], &parent);
    assert_eq!(created.status.code(), Some(0));
    let created_json = json_stdout(&created);
    assert_eq!(created_json["ok"], true);
    assert_eq!(created_json["command"], "new");

    let version = stasis(&["--json", "--version"], &parent);
    assert_eq!(version.status.code(), Some(0));
    assert_eq!(json_stdout(&version)["command"], "version");

    let checked = stasis(&["--json", "check"], &project.join("src"));
    assert_eq!(checked.status.code(), Some(0));
    let checked_json = json_stdout(&checked);
    assert_eq!(checked_json["command"], "check");
    assert_eq!(checked_json["result"]["name"], "demo");

    let missing = stasis(&["--json", "--workspace", "missing", "check"], &project);
    assert_eq!(missing.status.code(), Some(1));
    assert!(json_stderr(&missing)["message"]
        .as_str()
        .unwrap_or_default()
        .contains("does not exist"));

    fs::remove_dir_all(&parent).ok();
}

#[test]
fn usage_compile_test_and_guest_exit_codes_are_stable() {
    let parent = temp_dir("failures");
    fs::create_dir_all(&parent).expect("create temp parent");
    let project = parent.join("demo");
    let created = stasis(&["new", "demo", "--dir", "demo"], &parent);
    assert_eq!(created.status.code(), Some(0));

    let usage = stasis(&["--json", "build", "--unknown"], &project);
    assert_eq!(usage.status.code(), Some(2));
    assert_eq!(json_stderr(&usage)["code"], "usage_error");

    fs::write(project.join("src/main.stasis"), "function main(: i32 {\n")
        .expect("write invalid source");
    let compile = stasis(&["--json", "check"], &project);
    assert_eq!(compile.status.code(), Some(1));
    assert_eq!(json_stderr(&compile)["code"], "command_failed");

    fs::write(
        project.join("tests/main.test.stasis"),
        "test `fails`(): bool {\n    return false;\n}\n",
    )
    .expect("write failing test");
    let tests = stasis(&["--json", "test"], &project);
    assert_eq!(tests.status.code(), Some(1));
    let test_json = json_stderr(&tests);
    assert_eq!(test_json["code"], "command_failed");
    assert!(test_json["message"]
        .as_str()
        .unwrap_or_default()
        .contains("fails"));

    fs::write(
        project.join("src/main.stasis"),
        "function main(): i32 {\n    return 7;\n}\n",
    )
    .expect("write runnable source");
    let run = stasis(&["--json", "run", "--headless"], &project);
    assert_eq!(run.status.code(), Some(7));
    let run_json = json_stdout(&run);
    assert_eq!(run_json["result"]["exit_code"], 7);

    fs::remove_dir_all(&parent).ok();
}
