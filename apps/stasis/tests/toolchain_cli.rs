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

#[test]
fn package_mobile_builds_android_and_ios_projects_from_one_entry() {
    let parent = temp_dir("mobile_package");
    fs::create_dir_all(&parent).expect("create temp parent");
    let project = parent.join("mobile_game");
    let created = stasis(&["new", "mobile_game", "--dir", "mobile_game"], &parent);
    assert_eq!(created.status.code(), Some(0));
    fs::write(
        project.join("src/main.stasis"),
        "function main(): i32 { return 0; }\nfunction tick(): i32 { return 0; }\nfunction render(): i32 { return 0; }\n",
    )
    .expect("write mobile entry");
    fs::create_dir_all(project.join("assets")).expect("create assets");
    fs::write(
        project.join("assets/manifest.json"),
        "{\n  \"schema\": \"stasis-assets\",\n  \"version\": 1,\n  \"assets\": []\n}\n",
    )
    .expect("write asset manifest");

    for (target, output) in [("android-arm64", "android"), ("ios-arm64", "ios")] {
        let packaged = stasis(
            &[
                "package-mobile",
                "--target",
                target,
                "--entry",
                "src/main.stasis",
                "--out",
                output,
            ],
            &project,
        );
        assert_eq!(
            packaged.status.code(),
            Some(0),
            "stdout={} stderr={}",
            String::from_utf8_lossy(&packaged.stdout),
            String::from_utf8_lossy(&packaged.stderr)
        );
        assert!(project
            .join(output)
            .join("stasis_mobile_package.json")
            .is_file());
        assert!(project
            .join(output)
            .join("aot/mobile_aot_bundle_manifest.json")
            .is_file());
    }
    assert!(project
        .join("android/android/app/src/main/cpp/CMakeLists.txt")
        .is_file());
    assert!(project
        .join("android/android/app/src/main/assets/stasis_game/assets/manifest.json")
        .is_file());
    assert!(project
        .join("ios/ios/StasisMobile.xcodeproj/project.pbxproj")
        .is_file());
    assert!(project
        .join("ios/ios/StasisMobile/stasis_game/assets/manifest.json")
        .is_file());
    assert!(!walk_files(&project.join("android"))
        .iter()
        .any(|path| path.extension().and_then(|value| value.to_str()) == Some("stasis")));
    assert!(!walk_files(&project.join("ios"))
        .iter()
        .any(|path| path.extension().and_then(|value| value.to_str()) == Some("stasis")));

    fs::write(project.join("src/main.stasis"), "function main(: i32 {\n")
        .expect("write invalid mobile entry");
    let failed = stasis(
        &[
            "package-mobile",
            "--target",
            "android-arm64",
            "--out",
            "broken",
        ],
        &project,
    );
    assert_eq!(failed.status.code(), Some(1));
    assert!(!project.join("broken").exists());
    assert!(!project.join(".broken.staging").exists());

    fs::remove_dir_all(&parent).ok();
}

#[test]
fn semantic_symbol_cli_supports_inline_crud_and_stale_guards() {
    let parent = temp_dir("semantic_inline");
    fs::create_dir_all(&parent).expect("create temp parent");
    let project = parent.join("demo");
    let created = stasis(&["new", "demo", "--dir", "demo"], &parent);
    assert_eq!(created.status.code(), Some(0));

    let added = stasis(
        &[
            "--json",
            "symbol",
            "add",
            "helper",
            "--kind",
            "function",
            "--file",
            "src/main.stasis",
            "--source",
            "// Inline helper.\nfunction helper(): i32 { return 4; }",
        ],
        &project,
    );
    assert_eq!(
        added.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&added.stderr)
    );

    let found = stasis(
        &["--json", "symbol", "find", "helper", "--kind", "function"],
        &project,
    );
    assert_eq!(found.status.code(), Some(0));
    assert_eq!(
        json_stdout(&found)["result"]["matches"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let normalized_list = stasis(
        &[
            "--json",
            "symbol",
            "list",
            "--kind",
            "function",
            "--file",
            ".\\src\\main.stasis",
        ],
        &project,
    );
    assert_eq!(normalized_list.status.code(), Some(0));
    assert!(json_stdout(&normalized_list)["result"]["items"]
        .as_array()
        .expect("normalized items")
        .iter()
        .any(|item| item["name"] == "helper"));

    let read = stasis(
        &[
            "--json",
            "symbol",
            "read",
            "helper",
            "--kind",
            "function",
            "--file",
            "src/main.stasis",
        ],
        &project,
    );
    assert_eq!(read.status.code(), Some(0));
    let read_json = json_stdout(&read);
    assert!(read_json["result"]["item"]["source"]
        .as_str()
        .unwrap()
        .starts_with("// Inline helper."));
    let original_hash = read_json["result"]["item"]["source_hash"]
        .as_str()
        .expect("source hash")
        .to_string();

    let preview = stasis(
        &[
            "--json",
            "symbol",
            "update",
            "helper",
            "--kind",
            "function",
            "--file",
            "src/main.stasis",
            "--source",
            "function helper(): i32 {\n    return 5;\n}",
            "--expected-source-hash",
            &original_hash,
            "--dry-run",
        ],
        &project,
    );
    assert_eq!(preview.status.code(), Some(0));
    assert_eq!(json_stdout(&preview)["result"]["status"], "preview");
    assert!(fs::read_to_string(project.join("src/main.stasis"))
        .expect("preview source")
        .contains("return 4;"));

    fs::create_dir_all(project.join("edits")).expect("create edits");
    fs::write(
        project.join("edits/helper.stasis"),
        "function helper(): i32 { return 6; }\n",
    )
    .expect("write helper source");
    let conflicting_inputs = stasis(
        &[
            "--json",
            "symbol",
            "update",
            "helper",
            "--kind",
            "function",
            "--file",
            "src/main.stasis",
            "--source",
            "function helper(): i32 { return 5; }",
            "--source-file",
            "edits/helper.stasis",
        ],
        &project,
    );
    assert_eq!(conflicting_inputs.status.code(), Some(2));
    assert_eq!(json_stderr(&conflicting_inputs)["code"], "usage_error");

    let stale = stasis(
        &[
            "--json",
            "symbol",
            "update",
            "helper",
            "--kind",
            "function",
            "--file",
            "src/main.stasis",
            "--source",
            "function helper(): i32 { return 5; }",
            "--expected-source-hash",
            "0000000000000000000000000000000000000000000000000000000000000000",
        ],
        &project,
    );
    assert_eq!(stale.status.code(), Some(1));
    assert!(json_stderr(&stale)["message"]
        .as_str()
        .unwrap_or_default()
        .contains("stale semantic edit target"));

    let updated = stasis(
        &[
            "--json",
            "symbol",
            "update",
            "helper",
            "--kind",
            "function",
            "--file",
            "src/main.stasis",
            "--source",
            "function helper(): i32 { return 5; }",
            "--expected-source-hash",
            &original_hash,
        ],
        &project,
    );
    assert_eq!(updated.status.code(), Some(0));

    let updated_read = stasis(
        &["--json", "symbol", "read", "helper", "--kind", "function"],
        &project,
    );
    let updated_json = json_stdout(&updated_read);
    let updated_hash = updated_json["result"]["item"]["source_hash"]
        .as_str()
        .expect("updated hash")
        .to_string();
    assert!(updated_json["result"]["item"]["source"]
        .as_str()
        .unwrap()
        .contains("return 5;"));

    let deleted = stasis(
        &[
            "--json",
            "symbol",
            "delete",
            "helper",
            "--kind",
            "function",
            "--file",
            "src/main.stasis",
            "--expected-source-hash",
            &updated_hash,
        ],
        &project,
    );
    assert_eq!(deleted.status.code(), Some(0));
    let delete_receipt = json_stdout(&deleted)["result"]["receipt"]
        .as_str()
        .expect("delete receipt")
        .to_string();
    assert!(!fs::read_to_string(project.join("src/main.stasis"))
        .expect("deleted source")
        .contains("function helper"));

    let mut future_receipt: Value = serde_json::from_str(
        &fs::read_to_string(project.join(&delete_receipt)).expect("read delete receipt"),
    )
    .expect("parse delete receipt");
    future_receipt["schema_version"] = Value::from(2);
    let future_receipt_path = project.join("future-receipt.json");
    fs::write(
        &future_receipt_path,
        serde_json::to_string(&future_receipt).expect("serialize future receipt"),
    )
    .expect("write future receipt");
    let unsupported = stasis(
        &[
            "--json",
            "symbol",
            "revert",
            "--receipt",
            "future-receipt.json",
        ],
        &project,
    );
    assert_eq!(unsupported.status.code(), Some(1));
    assert!(json_stderr(&unsupported)["message"]
        .as_str()
        .unwrap_or_default()
        .contains("unsupported semantic edit receipt schema version 2"));
    assert!(!fs::read_to_string(project.join("src/main.stasis"))
        .expect("source after unsupported receipt")
        .contains("function helper"));

    let reverted = stasis(
        &["--json", "symbol", "revert", "--receipt", &delete_receipt],
        &project,
    );
    assert_eq!(reverted.status.code(), Some(0));
    assert!(fs::read_to_string(project.join("src/main.stasis"))
        .expect("reverted source")
        .contains("return 5;"));
    fs::remove_dir_all(parent).ok();
}

#[test]
fn semantic_symbol_cli_batch_apply_is_atomic_and_revertible() {
    let parent = temp_dir("semantic_batch");
    fs::create_dir_all(&parent).expect("create temp parent");
    let project = parent.join("demo");
    assert_eq!(
        stasis(&["new", "demo", "--dir", "demo"], &parent)
            .status
            .code(),
        Some(0)
    );
    let original_main = "import \"helper.stasis\";\nfunction main(): i32 { return helper(); }\n";
    let original_helper = "function helper(): i32 { return 1; }\n";
    fs::write(project.join("src/main.stasis"), original_main).expect("write main");
    fs::write(project.join("src/helper.stasis"), original_helper).expect("write helper");
    fs::create_dir_all(project.join("edits")).expect("create edits");
    let request = serde_json::json!({
        "schema_version": 1,
        "edits": [
            {
                "operation": "update",
                "target": {
                    "kind": "function",
                    "file": "src/main.stasis",
                    "name": "main"
                },
                "new_source": "// Batch main.\nfunction main(): i32 { return helper(); }"
            },
            {
                "operation": "update",
                "target": {
                    "kind": "function",
                    "file": "src/helper.stasis",
                    "name": "helper"
                },
                "new_source": "function helper(): i32 { return 2; }"
            }
        ]
    });
    fs::write(
        project.join("edits/batch.json"),
        serde_json::to_vec_pretty(&request).expect("serialize request"),
    )
    .expect("write request");

    let preview = stasis(
        &[
            "--json",
            "symbol",
            "apply",
            "--request",
            "edits/batch.json",
            "--dry-run",
            "--no-tests",
        ],
        &project,
    );
    assert_eq!(preview.status.code(), Some(0));
    let preview_json = json_stdout(&preview);
    assert_eq!(preview_json["result"]["status"], "preview");
    assert_eq!(
        preview_json["result"]["plan"]["changed_files"]
            .as_array()
            .expect("changed files")
            .len(),
        2
    );
    assert_eq!(
        fs::read_to_string(project.join("src/main.stasis")).expect("preview main"),
        original_main
    );
    assert_eq!(
        fs::read_to_string(project.join("src/helper.stasis")).expect("preview helper"),
        original_helper
    );

    let applied = stasis(
        &[
            "--json",
            "symbol",
            "apply",
            "--request",
            "edits/batch.json",
            "--no-tests",
        ],
        &project,
    );
    assert_eq!(applied.status.code(), Some(0));
    let receipt = json_stdout(&applied)["result"]["receipt"]
        .as_str()
        .expect("receipt")
        .to_string();
    assert!(fs::read_to_string(project.join("src/main.stasis"))
        .expect("applied main")
        .starts_with("import \"helper.stasis\";\n// Batch main."));
    assert!(fs::read_to_string(project.join("src/helper.stasis"))
        .expect("applied helper")
        .contains("return 2;"));

    let reverted = stasis(
        &[
            "--json",
            "symbol",
            "revert",
            "--receipt",
            &receipt,
            "--no-tests",
        ],
        &project,
    );
    assert_eq!(reverted.status.code(), Some(0));
    assert_eq!(
        fs::read_to_string(project.join("src/main.stasis")).expect("reverted main"),
        original_main
    );
    assert_eq!(
        fs::read_to_string(project.join("src/helper.stasis")).expect("reverted helper"),
        original_helper
    );

    let invalid_request = serde_json::json!({
        "schema_version": 1,
        "edits": [
            {
                "operation": "update",
                "target": {"kind": "function", "file": "src/main.stasis", "name": "main"},
                "new_source": "function main(): i32 { return 9; }"
            },
            {
                "operation": "update",
                "target": {"kind": "function", "file": "src/helper.stasis", "name": "missing"},
                "new_source": "function missing(): i32 { return 9; }"
            }
        ]
    });
    fs::write(
        project.join("edits/invalid-batch.json"),
        serde_json::to_vec_pretty(&invalid_request).expect("serialize invalid request"),
    )
    .expect("write invalid request");
    let invalid = stasis(
        &[
            "--json",
            "symbol",
            "apply",
            "--request",
            "edits/invalid-batch.json",
            "--no-tests",
        ],
        &project,
    );
    assert_eq!(invalid.status.code(), Some(1));
    assert_eq!(
        fs::read_to_string(project.join("src/main.stasis")).expect("atomic main"),
        original_main
    );
    assert_eq!(
        fs::read_to_string(project.join("src/helper.stasis")).expect("atomic helper"),
        original_helper
    );
    fs::remove_dir_all(parent).ok();
}

#[test]
fn semantic_symbol_cli_reapplies_edit_when_revert_tests_fail() {
    let parent = temp_dir("semantic_revert_failure");
    fs::create_dir_all(&parent).expect("create temp parent");
    let project = parent.join("demo");
    assert_eq!(
        stasis(&["new", "demo", "--dir", "demo"], &parent)
            .status
            .code(),
        Some(0)
    );
    let rejected_source = "function main(): i32 { return 1; }\n";
    let accepted_source = "function main(): i32 { return 0; }\n";
    fs::write(project.join("src/main.stasis"), rejected_source).expect("write rejected source");
    fs::write(
        project.join("tests/main.test.stasis"),
        "import \"../src/main.stasis\";\ntest `main remains zero`(): bool { return main() == 0; }\n",
    )
    .expect("write behavioral test");

    let applied = stasis(
        &[
            "--json",
            "symbol",
            "update",
            "main",
            "--kind",
            "function",
            "--file",
            "src/main.stasis",
            "--source",
            "function main(): i32 { return 0; }",
        ],
        &project,
    );
    assert_eq!(
        applied.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&applied.stderr)
    );
    let receipt = json_stdout(&applied)["result"]["receipt"]
        .as_str()
        .expect("receipt")
        .to_string();
    assert_eq!(
        fs::read_to_string(project.join("src/main.stasis")).expect("accepted source"),
        accepted_source
    );

    let reverted = stasis(
        &["--json", "symbol", "revert", "--receipt", &receipt],
        &project,
    );
    assert_eq!(reverted.status.code(), Some(1));
    assert!(json_stderr(&reverted)["message"]
        .as_str()
        .unwrap_or_default()
        .contains("edited sources were reapplied"));
    assert_eq!(
        fs::read_to_string(project.join("src/main.stasis")).expect("reapplied source"),
        accepted_source
    );
    fs::remove_dir_all(parent).ok();
}

fn walk_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).expect("read directory") {
            let path = entry.expect("read entry").path();
            if path.is_dir() {
                pending.push(path);
            } else {
                files.push(path);
            }
        }
    }
    files
}
