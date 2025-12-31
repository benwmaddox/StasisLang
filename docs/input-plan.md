# Unified input plan (mouse + touch + mobile taps)

This document proposes a single input model usable by desktop (mouse) and mobile (touch/taps), tuned for Brickout Revenge and other small games.

## Status (desktop MVP)

Implemented for desktop via SDL2:

- Stasis-facing API is exposed as per-field snapshot queries (rather than a returned struct pointer):
  - `input_pointer_count()` and `input_pointer_*` accessors
  - `input_viewport_*_px()` accessors
  - `input_dropped_pointers()` debug counter
- Visualization sample: `samples/input_pointers.stasis`

Web/WASM Pointer Events support remains planned work.

## Goals

- A single "input snapshot per frame" that Stasis code consumes deterministically.
- Support mouse pointer + buttons and touch contacts (single and multi-touch).
- Support "tap" and "drag" interactions without platform-specific branching in game code.
- Correct coordinate mapping to the rendered viewport (no mismatch when letterboxed/scaled).

## Non-goals (initially)

- Full text input / IME.
- Gamepad input (can be layered on later).
- Gesture recognition beyond basic tap/drag (pinch/rotate later).

## Core model: frame snapshots

The runtime collects platform events and produces an `InputFrame` snapshot each tick:

- The snapshot is immutable for the duration of the tick.
- Game code reads from it; it does not mutate device state.
- The runtime resets edge flags (`went_down`, `went_up`) each tick.

### Pointer-centric unification

Treat mouse and touch as pointers:

- Pointer `id`:
  - Mouse uses `id = 0`.
  - Touch uses stable per-contact ids (platform-provided ids mapped to slots).
- Pointer state per tick:
  - `is_down`: pointer is currently pressed/active (mouse primary button or touch contact).
  - `went_down`: became down this tick.
  - `went_up`: became up this tick.
  - `x_px`, `y_px`: position in pixels in the game viewport coordinate system.
  - `dx_px`, `dy_px`: delta since last tick in viewport pixels.
  - `x_n`, `y_n`: normalized [0,1] in viewport (optional but convenient).

### Capacity

Pick a fixed maximum pointer count to keep memory static/deterministic:

- `MAX_POINTERS = 8` (enough for typical multi-touch).
- If more are active, drop extras with a debug counter.

## Coordinate mapping

Correct mapping is critical. The runtime must define the viewport used for rendering and map device coordinates into it.

Proposed approach:

- The renderer/runtime maintains a `Viewport`:
  - `x_px`, `y_px`, `w_px`, `h_px` in window/surface coordinates.
  - This is the active area where the game renders (letterboxing accounted for).
- Input mapping:
  - Convert raw window/surface coordinates into viewport-local pixels:
    - `x_px = raw_x - viewport.x_px`
    - `y_px = raw_y - viewport.y_px`
  - Clamp to [0, viewport.w_px] / [0, viewport.h_px].
  - Normalize:
    - `x_n = x_px / viewport.w_px`
    - `y_n = y_px / viewport.h_px`

This ensures UI hit-testing and gameplay controls match what the player sees.

## Stasis-facing API

Provide a minimal, stable set of queries.

### Snapshot access

- `input_pointer_count() -> i32`
- `input_pointer_id(idx: i32) -> i32`
- `input_pointer_is_down(idx: i32) -> bool`
- `input_pointer_went_down(idx: i32) -> bool`
- `input_pointer_went_up(idx: i32) -> bool`
- `input_pointer_x_px(idx: i32) -> f32`
- `input_pointer_y_px(idx: i32) -> f32`
- `input_pointer_dx_px(idx: i32) -> f32`
- `input_pointer_dy_px(idx: i32) -> f32`
- `input_pointer_x_n(idx: i32) -> f32`
- `input_pointer_y_n(idx: i32) -> f32`
- `input_dropped_pointers() -> i32`

### Viewport access

- `input_viewport_x_px() -> i32`
- `input_viewport_y_px() -> i32`
- `input_viewport_w_px() -> i32`
- `input_viewport_h_px() -> i32`

### Convenience (optional)

Avoid baking too many heuristics into the runtime, but a few helpers are high-value:

- `input_primary_pointer() -> Pointer*`:
  - Chooses pointer 0 if mouse is present and active, else the first active touch pointer.
- `input_was_tapped(pointer_id, max_move_px, max_time_ms) -> bool`:
  - Derived from went_down/went_up + a small movement threshold.
  - If added, keep it deterministic by tracking per-pointer down time in the snapshot.

## Backend: SDL2 (desktop)

SDL2 can unify mouse and touch events under a single event pump.

Plan:

- Collect SDL events during the frame:
  - Mouse:
    - `SDL_MOUSEMOTION` -> update pointer 0 position and deltas.
    - `SDL_MOUSEBUTTONDOWN/UP` -> update `is_down/went_down/went_up` for pointer 0.
  - Touch:
    - `SDL_FINGERDOWN/MOTION/UP` -> map `fingerId` to a slot, update state.
- Convert SDL coordinates to viewport pixels:
  - SDL reports mouse in window coordinates; touch may be normalized; handle both.
- Maintain a mapping table:
  - `fingerId -> slot_index` for active touches.

## Backend: Web / mobile (Pointer Events)

Pointer Events unify mouse/touch/pen on the web.

Plan:

- Attach listeners to the canvas:
  - `pointerdown`, `pointermove`, `pointerup`, `pointercancel`.
- Call `setPointerCapture` on `pointerdown` so drags remain stable.
- Maintain `pointerId -> slot_index`.
- Convert client coordinates to canvas coordinates:
  - Use `getBoundingClientRect()` and map to canvas pixel space.
- Apply viewport mapping (letterbox) to get game-space pixels.

Prevent unwanted browser behavior:

- Use `touch-action: none` on the canvas to prevent scrolling gestures from hijacking input.

## Brickout Revenge control mapping

Concrete desired behaviors:

- Paddle:
  - Primary pointer x controls paddle target x.
  - Dragging is continuous; taps do not require motion.
- Launch/shoot:
  - A tap (went_down) can trigger launch.
  - Alternatively, tap on a UI region triggers actions; rely on normalized coords for hit tests.

## Reliability and diagnostics

- Expose debug counters:
  - dropped pointers, events processed, last pointer id set.
- Provide a simple on-screen overlay sample that shows:
  - pointer positions, is_down, went_down/up.

## Implementation milestones

- M1: Define C structs for `InputFrame`, `Pointer`, and `Viewport` in `runtime/` headers.
- M2: Implement SDL2 event collection into `InputFrame`.
- M3: Expose snapshot to the runner and Stasis.
- M4: Add a small sample that visualizes pointers.
- M5: Implement web Pointer Events mapping when WASM path is available.
