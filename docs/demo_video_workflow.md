# Stasis Demo Video Workflow

## Goal

Create a repeatable Windows workflow for Stasis demo videos that can:

- record gameplay from a running Stasis game
- capture live narration from a microphone
- keep game audio and narration on separate tracks during capture
- export a final shareable MP4 file

## Recommendation

Use:

- OBS Studio for capture
- FFmpeg for final MP4 export

For agent-driven demos that I can run myself, use:

- FFmpeg window capture
- Windows text-to-speech narration
- a self-running or scripted-input Stasis sample

This is the most practical current setup for Stasis on Windows because:

- Stasis games already run well in a normal desktop window
- OBS is good at window/game capture and microphone capture
- OBS supports multiple audio tracks in recordings
- FFmpeg is reliable for final audio mixing and MP4 export

The narrower agent-driven path is still useful because it is something I can execute directly in this environment:

- capture a Stasis game window with FFmpeg
- generate narration with Windows speech synthesis
- drive a simple sample with scripted input
- export the final MP4 automatically

## Why this setup

The current Stasis game path is native and windowed on Windows, which maps cleanly to OBS capture.

OBS is the right capture tool because its official docs cover:

- Game Capture and Window Capture
- Audio Input Capture for microphones
- per-application audio capture on Windows
- multi-track recording in Advanced Output mode
- recording to MKV and remuxing to MP4

FFmpeg is the right final export tool because it can:

- mix game audio and narration
- normalize narration loudness
- encode a final H.264/AAC MP4
- move MP4 metadata to the front of the file with `+faststart`

## Recommended Stasis capture setup

### 1. Launch the game you want to record

Typical command:

```powershell
.\stasis.exe play samples\bucket_catcher.stasis
```

For a cleaner demo run:

- close unrelated windows
- use a stable window size before recording
- avoid watch-mode edits while recording unless the video is specifically about hot swap

### 2. Create an OBS scene for Stasis

Recommended sources:

- `Game Capture` if OBS can lock onto the Stasis window cleanly
- otherwise `Window Capture` targeting the Stasis game window
- `Application Audio Capture (BETA)` or capture-audio-enabled Window/Game Capture for game audio
- `Audio Input Capture` for your microphone

If Game Capture is unreliable for a specific sample, use Window Capture. Stasis demos do not need hidden overlays or anti-cheat compatibility, so the simpler source is usually fine.

### 3. Configure OBS output for editing, not just quick sharing

In OBS:

- set Output Mode to `Advanced`
- record to `mkv`
- enable multiple audio tracks

Recommended track layout:

- Track 1: full mix
- Track 2: game audio only
- Track 3: microphone only

This gives you:

- an immediately playable track in the raw recording
- isolated game audio and narration for final export

### 4. Suggested OBS recording settings

Use OBS's local-recording guidance as the baseline.

Practical Stasis recommendation:

- Base Canvas: match the game window or display you are capturing
- Output Resolution: `1920x1080` if the source looks good there
- FPS: `60` for action-heavy samples, otherwise `30` is acceptable
- Encoder:
  - `NVENC`/`QuickSync`/`AMF` if available
  - otherwise `x264`

Reasonable quality targets, based on OBS guidance:

- `x264`: `CRF 18`
- `NVENC`: `CQP 18-20`

Those values are an implementation choice inside OBS's recommended quality range, not a hard requirement.

## Narration options

### Option A: Narrate live while recording

This is the simplest path.

Use OBS microphone capture while you play the game and explain:

- what the game is
- what Stasis feature is being shown
- what the player is seeing

Best when:

- the demo is short
- you already know the talking points
- you want the most natural timing

### Option B: Record narration separately

Record the gameplay first, then record narration as a separate WAV or MP3 file.

Best when:

- you want tighter scripting
- you want to retry voice-only takes without replaying the game
- you want cleaner post-production

The helper script in `tools/windows/export-demo-video.ps1` supports both approaches.

## Final MP4 export workflow

### Fast path

If you only need a quick uploadable file:

- record to MKV in OBS
- use OBS `File -> Remux Recordings`
- output MP4

This is fine for rough internal demos.

### Recommended final path

For a proper Stasis demo video:

1. Record raw gameplay in OBS with separate audio tracks.
2. Export the final file with FFmpeg using the helper script.
3. Produce a single H.264/AAC MP4 for sharing.

## Agent-driven Stasis demo path

If the goal is for the assistant to create the demo rather than just document a workflow for a human, the realistic scope is:

- choose a game that can run unattended or be driven by simple scripted input
- synthesize narration with Windows TTS rather than a live human voice track
- capture the game window with FFmpeg
- export the final MP4 automatically

### Chosen sample

The current best fit is `samples\bucket_catcher.stasis` because:

- it is visually clear
- it runs in a single stable window
- it responds to simple left/right keyboard input
- scripted arrow-key presses are enough to create usable footage

### Agent-driven helper scripts

- `tools\windows\new-demo-narration.ps1`
  - converts narration text into a WAV file with Windows speech synthesis
- `tools\windows\export-demo-video.ps1`
  - turns captured footage plus narration into a final MP4
- `tools\windows\new-bucket-catcher-demo-video.ps1`
  - launches Bucket Catcher
  - sends scripted left/right input
  - captures the game window with FFmpeg
  - synthesizes narration
  - writes the final MP4

### Agent-driven example

```powershell
.\tools\windows\new-bucket-catcher-demo-video.ps1 `
  -OutputPath recordings\bucket-catcher-demo.mp4 `
  -DurationSeconds 12
```

This path intentionally favors reliability over perfect human-like play.

## Helper script

Use:

```powershell
tools\windows\export-demo-video.ps1
```

### Common usage: OBS recording with separate game + mic tracks

Assume:

- Track 1 = full mix
- Track 2 = game audio only
- Track 3 = mic only

Then run:

```powershell
.\tools\windows\export-demo-video.ps1 `
  -VideoPath recordings\bucket-demo.mkv `
  -OutputPath recordings\bucket-demo-final.mp4 `
  -GameAudioStreamIndex 1 `
  -NarrationStreamIndex 2
```

Note: stream indexes above are FFmpeg-style zero-based audio stream indexes inside the input file.

### Common usage: gameplay video plus separate narration file

```powershell
.\tools\windows\export-demo-video.ps1 `
  -VideoPath recordings\bucket-demo.mkv `
  -NarrationPath audio\bucket-voice.wav `
  -OutputPath recordings\bucket-demo-final.mp4 `
  -GameAudioStreamIndex 1
```

### What the script does

- reads gameplay video
- takes either:
  - embedded narration from a separate audio stream, or
  - a separate narration file
- reduces game audio a bit
- normalizes/boosts narration
- mixes both together
- exports H.264 video + AAC audio MP4
- applies `+faststart` for web-friendly playback

## Recommended Stasis demo structure

A good Stasis demo video usually needs:

1. Very short title card or spoken intro
2. 20-60 seconds of clean gameplay
3. Callouts for:
   - the sample name
   - what engine/runtime capability is being shown
   - whether the footage is JIT/dev or release/AOT
4. Clean ending frame

Good initial demo candidates:

- `samples\bucket_catcher.stasis`
- `samples\perf_balls_bricks.stasis`
- `samples\brickout_revenge\brickout_revenge_v1.stasis`

## Practical guidance for Stasis-specific recordings

- Prefer windowed capture over full desktop capture when possible.
- Record one feature per clip.
- If the video is about gameplay, do not show compiler logs unless they matter.
- If the video is about hot swap, capture both the game window and the relevant console output.
- Keep narration slightly louder than game audio.
- Keep raw MKV recordings until the MP4 is approved.

## Limitations

This repo does not currently include:

- OBS installation
- a full non-linear editor workflow
- general-purpose gameplay AI

The helper here is focused on two concrete paths:

- take a human-recorded capture and turn it into a clean MP4 demo
- let the assistant create a narrow scripted demo for Bucket Catcher

## References

Official sources used for this workflow:

- OBS Quick Start Guide: <https://obsproject.com/kb/quick-start-guide>
- OBS Game Capture Setup Guide: <https://obsproject.com/kb/game-capture-setup-guide>
- OBS Audio Sources: <https://obsproject.com/kb/audio-sources>
- OBS Application Audio Capture Guide: <https://obsproject.com/kb/application-audio-capture-guide/>
- OBS Advanced Recording Guide And Multi Track Audio: <https://obsproject.com/kb/advanced-recording-guide-and-multi-track-audio>
- OBS Multiple Audio Track Recording Guide: <https://obsproject.com/kb/multiple-audio-track-recording-guide>
- OBS Standard Recording Output Guide: <https://obsproject.com/kb/standard-recording-output-guide>
- FFmpeg documentation: <https://ffmpeg.org/ffmpeg-all.html>
- FFmpeg filters documentation: <https://ffmpeg.org/ffmpeg-filters.html>
