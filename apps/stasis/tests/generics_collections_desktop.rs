#![cfg(feature = "packaged-desktop-acceptance")]

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);
const EVIDENCE_DIR: &str = "STASIS_GENERICS_DESKTOP_EVIDENCE_DIR";
const PACKAGE_TIMEOUT: Duration = Duration::from_secs(8 * 60);
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(90);

struct TestTree(PathBuf);

impl Drop for TestTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct CompletedProcess {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("canonical repository root")
}

fn sample_root() -> PathBuf {
    repository_root().join("samples/generics_collections")
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("create generics desktop fixture directory");
    let mut entries = fs::read_dir(source)
        .expect("read generics desktop fixture directory")
        .collect::<Result<Vec<_>, _>>()
        .expect("enumerate generics desktop fixture directory");
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let file_type = entry.file_type().expect("generics fixture file type");
        assert!(!file_type.is_symlink(), "fixture symlinks are unsupported");
        let destination = destination.join(entry.file_name());
        if file_type.is_dir() {
            copy_tree(&entry.path(), &destination);
        } else if file_type.is_file() {
            fs::copy(entry.path(), destination).expect("copy generics desktop fixture file");
        }
    }
}

fn copy_fixture(project: &Path) {
    fs::create_dir_all(project).expect("create generics desktop fixture root");
    fs::copy(
        sample_root().join("stasis.json"),
        project.join("stasis.json"),
    )
    .expect("copy generics manifest");
    for relative in ["assets", "src", "vendor"] {
        copy_tree(&sample_root().join(relative), &project.join(relative));
    }
}

fn finish_child(mut child: Child, description: &str, timeout: Duration) -> CompletedProcess {
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let stdout_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).expect("read child stdout");
        bytes
    });
    let stderr_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).expect("read child stderr");
        bytes
    });
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll child") {
            break status;
        }
        if started.elapsed() >= timeout {
            #[cfg(windows)]
            let _ = Command::new("taskkill.exe")
                .args(["/PID", &child.id().to_string(), "/T", "/F"])
                .status();
            #[cfg(not(windows))]
            child.kill().ok();
            child.wait().ok();
            let stdout = stdout_reader
                .join()
                .expect("join timed-out child stdout reader");
            let stderr = stderr_reader
                .join()
                .expect("join timed-out child stderr reader");
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
        stdout: stdout_reader.join().expect("join child stdout reader"),
        stderr: stderr_reader.join().expect("join child stderr reader"),
    }
}

fn launch(mut command: Command, description: &str, timeout: Duration) -> CompletedProcess {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let child = command
        .spawn()
        .unwrap_or_else(|error| panic!("start {description}: {error}"));
    finish_child(child, description, timeout)
}

fn packaged_executable(package: &Path) -> PathBuf {
    if cfg!(windows) {
        package.join("generics_collections.exe")
    } else if cfg!(target_os = "macos") {
        package.join("generics_collections.app/Contents/MacOS/generics_collections")
    } else {
        package.join("generics_collections")
    }
}

fn payload_root(package: &Path) -> PathBuf {
    if cfg!(windows) {
        package.join("app")
    } else {
        package.to_path_buf()
    }
}

fn sha256(path: &Path) -> String {
    format!(
        "{:x}",
        Sha256::digest(
            fs::read(path)
                .unwrap_or_else(|error| panic!("read artifact {}: {error}", path.display()))
        )
    )
}

fn package_sha256(root: &Path) -> String {
    fn files(root: &Path, directory: &Path, output: &mut Vec<PathBuf>) {
        let mut entries = fs::read_dir(directory)
            .expect("read desktop package directory")
            .collect::<Result<Vec<_>, _>>()
            .expect("enumerate desktop package directory");
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            if path.is_dir() {
                files(root, &path, output);
            } else if path.is_file() {
                output.push(
                    path.strip_prefix(root)
                        .expect("package-relative file")
                        .to_path_buf(),
                );
            }
        }
    }
    let mut paths = Vec::new();
    files(root, root, &mut paths);
    paths.sort();
    let mut digest = Sha256::new();
    for relative in paths {
        digest.update(relative.to_string_lossy().replace('\\', "/").as_bytes());
        digest.update([0]);
        digest.update(fs::read(root.join(&relative)).expect("read package hash input"));
        digest.update([0]);
    }
    format!("{:x}", digest.finalize())
}

fn assert_binary_only_package(package: &Path) {
    fn inspect(path: &Path) {
        for entry in fs::read_dir(path).expect("read package closure") {
            let entry = entry.expect("package closure entry");
            let child = entry.path();
            let file_type = entry.file_type().expect("package closure file type");
            assert!(
                !file_type.is_symlink(),
                "package closure contains a symlink"
            );
            let name = child
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or_default();
            if file_type.is_dir() {
                assert!(
                    !matches!(name, "src" | "crates" | "tests"),
                    "desktop package leaked source tree {}",
                    child.display()
                );
                inspect(&child);
            } else {
                let extension = child.extension().and_then(|value| value.to_str());
                assert!(
                    !matches!(extension, Some("stasis" | "rs" | "rlib" | "rmeta" | "toml")),
                    "desktop package leaked source or compiler metadata: {}",
                    child.display()
                );
                assert_ne!(
                    name, "Cargo.toml",
                    "desktop package leaked compiler metadata"
                );
                assert_ne!(
                    name, "Cargo.lock",
                    "desktop package leaked compiler metadata"
                );
                assert!(
                    !matches!(
                        name,
                        "stasis" | "stasis.exe" | "rustc" | "rustc.exe" | "cargo" | "cargo.exe"
                    ),
                    "desktop package leaked a compiler tool: {}",
                    child.display()
                );
            }
        }
    }
    inspect(package);
}

#[test]
fn full_generics_desktop_package_launches_with_provenance_and_digest_frame() {
    if let Ok(expected_arch) = std::env::var("STASIS_EXPECTED_HOST_ARCH") {
        assert_eq!(
            std::env::consts::ARCH,
            expected_arch,
            "unexpected CI host architecture"
        );
    }
    let tree = TestTree(std::env::temp_dir().join(format!(
        "stasis-generics-desktop-{}-{}",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::SeqCst)
    )));
    let project = tree.0.join("project");
    copy_fixture(&project);

    let mut package_command = Command::new(env!("CARGO_BIN_EXE_stasis"));
    package_command
        .args([
            "package",
            "--target",
            "desktop",
            "--development-build",
            "--out",
            "dist",
            "--json",
        ])
        .current_dir(&project);
    let packaged = launch(
        package_command,
        "full generics desktop package",
        PACKAGE_TIMEOUT,
    );
    assert!(
        packaged.status.success(),
        "desktop package failed: status={} stdout={} stderr={}",
        packaged.status,
        String::from_utf8_lossy(&packaged.stdout),
        String::from_utf8_lossy(&packaged.stderr)
    );

    let package = project.join("dist");
    let payload = payload_root(&package);
    let executable = packaged_executable(&package);
    let provenance_path = payload.join("stasis_provenance.json");
    assert!(
        executable.is_file(),
        "missing packaged executable {}",
        executable.display()
    );
    assert!(provenance_path.is_file(), "missing package provenance");
    assert_binary_only_package(&package);

    let provenance: Value = serde_json::from_slice(
        &fs::read(&provenance_path).expect("read desktop package provenance"),
    )
    .expect("parse desktop package provenance");
    assert_eq!(provenance["schema"], "stasis.release_provenance.v1");
    assert_eq!(provenance["development_build"], true);
    let manifest_hash = sha256(&project.join("stasis.json"));
    let entry_hash = sha256(&project.join("src/main.stasis"));
    let project_provenance = &provenance["desktop_package"]["project"];
    assert_eq!(project_provenance["manifest"]["path"], "stasis.json");
    assert_eq!(project_provenance["manifest"]["sha256"], manifest_hash);
    assert_eq!(project_provenance["entry"]["path"], "src/main.stasis");
    assert_eq!(project_provenance["entry"]["sha256"], entry_hash);
    assert_eq!(
        project_provenance["reachable_sources"]["src/main.stasis"],
        entry_hash
    );
    assert!(
        project_provenance["reachable_sources"]["vendor/stasis/stdlib/graphics.stasis"]
            .as_str()
            .is_some_and(|hash| hash.len() == 64),
        "desktop provenance omitted reachable graphics source"
    );
    assert_eq!(
        project_provenance["vendor"]["recorded_sha256"],
        project_provenance["vendor"]["actual_sha256"]
    );

    let executable_name = executable
        .file_name()
        .and_then(|value| value.to_str())
        .expect("packaged executable file name");
    let launch_path = if cfg!(windows) {
        payload.join(format!("{executable_name}.launch"))
    } else {
        executable.with_file_name(format!("{executable_name}.launch"))
    };
    let (launch_manifest, lifecycle_owner) = if cfg!(windows) {
        assert!(
            !launch_path.is_file(),
            "Windows monolithic package must not require a runner launch sidecar: {}",
            launch_path.display()
        );
        (None, "windows_monolithic_generated_bindings")
    } else {
        let launch_manifest =
            fs::read_to_string(&launch_path).expect("read packaged desktop launch manifest");
        let lifecycle_versions = launch_manifest
            .lines()
            .filter_map(|line| line.strip_prefix("render_construction_lifecycle_version="))
            .collect::<Vec<_>>();
        assert_eq!(
            lifecycle_versions,
            vec!["1"],
            "packaged desktop launch manifest must contain exactly one authoritative lifecycle-v1 entry: {}",
            launch_path.display()
        );
        (Some(launch_manifest), "non_monolithic_generated_bridge")
    };

    let screenshot = tree.0.join("desktop-frame.png");
    let mut launch_command = Command::new(&executable);
    launch_command
        .current_dir(&package)
        .env("STASIS_SCREENSHOT_ONCE", &screenshot)
        .env("STASIS_SCREENSHOT_FRAME", "2")
        .env("STASIS_EXIT_AFTER_SCREENSHOT", "1")
        .env("STASIS_RECORDING_PRESENTATION", "1")
        .env("STASIS_RECORDING_WIDTH", "640")
        .env("STASIS_RECORDING_HEIGHT", "360")
        .env("STASIS_RECORDING_FPS", "60")
        .env("SDL_RENDER_DRIVER", "software")
        .env("SDL_AUDIODRIVER", "dummy")
        .env("STASIS_RUNNER_DIAG", "1");
    let launched = launch(
        launch_command,
        "packaged generics desktop runtime",
        LAUNCH_TIMEOUT,
    );
    assert!(
        launched.status.success(),
        "packaged runtime failed (Windows Application Control is a blocker, not a skip): status={} stdout={} stderr={}",
        launched.status,
        String::from_utf8_lossy(&launched.stdout),
        String::from_utf8_lossy(&launched.stderr)
    );
    let runner_diagnostics = format!(
        "{}\n{}",
        String::from_utf8_lossy(&launched.stdout),
        String::from_utf8_lossy(&launched.stderr)
    );
    let lifecycle_diagnostic = if cfg!(windows) {
        assert!(
            !runner_diagnostics.contains("invalid_magic"),
            "Windows monolithic render reported an invalid command header; diagnostics={runner_diagnostics}"
        );
        None
    } else {
        let expected = "RUNNER_DIAG: render_construction_lifecycle_version=1";
        assert!(
            runner_diagnostics.contains(expected),
            "packaged runner did not report lifecycle-v1 ownership; diagnostics={runner_diagnostics}"
        );
        Some(expected)
    };
    assert!(
        screenshot.is_file(),
        "packaged runtime did not capture a frame"
    );
    let image = image::open(&screenshot)
        .expect("decode packaged desktop frame")
        .to_rgba8();
    let mut teal_pixels = 0_u64;
    let mut failure_pixels = 0_u64;
    let mut background_pixels = 0_u64;
    for pixel in image.pixels() {
        if pixel[1] > 130 && pixel[1] > pixel[0].saturating_add(60) && pixel[2] > 70 {
            teal_pixels += 1;
        }
        if pixel[0] > 140 && pixel[0] > pixel[1].saturating_add(80) {
            failure_pixels += 1;
        }
        if pixel[2] > pixel[0].saturating_add(15) && pixel[2] > pixel[1] {
            background_pixels += 1;
        }
    }
    let frame_pixels = u64::from(image.width()) * u64::from(image.height());
    assert!(
        teal_pixels > frame_pixels / 100,
        "digest-success rectangle missing: {teal_pixels}/{frame_pixels} pixels"
    );
    assert!(
        failure_pixels < 100,
        "digest-failure rectangle rendered: {failure_pixels}"
    );
    assert!(
        background_pixels > frame_pixels / 2,
        "authored dark-blue background missing: {background_pixels}/{frame_pixels} pixels"
    );

    if let Some(evidence) = std::env::var_os(EVIDENCE_DIR).map(PathBuf::from) {
        fs::create_dir_all(&evidence).expect("create desktop package evidence directory");
        fs::copy(&screenshot, evidence.join("desktop-frame.png"))
            .expect("copy desktop frame evidence");
        fs::copy(
            &provenance_path,
            evidence.join("desktop-package-provenance.json"),
        )
        .expect("copy desktop provenance evidence");
        let receipt = json!({
            "schema": "stasis.generics_collections_desktop_acceptance.v1",
            "host": { "os": std::env::consts::OS, "arch": std::env::consts::ARCH },
            "entry": "src/main.stasis",
            "results": { "main": 0, "tick": 0, "state_digest": 507 },
            "digest_evidence": "the production render emits teal only when generics_collections_digest_value is exactly 507",
            "render_construction_lifecycle": {
                "version": 1,
                "owner": lifecycle_owner,
                "launch_manifest": launch_manifest,
                "runner_diagnostic": lifecycle_diagnostic,
            },
            "frame": {
                "width": image.width(), "height": image.height(),
                "teal_pixels": teal_pixels, "failure_pixels": failure_pixels,
                "background_pixels": background_pixels,
                "sha256": sha256(&screenshot),
            },
            "artifacts": {
                "executable_sha256": sha256(&executable),
                "package_sha256": package_sha256(&package),
                "provenance_sha256": sha256(&provenance_path),
            },
            "payload": { "contains_source": false, "contains_compiler": false },
            "launch": {
                "status": launched.status.code(),
                "stdout": String::from_utf8_lossy(&launched.stdout),
                "stderr": String::from_utf8_lossy(&launched.stderr),
            }
        });
        fs::write(
            evidence.join("desktop-package-receipt.json"),
            serde_json::to_vec_pretty(&receipt).expect("serialize desktop package receipt"),
        )
        .expect("write desktop package receipt");
    }
}
