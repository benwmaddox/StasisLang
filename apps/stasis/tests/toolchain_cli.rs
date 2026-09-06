use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

fn temp_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "stasis_cli_integration_{name}_{}_{}",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::SeqCst)
    ))
}

fn crlf(source: &str) -> String {
    source.replace('\n', "\r\n")
}

fn stasis(args: &[&str], cwd: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_stasis"))
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("run stasis CLI")
}

fn stasis_with_stdin(args: &[&str], cwd: &Path, input: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_stasis"))
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start stasis CLI");
    child
        .stdin
        .take()
        .expect("stasis stdin")
        .write_all(input.as_bytes())
        .expect("write stasis stdin");
    child.wait_with_output().expect("wait for stasis CLI")
}

fn git(args: &[&str], cwd: &Path) -> Output {
    Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("run git")
}

fn git_with_stasis_on_path(args: &[&str], cwd: &Path) -> Output {
    let stasis_executable = Path::new(env!("CARGO_BIN_EXE_stasis"));
    let stasis_directory = stasis_executable.parent().expect("stasis binary directory");
    let path = std::env::join_paths(
        std::iter::once(stasis_directory.to_path_buf()).chain(
            std::env::var_os("PATH")
                .as_deref()
                .map(std::env::split_paths)
                .into_iter()
                .flatten(),
        ),
    )
    .expect("compose test PATH");
    Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("PATH", path)
        .output()
        .expect("run git with stasis on PATH")
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

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("create copied tree directory");
    for entry in fs::read_dir(source).expect("read copied tree directory") {
        let entry = entry.expect("read copied tree entry");
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if source_path.is_dir() {
            copy_tree(&source_path, &destination_path);
        } else {
            fs::copy(&source_path, &destination_path).expect("copy tree file");
        }
    }
}

#[test]
fn help_explains_how_to_build_each_supported_target() {
    let output = stasis(&["help"], Path::new(env!("CARGO_MANIFEST_DIR")));
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let help = String::from_utf8(output.stdout).expect("UTF-8 help output");

    for expected in [
        "Build targets:",
        "Windows, Linux, or macOS (current host)",
        "stasis build --mode release",
        "stasis package --target desktop",
        "stasis package --target web",
        "stasis package-mobile --target android-arm64",
        "stasis package-mobile --target android-x86_64 --development-build",
        "stasis package-mobile --target ios-arm64",
        "prepare",
        "Mobile commands create Gradle or Xcode projects",
    ] {
        assert!(help.contains(expected), "missing help text: {expected}");
    }
}

#[test]
fn build_confirmation_reports_elapsed_time() {
    let project = temp_dir("build_elapsed");
    fs::create_dir_all(project.join("src")).expect("source directory");
    fs::write(
        project.join("stasis.json"),
        r#"{"manifest_version":1,"name":"build_elapsed","entry":"src/main.stasis","tests":"tests","output":"build"}"#,
    )
    .expect("workspace manifest");
    fs::write(
        project.join("src/main.stasis"),
        "function main(): i32 { return 0; }\n",
    )
    .expect("entry source");

    let output = stasis(&["build", "--mode", "dev"], &project);
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 build output");
    let confirmation = stdout.lines().last().expect("elapsed confirmation");
    assert!(confirmation.starts_with("Completed in "), "{stdout}");
    assert!(confirmation.ends_with('.'), "{stdout}");

    let json_output = stasis(&["--json", "build", "--mode", "dev"], &project);
    let json = json_stdout(&json_output);
    assert_eq!(json["command"], "build");
    assert!(json["result"].get("elapsed_ms").is_none());

    fs::remove_dir_all(project).ok();
}

#[test]
fn check_reports_structured_asset_diagnostics_and_build_is_atomic() {
    let project = temp_dir("asset_validation");
    fs::create_dir_all(project.join("src")).expect("source directory");
    fs::create_dir_all(project.join("assets/fonts")).expect("asset directory");
    fs::write(
        project.join("stasis.json"),
        r#"{"manifest_version":1,"name":"asset_validation","entry":"src/main.stasis","tests":"tests","output":"build"}"#,
    )
    .expect("workspace manifest");
    fs::write(project.join("assets/fonts/ui.ttf"), b"font").expect("font asset");
    fs::write(
        project.join("src/main.stasis"),
        concat!(
            "function @asset_path(path) request_font(path: string, size: i32): i32 { return 1; }\n",
            "function main(): i32 { return request_font(\"../assets/fonts/UI.ttf\", 16); }\n"
        ),
    )
    .expect("entry source");

    let checked = stasis(&["--json", "check"], &project);
    assert_eq!(checked.status.code(), Some(1));
    let error = json_stderr(&checked);
    assert_eq!(error["code"], "asset_validation_failed");
    assert_eq!(error["diagnostics"][0]["code"], "asset_path_case_mismatch");
    assert_eq!(error["diagnostics"][0]["api"], "request_font");
    assert_eq!(
        error["diagnostics"][0]["logical_path"],
        "../assets/fonts/UI.ttf"
    );
    assert!(error["diagnostics"][0]["start"].as_u64().is_some());
    assert!(error["diagnostics"][0]["attempted_paths"]
        .as_array()
        .is_some_and(|paths| paths.len() == 2));

    let built = stasis(&["--json", "build", "--mode", "dev"], &project);
    assert_eq!(built.status.code(), Some(1));
    assert!(!project.join("build/dev-build.json").exists());

    fs::write(
        project.join("src/main.stasis"),
        concat!(
            "function @asset_path(path) request_font(path: string, size: i32): i32 { return 1; }\n",
            "function main(): i32 { return request_font(\"../assets/fonts/ui.ttf\", 16); }\n"
        ),
    )
    .expect("corrected entry source");
    let checked = stasis(&["--json", "check"], &project);
    assert_eq!(checked.status.code(), Some(0));
    assert_eq!(json_stdout(&checked)["result"]["name"], "asset_validation");

    fs::write(
        project.join("src/main.stasis"),
        concat!(
            "function @asset_path(path) request_font(path: string, size: i32): i32 { return 1; }\n",
            "const FONT_PATH: string = \"../assets/fonts/ui.ttf\";\n",
            "function choose_font(FONT_PATH: string): i32 { return request_font(FONT_PATH, 16); }\n",
            "function main(): i32 { return choose_font(\"../assets/fonts/ui.ttf\"); }\n"
        ),
    )
    .expect("shadowed asset source");
    let checked = stasis(&["--json", "check"], &project);
    assert_eq!(checked.status.code(), Some(1));
    let error = json_stderr(&checked);
    assert_eq!(error["code"], "asset_validation_failed");
    assert_eq!(
        error["diagnostics"][0]["code"],
        "asset_dynamic_path_undeclared"
    );
    assert_eq!(error["diagnostics"][0]["logical_path"], Value::Null);

    fs::create_dir_all(project.join("tests")).expect("test directory");
    fs::write(
        project.join("tests/assets.test.stasis"),
        concat!(
            "function @asset_path(path) request_font(path: string, size: i32): i32 { return 1; }\n",
            "test `invalid asset is never executed`(): bool {\n",
            "    return request_font(\"../assets/fonts/missing.ttf\", 16) > 0;\n",
            "}\n"
        ),
    )
    .expect("asset test source");
    let tested = stasis(&["--json", "test"], &project);
    assert_eq!(tested.status.code(), Some(1));
    assert_eq!(json_stderr(&tested)["code"], "asset_validation_failed");

    fs::remove_dir_all(project).ok();
}

#[test]
fn check_resolves_nested_asset_calls_from_the_entry_source_boundary() {
    let project = temp_dir("nested_entry_asset_boundary");
    fs::create_dir_all(project.join("src/game/view")).expect("nested source directory");
    fs::create_dir_all(project.join("assets/fonts")).expect("asset directory");
    fs::write(
        project.join("stasis.json"),
        r#"{"manifest_version":1,"name":"nested_entry_asset_boundary","entry":"src/main.stasis","tests":"tests","output":"build"}"#,
    )
    .expect("workspace manifest");
    fs::write(project.join("assets/fonts/ui.ttf"), b"font").expect("font asset");
    fs::write(
        project.join("src/main.stasis"),
        concat!(
            "import \"game/view/assets.stasis\";\n",
            "function main(): i32 { return load_ui_font(); }\n",
        ),
    )
    .expect("entry source");
    fs::write(
        project.join("src/game/view/assets.stasis"),
        concat!(
            "function @asset_path(path) request_font(path: string, size: i32): i32 { return 1; }\n",
            "function load_ui_font(): i32 { return request_font(\"../assets/fonts/ui.ttf\", 16); }\n",
        ),
    )
    .expect("nested asset source");

    let checked = stasis(&["--json", "check"], &project);
    assert_eq!(
        checked.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&checked.stderr)
    );
    assert_eq!(
        json_stdout(&checked)["result"]["name"],
        "nested_entry_asset_boundary"
    );

    fs::remove_dir_all(project).ok();
}

fn lsp_frame(message: Value) -> String {
    let body = message.to_string();
    format!("Content-Length: {}\r\n\r\n{body}", body.len())
}

fn lsp_messages(bytes: &[u8]) -> Vec<Value> {
    let mut remaining = bytes;
    let mut messages = Vec::new();
    while !remaining.is_empty() {
        let header_end = remaining
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .expect("LSP header terminator");
        let header = std::str::from_utf8(&remaining[..header_end]).expect("LSP header UTF-8");
        let length = header
            .lines()
            .find_map(|line| line.strip_prefix("Content-Length: "))
            .expect("Content-Length header")
            .parse::<usize>()
            .expect("Content-Length value");
        let body_start = header_end + 4;
        let body_end = body_start + length;
        messages.push(
            serde_json::from_slice(&remaining[body_start..body_end]).expect("LSP JSON message"),
        );
        remaining = &remaining[body_end..];
    }
    messages
}

fn file_uri(path: &Path) -> String {
    let path = path.to_string_lossy().replace('\\', "/");
    if path.starts_with('/') {
        format!("file://{path}")
    } else {
        format!("file:///{path}")
    }
}

#[test]
fn lsp_stdio_publishes_and_clears_compiler_diagnostics() {
    let project = temp_dir("lsp_diagnostics");
    fs::create_dir_all(project.join("src")).expect("create LSP fixture");
    fs::write(
        project.join("stasis.json"),
        r#"{"manifest_version":1,"name":"lsp_fixture","entry":"src/main.stasis","tests":"tests","output":"build"}"#,
    )
    .expect("write LSP manifest");
    let source_path = project.join("src/main.stasis");
    fs::write(&source_path, "function main(): i32 { return 0; }\n").expect("write LSP source");
    let stale_cache = project.join(".stasis_cache/toolchain/stale.bin");
    fs::create_dir_all(stale_cache.parent().expect("stale cache parent"))
        .expect("create stale cache fixture");
    fs::write(&stale_cache, "stale").expect("write stale cache fixture");
    let stale_time = SystemTime::UNIX_EPOCH + Duration::from_secs(1);
    fs::File::options()
        .write(true)
        .open(&stale_cache)
        .expect("open stale cache fixture")
        .set_times(
            fs::FileTimes::new()
                .set_accessed(stale_time)
                .set_modified(stale_time),
        )
        .expect("age stale cache fixture");
    let uri = file_uri(&source_path);
    let fixed_source = "// Adds two values.\nfunction add_score(amount: i32, bonus: i32): i32 { return amount + bonus; }\nfunction main(): i32 { return add_score(1, 2); }\n";
    let input = [
        lsp_frame(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "capabilities": {} }
        })),
        lsp_frame(json!({
            "jsonrpc": "2.0",
            "method": "initialized",
            "params": {}
        })),
        lsp_frame(json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {
                "textDocument": {
                    "uri": uri.clone(),
                    "languageId": "stasis",
                    "version": 1,
                    "text": "function main(): i32 { return 0; }\nfunction broken(): i32 { while (true) { return 1; } }\n"
                }
            }
        })),
        lsp_frame(json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didChange",
            "params": {
                "textDocument": { "uri": uri.clone(), "version": 2 },
                "contentChanges": [{
                    "text": fixed_source
                }]
            }
        })),
        lsp_frame(json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "textDocument/completion",
            "params": {
                "textDocument": { "uri": uri.clone() },
                "position": { "line": 2, "character": 35 }
            }
        })),
        lsp_frame(json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "textDocument/hover",
            "params": {
                "textDocument": { "uri": uri.clone() },
                "position": { "line": 2, "character": 32 }
            }
        })),
        lsp_frame(json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "textDocument/signatureHelp",
            "params": {
                "textDocument": { "uri": uri.clone() },
                "position": { "line": 2, "character": 43 }
            }
        })),
        lsp_frame(json!({
            "jsonrpc": "2.0",
            "id": 6,
            "method": "textDocument/inlayHint",
            "params": {
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 0, "character": 0 },
                    "end": { "line": 3, "character": 0 }
                }
            }
        })),
        lsp_frame(json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "textDocument/definition",
            "params": {
                "textDocument": { "uri": uri },
                "position": { "line": 0, "character": 0 }
            }
        })),
        lsp_frame(json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "shutdown",
            "params": null
        })),
        lsp_frame(json!({
            "jsonrpc": "2.0",
            "method": "exit",
            "params": null
        })),
    ]
    .concat();

    let output = stasis_with_stdin(&["lsp", "--stdio"], &project, &input);
    assert_eq!(
        output.status.code(),
        Some(0),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let messages = lsp_messages(&output.stdout);
    assert!(String::from_utf8_lossy(&output.stderr).contains("cache_cleanup_removed_files=1"));
    assert_eq!(messages[0]["id"], 1);
    assert_eq!(
        messages[0]["result"]["capabilities"]["positionEncoding"],
        "utf-16"
    );
    assert_eq!(messages[0]["result"]["capabilities"]["hoverProvider"], true);
    assert!(messages[0]["result"]["capabilities"]["completionProvider"].is_object());
    assert!(messages[0]["result"]["capabilities"]["signatureHelpProvider"].is_object());
    assert!(messages[0]["result"]["capabilities"]["inlayHintProvider"].is_object());
    let diagnostics = messages
        .iter()
        .filter(|message| message["method"] == "textDocument/publishDiagnostics")
        .collect::<Vec<_>>();
    assert_eq!(diagnostics.len(), 2);
    assert_eq!(diagnostics[0]["params"]["version"], 1);
    assert_eq!(diagnostics[0]["params"]["diagnostics"][0]["severity"], 1);
    assert!(diagnostics[0]["params"]["diagnostics"][0]["message"]
        .as_str()
        .unwrap_or_default()
        .contains("while"));
    assert_eq!(diagnostics[1]["params"]["version"], 2);
    assert_eq!(diagnostics[1]["params"]["diagnostics"], json!([]));
    let completion = messages
        .iter()
        .find(|message| message["id"] == 3)
        .expect("completion response");
    assert!(completion["result"]["items"]
        .as_array()
        .is_some_and(|items| items.iter().any(|item| item["label"] == "add_score")));
    let hover = messages
        .iter()
        .find(|message| message["id"] == 4)
        .expect("hover response");
    assert!(hover["result"]["contents"]["value"]
        .as_str()
        .unwrap_or_default()
        .contains("Adds two values"));
    let signature = messages
        .iter()
        .find(|message| message["id"] == 5)
        .expect("signature response");
    assert_eq!(signature["result"]["activeParameter"], 1);
    assert_eq!(
        signature["result"]["signatures"][0]["label"],
        "add_score(amount: i32, bonus: i32): i32"
    );
    let inlays = messages
        .iter()
        .find(|message| message["id"] == 6)
        .expect("inlay-hint response");
    assert!(inlays["result"].as_array().is_some_and(|hints| {
        hints.iter().any(|hint| hint["label"] == "amount:")
            && hints.iter().any(|hint| hint["label"] == "bonus:")
    }));
    let definition_miss = messages
        .iter()
        .find(|message| message["id"] == 7)
        .expect("definition response");
    assert_eq!(definition_miss["result"], Value::Null);
    assert!(definition_miss.get("error").is_none());
    assert!(messages
        .iter()
        .any(|message| message["id"] == 2 && message["result"].is_null()));
    fs::remove_dir_all(project).ok();
}

fn assert_generated_knowledge(project: &Path) {
    let knowledge_source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/knowledge");
    let knowledge_documents = [
        "README.md",
        "a-little-stasis/01-three-entry-points.md",
        "a-little-stasis/02-state-has-owners.md",
        "a-little-stasis/03-a-tick-is-an-ordered-recipe.md",
        "a-little-stasis/04-input-crosses-a-boundary.md",
        "a-little-stasis/05-bounded-storage-is-policy.md",
        "a-little-stasis/06-query-materialize-commit.md",
        "a-little-stasis/07-test-systems-not-balance-numbers.md",
        "a-little-stasis/08-projection-is-not-authority.md",
        "practical-examples/breakout-remove-one-brick-per-collision.md",
        "practical-examples/platformer-land-in-the-crossing-tick.md",
        "practical-examples/pong-score-after-the-ball-crosses-the-goal.md",
        "practical-examples/snake-reject-a-reverse-turn.md",
        "geometry-and-collision.md",
        "loading-screens.md",
        "semantic-edit-and-validation.md",
    ];
    for document in knowledge_documents.iter().copied() {
        assert_eq!(
            fs::read(project.join("vendor/stasis/docs").join(document))
                .expect("read generated knowledge document"),
            fs::read(knowledge_source.join(document)).expect("read source knowledge document"),
            "generated knowledge document differs: {document}"
        );
    }
    let knowledge_examples = [
        "examples/src/breakout_brick.stasis",
        "examples/src/game_patterns.stasis",
        "examples/src/platformer_landing.stasis",
        "examples/src/pong_goal.stasis",
        "examples/src/snake_turn.stasis",
        "examples/src/loading_screen.stasis",
        "examples/assets/hero.svg",
        "examples/assets/music.wav",
        "media/loading-screen/success.mp4",
        "media/loading-screen/failure.mp4",
        "media/loading-screen/loading.png",
        "media/loading-screen/progress.png",
        "media/loading-screen/gameplay.png",
        "media/loading-screen/error.png",
        "examples/stasis.json",
        "examples/tests/breakout_brick.test.stasis",
        "examples/tests/game_patterns.test.stasis",
        "examples/tests/platformer_landing.test.stasis",
        "examples/tests/pong_goal.test.stasis",
        "examples/tests/snake_turn.test.stasis",
        "examples/tests/loading_screen.test.stasis",
    ];
    for example in knowledge_examples.iter().copied() {
        assert_eq!(
            fs::read(project.join("vendor/stasis/docs").join(example))
                .expect("read generated knowledge example"),
            fs::read(knowledge_source.join(example)).expect("read source knowledge example"),
            "generated knowledge example differs: {example}"
        );
    }

    let compiled_examples = knowledge_examples
        .iter()
        .filter(|path| path.ends_with(".stasis"))
        .map(|path| {
            fs::read_to_string(knowledge_source.join(path))
                .expect("read knowledge example Stasis source")
        })
        .collect::<Vec<_>>()
        .join("\n")
        .replace("\r\n", "\n");
    let mut checked_stasis_blocks = 0;
    for document in knowledge_documents.iter().copied() {
        let path = knowledge_source.join(document);
        let markdown = fs::read_to_string(&path)
            .expect("read knowledge Markdown")
            .replace("\r\n", "\n");
        let mut remaining = markdown.as_str();
        while let Some(start) = remaining.find("```stasis\n") {
            let block_start = start + "```stasis\n".len();
            let after_start = &remaining[block_start..];
            let end = after_start
                .find("\n```")
                .expect("close Stasis Markdown fence");
            let block = &after_start[..end];
            assert!(
                compiled_examples.contains(block),
                "Stasis block in {} is not an exact compiler-checked excerpt:\n{block}",
                path.display()
            );
            checked_stasis_blocks += 1;
            remaining = &after_start[end + "\n```".len()..];
        }
    }
    assert!(
        checked_stasis_blocks > 0,
        "no Stasis Markdown blocks checked"
    );

    let generated_examples = project.join("vendor/stasis/docs/examples");
    let vendor_before = snapshot_project_bytes(&project.join("vendor/stasis"));
    let runnable_examples = project.join("build/knowledge-examples");
    copy_tree(&generated_examples, &runnable_examples);
    let examples_prepared = stasis(
        &[
            "--json",
            "--workspace",
            "build/knowledge-examples",
            "vendor",
            "update",
        ],
        project,
    );
    assert_eq!(
        examples_prepared.status.code(),
        Some(0),
        "copied knowledge vendor update failed: stdout={} stderr={}",
        String::from_utf8_lossy(&examples_prepared.stdout),
        String::from_utf8_lossy(&examples_prepared.stderr)
    );

    let examples_checked = stasis(
        &["--json", "--workspace", "build/knowledge-examples", "check"],
        project,
    );
    assert_eq!(
        examples_checked.status.code(),
        Some(0),
        "generated knowledge examples check failed: stdout={} stderr={}",
        String::from_utf8_lossy(&examples_checked.stdout),
        String::from_utf8_lossy(&examples_checked.stderr)
    );
    let examples_tested = stasis(
        &["--json", "--workspace", "build/knowledge-examples", "test"],
        project,
    );
    assert_eq!(
        examples_tested.status.code(),
        Some(0),
        "generated knowledge examples test failed: stdout={} stderr={}",
        String::from_utf8_lossy(&examples_tested.stdout),
        String::from_utf8_lossy(&examples_tested.stderr)
    );
    let examples_test_result = json_stdout(&examples_tested);
    assert!(
        examples_test_result["result"]["tests_passed"]
            .as_u64()
            .is_some_and(|count| count > 0),
        "generated knowledge examples discovered no tests"
    );
    assert_eq!(examples_test_result["result"]["tests_failed"], 0);

    assert!(
        !generated_examples.join(".stasis_cache").exists(),
        "running the knowledge examples changed the vendored package"
    );
    assert_eq!(
        snapshot_project_bytes(&project.join("vendor/stasis")),
        vendor_before,
        "running the knowledge examples changed the vendor fingerprint inputs"
    );
    let symbols = stasis(&["--json", "symbol", "list", "--limit", "1"], project);
    assert_eq!(
        symbols.status.code(),
        Some(0),
        "parent read-only symbol query rejected vendor snapshot: stdout={} stderr={}",
        String::from_utf8_lossy(&symbols.stdout),
        String::from_utf8_lossy(&symbols.stderr)
    );
}

#[test]
fn generated_knowledge_examples_compile_and_test() {
    let parent = temp_dir("knowledge_examples");
    fs::create_dir_all(&parent).expect("create temp parent");
    let project = parent.join("demo");

    let created = stasis(&["--json", "new", "demo", "--dir", "demo"], &parent);
    assert_eq!(
        created.status.code(),
        Some(0),
        "generated knowledge project creation failed: stdout={} stderr={}",
        String::from_utf8_lossy(&created.stdout),
        String::from_utf8_lossy(&created.stderr)
    );
    assert_generated_knowledge(&project);

    fs::remove_dir_all(parent).ok();
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
    assert_eq!(created_json["result"]["github_actions"], true);
    let manifest: Value = serde_json::from_slice(
        &fs::read(project.join("stasis.json")).expect("read generated manifest"),
    )
    .expect("parse generated manifest");
    assert_eq!(
        fs::read_to_string(project.join("assets/manifest.json"))
            .expect("read generated asset manifest"),
        "{\n  \"schema\": \"stasis-assets\",\n  \"version\": 1,\n  \"assets\": []\n}\n"
    );
    let asset_manifest: Value = serde_json::from_str(
        &fs::read_to_string(project.join("assets/manifest.json"))
            .expect("read generated asset manifest"),
    )
    .expect("parse generated asset manifest");
    assert_eq!(
        asset_manifest,
        json!({
            "schema": "stasis-assets",
            "version": 1,
            "assets": [],
        })
    );
    assert!(manifest["vendor"]["stasis"].get("update_policy").is_none());
    assert_eq!(
        manifest["vendor"]["stasis"]["sha256"]
            .as_str()
            .map(str::len),
        Some(64)
    );
    let vendor_status = stasis(&["--json", "vendor", "status"], &project);
    assert_eq!(vendor_status.status.code(), Some(0));
    assert_eq!(json_stdout(&vendor_status)["result"]["current"], true);
    let agent_guide = fs::read_to_string(project.join("AGENTS.md")).expect("read agent guide");
    assert!(agent_guide.contains("stasis --json symbol list"));
    assert!(agent_guide.contains("stasis --json symbol references SYMBOL"));
    assert!(agent_guide.contains("stasis validate PATH OP VALUE --frames N"));
    assert!(agent_guide.contains("## Theory-building practice"));
    assert!(agent_guide.contains("Mapping:"));
    assert!(agent_guide.contains("Rationale:"));
    assert!(agent_guide.contains("Extension:"));
    assert!(agent_guide.contains("Read `PROJECT_ARCHITECTURE.md`"));
    let claude_guide = fs::read_to_string(project.join("CLAUDE.md")).expect("read Claude guide");
    assert_eq!(claude_guide, "# CLAUDE.md\n\n@AGENTS.md\n");
    let architecture_guide = fs::read_to_string(project.join("PROJECT_ARCHITECTURE.md"))
        .expect("read project architecture guide");
    assert_eq!(
        architecture_guide,
        include_str!("../../../docs/project_architecture.md")
    );
    assert!(project.join(".git").is_dir());
    assert_eq!(
        String::from_utf8_lossy(
            &git(&["config", "--local", "--get", "core.hooksPath"], &project).stdout
        )
        .trim(),
        ".githooks"
    );
    let pre_commit = fs::read_to_string(project.join(".githooks/pre-commit"))
        .expect("read generated pre-commit hook");
    assert!(pre_commit.contains("stasis format --check"));
    assert_eq!(
        fs::read(project.join(".gitattributes")).expect("read generated Git attributes"),
        b"*.[sS][vV][gG] text eol=lf\n"
    );
    assert_eq!(
        fs::read(project.join(".gitignore")).expect("read generated Git ignore"),
        b"# Track vendor/stasis/stdlib and vendor/stasis/docs together.\n"
    );
    assert_eq!(
        git(
            &[
                "check-ignore",
                "--quiet",
                "--no-index",
                "--",
                "vendor/stasis/docs/README.md",
            ],
            &project
        )
        .status
        .code(),
        Some(1)
    );
    assert!(!git(
        &[
            "check-ignore",
            "--quiet",
            "--no-index",
            "--",
            "vendor/stasis/stdlib/internal/host_frame_raw.stasis",
        ],
        &project
    )
    .status
    .success());
    for path in [
        "assets/example.svg",
        "assets/example.SVG",
        "assets/example.SvG",
    ] {
        let svg_eol = git(&["check-attr", "eol", "--", path], &project);
        assert!(svg_eol.status.success());
        assert_eq!(
            String::from_utf8_lossy(&svg_eol.stdout),
            format!("{path}: eol: lf\n")
        );
    }
    assert_eq!(
        fs::read_to_string(project.join(".vscode/settings.json"))
            .expect("read generated VS Code settings"),
        "{\n  \"[stasis]\": {\n    \"editor.defaultFormatter\": \"stasislang.stasis\",\n    \"editor.formatOnSave\": true\n  }\n}\n"
    );
    assert_eq!(
        fs::read_to_string(project.join(".vscode/extensions.json"))
            .expect("read generated VS Code recommendations"),
        "{\n  \"recommendations\": [\n    \"stasislang.stasis\"\n  ]\n}\n"
    );
    assert!(project
        .join("vendor/stasis/stdlib/internal/host_frame_raw.stasis")
        .is_file());
    assert!(project
        .join("vendor/stasis/stdlib/internal/gfx_cmd.stasis")
        .is_file());
    assert_generated_knowledge(&project);
    assert!(!project.join("vendor/stasis/src").exists());
    assert!(!project.join("vendor/stasis/runtime").exists());
    assert!(!project.join("vendor/stasis/stdlib/gfx_cmd.stasis").exists());

    fs::create_dir_all(project.join("src/game")).expect("create nested game source directory");
    fs::write(
        project.join("src/main.stasis"),
        crlf(
            "import \"game/player.stasis\";\n\nfunction main(): i32 {\n    return player_ready();\n}\n",
        ),
    )
    .expect("write generated-project package import smoke entry");
    fs::write(
        project.join("src/game/player.stasis"),
        crlf(
            "import \"/vendor/stasis/stdlib/graphics.stasis\";\n\nfunction player_ready(): i32 {\n    return 1;\n}\n",
        ),
    )
    .expect("write nested generated-project package import smoke");

    let formatted = stasis(&["--json", "fmt", "--check"], &project);
    assert_eq!(formatted.status.code(), Some(0));
    let formatted_json = json_stdout(&formatted);
    assert_eq!(formatted_json["command"], "fmt");

    let format_alias = stasis(&["--json", "format", "--check"], &project);
    assert_eq!(format_alias.status.code(), Some(0));
    let format_alias_json = json_stdout(&format_alias);
    assert_eq!(format_alias_json["command"], "fmt");
    assert_eq!(format_alias_json["result"], formatted_json["result"]);

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

    let build_ready = stasis(&["build", "--mode", "dev"], &project);
    assert_eq!(
        build_ready.status.code(),
        Some(0),
        "generated project build should have its canonical asset manifest: stdout={} stderr={}",
        String::from_utf8_lossy(&build_ready.stdout),
        String::from_utf8_lossy(&build_ready.stderr)
    );

    fs::remove_dir_all(&parent).ok();
}

#[test]
fn new_always_generates_github_actions() {
    let parent = temp_dir("github_actions_development");
    fs::create_dir_all(&parent).expect("create temp parent");
    let project = parent.join("demo");

    let output = stasis(&["--json", "new", "demo", "--dir", "demo"], &parent);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(json_stdout(&output)["result"]["github_actions"], true);
    for path in [
        ".github/workflows/stasis-pr.yml",
        ".github/workflows/stasis-weekly.yml",
        "tools/restore-stasis-release.ps1",
        "tools/resolve-stasis-nightly.ps1",
    ] {
        assert!(project.join(path).is_file(), "missing generated {path}");
    }
    let pr = fs::read_to_string(project.join(".github/workflows/stasis-pr.yml"))
        .expect("read generated PR workflow");
    assert!(pr.contains("stasis --json vendor status --workspace ."));
    assert!(pr.contains("$status.result.current -ne $true"));
    let restore = fs::read_to_string(project.join("tools/restore-stasis-release.ps1"))
        .expect("read generated restore helper");
    assert!(restore.contains("StartsWith($prefix, $pathComparison)"));
    assert!(!restore.contains("StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)"));
    let manifest: Value =
        serde_json::from_slice(&fs::read(project.join("stasis.json")).expect("read manifest"))
            .expect("parse manifest");
    assert_eq!(
        manifest["vendor"]["stasis"]["release_id"],
        option_env!("STASIS_RELEASE_ID").unwrap_or("development")
    );

    let initialized = parent.join("initialized");
    fs::create_dir_all(&initialized).expect("create init directory");
    let init = stasis(&["--json", "init", "--name", "initialized"], &initialized);
    assert_eq!(init.status.code(), Some(0));
    assert_eq!(json_stdout(&init)["result"]["github_actions"], false);
    assert!(!initialized.join(".github").exists());
    assert!(!initialized.join("tools").exists());
    fs::remove_dir_all(parent).ok();
}

#[test]
fn init_preserves_existing_svg_line_ending_policy() {
    let parent = temp_dir("init_existing_gitattributes");
    let project = parent.join("demo");
    fs::create_dir_all(&project).expect("create project directory");
    fs::write(project.join(".gitattributes"), "*.png binary\n").expect("write existing policy");

    let initialized = stasis(&["--json", "init", "--name", "demo", "."], &project);
    assert_eq!(initialized.status.code(), Some(0));
    assert_eq!(
        fs::read_to_string(project.join(".gitattributes")).expect("read existing Git attributes"),
        "*.png binary\n"
    );

    fs::remove_dir_all(&parent).ok();
}

#[test]
fn init_preserves_existing_git_ignore_policy() {
    let parent = temp_dir("init_existing_gitignore");
    let project = parent.join("demo");
    fs::create_dir_all(&project).expect("create project directory");
    fs::write(project.join(".gitignore"), "custom-cache/\n")
        .expect("write existing Git ignore policy");

    let initialized = stasis(&["--json", "init", "--name", "demo", "."], &project);
    assert_eq!(initialized.status.code(), Some(0));
    assert_eq!(
        fs::read_to_string(project.join(".gitignore")).expect("read existing Git ignore policy"),
        "custom-cache/\n"
    );

    fs::remove_dir_all(&parent).ok();
}

#[test]
fn new_refuses_to_overwrite_existing_svg_line_ending_policy() {
    let parent = temp_dir("new_existing_gitattributes");
    let project = parent.join("demo");
    fs::create_dir_all(&project).expect("create project directory");
    fs::write(project.join(".gitattributes"), "*.png binary\n").expect("write existing policy");

    let created = stasis(&["--json", "new", "demo", "--dir", "demo"], &parent);
    assert_ne!(created.status.code(), Some(0));
    assert_eq!(
        fs::read_to_string(project.join(".gitattributes")).expect("read existing Git attributes"),
        "*.png binary\n"
    );
    assert!(!project.join("stasis.json").exists());

    fs::remove_dir_all(&parent).ok();
}

#[test]
fn new_refuses_to_overwrite_existing_git_ignore_policy() {
    let parent = temp_dir("new_existing_gitignore");
    let project = parent.join("demo");
    fs::create_dir_all(&project).expect("create project directory");
    fs::write(project.join(".gitignore"), "custom-cache/\n")
        .expect("write existing Git ignore policy");

    let created = stasis(&["--json", "new", "demo", "--dir", "demo"], &parent);
    assert_ne!(created.status.code(), Some(0));
    assert_eq!(
        fs::read_to_string(project.join(".gitignore")).expect("read existing Git ignore policy"),
        "custom-cache/\n"
    );
    assert!(!project.join("stasis.json").exists());

    fs::remove_dir_all(&parent).ok();
}

#[test]
fn new_project_blocks_unformatted_commits() {
    let parent = temp_dir("format_hook");
    fs::create_dir_all(&parent).expect("create temp parent");
    let project = parent.join("demo");
    assert_eq!(
        stasis(&["new", "demo", "--dir", "demo"], &parent)
            .status
            .code(),
        Some(0)
    );
    assert!(git(&["config", "user.name", "Stasis Test"], &project)
        .status
        .success());
    assert!(git(
        &["config", "user.email", "stasis@example.invalid"],
        &project
    )
    .status
    .success());
    assert!(git(&["config", "commit.gpgsign", "false"], &project)
        .status
        .success());

    fs::write(
        project.join("src/main.stasis"),
        "function main():i32{return 0;}\n",
    )
    .expect("write unformatted source");
    assert!(git(&["add", "-A"], &project).status.success());
    let blocked = git_with_stasis_on_path(&["commit", "-m", "unformatted"], &project);
    assert!(!blocked.status.success());
    assert!(
        String::from_utf8_lossy(&blocked.stderr).contains("formatting required"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&blocked.stdout),
        String::from_utf8_lossy(&blocked.stderr)
    );
    assert_eq!(
        fs::read_to_string(project.join("src/main.stasis")).expect("read hook-formatted source"),
        "function main(): i32 {\n    return 0;\n}\n"
    );
    let still_blocked = git_with_stasis_on_path(&["commit", "-m", "still unformatted"], &project);
    assert!(!still_blocked.status.success());
    assert!(String::from_utf8_lossy(&still_blocked.stderr).contains("stage the formatted"));

    assert!(git(&["add", "-A"], &project).status.success());
    let committed = git_with_stasis_on_path(&["commit", "-m", "formatted"], &project);
    assert!(
        committed.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&committed.stdout),
        String::from_utf8_lossy(&committed.stderr)
    );

    fs::remove_dir_all(&parent).ok();
}

#[test]
fn init_includes_the_project_architecture_guide() {
    let parent = temp_dir("init_architecture");
    let project = parent.join("existing");
    fs::create_dir_all(&project).expect("create existing project directory");

    let initialized = stasis(&["--json", "init", "--name", "existing", "."], &project);
    assert_eq!(initialized.status.code(), Some(0));
    assert_eq!(json_stdout(&initialized)["command"], "init");
    assert_eq!(
        fs::read_to_string(project.join("PROJECT_ARCHITECTURE.md"))
            .expect("read project architecture guide"),
        include_str!("../../../docs/project_architecture.md")
    );
    assert!(!project.join(".git").exists());
    assert!(!project.join(".githooks/pre-commit").exists());

    fs::remove_dir_all(&parent).ok();
}

#[test]
fn format_alias_applies_canonical_layout_and_fmt_check_enforces_it() {
    let parent = temp_dir("opinionated_format");
    fs::create_dir_all(&parent).expect("create temp parent");
    let project = parent.join("demo");
    assert_eq!(
        stasis(&["new", "demo", "--dir", "demo"], &parent)
            .status
            .code(),
        Some(0)
    );
    let entry = project.join("src/main.stasis");
    fs::write(
        &entry,
        "struct Player{health:i32;active:bool;}enum Mode{Menu Playing}global score:i32;global player:Player;function update(amount:i32):void{if(amount>0){player.health+=amount;}else{player.health=0;}}function main():i32{update(1);return player.health;}\n",
    )
    .expect("write unformatted fixture");

    let before = stasis(&["--json", "fmt", "--check"], &project);
    assert_eq!(before.status.code(), Some(1));
    assert!(json_stderr(&before)["message"]
        .as_str()
        .unwrap_or_default()
        .contains("src/main.stasis"));

    let formatted = stasis(&["--json", "format"], &project);
    assert_eq!(formatted.status.code(), Some(0));
    assert_eq!(json_stdout(&formatted)["command"], "fmt");
    assert_eq!(
        fs::read_to_string(&entry).expect("read formatted fixture"),
        "struct Player {\n    health: i32;\n    active: bool;\n}\n\nenum Mode {\n    Menu,\n    Playing,\n}\n\nglobal score: i32;\nglobal player: Player;\n\nfunction update(amount: i32): void {\n    if (amount > 0) {\n        player.health += amount;\n    } else {\n        player.health = 0;\n    }\n}\n\nfunction main(): i32 {\n    update(1);\n    return player.health;\n}\n"
    );

    let fixed_modified = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    fs::File::options()
        .write(true)
        .open(&entry)
        .expect("open formatted fixture without truncating")
        .set_times(fs::FileTimes::new().set_modified(fixed_modified))
        .expect("set fixture modification time");
    let baseline_modified = fs::metadata(&entry)
        .expect("read fixture metadata")
        .modified()
        .expect("read fixture modification time");

    for command in ["fmt", "format"] {
        let unchanged = stasis(&["--json", command], &project);
        assert_eq!(unchanged.status.code(), Some(0));
        assert_eq!(json_stdout(&unchanged)["result"]["changed"], json!([]));
        assert_eq!(
            fs::metadata(&entry)
                .expect("read unchanged fixture metadata")
                .modified()
                .expect("read unchanged fixture modification time"),
            baseline_modified,
            "{command} rewrote an unchanged source file"
        );
    }

    assert_eq!(stasis(&["fmt", "--check"], &project).status.code(), Some(0));
    let checked = stasis(&["check"], &project);
    assert_eq!(
        checked.status.code(),
        Some(0),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&checked.stdout),
        String::from_utf8_lossy(&checked.stderr)
    );
    fs::remove_dir_all(&parent).ok();
}

#[test]
fn format_accepts_explicit_sources_without_a_project_manifest() {
    let root = temp_dir("explicit_format_paths");
    fs::create_dir_all(&root).expect("create standalone source directory");
    let source = root.join("standalone.stasis");
    fs::write(&source, "function main():i32{return 0;}\n")
        .expect("write standalone unformatted source");

    let before = stasis(&["format", "--check", "standalone.stasis"], &root);
    assert_eq!(before.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&before.stderr).contains("formatting required"));
    assert!(stasis(&["format", "standalone.stasis"], &root)
        .status
        .success());
    assert_eq!(
        fs::read_to_string(&source).expect("read standalone formatted source"),
        "function main(): i32 {\n    return 0;\n}\n"
    );

    fs::remove_dir_all(&root).ok();
}

#[test]
fn format_stdin_returns_only_canonical_source_without_a_manifest() {
    let root = temp_dir("format_stdin");
    fs::create_dir_all(&root).expect("create stdin format directory");
    let output = stasis_with_stdin(
        &["format", "--stdin"],
        &root,
        "enum Mode{Menu Playing}function main():i32{return 0;}\n",
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    assert_eq!(
        String::from_utf8(output.stdout).expect("formatted UTF-8"),
        "enum Mode {\n    Menu,\n    Playing,\n}\n\nfunction main(): i32 {\n    return 0;\n}\n"
    );

    let rejected = stasis_with_stdin(&["--json", "format", "--stdin"], &root, "");
    assert_eq!(rejected.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("stdout is the formatted source"));
    fs::remove_dir_all(root).ok();
}

#[test]
fn inspect_reports_compiler_state_memory_and_capacity_projection() {
    let parent = temp_dir("memory_report");
    fs::create_dir_all(&parent).expect("create temp parent");
    let project = parent.join("demo");
    assert_eq!(
        stasis(&["new", "demo", "--dir", "demo"], &parent)
            .status
            .code(),
        Some(0)
    );
    fs::write(
        project.join("src/main.stasis"),
        "import \"../tests/stasis/seams/state.stasis\";\nfunction main(): i32 { return state.score; }\n",
    )
    .expect("write memory entry fixture");
    fs::create_dir_all(project.join("tests/stasis/seams"))
        .expect("create explicit graphics seam directory");
    fs::write(
        project.join("tests/stasis/seams/state.stasis"),
        "struct Enemy { hp: i32; speed: f64; }\n\
         struct GameState { score: i32; enemies: Enemy[4]; }\n\
         global state: GameState;\n\
         global render_cmd_i32: i32[8];\n",
    )
    .expect("write imported memory fixture");

    let inspected = stasis(
        &[
            "--json",
            "inspect",
            "--capacity",
            "state.enemies=8",
            "--mobile-budget-bytes",
            "64",
        ],
        &project,
    );
    assert_eq!(
        inspected.status.code(),
        Some(0),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&inspected.stdout),
        String::from_utf8_lossy(&inspected.stderr)
    );
    let result = json_stdout(&inspected);
    let memory = &result["result"]["memory"];
    assert_eq!(memory["storage_model"], "soa_direct_bindings");
    assert_eq!(memory["capacity_changes"][0]["path"], "state.enemies");
    assert_eq!(memory["capacity_changes"][0]["delta_bytes"], 48);
    assert!(memory["structs"]
        .as_array()
        .is_some_and(|items| items.iter().any(|item| item["path"] == "state")));
    assert!(memory["command_buffers"]
        .as_array()
        .is_some_and(|items| items.iter().any(|item| item["path"] == "render_cmd_i32")));
    assert!(memory["warnings"]
        .as_array()
        .is_some_and(|items| items.iter().any(|item| item
            .as_str()
            .unwrap_or_default()
            .contains("mobile snapshot budget"))));

    let invalid = stasis(&["--json", "inspect", "--capacity", "missing=4"], &project);
    assert_eq!(invalid.status.code(), Some(1));
    assert!(json_stderr(&invalid)["message"]
        .as_str()
        .unwrap_or_default()
        .contains("not found in compiler collection metadata"));
    fs::remove_dir_all(parent).ok();
}

#[test]
fn inspect_reports_nested_costs_tick_budget_layout_and_mobile_estimates() {
    let project = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/bounded_performance");
    let inspected = stasis(&["--json", "inspect"], &project);
    assert_eq!(
        inspected.status.code(),
        Some(0),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&inspected.stdout),
        String::from_utf8_lossy(&inspected.stderr)
    );
    let result = json_stdout(&inspected);
    let performance = &result["result"]["performance"];
    assert_eq!(performance["schema_version"], 1);
    assert_eq!(performance["tick_budget_us"], 1);
    let expensive = performance["functions"]
        .as_array()
        .and_then(|functions| {
            functions
                .iter()
                .find(|function| function["function"] == "expensive_scan")
        })
        .expect("expensive scan report");
    assert_eq!(expensive["worst_nested_iteration_product"], 512);
    assert_eq!(expensive["structural_bound_complete"], true);
    assert!(expensive["fields_scanned"]
        .as_array()
        .is_some_and(|fields| fields.iter().any(|field| {
            field["path"] == "particles[*].score"
                && field["conservative_max_visits"] == 512
                && field["conservative_max_bytes"] == 2048
        })));
    assert!(expensive["fields_scanned"]
        .as_array()
        .is_some_and(|fields| fields.iter().any(|field| {
            field["path"] == "values[*]"
                && field["element_bytes"] == 4
                && field["conservative_max_visits"] == 5
        })));
    assert!(expensive["pools_iterated"]
        .as_array()
        .is_some_and(|pools| pools
            .iter()
            .any(|pool| { pool["path"] == "values" && pool["bytes_per_element"] == 4 })));
    assert!(expensive["pools_iterated"]
        .as_array()
        .is_some_and(|pools| pools.iter().any(|pool| pool["path"] == "particles")));

    let layout = performance["layout_choices"]
        .as_array()
        .and_then(|layouts| layouts.iter().find(|layout| layout["path"] == "particles"))
        .expect("particle layout choice");
    assert_eq!(layout["active_layout"], "soa");
    assert!(layout["aos_padding_bytes_per_element"]
        .as_u64()
        .is_some_and(|padding| padding > 0));
    assert!(performance["mobile"]["aot_object_code_bytes"]
        .as_u64()
        .is_some_and(|bytes| bytes > 0));
    assert!(performance["mobile"]["package_estimate_bytes"]
        .as_u64()
        .is_some_and(|bytes| bytes > 512 * 1024));
}

#[cfg(windows)]
#[test]
fn play_reports_real_tick_budget_average_p99_and_overruns() {
    let project = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/bounded_performance");
    let entry = project.join("src/main.stasis");
    let output = stasis(
        &[
            "play",
            entry.to_str().expect("entry path"),
            "--watch-dir",
            project.to_str().expect("project path"),
            "--ticks",
            "3",
        ],
        &project,
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let report_line = stdout
        .lines()
        .find(|line| line.contains("[tick-budget]"))
        .expect("tick budget report");
    let report = &report_line[report_line.find("[tick-budget]").expect("report marker")..];
    assert!(report.contains("generation=0 budget_us=1 samples=3"));
    assert!(report.contains("average_us="));
    assert!(report.contains("p99_us="));
    let overruns = report
        .split_whitespace()
        .find_map(|field| field.strip_prefix("overruns="))
        .and_then(|value| value.parse::<u64>().ok())
        .expect("overrun count");
    assert!(
        overruns > 0,
        "expected real tick work to exceed 1 us: {report}"
    );
}

#[test]
fn fresh_runtime_validation_runs_in_a_separate_cli_process() {
    let parent = temp_dir("fresh_validation");
    fs::create_dir_all(&parent).expect("create temp parent");
    let project = parent.join("demo");
    assert_eq!(
        stasis(&["new", "demo", "--dir", "demo"], &parent)
            .status
            .code(),
        Some(0)
    );
    fs::write(
        project.join("src/main.stasis"),
        "global State { value: i32; rendered: i32; }\nfunction main(): i32 { State.value = 1; return 0; }\nfunction tick(): i32 { State.value += 1; return 0; }\nfunction render(): i32 { State.rendered = 1; return 0; }\n",
    )
    .expect("write validation game");
    let requirements = r#"[{"path":"State.value","op":"eq","value":3},{"path":"State.rendered","op":"eq","value":1}]"#;

    let output = stasis(
        &[
            "--json",
            "__validate-runtime",
            "--frames",
            "2",
            "--requirements-json",
            requirements,
        ],
        &project,
    );

    assert_eq!(output.status.code(), Some(0));
    let result = json_stdout(&output);
    assert_eq!(result["command"], "__validate-runtime");
    assert_eq!(result["result"]["baseline"], "fresh");
    assert_eq!(result["result"]["requirements_met"], true);

    let human_validation = stasis(
        &[
            "--json",
            "validate",
            "State.value",
            "eq",
            "3",
            "--frames",
            "2",
        ],
        &project,
    );
    assert_eq!(human_validation.status.code(), Some(0));
    assert_eq!(
        json_stdout(&human_validation)["result"]["requirements_met"],
        true
    );

    let references = stasis(&["--json", "symbol", "references", "State.value"], &project);
    assert_eq!(references.status.code(), Some(0));
    assert!(json_stdout(&references)["result"]["references"]
        .as_array()
        .is_some_and(|references| references.len() >= 2));
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
fn headless_ticks_and_seeded_scenarios_are_deterministic_and_reproducible() {
    let parent = temp_dir("headless_scenarios");
    fs::create_dir_all(&parent).expect("create temp parent");
    let project = parent.join("demo");
    assert_eq!(
        stasis(&["new", "demo", "--dir", "demo"], &parent)
            .status
            .code(),
        Some(0)
    );
    fs::write(
        project.join("src/main.stasis"),
        "global ticks: i32;\nglobal render_calls: i32;\nglobal seed: i32;\nglobal base: i32;\nglobal bad: i32;\nglobal checksum: i32;\nglobal values: i32[2];\nfunction main(): i32 { ticks = 0; render_calls = 0; seed = 0; base = 0; bad = 0; checksum = 0; values[0] = 0; values[1] = 0; return 0; }\nfunction tick(): i32 { ticks += 1; seed += 1; values[1] += 1; checksum = base * 10000 + seed * 100 + ticks + values[0]; if (ticks > 3) { bad = 1; } return 0; }\nfunction render(): i32 { render_calls += 1; return 0; }\n",
    )
    .expect("write headless game");

    let first = stasis(
        &[
            "--json",
            "run",
            "--headless",
            "--ticks",
            "3",
            "--fast-forward",
        ],
        &project,
    );
    assert_eq!(
        first.status.code(),
        Some(0),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&first.stdout),
        String::from_utf8_lossy(&first.stderr)
    );
    let first_json = json_stdout(&first);
    assert_eq!(first_json["result"]["ticks_executed"], 3);
    assert_eq!(first_json["result"]["fast_forward"], true);
    let first_hash = first_json["result"]["state_hash"]
        .as_str()
        .expect("state hash")
        .to_string();
    assert_eq!(first_hash.len(), 64);

    let second = stasis(
        &[
            "--json",
            "run",
            "--headless",
            "--ticks",
            "3",
            "--fast-forward",
        ],
        &project,
    );
    assert_eq!(second.status.code(), Some(0));
    assert_eq!(json_stdout(&second)["result"]["state_hash"], first_hash);

    fs::write(
        project.join("tests/baseline.state.json"),
        "{\"base\":7,\"values[0]\":11}\n",
    )
    .expect("write saved state");
    let scenario_path = project.join("tests/determinism with spaces.scenario.json");
    fs::write(
        &scenario_path,
        r#"{
  "schema_version": 1,
  "name": "seeded isolation",
  "ticks": 3,
  "state_file": "baseline.state.json",
  "invariants": [
    {"path": "base", "op": "eq", "value": 7},
    {"path": "bad", "op": "eq", "value": 0},
    {"path": "values[0]", "op": "eq", "value": 11},
    {"path": "values[1]", "op": "lte", "value": 3},
    {"path": "render_calls", "op": "eq", "value": 0}
  ],
  "property": {"seed_path": "seed", "seeds": [2, 5]}
}
"#,
    )
    .expect("write passing scenario");
    let scenarios = stasis(&["--json", "test"], &project);
    assert_eq!(
        scenarios.status.code(),
        Some(0),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&scenarios.stdout),
        String::from_utf8_lossy(&scenarios.stderr)
    );
    let scenarios_json = json_stdout(&scenarios);
    assert_eq!(scenarios_json["result"]["scenarios_discovered"], 1);
    assert_eq!(scenarios_json["result"]["scenario_cases_run"], 2);
    assert_eq!(scenarios_json["result"]["scenario_cases_passed"], 2);

    fs::write(
        &scenario_path,
        r#"{
  "schema_version": 1,
  "name": "seeded isolation",
  "ticks": 3,
  "state_file": "baseline.state.json",
  "invariants": [{"path": "ticks", "op": "lt", "value": 2}],
  "property": {"seed_path": "seed", "seeds": [2, 5]}
}
"#,
    )
    .expect("write failing scenario");
    let failed = stasis(&["--json", "test"], &project);
    assert_eq!(failed.status.code(), Some(1));
    let receipt_dir = project.join("build/headless-replays");
    let receipts = fs::read_dir(&receipt_dir)
        .expect("read receipt directory")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect receipts");
    assert_eq!(receipts.len(), 1);
    let receipt = receipts[0].path();
    let receipt_json: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(&receipt).expect("read deterministic failure receipt"),
    )
    .expect("parse deterministic failure receipt");
    assert_eq!(receipt_json["seed"], 2);
    assert_eq!(receipt_json["failed_tick"], 2);
    assert_eq!(receipt_json["observed_hashes_truncated"], false);
    assert_eq!(
        receipt_json["observed_hashes"].as_array().map(Vec::len),
        Some(2)
    );
    assert_eq!(
        receipt_json["scenario"],
        "tests/determinism with spaces.scenario.json"
    );
    assert_eq!(
        receipt_json["rerun"],
        "stasis test \"tests/determinism with spaces.scenario.json\""
    );
    assert_eq!(
        receipt_json["rerun_argv"],
        serde_json::json!(["test", "tests/determinism with spaces.scenario.json"])
    );

    fs::write(
        &scenario_path,
        r#"{
  "schema_version": 1,
  "name": "seeded isolation",
  "ticks": 3,
  "state_file": "baseline.state.json",
  "invariants": [{"path": "values[0]", "op": "eq", "value": 11}],
  "expected_hashes": [
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000"
  ],
  "property": {"seed_path": "seed", "seeds": [2]}
}
"#,
    )
    .expect("write hash mismatch scenario");
    let hash_failed = stasis(&["--json", "test"], &project);
    assert_eq!(hash_failed.status.code(), Some(1));
    let hash_receipt: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&receipt).expect("read hash mismatch receipt"))
            .expect("parse hash mismatch receipt");
    assert_eq!(hash_receipt["failed_tick"], 1);
    assert!(hash_receipt["reason"]
        .as_str()
        .is_some_and(|reason| reason.contains("state hash mismatch")));
    assert_eq!(
        hash_receipt["observed_hashes"].as_array().map(Vec::len),
        Some(1)
    );

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
    let listed_items = listed_json["result"]["items"].as_array().expect("items");
    assert!(listed_items.iter().all(|item| item["kind"] != "imports"));
    assert!(listed_items
        .iter()
        .all(|item| item.get("source").is_none() && item.get("source_hash").is_none()));
    assert!(listed_items
        .iter()
        .any(|item| item["name"] == "tick" && item["file"] == "src/main.stasis"));
    assert_eq!(
        listed_json["result"]["files"],
        json!(["src/main.stasis", "src/old.stasis"])
    );
    assert_eq!(
        listed_json["result"]["imports"],
        json!({"src/main.stasis": ["src/old.stasis"], "src/old.stasis": []})
    );
    assert!(listed_items.iter().any(|item| item["name"] == "old_value"));

    let widened = stasis(
        &[
            "--json",
            "symbol",
            "list",
            "--file",
            "src/main.stasis",
            "--file",
            "src/old.stasis",
        ],
        &project,
    );
    assert_eq!(widened.status.code(), Some(0));
    let widened_json = json_stdout(&widened);
    assert_eq!(
        widened_json["result"]["files"],
        json!(["src/main.stasis", "src/old.stasis"])
    );
    assert_eq!(
        widened_json["result"]["imports"],
        json!({"src/main.stasis": ["src/old.stasis"], "src/old.stasis": []})
    );
    assert!(widened_json["result"]["items"]
        .as_array()
        .expect("widened items")
        .iter()
        .any(|item| item["name"] == "old_value"));

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
fn semantic_symbol_queries_are_read_only_in_a_linked_vendor_worktree() {
    let parent = temp_dir("semantic_symbols_vendor_readonly");
    fs::create_dir_all(&parent).expect("create temp parent");
    let project = parent.join("consumer");
    let created = stasis(&["new", "consumer", "--dir", "consumer"], &parent);
    assert_eq!(
        created.status.code(),
        Some(0),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&created.stdout),
        String::from_utf8_lossy(&created.stderr)
    );
    fs::write(
        project.join("src/main.stasis"),
        concat!(
            "import \"/vendor/stasis/stdlib/graphics.stasis\";\n",
            "import \"/vendor/stasis/stdlib/network_client.stasis\";\n",
            "function main(): i32 { return network_client_supported(); }\n",
        ),
    )
    .expect("write vendor consumer entry");
    assert!(git(&["config", "user.name", "Stasis Test"], &project)
        .status
        .success());
    assert!(git(
        &["config", "user.email", "stasis@example.invalid"],
        &project,
    )
    .status
    .success());
    assert!(git(&["add", "-A"], &project).status.success());
    let committed = git(&["commit", "--no-verify", "-m", "initial"], &project);
    assert!(
        committed.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&committed.stdout),
        String::from_utf8_lossy(&committed.stderr)
    );

    let linked = parent.join("consumer-linked");
    let linked_arg = linked.to_string_lossy().to_string();
    let worktree = git(
        &["worktree", "add", "--detach", &linked_arg, "HEAD"],
        &project,
    );
    assert!(
        worktree.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&worktree.stdout),
        String::from_utf8_lossy(&worktree.stderr)
    );
    let vendor_updated = stasis(&["--json", "vendor", "update"], &linked);
    assert_eq!(
        vendor_updated.status.code(),
        Some(0),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&vendor_updated.stdout),
        String::from_utf8_lossy(&vendor_updated.stderr)
    );
    let stale_cache = linked.join(".stasis_cache/toolchain/stale.bin");
    fs::create_dir_all(stale_cache.parent().expect("stale cache parent"))
        .expect("create stale cache fixture");
    fs::write(&stale_cache, "stale").expect("write stale cache fixture");
    let stale_time = SystemTime::UNIX_EPOCH + Duration::from_secs(1);
    fs::File::options()
        .write(true)
        .open(&stale_cache)
        .expect("open stale cache fixture")
        .set_times(
            fs::FileTimes::new()
                .set_accessed(stale_time)
                .set_modified(stale_time),
        )
        .expect("age stale cache fixture");

    let before_noop_prepare = snapshot_project_bytes(&linked);
    let noop_prepare = stasis(&["--json", "prepare"], &linked);
    assert_eq!(noop_prepare.status.code(), Some(0));
    assert_eq!(json_stdout(&noop_prepare)["result"]["prepared"], false);
    assert_eq!(snapshot_project_bytes(&linked), before_noop_prepare);

    let run_query = |args: &[&str]| {
        let before = snapshot_project_bytes(&linked);
        let output = stasis(args, &linked);
        assert_eq!(
            snapshot_project_bytes(&linked),
            before,
            "query changed project bytes for {:?}",
            args
        );
        output
    };

    let listed = run_query(&["--json", "symbol", "list", "--limit", "200"]);
    assert_eq!(
        listed.status.code(),
        Some(0),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&listed.stdout),
        String::from_utf8_lossy(&listed.stderr)
    );
    let listed_json = json_stdout(&listed);
    let listed_files = listed_json["result"]["files"]
        .as_array()
        .expect("default query files");
    assert!(listed_files
        .iter()
        .any(|file| file == "vendor/stasis/stdlib/graphics.stasis"));
    assert!(listed_files
        .iter()
        .any(|file| file == "vendor/stasis/stdlib/network_client.stasis"));
    let listed_items = listed_json["result"]["items"]
        .as_array()
        .expect("default query items");
    assert!(listed_items
        .iter()
        .any(|item| item["name"] == "network_client_supported"));

    for (file, name) in [
        ("vendor/stasis/stdlib/host_frame.stasis", "refresh"),
        (
            "vendor/stasis/stdlib/network_client.stasis",
            "network_client_supported",
        ),
    ] {
        let scoped = run_query(&[
            "--json", "symbol", "list", "--file", file, "--kind", "function",
        ]);
        assert_eq!(scoped.status.code(), Some(0));
        let scoped_json = json_stdout(&scoped);
        let scoped_items = scoped_json["result"]["items"]
            .as_array()
            .expect("scoped query items");
        assert!(scoped_items.iter().any(|item| item["name"] == name));
    }

    let found = run_query(&[
        "--json",
        "symbol",
        "find",
        "network_client_supported",
        "--kind",
        "function",
        "--file",
        "vendor/stasis/stdlib/network_client.stasis",
    ]);
    assert_eq!(found.status.code(), Some(0));
    assert_eq!(
        json_stdout(&found)["result"]["matches"]
            .as_array()
            .expect("vendor symbol matches")
            .len(),
        1
    );

    let read = run_query(&[
        "--json",
        "symbol",
        "read",
        "refresh",
        "--kind",
        "function",
        "--file",
        "vendor/stasis/stdlib/host_frame.stasis",
    ]);
    assert_eq!(read.status.code(), Some(0));
    assert!(json_stdout(&read)["result"]["item"]["source"]
        .as_str()
        .is_some_and(|source| source.contains("function refresh")));

    let references = run_query(&["--json", "symbol", "references", "network_client_supported"]);
    assert_eq!(references.status.code(), Some(0));
    assert!(json_stdout(&references)["result"]["references"]
        .as_array()
        .is_some_and(|items| {
            items
                .iter()
                .any(|item| item["file"] == "vendor/stasis/stdlib/network_client.stasis")
        }));

    let manifest_path = linked.join("stasis.json");
    let mut byte_current_manifest: Value =
        serde_json::from_slice(&fs::read(&manifest_path).expect("current manifest"))
            .expect("parse current manifest");
    byte_current_manifest["vendor"]["stasis"]["release_id"] =
        Value::String("older-toolchain".to_string());
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&byte_current_manifest).expect("serialize byte-current manifest"),
    )
    .expect("write byte-current manifest");
    assert_eq!(
        run_query(&["--json", "symbol", "list"]).status.code(),
        Some(0)
    );

    let mut inconsistent_manifest: Value = serde_json::from_slice(
        &fs::read(&manifest_path).expect("manifest after release-only edit"),
    )
    .expect("parse inconsistent manifest");
    inconsistent_manifest["vendor"]["stasis"]["sha256"] = Value::String("0".repeat(64));
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&inconsistent_manifest).expect("serialize inconsistent manifest"),
    )
    .expect("write inconsistent manifest");
    let before_inconsistent = snapshot_project_bytes(&linked);
    let inconsistent = stasis(&["--json", "symbol", "list"], &linked);
    assert_eq!(inconsistent.status.code(), Some(1));
    assert_eq!(
        json_stderr(&inconsistent)["message"],
        "read-only symbol query did not update files: checked-in vendor snapshot has an inconsistent manifest fingerprint; run 'stasis vendor status' then 'stasis vendor update'"
    );
    assert_eq!(snapshot_project_bytes(&linked), before_inconsistent);
    assert_eq!(
        stasis(&["--json", "vendor", "update"], &linked)
            .status
            .code(),
        Some(0)
    );
    assert_eq!(
        run_query(&["--json", "symbol", "list"]).status.code(),
        Some(0)
    );

    fs::remove_dir_all(linked.join("vendor/stasis")).expect("remove vendor snapshot");
    let before_missing = snapshot_project_bytes(&linked);
    let missing = stasis(&["--json", "symbol", "list"], &linked);
    assert_eq!(missing.status.code(), Some(1));
    assert_eq!(
        json_stderr(&missing)["message"],
        "read-only symbol query did not update files: checked-in vendor snapshot is missing; run 'stasis vendor status' then 'stasis vendor update'"
    );
    assert_eq!(snapshot_project_bytes(&linked), before_missing);
    let repaired = stasis(&["--json", "vendor", "update"], &linked);
    assert_eq!(
        repaired.status.code(),
        Some(0),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&repaired.stdout),
        String::from_utf8_lossy(&repaired.stderr)
    );
    assert_eq!(
        run_query(&["--json", "symbol", "list"]).status.code(),
        Some(0)
    );

    let edited_vendor = linked.join("vendor/stasis/stdlib/graphics.stasis");
    let mut edited_source = fs::read_to_string(&edited_vendor).expect("read vendor source");
    edited_source.push_str("// local vendor edit\n");
    fs::write(&edited_vendor, edited_source).expect("edit vendor source");
    let before_local_change = snapshot_project_bytes(&linked);
    let local_change = stasis(&["--json", "symbol", "list"], &linked);
    assert_eq!(local_change.status.code(), Some(1));
    assert_eq!(
        json_stderr(&local_change)["message"],
        "read-only symbol query did not update files: checked-in vendor snapshot has local changes; run 'stasis vendor status' then 'stasis vendor update'"
    );
    assert_eq!(snapshot_project_bytes(&linked), before_local_change);
    assert_eq!(
        stasis(&["--json", "vendor", "update"], &linked)
            .status
            .code(),
        Some(0)
    );
    assert_eq!(
        run_query(&["--json", "symbol", "list"]).status.code(),
        Some(0)
    );

    let mut stale_source = fs::read_to_string(&edited_vendor).expect("read repaired vendor");
    stale_source.push_str("// stale toolchain fixture\n");
    fs::write(&edited_vendor, stale_source).expect("write stale vendor fixture");
    let status = stasis(&["--json", "vendor", "status"], &linked);
    assert_eq!(status.status.code(), Some(0));
    let actual_sha256 = json_stdout(&status)["result"]["actual_sha256"]
        .as_str()
        .expect("stale fixture hash")
        .to_string();
    let mut manifest: Value = serde_json::from_slice(&fs::read(&manifest_path).expect("manifest"))
        .expect("parse manifest");
    manifest["vendor"]["stasis"]["sha256"] = Value::String(actual_sha256);
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).expect("serialize stale manifest"),
    )
    .expect("write stale manifest");
    let before_stale = snapshot_project_bytes(&linked);
    let stale = stasis(&["--json", "symbol", "list"], &linked);
    assert_eq!(stale.status.code(), Some(1));
    assert_eq!(
        json_stderr(&stale)["message"],
        "read-only symbol query did not update files: checked-in vendor snapshot is stale for the selected toolchain; run 'stasis vendor status' then 'stasis vendor update'"
    );
    assert_eq!(snapshot_project_bytes(&linked), before_stale);
    assert_eq!(
        stasis(&["--json", "vendor", "update"], &linked)
            .status
            .code(),
        Some(0)
    );
    assert_eq!(
        run_query(&["--json", "symbol", "list"]).status.code(),
        Some(0)
    );

    let removed = git(&["worktree", "remove", "--force", &linked_arg], &project);
    assert!(
        removed.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&removed.stdout),
        String::from_utf8_lossy(&removed.stderr)
    );
    fs::remove_dir_all(parent).ok();
}

#[test]
fn toolchain_stdlib_queries_require_explicit_prepare_in_a_linked_worktree() {
    let parent = temp_dir("semantic_symbols_toolchain_readonly");
    fs::create_dir_all(&parent).expect("create temp parent");
    let project = parent.join("consumer");
    let created = stasis(&["new", "consumer", "--dir", "consumer"], &parent);
    assert_eq!(
        created.status.code(),
        Some(0),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&created.stdout),
        String::from_utf8_lossy(&created.stderr)
    );

    let manifest_path = project.join("stasis.json");
    let mut manifest: Value =
        serde_json::from_slice(&fs::read(&manifest_path).expect("read generated manifest"))
            .expect("parse generated manifest");
    manifest["stdlib"] = Value::String("toolchain".to_string());
    manifest
        .as_object_mut()
        .expect("manifest object")
        .remove("vendor");
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).expect("serialize toolchain manifest"),
    )
    .expect("write toolchain manifest");
    fs::write(
        project.join("src/main.stasis"),
        concat!(
            "import \"/.stasis_cache/toolchain/src/stdlib/graphics.stasis\";\n",
            "import \"/.stasis_cache/toolchain/src/stdlib/network_client.stasis\";\n",
            "function main(): i32 { return network_client_supported(); }\n",
        ),
    )
    .expect("write toolchain consumer entry");
    fs::remove_dir_all(project.join("vendor")).expect("remove unused vendor snapshot");
    if project.join(".stasis_cache").exists() {
        fs::remove_dir_all(project.join(".stasis_cache")).expect("remove initial toolchain cache");
    }
    assert!(git(&["config", "user.name", "Stasis Test"], &project)
        .status
        .success());
    assert!(git(
        &["config", "user.email", "stasis@example.invalid"],
        &project,
    )
    .status
    .success());
    assert!(git(&["add", "-A"], &project).status.success());
    let committed = git(&["commit", "--no-verify", "-m", "initial"], &project);
    assert!(
        committed.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&committed.stdout),
        String::from_utf8_lossy(&committed.stderr)
    );

    let linked = parent.join("consumer-linked");
    let linked_arg = linked.to_string_lossy().to_string();
    let worktree = git(
        &["worktree", "add", "--detach", &linked_arg, "HEAD"],
        &project,
    );
    assert!(
        worktree.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&worktree.stdout),
        String::from_utf8_lossy(&worktree.stderr)
    );

    let run_query = |args: &[&str]| {
        let before = snapshot_project_bytes(&linked);
        let output = stasis(args, &linked);
        assert_eq!(
            snapshot_project_bytes(&linked),
            before,
            "query changed project bytes for {:?}",
            args
        );
        output
    };

    let missing = run_query(&["--json", "symbol", "list"]);
    assert_eq!(missing.status.code(), Some(1));
    assert_eq!(
        json_stderr(&missing)["message"],
        "read-only symbol query did not update files: toolchain stdlib cache is missing or unprepared; run 'stasis prepare'"
    );

    let prepared = stasis(&["--json", "prepare"], &linked);
    assert_eq!(
        prepared.status.code(),
        Some(0),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&prepared.stdout),
        String::from_utf8_lossy(&prepared.stderr)
    );
    assert_eq!(json_stdout(&prepared)["result"]["prepared"], true);
    assert!(linked
        .join(".stasis_cache/toolchain/src/.toolchain-sha256")
        .is_file());

    let listed = run_query(&["--json", "symbol", "list", "--limit", "200"]);
    assert_eq!(listed.status.code(), Some(0));
    let listed_json = json_stdout(&listed);
    let listed_files = listed_json["result"]["files"]
        .as_array()
        .expect("toolchain query files");
    assert!(listed_files
        .iter()
        .any(|file| file == ".stasis_cache/toolchain/src/stdlib/graphics.stasis"));
    assert!(listed_files
        .iter()
        .any(|file| file == ".stasis_cache/toolchain/src/stdlib/network_client.stasis"));
    let listed_items = listed_json["result"]["items"]
        .as_array()
        .expect("toolchain query items");
    assert!(listed_items
        .iter()
        .any(|item| item["name"] == "network_client_supported"));

    for (file, name) in [
        (
            ".stasis_cache/toolchain/src/stdlib/host_frame.stasis",
            "refresh",
        ),
        (
            ".stasis_cache/toolchain/src/stdlib/network_client.stasis",
            "network_client_supported",
        ),
    ] {
        let scoped = run_query(&[
            "--json", "symbol", "list", "--file", file, "--kind", "function",
        ]);
        assert_eq!(scoped.status.code(), Some(0));
        assert!(json_stdout(&scoped)["result"]["items"]
            .as_array()
            .expect("scoped toolchain items")
            .iter()
            .any(|item| item["name"] == name));
    }

    let read = run_query(&[
        "--json",
        "symbol",
        "read",
        "refresh",
        "--kind",
        "function",
        "--file",
        ".stasis_cache/toolchain/src/stdlib/host_frame.stasis",
    ]);
    assert_eq!(read.status.code(), Some(0));
    assert!(json_stdout(&read)["result"]["item"]["source"]
        .as_str()
        .is_some_and(|source| source.contains("function refresh")));

    let references = run_query(&["--json", "symbol", "references", "network_client_supported"]);
    assert_eq!(references.status.code(), Some(0));
    assert!(json_stdout(&references)["result"]["references"]
        .as_array()
        .is_some_and(|items| {
            items.iter().any(|item| {
                item["file"] == ".stasis_cache/toolchain/src/stdlib/network_client.stasis"
            })
        }));

    let marker = linked.join(".stasis_cache/toolchain/src/.toolchain-sha256");
    fs::write(&marker, "stale-toolchain\n").expect("write stale toolchain marker");
    let before_stale = snapshot_project_bytes(&linked);
    let stale = stasis(&["--json", "symbol", "list"], &linked);
    assert_eq!(stale.status.code(), Some(1));
    assert_eq!(
        json_stderr(&stale)["message"],
        "read-only symbol query did not update files: toolchain stdlib cache is stale for the selected toolchain; run 'stasis prepare'"
    );
    assert_eq!(snapshot_project_bytes(&linked), before_stale);

    let repaired = stasis(&["--json", "prepare"], &linked);
    assert_eq!(repaired.status.code(), Some(0));
    assert_eq!(json_stdout(&repaired)["result"]["prepared"], true);
    assert_eq!(
        run_query(&["--json", "symbol", "list"]).status.code(),
        Some(0)
    );

    let removed = git(&["worktree", "remove", "--force", &linked_arg], &project);
    assert!(
        removed.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&removed.stdout),
        String::from_utf8_lossy(&removed.stderr)
    );
    fs::remove_dir_all(parent).ok();
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
        "import \"/vendor/stasis/stdlib/graphics.stasis\";\nfunction main(): i32 { return load_font(\"/assets/fonts/ui.ttf\", 16); }\nfunction tick(): i32 { return 0; }\nfunction render(): i32 { return 0; }\n",
    )
    .expect("write mobile entry");
    fs::create_dir_all(project.join("assets/fonts")).expect("create assets");
    fs::write(project.join("assets/fonts/ui.ttf"), b"font").expect("write font asset");
    fs::write(
        project.join("assets/manifest.json"),
        "{\n  \"schema\": \"stasis-assets\",\n  \"version\": 1,\n  \"assets\": [\n    {\"id\":\"ui_font\",\"path\":\"assets/fonts/ui.ttf\",\"content_sha256\":\"795ea3efa43d0872b63bf0067be97553b46983e4f075097669391e9d15388ecc\",\"format\":{\"kind\":\"font\",\"encoding\":\"ttf\"},\"dependencies\":[]}\n  ]\n}\n",
    )
    .expect("write asset manifest");

    for (target, output) in [
        ("android-arm64", "android"),
        ("android-x86_64", "android_x86"),
        ("ios-arm64", "ios"),
    ] {
        let packaged = stasis(
            &[
                "package-mobile",
                "--target",
                target,
                "--entry",
                "src/main.stasis",
                "--out",
                output,
                "--development-build",
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
        assert!(
            String::from_utf8_lossy(&packaged.stdout).contains("\nCompleted in "),
            "stdout={}",
            String::from_utf8_lossy(&packaged.stdout)
        );
        assert!(project
            .join(output)
            .join("stasis_mobile_package.json")
            .is_file());
        let provenance: Value = serde_json::from_str(
            &fs::read_to_string(project.join(output).join("stasis_provenance.json"))
                .expect("read package provenance"),
        )
        .expect("parse package provenance");
        assert_eq!(provenance["schema"], "stasis.release_provenance.v1");
        assert_eq!(provenance["development_build"], true);
        assert_eq!(provenance["dirty_state"], true);
        assert!(provenance["mobile_shell_sources"].as_object().is_some_and(
            |sources| sources.contains_key("mobile/shells/common/stasis_mobile_main.c")
        ));
        assert!(provenance["runtime_sources"]
            .as_object()
            .is_some_and(|sources| sources.contains_key("runtime/stasis_renderer_lifecycle.h")));
        assert!(provenance["runtime_sources"]
            .as_object()
            .is_some_and(|sources| sources.contains_key("runtime/stasis_audio_assets.c")));
        assert!(provenance["runtime_sources"]
            .as_object()
            .is_some_and(|sources| sources.contains_key("runtime/minimp3.h")));
        assert!(project
            .join(output)
            .join("runtime")
            .join("stasis_audio_assets.h")
            .is_file());
        assert!(project
            .join(output)
            .join("runtime")
            .join("minimp3_ex.h")
            .is_file());
        assert!(project
            .join(output)
            .join("runtime")
            .join("MINIMP3-LICENSE.txt")
            .is_file());
        let receipt: Value = serde_json::from_str(
            &fs::read_to_string(project.join(output).join("stasis_mobile_package.json"))
                .expect("read package receipt"),
        )
        .expect("parse package receipt");
        assert_eq!(receipt["provenance"], "stasis_provenance.json");
        assert_eq!(receipt["development_build"], true);
        assert!(fs::read_to_string(
            project
                .join(output)
                .join("common/stasis_package_provenance.h")
        )
        .expect("read provenance header")
        .contains("non-release development build"));
        let aot_manifest_path = project
            .join(output)
            .join("aot/mobile_aot_bundle_manifest.json");
        assert!(aot_manifest_path.is_file());
        let aot_manifest: Value = serde_json::from_str(
            &fs::read_to_string(&aot_manifest_path).expect("read mobile AOT manifest"),
        )
        .expect("parse mobile AOT manifest");
        for field in [
            "engine_manifest",
            "symbols_header",
            "bindings_source",
            "asset_root",
            "asset_manifest",
        ] {
            let path = aot_manifest[field].as_str().expect("manifest path");
            assert!(!Path::new(path).is_absolute(), "{field} must be relative");
            assert!(!path.contains(".staging"), "{field} must survive publish");
        }
        let engine_manifest = fs::read_to_string(
            aot_manifest_path
                .parent()
                .expect("mobile AOT manifest parent")
                .join(
                    aot_manifest["engine_manifest"]
                        .as_str()
                        .expect("engine manifest path"),
                ),
        )
        .expect("read engine manifest");
        assert!(
            engine_manifest.contains("\"path\":\"gfx_cmd_f32\",\"max_length\":146564"),
            "mobile render ABI must publish the full f32 command buffer"
        );
        assert!(aot_manifest["objects"]
            .as_array()
            .expect("manifest objects")
            .iter()
            .all(|entry| entry["path"].as_str().is_some_and(|path| {
                !Path::new(path).is_absolute() && !path.contains(".staging")
            })));
    }
    let android_cmake_path = project.join("android/android/app/src/main/cpp/CMakeLists.txt");
    assert!(android_cmake_path.is_file());
    let android_cmake = fs::read_to_string(&android_cmake_path).expect("read Android CMake");
    assert!(android_cmake.contains("set(SDLIMAGE_BACKEND_STB ON CACHE BOOL \"\" FORCE)"));
    assert!(android_cmake.contains("set(SDLIMAGE_PNG ON CACHE BOOL \"\" FORCE)"));
    assert!(android_cmake.contains("set(SDLIMAGE_VENDORED OFF CACHE BOOL \"\" FORCE)"));
    assert!(android_cmake.contains("set(SDLIMAGE_PNG_LIBPNG OFF CACHE BOOL \"\" FORCE)"));
    assert!(project
        .join("android/android/app/src/main/assets/stasis_game/assets/manifest.json")
        .is_file());
    assert!(project
        .join("android/android/app/src/main/assets/stasis_game/assets/fonts/ui.ttf")
        .is_file());
    let arm64_gradle = fs::read_to_string(project.join("android/android/app/build.gradle"))
        .expect("read arm64 Gradle");
    assert!(arm64_gradle.contains("abiFilters 'arm64-v8a'"));
    assert!(!arm64_gradle.contains("abiFilters 'x86_64'"));
    let x86_gradle = fs::read_to_string(project.join("android_x86/android/app/build.gradle"))
        .expect("read x86_64 Gradle");
    assert!(x86_gradle.contains("abiFilters 'x86_64'"));
    assert!(!x86_gradle.contains("abiFilters 'arm64-v8a'"));
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

    let graphics_source = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../runtime/stasis_graphics.c"),
    )
    .expect("read desktop graphics runtime");
    assert!(graphics_source.contains("Stasis package provenance: path=%s manifest=%s"));

    let refused = stasis(
        &[
            "package-mobile",
            "--target",
            "android-x86_64",
            "--out",
            "x86_release",
        ],
        &project,
    );
    assert_eq!(refused.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&refused.stderr)
        .contains("android-x86_64 is a test-only emulator target; pass --development-build"));
    assert!(!project.join("x86_release").exists());

    if !cfg!(target_os = "macos") {
        let manifest_path = project.join("stasis.json");
        let mut network_manifest: Value = serde_json::from_slice(
            &fs::read(&manifest_path).expect("read mobile manifest for network fixture"),
        )
        .expect("parse mobile manifest for network fixture");
        network_manifest["capabilities"] = json!({"network": true});
        network_manifest["web"] = json!({"entry": "src/main.stasis"});
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&network_manifest).expect("encode network fixture manifest"),
        )
        .expect("write network fixture manifest");
        let network_ios = stasis(
            &[
                "package-mobile",
                "--target",
                "ios-arm64",
                "--out",
                "network_ios",
                "--development-build",
            ],
            &project,
        );
        assert_eq!(network_ios.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&network_ios.stderr)
            .contains("requires a macOS host with Xcode"));
        assert!(!project.join("network_ios").exists());
        assert!(!project.join(".network_ios.staging").exists());
    }

    fs::write(project.join("src/main.stasis"), "function main(: i32 {\n")
        .expect("write invalid mobile entry");
    let failed = stasis(
        &[
            "package-mobile",
            "--target",
            "android-arm64",
            "--out",
            "broken",
            "--development-build",
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
    future_receipt["schema_version"] = Value::from(3);
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
        .contains("unsupported semantic edit receipt schema version 3"));
    assert!(!fs::read_to_string(project.join("src/main.stasis"))
        .expect("source after unsupported receipt")
        .contains("function helper"));

    let mut legacy_receipt = future_receipt;
    legacy_receipt["schema_version"] = Value::from(1);
    fs::write(
        project.join("legacy-receipt.json"),
        serde_json::to_string(&legacy_receipt).expect("serialize legacy receipt"),
    )
    .expect("write legacy receipt");
    let legacy_preview = stasis(
        &[
            "--json",
            "symbol",
            "revert",
            "--receipt",
            "legacy-receipt.json",
            "--dry-run",
        ],
        &project,
    );
    assert_eq!(legacy_preview.status.code(), Some(0));
    assert_eq!(
        json_stdout(&legacy_preview)["result"]["status"],
        "revert_preview"
    );
    assert!(!fs::read_to_string(project.join("src/main.stasis"))
        .expect("source after legacy preview")
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

#[cfg(windows)]
#[test]
fn tui_live_cli_updates_mutates_and_undoes_while_process_stays_alive() {
    let parent = temp_dir("interactive_live");
    fs::create_dir_all(&parent).expect("create temp parent");
    let project = parent.join("demo");
    assert_eq!(
        stasis(&["new", "demo", "--dir", "demo"], &parent)
            .status
            .code(),
        Some(0)
    );
    fs::write(
        project.join("src/main.stasis"),
        "struct Player { hp: i32; }\nglobal score: i32;\nglobal swaps: i32;\nfunction main(): i32 { score = 1; swaps = 0; return 0; }\nfunction tick(): i32 { score += 1; return 0; }\nfunction render(): i32 { return 0; }\nfunction on_code_swap(): void { swaps += 1; return; }\nfunction damage(player: Player, amount: i32): i32 { let hero: Player; return amount; }\n",
    )
    .expect("write live project");
    fs::write(
        project.join("tests/main.test.stasis"),
        "test `live edit remains valid`(): bool { return 1 == 1; }\n",
    )
    .expect("write live test");
    fs::write(
        project.join("live.commands"),
        ":palette hrohp --owner damage --file src/main.stasis\n:palette :pa\n:complete sco\n:pause\n:update function tick src/main.stasis\nfunction tick(): i32 { score += 4; return 0; }\n:end\n:inspect swaps\n:set score 10\n:step 1\n:inspect score\n:undo\n:inspect swaps\n:step 1\n:inspect score\n:quit\n",
    )
    .expect("write live script");

    let output = stasis(
        &[
            "tui",
            "src/main.stasis",
            "--live-script",
            "live.commands",
            "--live-json",
        ],
        &project,
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let responses = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| line.starts_with('{'))
        .map(|line| serde_json::from_str::<Value>(line).expect("live response JSON"))
        .collect::<Vec<_>>();
    assert!(responses.iter().all(|response| response["ok"] == true));
    assert!(responses.iter().all(|response| response["tick"].is_u64()));
    let palettes = responses
        .iter()
        .filter(|response| response["kind"] == "palette")
        .collect::<Vec<_>>();
    assert_eq!(palettes[0]["data"]["items"][0]["text"], "hero.hp");
    assert_eq!(palettes[0]["data"]["items"][0]["kind"], "field");
    assert_eq!(palettes[1]["data"]["items"][0]["text"], ":pause");
    assert!(responses
        .iter()
        .any(|response| response["kind"] == "completion_preparing"));
    let completion = responses
        .iter()
        .find(|response| response["kind"] == "completion")
        .expect("background completion result");
    assert_eq!(completion["data"]["items"][0]["text"], "score");
    let inspected = responses
        .iter()
        .filter(|response| response["kind"] == "inspection")
        .map(|response| {
            response["data"]["value"]["value"]
                .as_i64()
                .expect("i32 value")
        })
        .collect::<Vec<_>>();
    assert_eq!(inspected, vec![1, 14, 2, 15]);
    assert!(fs::read_to_string(project.join("src/main.stasis"))
        .expect("final source")
        .contains("score += 1"));

    fs::write(
        project.join("failed-live.commands"),
        ":pause\n:inspect missing_global\n:quit\n",
    )
    .expect("write failing live script");
    let failed = stasis(
        &[
            "tui",
            "src/main.stasis",
            "--live-script",
            "failed-live.commands",
            "--live-json",
        ],
        &project,
    );
    assert_eq!(
        failed.status.code(),
        Some(1),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&failed.stdout),
        String::from_utf8_lossy(&failed.stderr)
    );
    assert!(String::from_utf8_lossy(&failed.stdout)
        .lines()
        .filter(|line| line.starts_with('{'))
        .map(|line| serde_json::from_str::<Value>(line).expect("failed live response JSON"))
        .any(|response| response["ok"] == false));

    fs::write(
        project.join("human-live.commands"),
        ":palette hrohp --owner damage --file src/main.stasis\n:pause\n:inspect score\n:status\n:quit\n",
    )
    .expect("write human live script");
    let human = stasis(
        &[
            "tui",
            "src/main.stasis",
            "--live-script",
            "human-live.commands",
        ],
        &project,
    );
    assert_eq!(
        human.status.code(),
        Some(0),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&human.stdout),
        String::from_utf8_lossy(&human.stderr)
    );
    let human_stdout = String::from_utf8_lossy(&human.stdout);
    assert!(human_stdout.contains("paused"));
    assert!(human_stdout.contains("hero.hp  field"));
    assert!(human_stdout.contains("score: i32 ="));
    assert!(human_stdout.contains("edits 0/0"));
    assert!(human_stdout.contains("session closed"));
    assert!(!human_stdout.contains("@ tick"));
    assert!(!human_stdout.contains("[live tick"));
    assert!(!human_stdout.contains("{\"path\""));

    fs::write(
        project.join("unfinished-live.commands"),
        ":update function tick src/main.stasis\nfunction tick(): i32 { score += 9; return 0; }\n",
    )
    .expect("write unfinished live script");
    let unfinished = stasis(
        &[
            "tui",
            "src/main.stasis",
            "--live-script",
            "unfinished-live.commands",
        ],
        &project,
    );
    assert_eq!(
        unfinished.status.code(),
        Some(1),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&unfinished.stdout),
        String::from_utf8_lossy(&unfinished.stderr)
    );
    assert!(String::from_utf8_lossy(&unfinished.stderr)
        .contains("live script ended with unfinished multiline input"));
    fs::remove_dir_all(parent).ok();
}

#[cfg(windows)]
#[test]
fn tui_live_stdio_keeps_a_jsonl_editor_session_open() {
    let parent = temp_dir("live_stdio");
    fs::create_dir_all(&parent).expect("create temp parent");
    let project = parent.join("demo");
    assert_eq!(
        stasis(&["new", "demo", "--dir", "demo"], &parent)
            .status
            .code(),
        Some(0)
    );
    fs::write(
        project.join("src/main.stasis"),
        "global score: i32;\nfunction main(): i32 { score = 1; return 0; }\nfunction tick(): i32 { score += 1; return 0; }\nfunction render(): i32 { return 0; }\nfunction on_code_swap(): void { return; }\n",
    )
    .expect("write stdio live project");

    let output = stasis_with_stdin(
        &["tui", "src/main.stasis", "--live-stdio"],
        &project,
        concat!(
            "{\"schema_version\":1,\"request_id\":101,\"type\":\"pause\"}\n",
            "{\"schema_version\":1,\"request_id\":102,\"type\":\"inspect\",\"path\":\"score\"}\n",
            "{\"schema_version\":1,\"request_id\":103,\"type\":\"watch\",\"path\":\"score\"}\n",
            "{\"schema_version\":1,\"request_id\":104,\"type\":\"step\",\"ticks\":1}\n",
            "{\"schema_version\":1,\"request_id\":105,\"type\":\"inspect\",\"path\":\"score\"}\n",
            "{\"schema_version\":1,\"request_id\":106,\"type\":\"quit\"}\n",
        ),
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let responses = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("stdio response JSON"))
        .collect::<Vec<_>>();
    assert!(responses
        .iter()
        .all(|response| response["schema_version"] == 1));
    assert!(responses
        .iter()
        .any(|response| response["kind"] == "watch_added"));
    assert!(responses
        .iter()
        .any(|response| response["kind"] == "quitting"));
    assert!(responses
        .iter()
        .any(|response| response["request_id"] == 101));
    assert!(responses
        .iter()
        .any(|response| response["request_id"] == 106));
    let inspected = responses
        .iter()
        .filter(|response| response["kind"] == "inspection")
        .map(|response| {
            response["data"]["value"]["value"]
                .as_i64()
                .expect("i32 value")
        })
        .collect::<Vec<_>>();
    assert_eq!(inspected, vec![1, 2]);
    fs::remove_dir_all(parent).ok();
}

#[cfg(windows)]
#[test]
fn tui_discovers_entry_workspace_and_anchors_source_relative_assets() {
    let parent = temp_dir("tui_asset_root");
    let project = parent.join("demo");
    fs::create_dir_all(project.join("src")).expect("create source directory");
    fs::create_dir_all(project.join("assets")).expect("create asset directory");

    let sample = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/render_parity");
    let main_source =
        fs::read_to_string(sample.join("main.stasis")).expect("read render parity entry");
    let rooted_source = main_source.replace("\"assets/", "\"/assets/");
    fs::write(project.join("src/main.stasis"), rooted_source).expect("write rooted entry");
    fs::copy(
        sample.join("frame.stasis"),
        project.join("src/frame.stasis"),
    )
    .expect("copy frame module");
    for asset in [
        "opaque.svg",
        "translucent.svg",
        "full_canvas.svg",
        "parity.ttf",
    ] {
        fs::copy(
            sample.join("assets").join(asset),
            project.join("assets").join(asset),
        )
        .expect("copy render asset");
    }
    fs::write(
        project.join("stasis.json"),
        "{\n  \"manifest_version\": 1,\n  \"name\": \"TUI Asset Root\",\n  \"entry\": \"src/main.stasis\",\n  \"tests\": \"tests\",\n  \"output\": \"build\"\n}\n",
    )
    .expect("write manifest");
    fs::write(project.join("live.commands"), ":quit\n").expect("write live script");

    let rooted_output = stasis(
        &[
            "tui",
            "demo/src/main.stasis",
            "--live-script",
            "live.commands",
            "--live-json",
        ],
        &parent,
    );
    assert_eq!(
        rooted_output.status.code(),
        Some(0),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&rooted_output.stdout),
        String::from_utf8_lossy(&rooted_output.stderr)
    );
    assert!(String::from_utf8_lossy(&rooted_output.stdout).contains("\"kind\":\"quitting\""));
    assert!(!String::from_utf8_lossy(&rooted_output.stderr).contains("failed to open"));

    let legacy_source = main_source.replace("\"assets/", "\"../assets/");
    fs::write(project.join("src/main.stasis"), legacy_source).expect("write legacy entry");
    let output = stasis(
        &[
            "tui",
            "demo/src/main.stasis",
            "--live-script",
            "live.commands",
            "--live-json",
        ],
        &parent,
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("\"kind\":\"quitting\""));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("failed to open"));

    let manifest_entry = stasis(
        &["tui", "--live-script", "live.commands", "--live-json"],
        &project,
    );
    assert_eq!(
        manifest_entry.status.code(),
        Some(0),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&manifest_entry.stdout),
        String::from_utf8_lossy(&manifest_entry.stderr)
    );
    assert!(String::from_utf8_lossy(&manifest_entry.stdout).contains("\"kind\":\"quitting\""));
    assert!(!String::from_utf8_lossy(&manifest_entry.stderr).contains("failed to open"));

    let removed_alias = stasis(&["run", "--interactive"], &project);
    assert_eq!(removed_alias.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&removed_alias.stderr).contains("unexpected argument"));

    fs::remove_dir_all(parent).ok();
}

#[test]
#[cfg(windows)]
fn state_inspection_sample_browses_state_and_watches_live_runtime() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");

    let output = stasis(
        &[
            "tui",
            "samples/state_inspection/src/main.stasis",
            "--live-script",
            "live.commands",
        ],
        &repository,
    );

    assert_eq!(
        output.status.code(),
        Some(0),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("state.enemies [4/4]"), "{stdout}");
    assert!(stdout.contains("state: SimulationState"), "{stdout}");
    assert!(
        stdout.contains("memory: 132 bytes; snapshot: 132 bytes"),
        "{stdout}"
    );
    assert!(stdout.contains("state.enemies[1].hp: i32 = 8"), "{stdout}");
    assert!(
        stdout.contains("state.enemies[?hp >= 8]: 2 match(es)"),
        "{stdout}"
    );
    assert!(
        stdout.contains("watching state.score + state.enemies[1].hp = 18"),
        "{stdout}"
    );
    assert!(stdout.contains("session closed"), "{stdout}");
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

fn snapshot_project_bytes(root: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files = walk_files_without_git(root)
        .into_iter()
        .map(|path| {
            let relative = path
                .strip_prefix(root)
                .expect("snapshot path under root")
                .to_string_lossy()
                .replace('\\', "/");
            (relative, fs::read(path).expect("read snapshot file"))
        })
        .collect::<Vec<_>>();
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files
}

fn walk_files_without_git(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).expect("read snapshot directory") {
            let path = entry.expect("read snapshot entry").path();
            if path.file_name().and_then(|name| name.to_str()) == Some(".git") {
                continue;
            }
            if path.is_dir() {
                pending.push(path);
            } else if path.is_file() {
                files.push(path);
            }
        }
    }
    files
}
