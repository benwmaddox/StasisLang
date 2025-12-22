# Stasis Graphics & Asteroids Roadmap

Goal: move beyond console IO to a vector-style GPU-rendered sample (Asteroids) while keeping deterministic memory/layout guarantees.

## Target Stack
- Host: C# runtime inside `Stasis.Cli` with SDL2 + OpenGL (fits LLVM/native, available on Win/Linux/macOS, minimal overhead).
- Build: ship prebuilt SDL2 binaries or document dev pre-reqs; add a thin GL loader (OpenTK-style bindings or manual `DllImport`).
- Rendering: immediate-mode line rendering (ship simple shader pair + dynamic vertex buffer).
- Frame pacing: target 60 fps (16ms frame budget) with a fixed-step loop; expose `get_time_ms`/`sleep_ms` to let Stasis code keep cadence when the host isn’t throttling.

## Language/Runtime Surface
- New built-ins (return `i32` unless noted):
  - `init_window(width: i32, height: i32, title: string[N]) -> bool`
  - `begin_frame() -> void`
  - `end_frame() -> void` (swaps buffers, polls input)
  - `clear(r: f32, g: f32, b: f32, a: f32) -> void`
  - `draw_line(x1: f32, y1: f32, x2: f32, y2: f32, r: f32, g: f32, b: f32, a: f32) -> void`
  - `is_key_down(key: i32) -> bool` (host defines key codes)
  - `get_time_ms() -> i32` (frame time/delta helper; may alias existing `time()` return)
  - `sleep_ms(ms: i32) -> void` (optional for fixed step pacing)
- Semantics: add to `SemanticAnalyzer` and `ModuleLowerer` built-in tables; lower to P/Invoke exports implemented in `Stasis.Cli`.
- Memory: keep vertex data on host; Stasis code issues draw calls via built-ins to avoid exposing GL buffers directly.

## Host Runtime Tasks
- Embed a render loop in `Stasis.Cli` that:
  - Initializes SDL + GL once when `init_window` is called.
  - Processes events each `end_frame` call (close requests, key states).
  - Maintains a key-state table exposed through `is_key_down`.
  - Binds a simple line shader and uploads per-draw vertices (2D clip-space).
  - Provides a monotonic clock for `get_time_ms`.
  - Optionally throttles to 60 fps in host code; Stasis side can also gate with `sleep_ms` to hold a 16ms step.
- Add disposal hooks to shut down SDL/GL cleanly when the Stasis program exits or requests window close.

## Asteroids Sample Outline
- Globals: ship positions/velocities, bullets, rocks, score, RNG seed (reuse `time()` default behavior).
- Main loop in Stasis:
  - Call `init_window(1024, 768, "Stasis Asteroids")`.
  - While running:
    - `begin_frame()`, `clear(0,0,0,1)`.
    - Read input via `is_key_down` for thrust/rotate/fire/quit.
    - Step physics with fixed `dt` of ~16ms (60 fps target).
    - Spawn bullets, wrap positions, detect collisions.
    - Issue `draw_line` for ship, rocks, bullets, HUD.
    - `end_frame()`; exit on close or keypress.
- Provide `samples/asteroids.stasis`; keep state deterministic apart from seed/time.

## Testing Strategy
- Compiler-level: add semantic tests ensuring new built-ins are recognized and argument counts match.
- Host integration: add a headless flag to `Stasis.Cli` to stub GL calls for CI, returning predictable outputs; run a minimal Stasis program that calls `init_window`/`begin_frame`/`end_frame` in a fake mode.
- Sample smoke: scripted run that executes the Asteroids sample for a few frames in headless mode and checks exit code/log.

## Milestones
1) Wire built-ins through semantic + lowering layers; stub host implementations (no-op/headless) and add tests.
2) Integrate SDL2 + OpenGL context creation and event/key handling; expose headless mode.
3) Implement draw pipeline (`clear`, `draw_line`, swap) and timing helpers.
4) Build `samples/asteroids.stasis` with fixed-step loop and minimal gameplay.
5) CI: add headless sample smoke test; document commands in README.

## Open Questions
- Should we support WebGPU/WASM directly for browser demos? (Future step; SDL/GL first.)
- Key codes: adopt SDL scancodes or define a small enum in docs for stability.
- Frame pacing: fixed-step with accumulator vs. variable-step based on `get_time_ms`.
