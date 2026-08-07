#![cfg(windows)]

use image::GenericImageView;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);
const TIMEOUT: Duration = Duration::from_secs(60);
const RELEASE_BUILD_TIMEOUT: Duration = Duration::from_secs(180);

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
        .unwrap()
}

fn temp_dir(name: &str) -> PathBuf {
    repository_root()
        .join("target/windows-launch-tests")
        .join(format!(
            "stasis_windows_launch_{name}_{}_{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::SeqCst)
        ))
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
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll child") {
            break status;
        }
        if started.elapsed() >= timeout {
            child.kill().ok();
            child.wait().ok();
            panic!("{description} exceeded {} seconds", timeout.as_secs());
        }
        thread::sleep(Duration::from_millis(25));
    };
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_end(&mut stdout)
        .unwrap();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_end(&mut stderr)
        .unwrap();
    CompletedProcess {
        status,
        stdout,
        stderr,
    }
}

fn launch(mut command: Command, description: &str) -> CompletedProcess {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let child = command
        .spawn()
        .unwrap_or_else(|error| panic!("start {description}: {error}"));
    finish_child(child, description, TIMEOUT)
}

fn launch_release_build(mut command: Command) -> CompletedProcess {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let child = command
        .spawn()
        .unwrap_or_else(|error| panic!("start release build: {error}"));
    finish_child(child, "release build", RELEASE_BUILD_TIMEOUT)
}

fn configure_capture(command: &mut Command, screenshot: &Path, exit_after: bool) {
    command
        .env("STASIS_SCREENSHOT_ONCE", screenshot)
        .env("STASIS_SCREENSHOT_FRAME", "2")
        .env("STASIS_GFX_LOG_SPRITES", "1");
    if exit_after {
        command.env("STASIS_EXIT_AFTER_SCREENSHOT", "1");
    } else {
        command.env_remove("STASIS_EXIT_AFTER_SCREENSHOT");
    }
}

fn assert_launch(description: &str, completed: CompletedProcess, screenshot: &Path) {
    let stdout = String::from_utf8_lossy(&completed.stdout);
    let stderr = String::from_utf8_lossy(&completed.stderr);
    assert!(
        completed.status.success(),
        "{description} failed with {:?}\nstdout={stdout}\nstderr={stderr}",
        completed.status.code()
    );
    let log = format!("{stdout}\n{stderr}").replace('\\', "/");
    assert!(
        log.contains("Stasis renderer loading screen presented:"),
        "{description} did not present the cross-platform startup loading frame: {log}"
    );
    assert!(
        log.contains("assets/smoke.ttf"),
        "{description} did not load the smoke font: {log}"
    );
    assert!(
        log.contains("Stasis render contract v2"),
        "{description} did not submit a frame"
    );
    let image = image::open(screenshot)
        .unwrap_or_else(|error| panic!("{description} did not capture a PNG: {error}"));
    assert_eq!(image.dimensions(), (320, 180), "{description} capture size");
    let rgba = image.to_rgba8();
    let png_pixel = rgba.get_pixel(80, 62).0;
    let svg_pixel = rgba.get_pixel(220, 50).0;
    assert!(
        png_pixel[0] > 180 && png_pixel[2] > 180,
        "{description} PNG pixel {png_pixel:?}"
    );
    assert!(svg_pixel[1] > 150, "{description} SVG pixel {svg_pixel:?}");
}

fn app_control_blocked(completed: &CompletedProcess) -> bool {
    !completed.status.success()
        && String::from_utf8_lossy(&completed.stderr)
            .contains("err=4551 An Application Control policy has blocked this file")
}

fn stasis_command(project: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_stasis"));
    let root = repository_root();
    command.current_dir(project);
    let runner = root.join("runtime/build/bin/Release/stasis_runner.exe");
    if std::env::var_os("STASIS_RUNTIME_RUNNER_PATH").is_none() && runner.is_file() {
        command.env("STASIS_RUNTIME_RUNNER_PATH", runner);
    }
    let graphics = root.join("runtime/build/bin/Release/stasis_graphics.dll");
    if std::env::var_os("STASIS_RUNTIME_DLL_PATH").is_none() && graphics.is_file() {
        command.env("STASIS_RUNTIME_DLL_PATH", graphics);
    }
    command
}

#[test]
fn every_supported_windows_game_launch_path_loads_assets_and_renders() {
    let root = repository_root();
    let fixture = root.join("samples/windows_launch_smoke");
    let test_tree = TestTree(temp_dir("matrix"));
    let parent = &test_tree.0;
    let project = parent.join("windows_launch_smoke");
    copy_tree(&fixture, &project);

    for case in ["play", "run_watch", "tui"] {
        let screenshot = parent.join(format!("{case}.png"));
        let mut command = stasis_command(&project);
        match case {
            "play" => {
                command.args([
                    "play",
                    "main.stasis",
                    "--ticks",
                    "2",
                    "--screenshot-frame",
                    "2",
                    "--screenshot",
                    screenshot.to_str().unwrap(),
                    "--exit-after-screenshot",
                ]);
            }
            "run_watch" => {
                command.args(["run", "--watch"]);
                configure_capture(&mut command, &screenshot, true);
            }
            "tui" => {
                command.args([
                    "tui",
                    "main.stasis",
                    "--live-script",
                    "live.commands",
                    "--live-json",
                ]);
                configure_capture(&mut command, &screenshot, false);
            }
            _ => unreachable!(),
        }
        assert_launch(case, launch(command, case), &screenshot);
    }

    let release = launch_release_build({
        let mut command = stasis_command(&project);
        command.args(["build", "--mode", "release"]);
        command
    });
    assert!(
        release.status.success(),
        "release build failed: {}",
        String::from_utf8_lossy(&release.stderr)
    );
    let release_screenshot = parent.join("release.png");
    let mut release_command = Command::new(project.join("build/windows_launch_smoke.exe"));
    release_command.current_dir(&project);
    configure_capture(&mut release_command, &release_screenshot, true);
    let release_run = launch(release_command, "release executable");
    let release_blocked = app_control_blocked(&release_run);
    if release_blocked {
        eprintln!("release executable was blocked by Windows Application Control (error 4551)");
    } else {
        assert_launch("release executable", release_run, &release_screenshot);
    }

    let package = launch(
        {
            let mut command = stasis_command(&project);
            command.args(["package", "--target", "desktop", "--development-build"]);
            command
        },
        "desktop package",
    );
    assert!(
        package.status.success(),
        "desktop package failed: {}",
        String::from_utf8_lossy(&package.stderr)
    );
    let package_root = project.join("dist/windows_launch_smoke-desktop");
    let package_screenshot = parent.join("package.png");
    let mut package_command = Command::new(package_root.join("windows_launch_smoke.exe"));
    package_command.current_dir(&package_root);
    configure_capture(&mut package_command, &package_screenshot, true);
    let package_run = launch(package_command, "packaged executable");
    let package_blocked = app_control_blocked(&package_run);
    if package_blocked {
        eprintln!("packaged executable was blocked by Windows Application Control (error 4551)");
    } else {
        assert_launch("packaged executable", package_run, &package_screenshot);
    }
    assert_eq!(
        release_blocked, package_blocked,
        "Application Control must not hide a failure in only one AOT launch path"
    );
}
