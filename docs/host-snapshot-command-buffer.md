# Host Snapshot + Command Buffer (host ABI direction)

This document proposes a practical path from today's "many small extern calls" host API to a hybrid:

- Snapshot in: one call per tick to read host state (time, viewport, input, flags).
- Command buffer out: one call per tick to submit rendering/audio/etc commands in bulk.

The goal is smoother resize behavior, fewer host boundary calls (important for WASM), easier headless testing, and a clearer host/runtime layering.

## Current situation (today)

The host boundary is effectively split across:

- `runtime/stasis_graphics.c`: SDL window ownership, event pumping, input snapshot (`g_input_frame`), plus rendering backend (OpenGL or SDL renderer). Exposes many extern-style functions (`begin_frame`, `draw_line`, `gfx_draw_sprite`, `gfx_window_width`, `input_pointer_x_px`, etc.).
- `stasis_runner.exe`: native launcher/loop that loads the compiled program and calls into it.
- `Stasis.Cli`: builds/links programs, runs the runner, and manages hot reload.

This works, but has two recurring problems:

1) Boundary call churn: lots of "query" calls and lots of tiny draw calls.
2) Resize smoothness: state derived from window size can get baked into pipeline state (e.g., shader source), and the update paths are not always centralized.

## Immediate fix (short term, no rewrite)

The current "maximize makes sprites disappear" class of bugs strongly suggests stale transforms or stale pipeline state during resize.

Do these first:

1) Stop baking window size into shaders
   - Use uniforms (or a single ortho matrix) for `window_w/window_h` in both line + sprite programs.
   - Resize becomes "update uniform/matrix", not "delete program + recompile shader".
   - Result: fewer resize hitch points and fewer "missed one pipeline" bugs.

2) Centralize resize handling
   - Add a single "apply resize" function that updates *all* window/viewport/projection state:
     - `g_window_width/g_window_height`
     - viewport (`glViewport` / renderer viewport)
     - ortho/projection matrix
     - any renderer/backend state that depends on size
   - Call it from every resize entry point:
     - SDL window events
     - `set_window_size`
     - `set_fullscreen`
     - any platform-specific "drawable size changed" hooks

These are independent of (and compatible with) the medium-term ABI direction below.

## Recommended direction (medium term): snapshot-in + command-buffer-out

### Tick model

Per tick:

1) Host pumps events and updates its internal state.
2) Host fills a HostFrame snapshot (in guest memory) via one call: `host_get_frame(...)`.
3) Program reads HostFrame for the full tick (read-only), then writes a command buffer.
4) Program submits the command buffer once: `host_submit_*(...)`.
5) Host executes commands and presents.

Key property: resize/viewport/DPI changes become *just fields in the HostFrame*, so every pipeline sees the same authoritative values.

### Why this matches Stasis

- Deterministic: HostFrame is a single snapshot (not time-varying queries mid-tick).
- Static memory: both HostFrame and command buffers are fixed-size global arrays (no hidden allocation).
- AoS -> SoA: command buffers can be SoA-friendly (separate streams per command type).
- WASM friendly: reduces imports/exports to a small stable surface.

## Command buffers in practice (what your game code does)

The "command buffer" is not a heap queue. It is just fixed-size global arrays plus a few counters.

Per tick, your program does:

1) `host_frame_refresh()` once, then read `host_*` accessors for the rest of the tick.
2) Reset command counters to 0 (`gfx_cmd_begin()`).
3) Append draw commands by writing into arrays (no host calls while building).
4) Submit once (`gfx_cmd_submit()`), and the host executes the commands and presents.

### Concrete shape (v1)

Prefer a stream-per-command SoA layout with explicit counts:

- `gfx_cmd_i32[]`: header + packed i32 payloads (sprites, text glyph indices, etc.)
- `gfx_cmd_f32[]`: packed f32 payloads (lines, sprite transforms if desired, etc.)

Example header layout in `gfx_cmd_i32[]` (indices are illustrative, reserve space up front):

- `i32[0]`: `GFX_CMD_MAGIC` (debug aid)
- `i32[1]`: `GFX_CMD_VERSION`
- `i32[2]`: `GFX_CMD_FLAGS` (bitfield: clear, present, etc.)
- `i32[3]`: `GFX_CMD_LINE_COUNT`
- `i32[4]`: `GFX_CMD_SPRITE_COUNT`
- `i32[5]`: `GFX_CMD_DROPPED_LINES`
- `i32[6]`: `GFX_CMD_DROPPED_SPRITES`
- `i32[7..31]`: reserved

Example payload layout:

- Lines in `gfx_cmd_f32[]` starting at `f32[0]`, stride 8:
  - `x1,y1,x2,y2,r,g,b,a`
  - capacity = `GFX_MAX_LINES * 8`
- Sprites packed in `gfx_cmd_i32[]` starting at `i32[32]`, stride N (choose one packing and freeze it):
  - `handle, x_px, y_px, w_px, h_px, rot_deg, a` (7 i32s) matches the existing `gfx_draw_sprite` ABI
  - capacity = `GFX_MAX_SPRITES * 7`

Deterministic overflow:

- If a stream is full, increment `DROPPED_*` and skip the write.
- Do not resize and do not partially write a command.

### Ordering

Ordering is defined by the order of fields in the command-buffer struct/layout.

Concretely: if the command buffer defines `clear`, then `lines`, then `sprites`, the host executes those streams in that order. If you need layers, express them as multiple fields/streams (e.g. `sprites_bg`, `sprites_world`, `sprites_ui`) in the desired order.

This keeps ordering deterministic without introducing a separate opcode stream.

### Coordinate space

Command coordinates are host pixels.

This implies:

- The host uses a pixel-perfect ortho/projection for all command execution.
- `x/y/w/h` are interpreted in pixels (define and keep consistent whether `x/y` are top-left or center for each command type).

HostFrame still carries viewport/window dimensions so game code can adapt, but command submission does not require a virtual-resolution transform.

### Relationship to today's `begin_frame/end_frame`

For the "one submit call per tick" ideal, `gfx_submit` should do the equivalent of:

1) begin frame (reset internal runtime queues, set current projection/uniforms)
2) execute the submitted commands
3) end frame (flush/present)

During migration, it is fine to keep the existing extern-call API for simple samples, while larger/high-churn paths switch to command buffers.

## ABI pieces

This section describes a v1-shaped ABI. It is intentionally conservative: fixed-size buffers, versioning, and "copy out" semantics.

### 1) HostFrame snapshot (already prototyped)

There is already a prototype in `src/host_frame.stasis` (kept in `src/` since stdlib modules currently cannot declare globals), and a native implementation in `runtime/stasis_graphics.c`:

- `extern function host_get_frame(out_i32: i32[], out_f32: f32[]): void;`
- `STASIS_EXPORT void stasis_host_get_frame(int32_t* out_i32, float* out_f32)`

Proposed changes to make it "production-shaped":

- Add a small header for versioning and per-tick flags.
- Add `dt` and a monotonic `frame_index` so systems can be written without relying on wall-clock deltas.
- Treat any unused indices as reserved for forward compatibility.

Suggested i32 header (example):

- `HOST_I_MAGIC`: constant (helps detect uninitialized buffers in debug)
- `HOST_I_VERSION`: increments when layout changes
- `HOST_I_FRAME_INDEX`: increments once per tick
- `HOST_I_TIME_MS`: monotonic ms since start (or since epoch; but monotonic is preferred)
- `HOST_I_DT_MS`: delta in ms since last tick (clamped, deterministic policy)
- `HOST_I_WINDOW_W_PX`, `HOST_I_WINDOW_H_PX`
- `HOST_I_VIEWPORT_X_PX`, `HOST_I_VIEWPORT_Y_PX`, `HOST_I_VIEWPORT_W_PX`, `HOST_I_VIEWPORT_H_PX`
- `HOST_I_FLAGS`: bitfield (resized, focus, etc.)
- `HOST_I_POINTER_COUNT`, `HOST_I_DROPPED_POINTERS`

Suggested f32 header (example):

- `HOST_F_DT_S`: `dt` in seconds for convenience (or omit if redundant)
- `HOST_F_DPI_SCALE_X`, `HOST_F_DPI_SCALE_Y` (if/when needed)

Pointer layout can stay as-is (id/buttons in i32, positions/deltas in f32).

### 2) Command buffers (new)

We want to move "high-churn outputs" to bulk submission, starting with graphics and (later) audio.

Two common encodings:

1) Generic opcode stream (byte/i32 stream with tags + payload sizes).
2) Fixed SoA streams per subsystem (lines stream, sprites stream, text stream, etc.).

For Stasis v1, prefer (2). Reasons:

- No variable-length parsing logic required in the host.
- Fixed layouts remain easy to version.
- Fast to validate (counts + bounds checks).
- Matches existing "batched" APIs (`draw_lines_f32`, `gfx_draw_sprites_i32`).

#### Graphics command buffer v1 (SoA)

Define a "gfx command buffer" as:

- Header i32 fields (version + counts)
- Payload arrays for each stream

Example layout (one possible split):

- `gfx_i32[]` header:
  - version
  - flags (e.g., "clear requested")
  - `line_count`
  - `sprite_count`
  - reserved
- `gfx_f32[]` payload for `lines`:
  - `line_count * 8` floats: `x1,y1,x2,y2,r,g,b,a`
- `gfx_i32[]` payload for `sprites` (if keeping `gfx_draw_sprites_i32`-style packing)
  - `sprite_count * SPRITE_STRIDE_I32`

Alternatively, make sprites a `gfx_f32[]` stream (handle as i32 + six f32, or all f32 with handle converted) if you want a single float stream.

Host-side API shape:

- `extern function gfx_submit(cmd_i32: i32[], cmd_f32: f32[]): void;`

Runtime implementation:

- Parse header, clamp counts, then execute:
  - for each line: enqueue like `stasis_draw_line` does today
  - for each sprite: enqueue like `stasis_gfx_draw_sprite` does today

Program-side stdlib shape:

- `gfx_cmd_begin()` resets write cursors.
- `gfx_cmd_line(...)` appends to the line stream (bounds checked; drop counter increments on overflow).
- `gfx_cmd_sprite(...)` appends to sprite stream.
- `gfx_cmd_submit()` calls `gfx_submit(...)`.

This preserves static memory and avoids per-call overhead.

#### Audio command buffer v1 (later)

The runtime already uses an f32 stereo ring buffer. A "command buffer" layer can standardize:

- A shared "producer writes N frames" protocol per tick.
- A single `audio_submit(frames_f32, frame_count)` import (or direct ring mapping).

The design should mirror graphics: fixed-size, deterministic overflow behavior, versioned header.

## Versioning and compatibility

The host boundary needs explicit versioning so older binaries can fail with a clear diagnostic instead of silently misinterpreting memory.

Recommended policy:

- Each snapshot/buffer has a `VERSION` integer in index 0/1.
- A mismatch is a hard error unless the host explicitly supports a known compat path.
- Reserve unused indices for forward expansion.

## Debugging and testing wins

Snapshot + command buffer unlocks cheap headless validation:

- Record HostFrame + command buffers for a tick range and replay them deterministically.
- Hash the command buffer to validate two code paths produce identical output streams (like the existing line benchmark does).
- A "null host" can accept commands and only validate bounds and invariants, enabling CI tests without SDL/GL.

## Migration plan (incremental, minimal disruption)

Phase 0: Fix resize smoothness in current runtime
- Refactor shaders to use uniforms/matrices (no window-size baked shader source).
- Centralize resize handling in `runtime/stasis_graphics.c`.

Phase 1: Make HostFrame v1 real
- Extend `src/host_frame.stasis` with version/flags/dt/frame_index.
- Update `runtime/stasis_graphics.c` `stasis_host_get_frame` to fill them.
- Add a small sample that prints HostFrame fields and validates invariants.

Phase 2: Add graphics command buffer v1 (parallel to existing extern calls)
- Add `gfx_submit(cmd_i32, cmd_f32)` as a new extern import.
- Add stdlib helpers that write to global command buffers.
- Add a sample that draws using the command buffer and compares to per-call drawing (hash-based).

Phase 3: Port high-churn drawing call sites
- Keep `draw_line` / `gfx_draw_sprite` for simplicity and compatibility.
- Prefer command buffers for "many draws per tick" code paths (particles, UI, tilemaps).

Phase 4: Fold more host queries into HostFrame
- Deprecate many `input_*` and `gfx_window_*` query calls in favor of HostFrame reads.

Phase 5: Prepare for a WASM host
- HostFrame becomes one import from JS, command buffers become one export or one import call.
- The native runner becomes "a host implementation", not the canonical execution model.
