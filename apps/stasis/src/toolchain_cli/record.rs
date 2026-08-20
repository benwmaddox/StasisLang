use super::{CommandResult, Workspace};
use clap::Args;
use image::GenericImageView;
use serde_json::json;
use stasis::{
    run_play_in_process_with_input_script_window_title_profile_and_capture, PlayFrameCaptureConfig,
};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_DIMENSION: u32 = 8192;
const MAX_FPS: u32 = 240;
const MAX_FRAMES: u64 = 999_999;

#[derive(Debug, Args)]
pub(super) struct RecordArgs {
    /// Override the manifest entry with a project-relative .stasis file.
    #[arg(value_name = "ENTRY")]
    pub(super) entry: Option<PathBuf>,
    /// Output directory for PNG frames, or an .mp4 file for H.264/AAC encoding.
    #[arg(long, value_name = "PATH")]
    pub(super) output: PathBuf,
    #[arg(long, value_name = "PIXELS")]
    pub(super) width: u32,
    #[arg(long, value_name = "PIXELS")]
    pub(super) height: u32,
    #[arg(long, value_name = "FPS")]
    pub(super) fps: u32,
    /// Capture exactly this many committed rendered frames.
    #[arg(
        long,
        visible_alias = "ticks",
        conflicts_with = "duration",
        required_unless_present = "duration"
    )]
    pub(super) frames: Option<u64>,
    /// Capture this many whole seconds (frames = duration * fps).
    #[arg(long, conflicts_with = "frames", required_unless_present = "frames")]
    pub(super) duration: Option<u64>,
    /// Apply the existing deterministic pointer input timeline.
    #[arg(long, value_name = "PATH")]
    pub(super) input_script: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputKind {
    PngSequence,
    Mp4,
}

pub(super) fn execute(workspace: &Workspace, args: RecordArgs) -> Result<CommandResult, String> {
    let frame_count = validate_args(&args)?;
    let output = absolute_path(&args.output)?;
    let parent = output
        .parent()
        .ok_or_else(|| format!("recording output has no parent: {}", output.display()))?;
    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "recording stage failed: could not create output parent {}: {error}",
            parent.display()
        )
    })?;
    if output.exists() {
        return Err(format!(
            "recording output already exists; refusing to replace {}",
            output.display()
        ));
    }
    let kind = output_kind(&output)?;
    if kind == OutputKind::Mp4 && (args.width % 2 != 0 || args.height % 2 != 0) {
        return Err(format!(
            "MP4 recording requires even dimensions for yuv420p (requested {}x{})",
            args.width, args.height
        ));
    }
    if kind == OutputKind::Mp4 {
        let audio_frames = frame_count
            .checked_mul(48_000)
            .ok_or_else(|| "MP4 recording audio sample schedule overflow".to_string())?
            / u64::from(args.fps);
        let audio_bytes = audio_frames
            .checked_mul(4)
            .ok_or_else(|| "MP4 recording WAV size overflow".to_string())?;
        if audio_bytes > u64::from(u32::MAX) - 36 {
            return Err("MP4 recording exceeds the bounded 4 GiB WAV staging limit".to_string());
        }
    }
    let entry = resolve_entry(workspace, args.entry.as_deref())?;
    let input_script = args
        .input_script
        .as_deref()
        .map(|path| resolve_workspace_path(workspace, path, "input script"))
        .transpose()?;
    let stage_root = parent.join(format!(
        ".stasis-recording-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |value| value.as_nanos())
    ));
    let frames_dir = stage_root.join("frames");
    if let Err(error) = fs::create_dir_all(&frames_dir) {
        cleanup_stage(&stage_root);
        return Err(format!(
            "recording stage failed: could not create {}: {error}",
            frames_dir.display()
        ));
    }

    let result = run_play_in_process_with_input_script_window_title_profile_and_capture(
        &entry,
        Some(&workspace.root),
        None,
        None,
        input_script.as_deref(),
        0,
        Some(frame_count),
        Some(&workspace.manifest.name),
        None,
        PlayFrameCaptureConfig {
            output_dir: frames_dir.clone(),
            width: args.width,
            height: args.height,
            fps: args.fps,
            frame_count,
            audio_output: (kind == OutputKind::Mp4).then(|| stage_root.join("audio.wav")),
        },
    );
    if let Err(error) = result {
        cleanup_stage(&stage_root);
        return Err(format!(
            "recording play stage failed (recording presentation=hidden-sdl-software, {}x{} at {} fps, entry {}, output {}): {error}",
            args.width,
            args.height,
            args.fps,
            entry.display(),
            output.display()
        ));
    }

    if let Err(error) = validate_frames(&frames_dir, args.width, args.height, frame_count) {
        cleanup_stage(&stage_root);
        return Err(format!(
            "recording validation failed (recording presentation=hidden-sdl-software, {}x{} at {} fps, output {}): {error}",
            args.width,
            args.height,
            args.fps,
            output.display()
        ));
    }
    if kind == OutputKind::Mp4 {
        let audio_path = stage_root.join("audio.wav");
        if let Err(error) = validate_wav(&audio_path, args.fps, frame_count) {
            cleanup_stage(&stage_root);
            return Err(format!(
                "recording audio validation failed (48 kHz stereo PCM16, output {}): {error}",
                output.display()
            ));
        }
    }

    // The stage is complete before this atomic same-volume no-replace rename.
    let publish_result = if output.exists() {
        Err(format!(
            "recording publish refused to replace destination {}",
            output.display()
        ))
    } else {
        match kind {
            OutputKind::PngSequence => {
                stasis_dynload::atomic_rename_no_replace(&frames_dir, &output).map_err(|error| {
                    format!(
                        "recording publish failed for PNG sequence {}: {error}",
                        output.display()
                    )
                })
            }
            OutputKind::Mp4 => encode_mp4(&frames_dir, &stage_root, &output, args.fps, frame_count),
        }
    };
    if let Err(error) = publish_result {
        cleanup_stage(&stage_root);
        return Err(error);
    }
    cleanup_stage(&stage_root);

    let format = match kind {
        OutputKind::PngSequence => "png-sequence",
        OutputKind::Mp4 => "mp4",
    };
    Ok(CommandResult::success(
        format!(
            "recorded {frame_count} frame(s) at {}x{} and {} fps to {}",
            args.width,
            args.height,
            args.fps,
            output.display()
        ),
        json!({
            "format": format,
            "output": output,
            "width": args.width,
            "height": args.height,
            "fps": args.fps,
            "frames": frame_count,
            "entry": entry,
        }),
    ))
}

fn cleanup_stage(stage_root: &Path) {
    match fs::remove_dir_all(stage_root) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => eprintln!(
            "recording cleanup warning: could not remove staging directory {}: {error}",
            stage_root.display()
        ),
    }
}

fn validate_args(args: &RecordArgs) -> Result<u64, String> {
    if args.width == 0 || args.width > MAX_DIMENSION {
        return Err(format!("--width must be between 1 and {MAX_DIMENSION}"));
    }
    if args.height == 0 || args.height > MAX_DIMENSION {
        return Err(format!("--height must be between 1 and {MAX_DIMENSION}"));
    }
    if args.fps == 0 || args.fps > MAX_FPS {
        return Err(format!("--fps must be between 1 and {MAX_FPS}"));
    }
    let frames = match (args.frames, args.duration) {
        (Some(frames), None) => frames,
        (None, Some(duration)) => duration
            .checked_mul(u64::from(args.fps))
            .ok_or_else(|| "--duration * --fps overflows the recording frame count".to_string())?,
        (Some(_), Some(_)) => return Err("--frames cannot be combined with --duration".to_string()),
        (None, None) => return Err("one of --frames or --duration is required".to_string()),
    };
    if frames == 0 || frames > MAX_FRAMES {
        return Err(format!(
            "recording frame count must be between 1 and {MAX_FRAMES}"
        ));
    }
    Ok(frames)
}

fn output_kind(path: &Path) -> Result<OutputKind, String> {
    match path.extension().and_then(|value| value.to_str()) {
        None => Ok(OutputKind::PngSequence),
        Some(extension) if extension.eq_ignore_ascii_case("mp4") => Ok(OutputKind::Mp4),
        Some(extension) => Err(format!(
            "unsupported recording output extension .{extension} for {}; use an extensionless PNG directory or .mp4",
            path.display()
        )),
    }
}

fn absolute_path(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        std::env::current_dir()
            .map(|directory| directory.join(path))
            .map_err(|error| {
                format!(
                    "failed to resolve recording path {}: {error}",
                    path.display()
                )
            })
    }
}

fn resolve_workspace_path(
    workspace: &Workspace,
    path: &Path,
    label: &str,
) -> Result<PathBuf, String> {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace.root.join(path)
    };
    let resolved = candidate
        .canonicalize()
        .map_err(|error| format!("failed to resolve {label} {}: {error}", candidate.display()))?;
    if !comparison_path(&resolved).starts_with(comparison_path(&workspace.root)) {
        return Err(format!(
            "{label} {} must stay under workspace {}",
            resolved.display(),
            workspace.root.display()
        ));
    }
    Ok(resolved)
}

fn comparison_path(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let value = path.to_string_lossy();
        if let Some(value) = value.strip_prefix(r"\\?\") {
            return PathBuf::from(value);
        }
    }
    path.to_path_buf()
}

fn resolve_entry(workspace: &Workspace, entry: Option<&Path>) -> Result<PathBuf, String> {
    let path = entry.unwrap_or_else(|| Path::new(&workspace.manifest.entry));
    let resolved = resolve_workspace_path(workspace, path, "recording entry")?;
    if resolved.extension().and_then(|value| value.to_str()) != Some("stasis") {
        return Err(format!(
            "recording entry must be a .stasis file: {}",
            resolved.display()
        ));
    }
    Ok(resolved)
}

fn validate_frames(
    frames_dir: &Path,
    width: u32,
    height: u32,
    expected: u64,
) -> Result<(), String> {
    let mut paths = fs::read_dir(frames_dir)
        .map_err(|error| format!("failed to inspect staged frames: {error}"))?
        .map(|entry| entry.map(|value| value.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("failed to inspect staged frame entry: {error}"))?;
    paths.sort();
    if paths.len() as u64 != expected {
        return Err(format!(
            "expected {expected} PNG frames, found {}",
            paths.len()
        ));
    }
    for (index, path) in paths.iter().enumerate() {
        let expected_name = format!("frame-{:06}.png", index + 1);
        if path.file_name().and_then(|value| value.to_str()) != Some(expected_name.as_str()) {
            return Err(format!(
                "unexpected frame ordering/name at {}",
                path.display()
            ));
        }
        let image = image::open(path).map_err(|error| {
            format!("failed to decode staged frame {}: {error}", path.display())
        })?;
        if image.dimensions() != (width, height) {
            return Err(format!(
                "frame {} has dimensions {}x{}, expected {}x{}",
                path.display(),
                image.width(),
                image.height(),
                width,
                height
            ));
        }
    }
    Ok(())
}

fn validate_wav(path: &Path, fps: u32, frame_count: u64) -> Result<(), String> {
    let mut file =
        fs::File::open(path).map_err(|error| format!("failed to open staged WAV: {error}"))?;
    let mut header = [0u8; 44];
    file.read_exact(&mut header)
        .map_err(|error| format!("failed to read staged WAV header: {error}"))?;
    if &header[0..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        return Err("WAV header is missing RIFF/WAVE signatures".to_string());
    }
    if &header[12..16] != b"fmt " || u16::from_le_bytes([header[20], header[21]]) != 1 {
        return Err("WAV is not uncompressed PCM".to_string());
    }
    let channels = u16::from_le_bytes([header[22], header[23]]);
    let sample_rate = u32::from_le_bytes([header[24], header[25], header[26], header[27]]);
    let bits = u16::from_le_bytes([header[34], header[35]]);
    let data_size = u32::from_le_bytes([header[40], header[41], header[42], header[43]]) as u64;
    let file_size = file
        .metadata()
        .map_err(|error| format!("failed to inspect staged WAV: {error}"))?
        .len();
    let expected_frames = frame_count
        .checked_mul(48_000)
        .ok_or_else(|| "WAV sample schedule overflow".to_string())?
        / u64::from(fps);
    let expected_bytes = expected_frames
        .checked_mul(4)
        .ok_or_else(|| "WAV size overflow".to_string())?;
    if channels != 2 || sample_rate != 48_000 || bits != 16 {
        return Err(format!(
            "expected PCM16 48 kHz stereo, got channels={channels} rate={sample_rate} bits={bits}"
        ));
    }
    if data_size != expected_bytes || file_size != 44 + data_size {
        return Err(format!(
            "expected {expected_frames} audio frames ({expected_bytes} bytes), got {} bytes",
            data_size
        ));
    }
    Ok(())
}

fn encode_mp4(
    frames_dir: &Path,
    stage_root: &Path,
    output: &Path,
    fps: u32,
    frame_count: u64,
) -> Result<(), String> {
    let staged_output = stage_root.join("recording.mp4");
    let pattern = frames_dir.join("frame-%06d.png");
    let audio = stage_root.join("audio.wav");
    let command = Command::new("ffmpeg")
        .args(ffmpeg_args(
            &pattern,
            &audio,
            &staged_output,
            fps,
            frame_count,
        ))
        .output()
        .map_err(|error| format!("MP4 encoder failure: could not start ffmpeg: {error}"))?;
    if !command.status.success() {
        let detail = String::from_utf8_lossy(&command.stderr).trim().to_string();
        return Err(format!(
            "MP4 encoder failure: ffmpeg exited with {}{}",
            command.status,
            if detail.is_empty() {
                String::new()
            } else {
                format!(": {detail}")
            }
        ));
    }
    let metadata = fs::metadata(&staged_output).map_err(|error| {
        format!(
            "MP4 encoder failure: ffmpeg produced no artifact {}: {error}",
            staged_output.display()
        )
    })?;
    if metadata.len() == 0 {
        return Err(format!(
            "MP4 encoder failure: ffmpeg produced an empty artifact {}",
            staged_output.display()
        ));
    }
    fs::remove_dir_all(frames_dir).map_err(|error| {
        format!(
            "recording cleanup failed before MP4 publication for {}: {error}",
            frames_dir.display()
        )
    })?;
    stasis_dynload::atomic_rename_no_replace(&staged_output, output).map_err(|error| {
        format!(
            "recording publish failed for MP4 {}: {error}",
            output.display()
        )
    })?;
    Ok(())
}

fn ffmpeg_args(
    pattern: &Path,
    audio: &Path,
    output: &Path,
    fps: u32,
    frame_count: u64,
) -> Vec<String> {
    vec![
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "error".to_string(),
        "-y".to_string(),
        "-framerate".to_string(),
        fps.to_string(),
        "-i".to_string(),
        pattern.display().to_string(),
        "-i".to_string(),
        audio.display().to_string(),
        "-map".to_string(),
        "0:v:0".to_string(),
        "-map".to_string(),
        "1:a:0".to_string(),
        "-frames:v".to_string(),
        frame_count.to_string(),
        "-c:v".to_string(),
        "libx264".to_string(),
        "-pix_fmt".to_string(),
        "yuv420p".to_string(),
        "-c:a".to_string(),
        "aac".to_string(),
        "-ar".to_string(),
        "48000".to_string(),
        "-ac".to_string(),
        "2".to_string(),
        "-r".to_string(),
        fps.to_string(),
        "-shortest".to_string(),
        "-f".to_string(),
        "mp4".to_string(),
        output.display().to_string(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args() -> RecordArgs {
        RecordArgs {
            entry: None,
            output: PathBuf::from("frames"),
            width: 640,
            height: 360,
            fps: 60,
            frames: Some(3),
            duration: None,
            input_script: None,
        }
    }

    #[test]
    fn duration_uses_checked_fps_product() {
        let mut value = args();
        value.frames = None;
        value.duration = Some(2);
        assert_eq!(validate_args(&value).expect("duration"), 120);
    }

    #[test]
    fn invalid_bounds_are_rejected_before_staging() {
        let mut value = args();
        value.width = MAX_DIMENSION + 1;
        assert!(validate_args(&value).is_err());
        let mut value = args();
        value.fps = MAX_FPS + 1;
        assert!(validate_args(&value).is_err());
        let mut value = args();
        value.frames = Some(0);
        assert!(validate_args(&value).is_err());
    }

    #[test]
    fn output_extension_contract_is_unambiguous() {
        assert_eq!(
            output_kind(Path::new("frames")),
            Ok(OutputKind::PngSequence)
        );
        assert_eq!(output_kind(Path::new("capture.MP4")), Ok(OutputKind::Mp4));
        assert!(output_kind(Path::new("capture.mov")).is_err());
    }

    #[test]
    fn ffmpeg_contract_preserves_exact_rate_and_count() {
        let args = ffmpeg_args(
            Path::new("frames/frame-%06d.png"),
            Path::new("audio.wav"),
            Path::new("out.mp4"),
            59,
            177,
        );
        assert!(args
            .windows(2)
            .any(|pair| pair[0] == "-framerate" && pair[1] == "59"));
        assert!(args
            .windows(2)
            .any(|pair| pair[0] == "-r" && pair[1] == "59"));
        assert!(args
            .windows(2)
            .any(|pair| pair[0] == "-frames:v" && pair[1] == "177"));
        assert!(args
            .windows(2)
            .any(|pair| pair[0] == "-pix_fmt" && pair[1] == "yuv420p"));
        assert!(args
            .windows(2)
            .any(|pair| pair[0] == "-map" && pair[1] == "0:v:0"));
        assert!(args
            .windows(2)
            .any(|pair| pair[0] == "-map" && pair[1] == "1:a:0"));
        assert!(args
            .windows(2)
            .any(|pair| pair[0] == "-c:a" && pair[1] == "aac"));
        assert!(args
            .windows(2)
            .any(|pair| pair[0] == "-ar" && pair[1] == "48000"));
        assert!(args
            .windows(2)
            .any(|pair| pair[0] == "-ac" && pair[1] == "2"));
    }
}
