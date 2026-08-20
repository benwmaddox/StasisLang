# Deterministic headless recording

`stasis record` runs the normal desktop JIT/play path with a hidden SDL software
surface. It never opens or focuses a visible window. The requested `--width` and
`--height` are physical output pixels; the guest's `init_window` remains the
logical canvas and uses the same fit/letterbox presentation as desktop play.

Capture a PNG sequence:

```powershell
stasis --workspace samples/windows_launch_smoke record `
  main.stasis `
  --output artifacts/review-frames `
  --width 640 --height 360 --fps 60 --frames 3 `
  --input-script record_input.json
```

An extensionless output is published as a directory containing
`frame-000001.png`, `frame-000002.png`, and so on. `--frames N` (or its
`--ticks N` alias) is an alternative to `--duration S`; duration is whole seconds and produces exactly `S * fps`
frames. The loop has zero wall-clock pacing, so the frame sequence is driven by
committed ticks and is repeatable for the same source, assets, backend, and input
script. Capture starts after `main()` and excludes the loading frame.

Use an `.mp4` output to encode the staged PNGs with FFmpeg:

```powershell
stasis --workspace samples/windows_launch_smoke record main.stasis `
  --output artifacts/review.mp4 `
  --width 640 --height 360 --fps 60 --frames 3
```

FFmpeg must be on `PATH`; the command uses H.264 (`libx264`), `yuv420p`, AAC,
and the requested input/output rate. MP4 recording also runs the existing game
mixer offline at 48 kHz, stereo, PCM16 before muxing it as AAC. Audio samples
for frame `n` are exactly `floor(n * 48000 / fps)`, so fractional rates do not
accumulate rounding drift and no physical audio device is opened. This captures
game-generated asset voices and `audio_push_f32_interleaved` samples only; it
does not capture a microphone or system audio. PNG mode does not stage offline
audio; guest code may still initialize and use the normal interactive audio API
when that path is requested.

For a checked-in audio example:

```powershell
stasis --workspace samples/audio_asset_playback record `
  audio_asset_playback.stasis `
  --output artifacts/audio-review.mp4 `
  --width 480 --height 270 --fps 60 --frames 60
```

PNG frames and the staged WAV are validated for exact count, dimensions,
format, and sample count before publication. MP4 encoding and publication are
staged, so an encoder, mixer, or renderer failure removes partial artifacts and
reports the stage, resolution, frame rate, output, and underlying cause.

Publication renames the fully validated sibling stage into the destination in
one same-volume operation; the final path is never populated incrementally.
An existing destination is rejected before publication, and owned partial
stages are cleaned up on failure.

The bounded desktop limits are 1..8192 pixels per dimension, 1..240 fps, and
1..999,999 frames. MP4 dimensions must be even because `yuv420p` requires
even chroma planes; PNG sequences may use odd dimensions.

The existing versioned input-script schema is applied at the same frame boundary
as `play --input-script`; pointer events therefore affect the captured output
deterministically. Rendering still uses the shipping JIT, graphics command
buffers, and pre-present PNG capture path. Raster pixels can differ across
graphics backends or driver versions.

## Codex visual review example

For a bounded review artifact, record a short deterministic take, inspect the
first and last PNG, then compare the same source with and without an input event:

```powershell
stasis --workspace samples/windows_launch_smoke record main.stasis `
  --output artifacts/codex-review `
  --width 640 --height 360 --fps 60 --frames 3 `
  --input-script record_input.json
```

Open `artifacts/codex-review/frame-000001.png` and
`frame-000003.png` in the Codex visual review pane. Keep the command, input
script, and renderer diagnostics with the artifact when reporting a mismatch.
