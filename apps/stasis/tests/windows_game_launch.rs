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

fn ffmpeg_directory(root: &Path) -> Option<PathBuf> {
    let code_root = root.parent().and_then(Path::parent).unwrap_or(root);
    for bundled in [
        code_root.join("ffmpeg-9.0.1-essentials_build/bin"),
        code_root.join("ffmpeg-9.0.1-essentials_build/ffmpeg-9.0.1-essentials_build/bin"),
    ] {
        if bundled.join("ffmpeg.exe").is_file() && bundled.join("ffprobe.exe").is_file() {
            return Some(bundled);
        }
    }
    let ffmpeg = Command::new("ffmpeg").arg("-version").output().ok();
    let ffprobe = Command::new("ffprobe").arg("-version").output().ok();
    if ffmpeg
        .as_ref()
        .is_some_and(|output| output.status.success())
        && ffprobe
            .as_ref()
            .is_some_and(|output| output.status.success())
    {
        Some(PathBuf::from(""))
    } else {
        None
    }
}

fn add_ffmpeg_path(command: &mut Command, directory: &Path) {
    if !directory.as_os_str().is_empty() {
        let current = std::env::var_os("PATH").unwrap_or_default();
        let mut paths = std::env::split_paths(&current).collect::<Vec<_>>();
        paths.insert(0, directory.to_path_buf());
        if let Ok(path) = std::env::join_paths(paths) {
            command.env("PATH", path);
        }
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

fn parse_dimensions(log: &str, key: &str) -> (u32, u32) {
    let value = log
        .split_whitespace()
        .find_map(|part| part.strip_prefix(key))
        .unwrap_or_else(|| panic!("missing {key} in presentation log: {log}"));
    let (width, height) = value
        .trim_end_matches(|character: char| !character.is_ascii_digit())
        .split_once('x')
        .unwrap_or_else(|| panic!("invalid {key} dimensions: {value}"));
    (
        width.parse().expect("numeric presentation width"),
        height.parse().expect("numeric presentation height"),
    )
}

fn assert_maximized_portrait(description: &str, completed: CompletedProcess, screenshot: &Path) {
    let stdout = String::from_utf8_lossy(&completed.stdout);
    let stderr = String::from_utf8_lossy(&completed.stderr);
    assert!(
        completed.status.success(),
        "{description} failed with {:?}\nstdout={stdout}\nstderr={stderr}",
        completed.status.code()
    );
    let log = format!("{stdout}\n{stderr}");
    let presentation = log
        .lines()
        .filter(|line| line.contains("Stasis window presentation: mode=maximized"))
        .last()
        .unwrap_or_else(|| panic!("{description} did not maximize its desktop window: {log}"));
    let mut modes = Vec::new();
    for line in log
        .lines()
        .filter(|line| line.contains("Stasis window presentation: mode="))
    {
        let mode = line
            .split_whitespace()
            .find_map(|part| part.strip_prefix("mode="))
            .expect("presentation mode token");
        if modes.last().copied() != Some(mode) {
            modes.push(mode);
        }
    }
    assert_eq!(
        modes,
        [
            "maximized",
            "fullscreen",
            "maximized",
            "windowed",
            "maximized"
        ],
        "{description} should apply tick requests at distinct host boundaries"
    );
    assert_eq!(
        parse_dimensions(presentation, "logical="),
        (360, 720),
        "{description} logical canvas"
    );
    let native = parse_dimensions(presentation, "native=");
    let bounds = parse_dimensions(presentation, "bounds=");
    let usable = parse_dimensions(presentation, "usable=");
    assert_eq!(
        native.0, usable.0,
        "{description} client width should fill the usable work area"
    );
    assert!(
        native.1 <= usable.1
            && usable.1 - native.1 < 128
            && bounds.0 >= usable.0
            && bounds.1 >= usable.1,
        "{description} should use normal maximized chrome inside the usable work area: native={native:?} bounds={bounds:?} usable={usable:?}"
    );

    let image = image::open(screenshot)
        .unwrap_or_else(|error| panic!("{description} did not capture a PNG: {error}"))
        .to_rgba8();
    let drawable = parse_dimensions(presentation, "drawable=");
    assert_eq!(
        image.height(),
        drawable.1,
        "{description} portrait content should use the full drawable height"
    );
    let aspect_error = (image.width() * 720).abs_diff(image.height() * 360);
    assert!(
        aspect_error <= 360,
        "{description} capture should preserve the 360x720 logical aspect within half a physical pixel: capture={}x{} error={aspect_error}",
        image.width(),
        image.height()
    );
    let last_x = image.width() - 1;
    let last_y = image.height() - 1;
    for (name, pixels) in [
        (
            "top",
            (0..image.width())
                .map(|x| *image.get_pixel(x, 0))
                .collect::<Vec<_>>(),
        ),
        (
            "right",
            (0..image.height())
                .map(|y| *image.get_pixel(last_x, y))
                .collect(),
        ),
        (
            "bottom",
            (0..image.width())
                .map(|x| *image.get_pixel(x, last_y))
                .collect(),
        ),
        (
            "left",
            (0..image.height())
                .map(|y| *image.get_pixel(0, y))
                .collect(),
        ),
    ] {
        assert!(
            pixels
                .iter()
                .any(|pixel| pixel.0[0] > 80 || pixel.0[1] > 80 || pixel.0[2] > 80),
            "{description} clipped the {name} logical framebuffer edge"
        );
    }
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

    let nested_launch_dir = project.join("nested/launch");
    fs::create_dir_all(&nested_launch_dir).expect("create nested manifest launch directory");
    for case in ["play", "play_manifest_nested", "run_watch", "tui"] {
        let screenshot = parent.join(format!("{case}.png"));
        let command_dir = if case == "play_manifest_nested" {
            &nested_launch_dir
        } else {
            &project
        };
        let mut command = stasis_command(command_dir);
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
            "play_manifest_nested" => {
                command.args([
                    "play",
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
    let package_payload = package_root.join("app");
    assert!(package_root.join("windows_launch_smoke.exe").is_file());
    assert!(package_payload.is_dir());
    for relative in [
        "assets/manifest.json",
        "stasis.json",
        "stasis_dynload.dll",
        "stasis_provenance.json",
        "stasis_graphics.dll",
        "windows_launch_smoke.dll",
        "windows_launch_smoke.exe.launch",
    ] {
        assert!(
            package_payload.join(relative).exists(),
            "Windows package payload should contain {relative}"
        );
    }
    assert!(!package_root.join("stasis_graphics.dll").exists());
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

#[test]
fn recording_matches_visible_play_letterbox_and_input_timeline() {
    let root = repository_root();
    let configured_runtime = std::env::var_os("STASIS_RUNTIME_DLL_PATH")
        .as_deref()
        .is_some_and(|path| Path::new(path).is_file());
    if !configured_runtime
        && !root
            .join("runtime/build/bin/Release/stasis_graphics.dll")
            .is_file()
    {
        eprintln!(
            "native recording integration skipped: runtime/build/bin/Release/stasis_graphics.dll is unavailable"
        );
        return;
    }
    let fixture = root.join("samples/windows_launch_smoke");
    let test_tree = TestTree(temp_dir("headless_recording"));
    let project = test_tree.0.join("windows_launch_smoke");
    copy_tree(&fixture, &project);

    let visible = test_tree.0.join("visible.png");
    let visible_run = launch(
        {
            let mut command = stasis_command(&project);
            command.args([
                "play",
                "main.stasis",
                "--ticks",
                "2",
                "--screenshot-frame",
                "2",
                "--screenshot",
                visible.to_str().unwrap(),
                "--exit-after-screenshot",
            ]);
            command
        },
        "visible parity play",
    );
    assert_launch("visible parity play", visible_run, &visible);
    let visible_image = image::open(&visible)
        .expect("visible parity screenshot")
        .to_rgba8();
    let parity_points = [
        (10, 10),   // background
        (310, 170), // background
        (84, 66),   // PNG sprite interior
        (236, 66),  // SVG sprite interior
    ];

    let baseline = test_tree.0.join("baseline");
    let replay_session = test_tree.0.join("baseline.replay.json");
    let baseline_run = launch(
        {
            let mut command = stasis_command(&project);
            command.args([
                "record",
                "main.stasis",
                "--output",
                baseline.to_str().unwrap(),
                "--width",
                "640",
                "--height",
                "400",
                "--fps",
                "60",
                "--frames",
                "3",
                "--record-replay",
                replay_session.to_str().unwrap(),
            ]);
            command
        },
        "headless recording baseline",
    );
    assert!(
        baseline_run.status.success(),
        "headless recording baseline failed: stdout={} stderr={}",
        String::from_utf8_lossy(&baseline_run.stdout),
        String::from_utf8_lossy(&baseline_run.stderr)
    );
    let baseline_log = format!(
        "{}\n{}",
        String::from_utf8_lossy(&baseline_run.stdout),
        String::from_utf8_lossy(&baseline_run.stderr)
    );
    assert!(
        baseline_log.contains("Stasis recording presentation: hidden=1"),
        "recording did not report hidden presentation: {baseline_log}"
    );
    let baseline_files = recording_frames(&baseline);
    assert_eq!(baseline_files.len(), 3);
    let replay_json: serde_json::Value =
        serde_json::from_slice(&fs::read(&replay_session).expect("read replay session"))
            .expect("parse replay session");
    assert_eq!(replay_json["schema_version"], 1);
    assert_eq!(replay_json["frames"].as_array().map(Vec::len), Some(3));
    assert!(replay_json["identity"].get("host_i32").is_none());

    let replayed = test_tree.0.join("replayed");
    let replay_run = launch(
        {
            let mut command = stasis_command(&project);
            command.args([
                "record",
                "main.stasis",
                "--output",
                replayed.to_str().unwrap(),
                "--width",
                "640",
                "--height",
                "400",
                "--fps",
                "60",
                "--frames",
                "3",
                "--replay",
                replay_session.to_str().unwrap(),
            ]);
            command
        },
        "headless replay recording",
    );
    assert!(
        replay_run.status.success(),
        "headless replay recording failed: stdout={} stderr={}",
        String::from_utf8_lossy(&replay_run.stdout),
        String::from_utf8_lossy(&replay_run.stderr)
    );
    let replayed_files = recording_frames(&replayed);
    assert_eq!(replayed_files.len(), baseline_files.len());
    for (baseline, replayed) in baseline_files.iter().zip(&replayed_files) {
        assert_eq!(
            fs::read(baseline).expect("read baseline frame"),
            fs::read(replayed).expect("read replayed frame"),
            "replayed PNG must exactly match its recorded source frame"
        );
    }
    let baseline_first = image::open(&baseline_files[0])
        .expect("baseline first frame")
        .to_rgba8();
    assert_eq!(baseline_first.dimensions(), (640, 400));
    assert_eq!(baseline_first.get_pixel(0, 0).0, [0, 0, 0, 255]);
    assert_eq!(baseline_first.get_pixel(639, 399).0, [0, 0, 0, 255]);
    for (x, y) in parity_points {
        assert_eq!(
            baseline_first.get_pixel(x * 2, y * 2 + 20).0,
            visible_image.get_pixel(x, y).0,
            "letterbox parity mismatch at visible pixel ({x},{y})"
        );
    }
    let visible_background = visible_image.get_pixel(10, 10).0;
    let mut visible_text_pixels = 0;
    for x in 50..150 {
        for y in 120..155 {
            if visible_image.get_pixel(x, y).0 != visible_background {
                visible_text_pixels += 1;
            }
        }
    }
    let mut recorded_text_pixels = 0;
    for x in 100..300 {
        for y in 260..330 {
            if baseline_first.get_pixel(x, y).0 != [0, 0, 0, 255] {
                recorded_text_pixels += 1;
            }
        }
    }
    assert!(
        visible_text_pixels > 0,
        "visible cached text oracle has no glyph pixels"
    );
    assert!(
        recorded_text_pixels > 0,
        "recorded cached text oracle has no glyph pixels"
    );

    let scripted = test_tree.0.join("scripted");
    let scripted_run = launch(
        {
            let mut command = stasis_command(&project);
            command.args([
                "record",
                "main.stasis",
                "--output",
                scripted.to_str().unwrap(),
                "--width",
                "640",
                "--height",
                "400",
                "--fps",
                "60",
                "--frames",
                "3",
                "--input-script",
                "record_input.json",
            ]);
            command
        },
        "headless recording input script",
    );
    assert!(
        scripted_run.status.success(),
        "headless recording input script failed: stdout={} stderr={}",
        String::from_utf8_lossy(&scripted_run.stdout),
        String::from_utf8_lossy(&scripted_run.stderr)
    );
    let scripted_files = recording_frames(&scripted);
    assert_eq!(scripted_files.len(), 3);
    let scripted_first = image::open(&scripted_files[0])
        .expect("scripted first frame")
        .to_rgba8();
    let scripted_pixel = scripted_first.get_pixel(20, 40).0;
    assert_ne!(scripted_pixel, baseline_first.get_pixel(20, 40).0);
    assert!(
        scripted_pixel[0] > scripted_pixel[2],
        "input script should switch the known fixture pixel to red: {scripted_pixel:?}"
    );

    let scripted_again = test_tree.0.join("scripted-again");
    let rerun = launch(
        {
            let mut command = stasis_command(&project);
            command.args([
                "record",
                "main.stasis",
                "--output",
                scripted_again.to_str().unwrap(),
                "--width",
                "640",
                "--height",
                "400",
                "--fps",
                "60",
                "--frames",
                "3",
                "--input-script",
                "record_input.json",
            ]);
            command
        },
        "headless recording input repeat",
    );
    assert!(rerun.status.success(), "input repeat failed");
    assert_eq!(
        fs::read(&scripted_files[0]).expect("scripted frame bytes"),
        fs::read(&recording_frames(&scripted_again)[0]).expect("repeated frame bytes")
    );

    if let Some(ffmpeg_dir) = ffmpeg_directory(&root) {
        let mp4 = test_tree.0.join("recording.mp4");
        let mp4_run = launch(
            {
                let mut command = stasis_command(&project);
                add_ffmpeg_path(&mut command, &ffmpeg_dir);
                command.args([
                    "record",
                    "main.stasis",
                    "--output",
                    mp4.to_str().unwrap(),
                    "--width",
                    "640",
                    "--height",
                    "400",
                    "--fps",
                    "60",
                    "--frames",
                    "3",
                ]);
                command
            },
            "headless recording MP4",
        );
        assert!(mp4_run.status.success(), "MP4 recording failed");
        assert!(fs::metadata(&mp4).expect("MP4 artifact").len() > 0);
        let ffprobe = if ffmpeg_dir.as_os_str().is_empty() {
            PathBuf::from("ffprobe")
        } else {
            ffmpeg_dir.join("ffprobe.exe")
        };
        if Command::new(&ffprobe).arg("-version").output().is_ok() {
            let probe = Command::new(&ffprobe)
                .args([
                    "-v",
                    "error",
                    "-count_frames",
                    "-select_streams",
                    "v:0",
                    "-show_entries",
                    "stream=r_frame_rate,nb_read_frames",
                    "-of",
                    "default=noprint_wrappers=1:nokey=1",
                ])
                .arg(&mp4)
                .output()
                .expect("run ffprobe");
            assert!(probe.status.success(), "ffprobe failed");
            let probe_text = String::from_utf8_lossy(&probe.stdout);
            assert!(
                probe_text.contains("60/1"),
                "unexpected MP4 rate: {probe_text}"
            );
            assert!(
                probe_text.lines().any(|line| line.trim() == "3"),
                "unexpected MP4 frame count: {probe_text}"
            );
            let audio_probe = Command::new(&ffprobe)
                .args([
                    "-v",
                    "error",
                    "-select_streams",
                    "a:0",
                    "-show_entries",
                    "stream=codec_name,sample_rate,channels",
                    "-of",
                    "default=noprint_wrappers=1:nokey=1",
                ])
                .arg(&mp4)
                .output()
                .expect("run audio ffprobe");
            assert!(audio_probe.status.success(), "audio ffprobe failed");
            let audio_text = String::from_utf8_lossy(&audio_probe.stdout);
            assert!(
                audio_text.contains("aac"),
                "unexpected audio codec: {audio_text}"
            );
            assert!(
                audio_text.contains("48000"),
                "unexpected audio rate: {audio_text}"
            );
            assert!(
                audio_text.lines().any(|line| line.trim() == "2"),
                "unexpected audio channels: {audio_text}"
            );
        }
    } else {
        eprintln!("ffmpeg unavailable; MP4 artifact validation skipped");
    }
}

#[test]
fn recording_audio_asset_mp4_is_non_silent_repeatable_and_aligned() {
    let root = repository_root();
    let Some(ffmpeg_dir) = ffmpeg_directory(&root) else {
        eprintln!("audio recording integration skipped: FFmpeg is unavailable");
        return;
    };
    let runtime = std::env::var_os("STASIS_RUNTIME_DLL_PATH")
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .or_else(|| {
            let path = root.join("runtime/build/bin/Release/stasis_graphics.dll");
            path.is_file().then_some(path)
        });
    let Some(runtime) = runtime else {
        eprintln!("audio recording integration skipped: graphics runtime is unavailable");
        return;
    };
    let fixture = root.join("samples/audio_asset_playback");
    let test_tree = TestTree(temp_dir("audio_recording"));
    let project = test_tree.0.join("audio_asset_playback");
    copy_tree(&fixture, &project);
    copy_tree(&root.join("src"), &project.join("vendor/stasis/src"));

    let record = |output: &Path, label: &str| {
        let mut command = stasis_command(&project);
        command.env("STASIS_RUNTIME_DLL_PATH", &runtime);
        add_ffmpeg_path(&mut command, &ffmpeg_dir);
        command.args([
            "record",
            "audio_asset_playback.stasis",
            "--output",
            output.to_str().unwrap(),
            "--width",
            "480",
            "--height",
            "270",
            "--fps",
            "60",
            "--frames",
            "60",
        ]);
        let completed = launch(command, label);
        assert!(
            completed.status.success(),
            "{label} failed: stdout={} stderr={}",
            String::from_utf8_lossy(&completed.stdout),
            String::from_utf8_lossy(&completed.stderr)
        );
    };
    let first = test_tree.0.join("audio-first.mp4");
    let second = test_tree.0.join("audio-second.mp4");
    record(&first, "audio recording first");
    record(&second, "audio recording repeat");

    let ffprobe = if ffmpeg_dir.as_os_str().is_empty() {
        PathBuf::from("ffprobe")
    } else {
        ffmpeg_dir.join("ffprobe.exe")
    };
    let probe = |path: &Path, selector: &str, entries: &str| -> serde_json::Value {
        let output = Command::new(&ffprobe)
            .args([
                "-v",
                "error",
                "-count_frames",
                "-select_streams",
                selector,
                "-show_entries",
                entries,
                "-of",
                "json",
            ])
            .arg(path)
            .output()
            .expect("run ffprobe");
        assert!(
            output.status.success(),
            "ffprobe failed: {:?}",
            output.status
        );
        let payload: serde_json::Value =
            serde_json::from_slice(&output.stdout).expect("parse ffprobe JSON");
        payload["streams"]
            .as_array()
            .and_then(|streams| streams.first())
            .cloned()
            .expect("ffprobe stream")
    };
    let video = probe(
        &first,
        "v:0",
        "stream=codec_name,r_frame_rate,nb_read_frames,start_time,duration",
    );
    assert_eq!(video["codec_name"], "h264", "unexpected video stream");
    assert_eq!(video["r_frame_rate"], "60/1", "unexpected video rate");
    assert_eq!(video["nb_read_frames"], "60", "unexpected video count");
    assert_eq!(video["start_time"], "0.000000", "unexpected video start");
    let audio = probe(
        &first,
        "a:0",
        "stream=codec_name,sample_rate,channels,start_time,duration",
    );
    assert_eq!(audio["codec_name"], "aac", "unexpected audio stream");
    assert_eq!(audio["sample_rate"], "48000", "unexpected audio rate");
    assert_eq!(audio["channels"], 2, "unexpected audio channels");
    assert_eq!(audio["start_time"], "0.000000", "unexpected audio start");
    let video_duration = video["duration"]
        .as_str()
        .expect("video duration")
        .parse::<f64>()
        .expect("numeric video duration");
    let audio_duration = audio["duration"]
        .as_str()
        .expect("audio duration")
        .parse::<f64>()
        .expect("numeric audio duration");
    assert!(
        (video_duration - 1.0).abs() < 0.02,
        "video duration {video_duration}"
    );
    assert!(
        (audio_duration - video_duration).abs() <= 1024.0 / 48_000.0,
        "A/V duration mismatch: {audio_duration} vs {video_duration}"
    );

    let decode = |path: &Path| {
        let output = Command::new(if ffmpeg_dir.as_os_str().is_empty() {
            PathBuf::from("ffmpeg")
        } else {
            ffmpeg_dir.join("ffmpeg.exe")
        })
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-map", "0:a:0", "-f", "s16le", "-acodec", "pcm_s16le", "-"])
        .output()
        .expect("decode audio");
        assert!(output.status.success(), "audio decode failed");
        output.stdout
    };
    let first_pcm = decode(&first);
    let second_pcm = decode(&second);
    assert!(
        !first_pcm.is_empty()
            && first_pcm
                .chunks_exact(2)
                .any(|sample| i16::from_le_bytes([sample[0], sample[1]]) != 0)
    );
    assert_eq!(first_pcm, second_pcm, "recorded PCM must be repeatable");
}

fn recording_frames(directory: &Path) -> Vec<PathBuf> {
    let mut frames = fs::read_dir(directory)
        .expect("recording output directory")
        .map(|entry| entry.expect("recording output entry").path())
        .collect::<Vec<_>>();
    frames.sort();
    assert!(frames
        .iter()
        .all(|path| path.extension().and_then(|value| value.to_str()) == Some("png")));
    frames
}

#[test]
fn maximized_portrait_preserves_canvas_in_jit_and_release() {
    let root = repository_root();
    let fixture = root.join("samples/maximized_portrait");
    let test_tree = TestTree(temp_dir("maximized_portrait"));
    let project = test_tree.0.join("maximized_portrait");
    copy_tree(&fixture, &project);
    copy_tree(&root.join("src"), &project.join("vendor/stasis/src"));

    let jit_screenshot = test_tree.0.join("jit-maximized.png");
    let jit = launch(
        {
            let mut command = stasis_command(&project);
            command.args([
                "play",
                "main.stasis",
                "--ticks",
                "6",
                "--screenshot-frame",
                "6",
                "--screenshot",
                jit_screenshot.to_str().unwrap(),
                "--exit-after-screenshot",
            ]);
            command
        },
        "maximized portrait JIT",
    );
    assert_maximized_portrait("maximized portrait JIT", jit, &jit_screenshot);

    let release = launch_release_build({
        let mut command = stasis_command(&project);
        command.args(["build", "--mode", "release"]);
        command
    });
    assert!(
        release.status.success(),
        "maximized portrait release build failed: {}",
        String::from_utf8_lossy(&release.stderr)
    );
    let release_screenshot = test_tree.0.join("release-maximized.png");
    let mut release_command = Command::new(project.join("build/maximized_portrait.exe"));
    release_command.current_dir(&project);
    configure_capture(&mut release_command, &release_screenshot, true);
    release_command.env("STASIS_SCREENSHOT_FRAME", "6");
    let release_run = launch(release_command, "maximized portrait release");
    if app_control_blocked(&release_run) {
        eprintln!("maximized portrait release executable was blocked by Windows Application Control (error 4551)");
    } else {
        assert_maximized_portrait(
            "maximized portrait release",
            release_run,
            &release_screenshot,
        );
    }
}

#[test]
fn release_audio_sample_loads_compressed_music_and_starts_playback() {
    let root = repository_root();
    let fixture = root.join("samples/audio_asset_playback");
    let test_tree = TestTree(temp_dir("audio_asset_playback"));
    let project = test_tree.0.join("audio_asset_playback");
    copy_tree(&fixture, &project);
    copy_tree(&root.join("src"), &project.join("vendor/stasis/src"));

    let release = launch_release_build({
        let mut command = stasis_command(&project);
        command
            .env(
                "STASIS_RUNTIME_DLL_PATH",
                root.join("runtime/build/bin/stasis_graphics.dll"),
            )
            .env(
                "STASIS_RUNTIME_RUNNER_PATH",
                root.join("runtime/build/bin/Debug/stasis_runner.exe"),
            );
        command.args(["build", "--mode", "release"]);
        command
    });
    assert!(
        release.status.success(),
        "audio release build failed: {}",
        String::from_utf8_lossy(&release.stderr)
    );

    let screenshot = test_tree.0.join("audio-release.png");
    let mut release_command = Command::new(project.join("build/audio_asset_playback.exe"));
    release_command
        .current_dir(&project)
        .env("SDL_AUDIO_DRIVER", "dummy");
    configure_capture(&mut release_command, &screenshot, true);
    let completed = launch(release_command, "audio release executable");
    let stdout = String::from_utf8_lossy(&completed.stdout);
    let stderr = String::from_utf8_lossy(&completed.stderr);
    assert!(
        completed.status.success(),
        "audio release executable failed with {:?}\nstdout={stdout}\nstderr={stderr}",
        completed.status.code()
    );
    assert!(
        screenshot.is_file(),
        "audio release executable did not submit its frame"
    );
    assert_eq!(
        image::open(&screenshot)
            .expect("open audio screenshot")
            .dimensions(),
        (480, 270)
    );
}
