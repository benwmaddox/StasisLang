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

## Browser network imports during native recording

The shipped `src/stdlib/network_client.stasis` module is a browser adapter. In
generated Web builds, its eight `stasis_web_network_*` imports remain bound to
the Web runtime's WebSocket, resume, and checkpoint adapter. The generated Web
runtime is unchanged by native recording.

When `stasis record` sees a capture configuration, the native JIT selects an
opt-in deterministic offline profile before its initial compile. That profile
exists only to make an imported browser module link while recording; it is not
a native networking implementation. Ordinary native `play` and `live` keep
the Web-only imports unresolved and fail closed.

The offline profile never opens a socket, reads or creates credentials,
accesses environment or storage, or mutates guest buffers. Its exact return
contract is:

| Import | Valid call | Invalid scalar arguments |
| --- | ---: | ---: |
| `stasis_web_network_supported` | `0` | n/a |
| `stasis_web_network_connect` | `-4` | n/a |
| `stasis_web_network_status` | `-4` | n/a |
| `stasis_web_network_poll` | `-4` | `-1` for capacity outside `0..=65536` |
| `stasis_web_network_send` | `-4` | `-1` for length outside `0..=65536` |
| `stasis_web_network_resume_seat` | `-1` | n/a |
| `stasis_web_network_last_sequence` | `0` | n/a |
| `stasis_web_network_checkpoint` | `-4` | `-1` for seat outside `-1..7` or negative sequence |

These values are deterministic and do not represent a connected native
session. Record commands read the workspace and vendor sources only; they do
not rewrite consumer files, vendored stdlib files, or generated Web assets.

Add `--record-replay artifacts/run.replay.json` to publish the sparse HostFrame-diff session
alongside the image output. To render a prior session instead of using live or scripted input, pass
`--replay artifacts/run.replay.json`; the requested frame count must equal the replay tick count.
This works for both PNG sequences and MP4 output. See [Record and replay](record_replay.md).

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
game-generated `AudioVoice` playback and `AudioStream.push()` samples only; it
does not capture a microphone or system audio. PNG mode does not stage offline
audio unless `--audio-device-profile` is supplied; that profile exercises the
guest stream API without adding an audio track to the PNG artifact.

## Simulate a device-backed audio stream

Pass `--audio-device-profile PATH` to `record` or `validate` to drive the public
`AudioStream` API against a deterministic virtual device. The bounded, strict
JSON profile is versioned and contains all of these fields:

```json
{
  "schema_version": 1,
  "tick_hz": 120,
  "callback_hz": 50,
  "refuse_push_for_ms": 17,
  "pause_windows": [{ "start_ms": 120, "end_ms": 160 }],
  "producer_stalls": [{ "after_tick": 5, "duration_ms": 80 }]
}
```

The file is limited to 32 KiB. `tick_hz` is 1..240; `callback_hz` is 1..1000.
For `record`, `tick_hz` must equal `--fps`; `validate` uses the profile's tick
rate for its fresh setup/tick/render sequence. The refusal delay and pause
endpoints are bounded to 600,000 ms. There may be at most 64 sorted,
non-overlapping pause windows and 64 producer stalls. Stall `after_tick`
values must increase strictly, and their combined duration is bounded to
600,000 ms. Unknown or duplicate fields, unsupported versions, unbounded or
unsorted events, and malformed values are rejected before guest startup.

The host advances virtual audio callbacks independently from guest ticks using
the 48 kHz sample clock. Producer stalls advance device health measurements but
do not change the requested video frame count or the exact WAV sample schedule.
For MP4/MP3 recording, the same callback output used for device health feeds the
WAV stage; callback samples drained only during producer stalls are discarded.
The WAV is bounded to exactly `floor(frames * 48000 / fps)` samples, using a
partial final callback append and silence padding only if needed.
The virtual device can refuse pushes during startup, pause and resume at the
listed windows, and continue draining while a producer stall is in effect.

`AudioStream.frames_wanted(target_frames)` refreshes `queued_frames` and returns
the deficit needed to fill the queue toward the target. A zero result or a
`push()` result smaller than requested can be transient: keep the stream alive,
refresh queue health, and retry. `available` reports availability and health;
it does not promise that the next push will be accepted. Android may refuse
stream pushes until the app has audio focus.

Profiled results include a stable `audio_health` JSON object with `status`,
`peak`, `rms`, `first_sound_frame` (`4294967295` means no audible frame),
`longest_silent_run_after_sound`, `trailing_silent_frames`, push
attempt/requested/accepted/refused counts, `underruns`, and `warnings`. No push
attempts are reported as `not_exercised`. A stream that was called but never
produced audible accepted frames is `failed`: `validate` exits unsuccessfully
and still returns the health object. `record` keeps a successfully published
visual/audio artifact and reports an audio warning in its summary and JSON.
Long trailing silence and underruns are reported as warnings so an intentional
pause or short producer stall does not make a retrying stream fail validation.

The checked-in producer fixtures exercise both sides of the contract:
`tests/fixtures/audio_device_profile/broken.stasis` disables its stream on the
first refusal, while `retry.stasis` continues to fill its queue. Their 120 Hz
and 50 Hz profiles also cover mismatched callback rates, pause/resume, and a
producer stall without requiring FFmpeg.

Use an `.mp3` output for audio-only recording. It advances the same hidden desktop
simulation, input, tick, render, and offline mixer loop but does not stage PNG frames:

```powershell
stasis --workspace samples/audio_asset_playback record `
  audio_asset_playback.stasis `
  --output artifacts/audio-review.mp3 `
  --width 480 --height 270 --fps 60 --frames 60
```

The MP3 contains only game-generated audio (asset voices and
`AudioStream.push()` samples), encoded by FFmpeg's `libmp3lame` at 48 kHz
stereo. MP3 container start/duration may include up to the codec's frame-bounded
encoder delay and padding; decoding through FFmpeg trims that metadata back to the
exact pre-encode sample schedule. No microphone, system audio, physical device, or
visible window is used.

For deterministic setup at each frame, pass `--before-tick FUNCTION`. The function
must have exactly this signature:

```text
function before_record(frame: i32): i32 { ... }
```

The hook is a required guest function, receives a zero-based frame index, and is
called exactly once in this order: deterministic input and live overrides, hook,
normal `tick()`, `render()`, then capture/mix. It may mutate normal guest state and
call other guest functions. A missing or ambiguous function, any signature mismatch,
an invocation failure, or a nonzero return identifies the function and frame and
publishes no artifact.

For a minimal copy/paste demo, add this function to each entry file used below first:

```text
function before_record(frame: i32): i32 {
    if (frame < 0) {
        return 1;
    }
    return 0;
}
```

The same hook can then be used with each output mode:

```powershell
stasis --workspace samples/windows_launch_smoke record main.stasis `
  --output artifacts/hooked-frames --width 640 --height 360 --fps 60 --frames 3 `
  --before-tick before_record

stasis --workspace samples/windows_launch_smoke record main.stasis `
  --output artifacts/hooked.mp4 --width 640 --height 360 --fps 60 --frames 3 `
  --before-tick before_record

stasis --workspace samples/audio_asset_playback record audio_asset_playback.stasis `
  --output artifacts/hooked.mp3 --width 480 --height 270 --fps 60 --duration 1 `
  --before-tick before_record
```

For a checked-in audio example:

```powershell
stasis --workspace samples/audio_asset_playback record `
  audio_asset_playback.stasis `
  --output artifacts/audio-review.mp4 `
  --width 480 --height 270 --fps 60 --frames 60
```

PNG frames (when requested) and the staged WAV (for MP4/MP3) are validated for exact count, dimensions,
format, and sample count before publication. MP4/MP3 encoding and publication are
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

Use PNG when one or a few representative states prove the visual claim. Use
MP4 when correctness depends on motion, timing, animation, audio, input, state
transitions, or a multi-step interaction. AI review should inspect the MP4 and,
when useful, selected PNG frames from the same deterministic take rather than
treating successful encoding as proof of correct behavior.

Every AI-authored work summary must include a `Visual evidence:` line naming
the inspected PNG and/or MP4 paths and the behavior they prove. Use
`Visual evidence: not applicable` for work with no user-visible behavior. If
capture was relevant but unavailable, report the remaining validation gap.
