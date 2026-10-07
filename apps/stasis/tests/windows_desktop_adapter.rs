#![cfg(windows)]

use serde_json::Value;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);
const PACKAGE_TIMEOUT: Duration = Duration::from_secs(300);
const RUN_TIMEOUT: Duration = Duration::from_secs(60);

struct CompletedProcess {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

struct TestTree(PathBuf);

impl Drop for TestTree {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).ok();
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

fn finish_child(mut child: Child, description: &str, timeout: Duration) -> CompletedProcess {
    let stdout_pipe = child.stdout.take().expect("capture child stdout");
    let stderr_pipe = child.stderr.take().expect("capture child stderr");
    let stdout_reader = thread::spawn(move || {
        let mut output = Vec::new();
        let mut pipe = stdout_pipe;
        pipe.read_to_end(&mut output).expect("read child stdout");
        output
    });
    let stderr_reader = thread::spawn(move || {
        let mut output = Vec::new();
        let mut pipe = stderr_pipe;
        pipe.read_to_end(&mut output).expect("read child stderr");
        output
    });
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll child") {
            break status;
        }
        if started.elapsed() >= timeout {
            child.kill().ok();
            child.wait().ok();
            let stdout = stdout_reader.join().expect("join stdout reader");
            let stderr = stderr_reader.join().expect("join stderr reader");
            panic!(
                "{description} exceeded {} seconds: stdout={} stderr={}",
                timeout.as_secs(),
                String::from_utf8_lossy(&stdout),
                String::from_utf8_lossy(&stderr)
            );
        }
        thread::sleep(Duration::from_millis(25));
    };
    CompletedProcess {
        status,
        stdout: stdout_reader.join().expect("join stdout reader"),
        stderr: stderr_reader.join().expect("join stderr reader"),
    }
}

fn run_command(mut command: Command, description: &str, timeout: Duration) -> CompletedProcess {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let child = command
        .spawn()
        .unwrap_or_else(|error| panic!("start {description}: {error}"));
    finish_child(child, description, timeout)
}

fn assert_success(description: &str, completed: &CompletedProcess) {
    assert!(
        completed.status.success(),
        "{description} failed with {:?}\nstdout={}\nstderr={}",
        completed.status.code(),
        String::from_utf8_lossy(&completed.stdout),
        String::from_utf8_lossy(&completed.stderr)
    );
}

fn materialize_toolchain_stdlib(project: &Path) {
    copy_tree(
        &repository_root().join("src/stdlib"),
        &project.join(".stasis_cache/toolchain/src/stdlib"),
    );
}

fn run_generation(root: &Path, toolchain: &Path, generation: u32) -> (Value, Value) {
    let project = root.join(format!("generation-{generation}"));
    copy_tree(
        &repository_root().join("tests/fixtures/windows_desktop_adapter_package"),
        &project,
    );
    materialize_toolchain_stdlib(&project);

    let mut package = Command::new(toolchain);
    package
        .current_dir(&project)
        .args(["package", "--target", "desktop", "--development-build"]);
    let package = run_command(
        package,
        &format!("desktop adapter package generation {generation}"),
        PACKAGE_TIMEOUT,
    );
    assert_success("desktop adapter package", &package);

    let package_root = project.join("dist/desktop_adapter_acceptance-desktop");
    let executable = package_root.join("desktop_adapter_acceptance.exe");
    let receipt_path = root.join(format!("generation-{generation}-receipt.json"));
    assert!(executable.is_file(), "missing packaged executable");
    let mut launch = Command::new(&executable);
    launch
        .current_dir(&package_root)
        .env("STASIS_ADAPTER_RECEIPT", &receipt_path);
    let launch = run_command(
        launch,
        &format!("desktop adapter package generation {generation}"),
        RUN_TIMEOUT,
    );
    assert_success("desktop adapter packaged executable", &launch);

    let receipt: Value = serde_json::from_slice(
        &fs::read(&receipt_path).expect("adapter shutdown receipt must exist"),
    )
    .expect("parse adapter shutdown receipt");
    assert_eq!(receipt["schema"], "stasis.desktop_adapter.acceptance.v1");
    assert_eq!(receipt["abi_version"], 1);
    assert_eq!(receipt["struct_size"], 32);
    assert_eq!(receipt["platform"], 1);
    assert_eq!(receipt["window_kind"], 1);
    assert_eq!(receipt["ownership"], 1);
    assert_eq!(receipt["hwnd_valid"], true);
    assert_eq!(receipt["initialize_count"], 1);
    assert_eq!(receipt["async_delivered"], 1);
    assert_eq!(receipt["foreground_count"], 1);
    assert_eq!(receipt["shutdown_count"], 1);
    assert_eq!(receipt["mailbox_failures"], 0);
    assert_eq!(receipt["same_thread"], true);
    let pumps = receipt["pump_count"].as_i64().expect("numeric pump count");
    assert!((6..=300).contains(&pumps), "bounded pump count: {pumps}");
    assert_eq!(
        receipt["mailbox_serial"].as_i64(),
        Some(pumps + 4),
        "initialize, async, foreground, and shutdown each publish once"
    );

    let provenance: Value = serde_json::from_slice(
        &fs::read(package_root.join("app/stasis_provenance.json"))
            .expect("desktop package provenance"),
    )
    .expect("parse desktop package provenance");
    let adapter = provenance["desktop_package"]["native_adapter"].clone();
    assert_eq!(adapter["abi_version"], 1);
    assert_eq!(adapter["source"], "native/desktop_adapter.c");
    assert_eq!(
        adapter["system_links"]["windows"],
        serde_json::json!(["user32"])
    );
    assert_eq!(
        adapter["sha256"].as_str().map(str::len),
        Some(64),
        "adapter input must have deterministic SHA-256 provenance"
    );
    (receipt, adapter)
}

#[test]
fn two_fresh_desktop_packages_honor_the_adapter_lifecycle_and_mailbox_contract() {
    let test_tree = TestTree(
        repository_root()
            .join("target/windows-desktop-adapter-tests")
            .join(format!(
                "adapter-{}-{}",
                std::process::id(),
                NEXT_TEMP.fetch_add(1, Ordering::SeqCst)
            )),
    );
    fs::create_dir_all(&test_tree.0).expect("create adapter acceptance root");
    let toolchain = test_tree.0.join("source-stasis.exe");
    fs::copy(env!("CARGO_BIN_EXE_stasis"), &toolchain)
        .expect("copy fresh CLI away from any stale sibling runtime");
    let (_, first_adapter) = run_generation(&test_tree.0, &toolchain, 1);
    let (_, second_adapter) = run_generation(&test_tree.0, &toolchain, 2);
    assert_eq!(
        first_adapter, second_adapter,
        "clean regenerations must capture identical adapter provenance"
    );
}
