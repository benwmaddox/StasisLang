# Cross-platform audio output plan (Handmade Hero-inspired)

This document describes a practical, low-latency, cross-platform plan for sound output in Stasis, inspired by the Handmade Hero "game generates samples; platform plays them" model.

## Status (desktop MVP)

Implemented for desktop via SDL2:

- Runtime ring buffer + audio callback: `runtime/stasis_graphics.c`
- Stasis-facing builtins: `audio_is_available`, `audio_get_sample_rate`, `audio_get_channels`, `audio_get_queued_frames`, `audio_get_underruns`, `audio_push_f32_interleaved`
- Example program: `samples/audio_sine.stasis` (prints queued frames + underruns)

WASM/WebAudio remains planned work.

## Goals

- Cross-platform sound output suitable for games (desktop first, then WASM/mobile).
- Stable low-latency playback with clear underrun reporting.
- A simple Stasis-facing API: the program produces PCM; the host/runtime consumes it.
- No allocations or locks in the real-time audio callback.

## Non-goals (initially)

- A full audio graph / DSP system, effects chains, MIDI, or streaming compressed audio formats.
- Perfect sample-accurate sync with rendering (we will aim for stable latency and drift bounds first).

## Core model (Handmade Hero split)

The runtime is split into two layers:

- Platform audio layer:
  - Owns the audio device (opens/closes).
  - Runs the real-time callback (or equivalent pull thread).
  - Maintains a ring buffer of PCM frames.
  - Tracks timing, queued frames, and underruns.

- Game (Stasis program):
  - Produces audio by writing samples into a provided buffer, or by pushing samples to the runtime.
  - Does not do device IO and does not depend on platform-specific APIs.

This yields a predictable contract:

- The platform layer is responsible for "how audio gets played".
- The Stasis program is responsible for "what audio should be played".

## Terminology and formats

- Sample: one channel value at a moment (e.g., left sample).
- Frame: one sample per channel (stereo frame = L + R).
- Interleaved stereo: LRLRLR... in memory.

Canonical internal format (v1):

- Channels: 2 (stereo).
- Sample format: `f32` native-endian.
- Nominal sample rate: 48000 Hz.

Notes:

- Backends can request different device formats and rely on conversion (SDL2) or keep device format fixed to `f32` if possible.
- Keep Stasis-facing format stable even if devices differ; introduce resampling only when required.

## Timing model and latency strategy

We want stable audio with bounded latency. We will track:

- `queued_frames`: how many frames are available for playback.
- `target_latency_frames`: a safety margin (e.g., 2-3 callbacks worth).
- `underrun_count`: increments when callback has to output silence due to insufficient queued frames.

Handmade Hero-inspired scheduling:

- Maintain a `running_frame_index` for audio that represents "how many frames have been played since start".
- Each game tick, compute how many frames we need to generate to keep `queued_frames` near `target_latency_frames`.
- Generate exactly that many frames and push them into the ring buffer.

This avoids tying audio generation directly to render frame rate and provides a clear place to add drift correction later.

## Proposed Stasis-facing API

We want the smallest surface that makes real games possible while keeping determinism and explicitness.

### Query

- `audio_is_available() -> bool`
- `audio_get_sample_rate() -> int`
- `audio_get_channels() -> int` (v1 always returns 2)
- `audio_get_queued_frames() -> int`
- `audio_get_underruns() -> int`

### Push model (simple to integrate with current CLI/runner)

- `audio_push_f32_interleaved(samples_ptr: *f32, frame_count: int) -> int`
  - Returns frames accepted (0..frame_count).
  - Never blocks.

### Optional pull/mixer model (better long-term)

Expose a convention rather than a syscall-heavy API:

- The host calls a function exported by the compiled module:
  - `game_get_sound_samples(out: *f32, frame_count: int) -> void`
- The host decides `frame_count` based on `queued_frames` and `target_latency_frames`.

This matches the Handmade Hero pattern closely and keeps audio generation centralized (one place to mix voices).

## Runtime C ABI (host <-> runtime)

Keep native/managed boundary explicit by providing a small C ABI that the runner (and later WASM glue) can call.

Suggested functions (names can change to match repo conventions):

- `int stasis_audio_init(int sample_rate, int channels, int target_latency_frames);`
- `void stasis_audio_shutdown(void);`
- `int stasis_audio_push_f32(const float *interleaved_lr, int frame_count);`
- `int stasis_audio_get_queued_frames(void);`
- `int stasis_audio_get_underruns(void);`

Implementation detail:

- Prefer a lock-free SPSC ring buffer (main thread producer, audio callback consumer).
- Fall back to a minimal spin/mutex if needed, but keep callback time short and bounded.

## Desktop backend (SDL2)

SDL2 is already in the repo's orbit for graphics, and it provides cross-platform audio output.

Plan:

- Use `SDL_OpenAudioDevice` with desired spec:
  - `freq = 48000`, `format = AUDIO_F32SYS`, `channels = 2`.
  - `samples` chosen to balance latency and callback cost (start with 512 or 1024).
- Provide an SDL audio callback:
  - Pull `nframes` from the ring buffer into `stream`.
  - If insufficient frames: zero-fill remainder and increment `underrun_count`.
- Start device with `SDL_PauseAudioDevice(dev, 0)`.

Edge cases:

- Device format mismatch:
  - Prefer requesting `AUDIO_F32SYS` and accepting SDL's provided format if different.
  - If SDL provides a different format, either:
    - Convert in producer (preferred, keeps callback simple), or
    - Convert in callback (not preferred; more CPU in RT path).

## WASM backend (WebAudio)

WebAudio is the practical path for browsers; use an AudioWorklet for predictable timing.

Plan:

- Use an `AudioWorkletProcessor` that pulls frames from a SharedArrayBuffer-backed ring buffer.
- The main thread (or a worker) pushes frames produced by the Stasis program into the shared ring.
- The worklet:
  - Pulls exactly `renderQuantum` frames each invocation.
  - On underrun, outputs zeros and increments an atomic underrun counter.

Shared memory:

- Use a header with atomic read/write indices plus a `Float32Array` region for audio frames.
- Use `Atomics` for index coordination.

Sample rate:

- WebAudio runs at device sample rate (often 48000 but not guaranteed).
- v1 approach: assume 48000 and accept minor pitch/tempo mismatch only if necessary for an early prototype.
- v2: add a simple resampler or generate at device rate by querying `audioContext.sampleRate`.

## Drift and resampling (later)

Drift sources:

- Game tick pacing is not sample-accurate.
- Device sample rate may differ from nominal.

Plan for v2:

- Track a "desired queued frames" cursor and apply a tiny correction (speed up/slow down generation) to keep queue stable.
- Add a simple linear resampler for the rare cases where we must convert 44100 <-> 48000.

## Diagnostics and reliability

We want immediate feedback when audio is misconfigured or starving:

- Expose counters:
  - `underrun_count`, `max_queued_frames_seen`, `min_queued_frames_seen`.
- Print a single-line summary in verbose/dev mode:
  - `AUDIO rate=48000 queued=NN underruns=K`
- Add a CLI flag to force stress:
  - e.g., `--audio-debug-sleep-ms` to intentionally starve and verify graceful silence.

## Testing strategy

Audio is hard to test end-to-end, so focus on deterministic components:

- Unit tests for the ring buffer (push/pull sizes, wraparound, overflow behavior).
- A non-real-time simulation test:
  - A "fake callback" consumes frames at fixed cadence.
  - A "fake game" produces frames at variable cadence.
  - Assert underrun and queued-frame bounds under known scheduling.

CI sanity:

- Ensure the runtime C code compiles on Linux and Windows (already supported by CI).
- Do not require opening an actual audio device in CI.

## Implementation milestones

- M1: Ring buffer + counters in `runtime/` with a local simulation harness.
- M2: SDL2 backend wired into the runner (desktop audio output).
- M3: Stasis sample: sine wave + underrun stats (`samples/audio_sine.stasis`).
- M4: WebAudio design + first worklet prototype (feature-flagged).
