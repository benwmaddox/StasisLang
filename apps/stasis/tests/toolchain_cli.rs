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

#[test]
fn semantic_symbol_cli_previews_applies_runs_and_reverts() {
    let parent = temp_dir("semantic_symbols");
    fs::create_dir_all(&parent).expect("create temp parent");
    let project = parent.join("demo");
    let created = stasis(&["new", "demo", "--dir", "demo"], &parent);
    assert_eq!(created.status.code(), Some(0));

    fs::write(
        project.join("src/main.stasis"),
        "import \"old.stasis\";\n\nconst LIMIT: i32 = 2;\n\nstruct Config { width: i32; }\n\nfunction main(): i32 { return tick(); }\n\n// Tick behavior.\nfunction tick(): i32 { return old_value(); }\n",
    )
    .expect("write main");
    fs::write(
        project.join("src/old.stasis"),
        "function old_value(): i32 { return 1; }\n",
    )
    .expect("write old module");
    fs::write(
        project.join("src/new.stasis"),
        "function new_value(): i32 { return 9; }\n",
    )
    .expect("write new module");
    fs::create_dir_all(project.join("edits")).expect("create edits");
    fs::write(
        project.join("edits/tick.stasis"),
        "// Tick behavior.\nfunction tick(): i32 {\n    import \"new.stasis\";\n    return new_value();\n}\n",
    )
    .expect("write edit");
    fs::write(
        project.join("edits/globals.stasis"),
        "const LIMIT: i32 = 4;\n",
    )
    .expect("write globals edit");
    fs::write(
        project.join("edits/config.stasis"),
        "struct Config { width: i32; height: i32; }\n",
    )
    .expect("write struct edit");

    let listed = stasis(&["--json", "symbol", "list"], &project);
    assert_eq!(listed.status.code(), Some(0));
    let listed_json = json_stdout(&listed);
    assert!(listed_json["result"]["items"]
        .as_array()
        .expect("items")
        .iter()
        .any(|item| item["kind"] == "imports" && item["file"] == "src/main.stasis"));

    let preview = stasis(
        &[
            "--json",
            "symbol",
            "update",
            "tick",
            "--kind",
            "function",
            "--file",
            "src/main.stasis",
            "--source-file",
            "edits/tick.stasis",
            "--dry-run",
        ],
        &project,
    );
    assert_eq!(preview.status.code(), Some(0));
    assert_eq!(json_stdout(&preview)["result"]["status"], "preview");
    assert!(fs::read_to_string(project.join("src/main.stasis"))
        .expect("preview source")
        .contains("old_value"));

    let applied = stasis(
        &[
            "--json",
            "symbol",
            "update",
            "tick",
            "--kind",
            "function",
            "--file",
            "src/main.stasis",
            "--source-file",
            "edits/tick.stasis",
        ],
        &project,
    );
    assert_eq!(
        applied.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&applied.stderr)
    );
    let applied_json = json_stdout(&applied);
    assert_eq!(applied_json["result"]["status"], "applied");
    let receipt = applied_json["result"]["receipt"]
        .as_str()
        .expect("receipt")
        .to_string();
    let updated = fs::read_to_string(project.join("src/main.stasis")).expect("updated source");
    assert!(updated.starts_with("import \"new.stasis\";\n"));
    assert!(!updated.contains("old.stasis"));
    assert!(!updated.contains("    import"));

    let run = stasis(&["--json", "run", "--headless"], &project);
    assert_eq!(run.status.code(), Some(9));
    assert_eq!(json_stdout(&run)["result"]["exit_code"], 9);

    let reverted = stasis(
        &["--json", "symbol", "revert", "--receipt", &receipt],
        &project,
    );
    assert_eq!(
        reverted.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&reverted.stderr)
    );
    assert_eq!(json_stdout(&reverted)["result"]["status"], "reverted");
    let restored = fs::read_to_string(project.join("src/main.stasis")).expect("restored source");
    assert!(restored.contains("old.stasis"));
    assert!(restored.contains("old_value"));

    fs::create_dir_all(project.join("build")).expect("create build");
    fs::remove_dir_all(project.join("build/semantic-edits")).expect("remove receipt directory");
    fs::write(
        project.join("build/semantic-edits"),
        "blocks receipt directory",
    )
    .expect("block receipt directory");
    let receipt_failure = stasis(
        &[
            "--json",
            "symbol",
            "update",
            "tick",
            "--kind",
            "function",
            "--file",
            "src/main.stasis",
            "--source-file",
            "edits/tick.stasis",
        ],
        &project,
    );
    assert_eq!(receipt_failure.status.code(), Some(1));
    assert!(json_stderr(&receipt_failure)["message"]
        .as_str()
        .unwrap_or_default()
        .contains("rolled back"));
    assert_eq!(
        fs::read_to_string(project.join("src/main.stasis")).expect("source after receipt failure"),
        restored
    );
    fs::remove_file(project.join("build/semantic-edits")).expect("remove receipt blocker");

    let globals = stasis(
        &[
            "--json",
            "symbol",
            "update",
            "globals",
            "--kind",
            "globals",
            "--file",
            "src/main.stasis",
            "--source-file",
            "edits/globals.stasis",
        ],
        &project,
    );
    assert_eq!(globals.status.code(), Some(0));
    let globals_receipt = json_stdout(&globals)["result"]["receipt"]
        .as_str()
        .expect("globals receipt")
        .to_string();
    assert!(fs::read_to_string(project.join("src/main.stasis"))
        .expect("constant source")
        .contains("LIMIT: i32 = 4"));

    let structure = stasis(
        &[
            "--json",
            "symbol",
            "update",
            "Config",
            "--kind",
            "struct",
            "--file",
            "src/main.stasis",
            "--source-file",
            "edits/config.stasis",
        ],
        &project,
    );
    assert_eq!(structure.status.code(), Some(0));
    let structure_receipt = json_stdout(&structure)["result"]["receipt"]
        .as_str()
        .expect("struct receipt")
        .to_string();
    assert!(fs::read_to_string(project.join("src/main.stasis"))
        .expect("struct source")
        .contains("height: i32"));

    assert_eq!(
        stasis(
            &[
                "--json",
                "symbol",
                "revert",
                "--receipt",
                &structure_receipt
            ],
            &project,
        )
        .status
        .code(),
        Some(0)
    );
    assert_eq!(
        stasis(
            &["--json", "symbol", "revert", "--receipt", &globals_receipt],
            &project,
        )
        .status
        .code(),
        Some(0)
    );

    fs::write(
        project.join("edits/bad_tick.stasis"),
        "function tick(): i32 { return missing_symbol(); }\n",
    )
    .expect("write invalid edit");
    let invalid = stasis(
        &[
            "--json",
            "symbol",
            "update",
            "tick",
            "--kind",
            "function",
            "--file",
            "src/main.stasis",
            "--source-file",
            "edits/bad_tick.stasis",
        ],
        &project,
    );
    assert_eq!(invalid.status.code(), Some(1));
    assert!(json_stderr(&invalid)["message"]
        .as_str()
        .unwrap_or_default()
        .contains("missing_symbol"));
    assert_eq!(
        fs::read_to_string(project.join("src/main.stasis")).expect("source after invalid edit"),
        restored
    );

    fs::write(
        project.join("edits/failing_test.stasis"),
        "test `new project is ready`(): bool {\n    return false;\n}\n",
    )
    .expect("write failing test edit");
    let original_test =
        fs::read_to_string(project.join("tests/main.test.stasis")).expect("read original test");
    let failing_test = stasis(
        &[
            "--json",
            "symbol",
            "update",
            "new project is ready",
            "--kind",
            "test",
            "--file",
            "tests/main.test.stasis",
            "--source-file",
            "edits/failing_test.stasis",
        ],
        &project,
    );
    assert_eq!(failing_test.status.code(), Some(1));
    assert!(json_stderr(&failing_test)["message"]
        .as_str()
        .unwrap_or_default()
        .contains("rolled back"));
    assert_eq!(
        fs::read_to_string(project.join("tests/main.test.stasis")).expect("test after rollback"),
        original_test
    );

    fs::remove_dir_all(&parent).ok();
}
