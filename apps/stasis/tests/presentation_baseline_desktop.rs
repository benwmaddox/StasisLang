#![cfg(all(feature = "packaged-desktop-acceptance", target_os = "linux"))]

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
const EVIDENCE_DIR: &str = "STASIS_PRESENTATION_DESKTOP_EVIDENCE_DIR";
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
    repository_root().join("samples/presentation_baseline")
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("create presentation fixture directory");
    let mut entries = fs::read_dir(source)
        .expect("read presentation fixture directory")
        .collect::<Result<Vec<_>, _>>()
        .expect("enumerate presentation fixture directory");
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let file_type = entry.file_type().expect("presentation fixture file type");
        assert!(!file_type.is_symlink(), "fixture symlinks are unsupported");
        let destination = destination.join(entry.file_name());
        if file_type.is_dir() {
            copy_tree(&entry.path(), &destination);
        } else if file_type.is_file() {
            fs::copy(entry.path(), destination).expect("copy presentation fixture file");
        }
    }
}

fn copy_fixture(project: &Path) {
    fs::create_dir_all(project).expect("create presentation fixture root");
    fs::copy(
        sample_root().join("stasis.json"),
        project.join("stasis.json"),
    )
    .expect("copy presentation manifest");
    copy_tree(&sample_root().join("src"), &project.join("src"));
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
            child.kill().ok();
            child.wait().ok();
            let stdout = stdout_reader.join().expect("join timed-out stdout reader");
            let stderr = stderr_reader.join().expect("join timed-out stderr reader");
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

fn near(pixel: &image::Rgba<u8>, expected: [u8; 3], delta: u8) -> bool {
    pixel[3] >= 245 && (0..3).all(|channel| pixel[channel].abs_diff(expected[channel]) <= delta)
}

#[test]
fn packaged_presentation_baseline_initializes_poisoned_target_without_guest_clear() {
    assert_eq!(
        std::env::consts::OS,
        "linux",
        "qualification requires Linux SDL"
    );
    if let Ok(expected_arch) = std::env::var("STASIS_EXPECTED_HOST_ARCH") {
        assert_eq!(
            std::env::consts::ARCH,
            expected_arch,
            "unexpected CI host architecture"
        );
    }
    let tree = TestTree(std::env::temp_dir().join(format!(
        "stasis-presentation-desktop-{}-{}",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::SeqCst)
    )));
    let project = tree.0.join("project");
    copy_fixture(&project);
    let source = fs::read_to_string(project.join("src/main.stasis")).expect("read fixture source");
    assert!(
        !source.contains("clear("),
        "fixture must not issue a guest clear"
    );
    assert!(
        source.contains("init_window(640, 360"),
        "fixture must retain its 640x360 logical contract"
    );

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
        "presentation desktop package",
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
    let executable = package.join("presentation_baseline");
    let provenance_path = package.join("stasis_provenance.json");
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
    assert_eq!(project_provenance["manifest"]["sha256"], manifest_hash);
    assert_eq!(project_provenance["entry"]["sha256"], entry_hash);
    assert_eq!(
        project_provenance["reachable_sources"]["src/main.stasis"],
        entry_hash
    );
    assert_eq!(
        project_provenance["vendor"]["recorded_sha256"],
        project_provenance["vendor"]["actual_sha256"]
    );

    let launch_path = executable.with_file_name("presentation_baseline.launch");
    let launch_manifest = fs::read_to_string(&launch_path).expect("read packaged launch manifest");
    let lifecycle_versions = launch_manifest
        .lines()
        .filter_map(|line| line.strip_prefix("render_construction_lifecycle_version="))
        .collect::<Vec<_>>();
    assert_eq!(lifecycle_versions, vec!["1"]);

    let screenshot = tree.0.join("presentation-desktop-frame.png");
    let mut launch_command = Command::new(&executable);
    launch_command
        .current_dir(&package)
        .env("STASIS_SCREENSHOT_ONCE", &screenshot)
        .env("STASIS_SCREENSHOT_FRAME", "1")
        .env("STASIS_EXIT_AFTER_SCREENSHOT", "1")
        .env("STASIS_RECORDING_PRESENTATION", "1")
        .env("STASIS_RECORDING_WIDTH", "640")
        .env("STASIS_RECORDING_HEIGHT", "360")
        .env("STASIS_RECORDING_FPS", "60")
        .env("STASIS_ENABLE_TEST_INPUT", "1")
        .env("STASIS_TEST_PRESENTATION_POISON_ONCE", "1")
        .env("SDL_RENDER_DRIVER", "software")
        .env("SDL_AUDIODRIVER", "dummy")
        .env("STASIS_RUNNER_DIAG", "1");
    let launched = launch(
        launch_command,
        "packaged presentation desktop runtime",
        LAUNCH_TIMEOUT,
    );
    assert!(
        launched.status.success(),
        "packaged runtime failed: status={} stdout={} stderr={}",
        launched.status,
        String::from_utf8_lossy(&launched.stdout),
        String::from_utf8_lossy(&launched.stderr)
    );
    let diagnostics = format!(
        "{}\n{}",
        String::from_utf8_lossy(&launched.stdout),
        String::from_utf8_lossy(&launched.stderr)
    );
    assert!(diagnostics.contains("render_construction_lifecycle_version=1"));
    assert!(diagnostics
        .contains("RUNNER_DIAG: presentation_poison target=physical-window state_restored=1"));
    assert!(
        screenshot.is_file(),
        "packaged runtime did not capture a frame"
    );

    let frame = image::open(&screenshot)
        .expect("decode packaged presentation frame")
        .to_rgba8();
    assert_eq!((frame.width(), frame.height()), (640, 360));
    let mut background_samples = 0_u64;
    let mut background_black = 0_u64;
    let mut red_samples = 0_u64;
    let mut red_matches = 0_u64;
    let mut magenta_pixels = 0_u64;
    for (x, y, pixel) in frame.enumerate_pixels() {
        if pixel[0] > 180 && pixel[1] < 80 && pixel[2] > 180 {
            magenta_pixels += 1;
        }
        if (80..240).contains(&x) && (45..135).contains(&y) {
            red_samples += 1;
            if near(pixel, [230, 38, 20], 32) {
                red_matches += 1;
            }
        } else if x % 3 == 0 && y % 3 == 0 {
            background_samples += 1;
            if near(pixel, [0, 0, 0], 8) {
                background_black += 1;
            }
        }
    }
    assert_eq!(magenta_pixels, 0, "physical target retained poison pixels");
    assert!(
        red_matches * 100 >= red_samples * 95,
        "red scene coverage is too low"
    );
    assert!(
        background_black * 1000 >= background_samples * 995,
        "logical background is not deterministic black"
    );

    if let Some(evidence) = std::env::var_os(EVIDENCE_DIR).map(PathBuf::from) {
        fs::create_dir_all(&evidence).expect("create presentation evidence directory");
        fs::copy(&screenshot, evidence.join("presentation-desktop-frame.png"))
            .expect("copy presentation frame evidence");
        fs::copy(
            &provenance_path,
            evidence.join("presentation-desktop-provenance.json"),
        )
        .expect("copy presentation provenance evidence");
        fs::write(
            evidence.join("presentation-desktop-runtime.log"),
            &diagnostics,
        )
        .expect("write presentation runtime log");
        let receipt = json!({
            "schema": "stasis.presentation_baseline_desktop.v1",
            "host": { "os": std::env::consts::OS, "arch": std::env::consts::ARCH },
            "entry": "src/main.stasis",
            "logical": [640, 360],
            "guest_clear": false,
            "poison_target": "physical-window",
            "poison_state_restored": true,
            "render_construction_lifecycle": {
                "version": 1,
                "owner": "non_monolithic_generated_bridge",
                "launch_manifest": launch_manifest,
            },
            "frame": {
                "width": frame.width(), "height": frame.height(),
                "logical_background_black_samples": [background_black, background_samples],
                "red_scene_samples": [red_matches, red_samples],
                "poisoned_magenta_pixels": magenta_pixels,
                "sha256": sha256(&screenshot),
            },
            "artifacts": {
                "source_sha256": entry_hash,
                "executable_sha256": sha256(&executable),
                "package_sha256": package_sha256(&package),
                "provenance_sha256": sha256(&provenance_path),
            },
            "payload": { "contains_source": false, "contains_compiler": false },
            "scope": {
                "hosted_linux_software_sdl": true,
                "all_gpu_compositors_qualified": false,
            },
            "launch": { "status": launched.status.code() },
        });
        fs::write(
            evidence.join("presentation-desktop-receipt.json"),
            serde_json::to_vec_pretty(&receipt).expect("serialize presentation receipt"),
        )
        .expect("write presentation receipt");
    }
}
