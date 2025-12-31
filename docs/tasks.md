# Tasks

This file is a lightweight, persistent checklist of upcoming work. It complements the more detailed design docs in `docs/`.

## Inbox

- [x] Fix PR `#24` (enum explicit values + SDL scancodes): resolve conflicts and restore CI green.
- [ ] Game dev readiness: P0 stdlib modules (`game_math`, `game_draw`, `game_collision`) + canonical UTF-8 buffer helpers (remove samples writing string headers directly).
- [ ] Game dev readiness: P1 input helpers (went_down/up, mapping), viewport/camera helpers, and draw batching helpers.
- [ ] Game dev readiness: P2 audio mixer layer (one-shots + loops) and more templates/examples.
- [ ] Follow through: implement `docs/audio-plan.md` (desktop SDL2 audio MVP first).
- [ ] Follow through: implement `docs/input-plan.md` (pointer snapshot, mouse + touch/taps).
- [ ] Follow through: implement `docs/aquarium-sample-plan.md` (add `samples/aquarium.stasis`).
- [ ] Follow through: execute `docs/data-hot-reload-plan.md` (end-to-end dev workflow + tests).
- [ ] Follow through: execute `docs/cranelift-backend-plan.md` (close remaining backend gaps).
- [ ] Follow through: execute `docs/hosts-first-class-plan.md` (host APIs, packaging, and ergonomics).
- [ ] Follow through: execute `docs/android-plan.md` (Android runtime build + host proof-of-concept).
- [ ] Follow through: execute `docs/brickout-android-debug-plan.md` (debug APK + adb asset push workflow).
- [ ] Follow through: execute `docs/self-hosted-compiler-plan.md` (bootstrap milestones).
- [ ] Follow through: execute `docs/svg-migration-plan.md` (finish SVG pipeline + validation).
- [ ] Stdlib/platform externs: support `@extern` no-body function declarations and implement them per-platform in the host/runtime so available APIs are visible in source.
- [ ] Maintenance: regularly scan open PRs for merge conflicts and fix by merging `main` into the PR branch (or rebasing) so PRs stay mergeable.
- [ ] Support compiling Markdown code blocks: allow `stasis build`/`stasis test` to accept `.md` inputs, extract ```stasis fenced blocks (and/or a `stasis` info string), and compile/test them so docs + samples stay valid.

## 1) Plan: Cross-platform sound output (Handmade Hero-inspired)

### Goals
- Cross-platform sound output for games and interactive samples (desktop first, then web/mobile).
- Low-latency, stable audio with clear underrun diagnostics.
- Simple mental model for Stasis programs: "game produces samples; platform plays them".

### Non-goals (initially)
- Full DAW-style audio graph, MIDI, effects chains, or streaming compressed formats.
- Perfect sample-accurate synchronization with rendering (we can iterate later).

### Constraints and assumptions
- Stasis core principle: deterministic behavior and explicit memory writes. The runtime may be "real-time" but should make nondeterminism explicit (device timing, underruns).
- Current runtime already depends on SDL2 for graphics; SDL2 audio is a reasonable first backend for desktop.
- WASM target likely needs WebAudio; mobile taps/input will likely come via the same event bridge as graphics.

### Proposed architecture (inspired by Handmade Hero)
- Keep a strict split between:
  - Platform layer: opens audio device, owns the real-time callback, manages a ring buffer, and reports timing.
  - Game layer (Stasis program): generates PCM samples into a provided buffer each frame (or in response to "need more samples").
- Use pull-based playback with an internal ring buffer:
  - Real-time audio callback pulls from ring buffer.
  - Main/game thread pushes generated samples into ring buffer.
  - If callback can't pull enough, output silence and record an underrun counter.
- Choose a canonical internal sample format:
  - Start with `f32` stereo interleaved (LRLR...), 48kHz nominal.
  - Allow device conversion (SDL can do this); keep internal format stable for Stasis.

### Stasis-facing API shape
- Add a minimal built-in/stdlib surface that does not leak platform details:
  - `audio_is_available() -> bool`
  - `audio_get_format() -> { sample_rate: int, channels: int }`
  - `audio_push_f32_interleaved(samples_ptr: *f32, frame_count: int) -> int` (returns frames accepted)
  - `audio_get_underruns() -> int`
- Consider a higher-level helper pattern for games:
  - A "mixer" function called by the host each frame that fills a buffer: `game_get_sound_samples(out: *f32, frame_count: int)`
  - The host decides `frame_count` based on queued latency and target safety margin.

### Desktop backend plan (SDL2)
- Add an SDL2 audio device wrapper in `runtime/`:
  - Open device with desired spec (48kHz, stereo, `AUDIO_F32SYS`).
  - Provide callback that drains from ring buffer.
  - Expose "queued frames" and "underruns" counters for diagnostics.
- Threading:
  - Use a lock-free ring buffer if possible; otherwise a minimal mutex around ring operations (keep callback time small).
  - Keep allocations out of the callback.

### Web/WASM backend plan (WebAudio)
- Implement an audio worklet that pulls from a SharedArrayBuffer ring buffer:
  - Main thread (or worker) pushes frames written by the Stasis program.
  - Worklet pulls frames; on underrun outputs zero.
- Bridge surface:
  - Same Stasis-facing API as desktop, implemented via host glue.
  - Keep format stable (`f32` stereo), adapt to device sample rate with a simple resampler only if required (start by assuming 48kHz).

### Milestones (concrete steps)
- [ ] Write `docs/audio-plan.md` with API, timing model, and examples.
- [ ] Implement `runtime` ring buffer + counters (no SDL yet), add a small C test harness that simulates push/pull.
- [ ] Wire SDL2 audio playback on Windows (and Linux/macOS if already supported).
- [ ] Expose minimal C ABI functions for the managed CLI/runner to call.
- [ ] Add a tiny Stasis sample that outputs a sine wave and prints underrun stats.
- [ ] Add WebAudio backend plan (and optionally first implementation) behind a feature flag.

### Acceptance criteria
- `stasis run samples/audio_sine.stasis` plays stable audio for 60s with 0 underruns on a typical dev machine.
- When forced to underrun (e.g., artificial sleep), runtime reports underruns deterministically and outputs silence (no crash).

## 2) Plan: Unified input for mouse + mobile taps (Brickout Revenge)

### Goals
- Single input model that supports:
  - Mouse pointer (move, left/right buttons, wheel optional).
  - Touch (single and multi-touch) mapped to pointer(s).
  - Mobile taps as first-class (tap-to-shoot/activate) for Brickout Revenge.
- Deterministic "input snapshot per frame" consumed by Stasis code.

### Proposed input model
- Build a frame-based input snapshot:
  - `InputFrame` contains a fixed-size array of pointers (e.g., 8).
  - Each pointer has: `id`, `is_down`, `went_down`, `went_up`, `x`, `y`, `dx`, `dy`.
  - For desktop mouse, pointer `id=0` is the cursor; buttons update `is_down` for "primary".
  - For touch, each contact maps to a pointer slot with stable `id` while down.
- Normalize coordinates:
  - Provide both pixel coordinates and normalized [0,1] coordinates relative to the game viewport.
  - Store the viewport scale/offset used by the renderer so input matches what the player sees.

### Platform backends
- Desktop (SDL2):
  - Consume SDL events: mouse motion/buttons, touch events if available.
  - Convert events into `InputFrame` updates.
- Web/mobile (WASM):
  - Prefer Pointer Events (`pointerdown/move/up/cancel`) so mouse/touch/pen share a path.
  - Keep a JS-side map from `pointerId` to pointer slot index.
  - Handle page scroll/zoom by preventing default on the canvas as appropriate.

### Stasis-facing API shape
- Minimal:
  - `input_get_frame() -> InputFrame*` (or copy-by-value if the language supports it cleanly)
  - `input_get_viewport() -> { x, y, w, h }`
- Optional helpers:
  - `input_primary_pointer()` (returns best-effort pointer for "tap or click").
  - `input_was_tapped()` for simple UI interactions (derived from went_down/went_up with distance threshold).

### Brickout Revenge-specific needs
- Paddle control:
  - Mouse move or finger drag sets paddle target position.
  - Tap to launch ball or activate powerup.
- UI:
  - Tap targets require stable hit testing; rely on normalized coordinates and viewport mapping.

### Milestones (concrete steps)
- [ ] Write `docs/input-plan.md` defining `InputFrame` and coordinate conventions.
- [ ] Implement SDL input collection into an `InputFrame` struct in `runtime/`.
- [ ] Expose C ABI to managed CLI/runner and to the Stasis program.
- [ ] Add a simple sample that draws pointer positions and prints went_down/went_up.
- [ ] Add web/mobile glue plan (Pointer Events) and implement when WASM target is wired.

### Acceptance criteria
- Desktop: mouse click/drag updates pointer state correctly at 60fps.
- Mobile/WASM: tap and drag on canvas works; no coordinate mismatch with rendering.

## 3) Plan: Mini sample - Aquarium (fish swim, feed, interact)

### Goals
- A compact, friendly sample that exercises:
  - Deterministic update loop, SoA-friendly entity storage, and rendering.
  - Input interactions (tap/click to drop food, drag to "stir" water).
  - Optional audio (bubbles, plop, ambient loop) once audio exists.
- Serves as a reference for "game-like" code structure in Stasis.

### Core mechanics (minimal and fun)
- Fish:
  - Swim with simple steering (wander + boundary avoidance).
  - When food exists, seek nearest food within radius.
  - When close enough, consume food and reduce hunger.
- Food pellets:
  - Spawn at pointer position, fall downward, slowly sink/settle.
  - Expire after N seconds if not eaten.
- Interaction:
  - Tap/click: spawn food.
  - Drag: apply a small velocity field impulse near the pointer ("stir") to push fish/pellets.

### Data and memory layout (SoA-friendly)
- Store fish attributes in parallel arrays:
  - `fish_x[]`, `fish_y[]`, `fish_vx[]`, `fish_vy[]`, `fish_hunger[]`, etc.
- Store pellets similarly with a max count and free list or compact-remove.
- Keep everything in static global memory; no hidden allocation.

### Rendering plan
- Use the existing graphics runtime:
  - Simple sprites or procedural shapes (triangles for fish, circles for pellets).
  - Background gradient + a few bubbles for life.
- Assets:
  - Start procedural; optionally add a small set of assets under `docs/assets/` or `examples/` later.
  - Reference existing docs: `docs/underwater-assets.md`.

### Audio plan (when available)
- One-shot sounds:
  - "plop" on food spawn, "chomp" on eat.
- Loop:
  - Soft ambient bubble loop mixed at low volume.

### Milestones (concrete steps)
- [ ] Write `docs/aquarium-sample-plan.md` with mechanics, data layout, and rendering primitives.
- [ ] Add `samples/aquarium.stasis` implementing fish + pellets + input spawning.
- [ ] Add a tiny config under `samples/aquarium/data/config.json` if needed (and keep it deterministic).
- [ ] Hook audio events once task (1) lands.

### Acceptance criteria
- `stasis run samples/aquarium.stasis --graphics` shows fish moving and responding to food taps/clicks.
- The sample is deterministic given a fixed seed and produces stable behavior across runs.
