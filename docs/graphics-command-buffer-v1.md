# Graphics Command Buffer (Prototype v1)

This document explains how the prototype graphics command buffer works in practice, how it is laid out in memory, and how it is intended to evolve.

## Why a command buffer?

The current extern-call model has two major costs:

- High host boundary call count (especially painful for WASM/JS hosts).
- Host state is queried ad-hoc, which complicates determinism and resize/viewport propagation.

The command-buffer model keeps **inputs** as a read-only snapshot (see HostFrame work) and makes **outputs** a single submission of a packed command buffer:

- Fewer host calls (typically 1 submit per tick).
- Deterministic ordering and stable ABI for native + WASM.
- Easy to benchmark and fuzz (buffer is just data).

## Coordinate system and ordering

- Coordinates are **host pixels** (the host's viewport pixel space).
- Command ordering is **defined by field/stream order in the layout** (not by a sort key):
  - clear -> lines -> sprites -> text -> present

## ABI surface (current)

Runtime exports:

- `stasis_gfx_submit(cmd_i32: *i32, cmd_f32: *f32) -> void`
- `stasis_gfx_submit_u8(cmd_i32: *i32, cmd_f32: *f32, cmd_u8: *u8) -> void` (adds text bytes)

Stasis builtins:

- `gfx_submit(cmd_i32: i32[], cmd_f32: f32[])`
- `gfx_submit_u8(cmd_i32: i32[], cmd_f32: f32[], cmd_u8: u8[])`

Helper module (recommended writer):

- `src/gfx_cmd.stasis` (`gfx_cmd_begin`, `gfx_cmd_clear`, `gfx_cmd_line`, `gfx_cmd_sprite`, `gfx_cmd_text`, `gfx_cmd_submit`, `gfx_cmd_submit_no_present`)
  - For bulk host mode: call `gfx_cmd_mark_present()` and let the host submit after `tick()`.

## Memory layout (v1)

The buffer is split into ordered "streams" stored in separate arrays:

- `cmd_i32[]`: header + sprite stream + text metadata stream
- `cmd_f32[]`: clear payload + line stream + text f32 payload
- `cmd_u8[]`: text UTF-8 bytes (NUL terminated)

All offsets below are **indices** (not bytes).

### `cmd_i32` header

| Index | Name | Meaning |
|---:|---|---|
| 0 | `magic` | `0x47584631` (`'GXF1'`) |
| 1 | `version` | `1` |
| 2 | `flags` | bit 0 = clear, bit 1 = present |
| 3 | `line_count` | number of line commands |
| 4 | `sprite_count` | number of sprite commands |
| 7 | `text_count` | number of text commands |
| 9 | `text_bytes_used` | bytes used in `cmd_u8` |

Unspecified header slots are reserved for future expansion.

### Clear payload (`cmd_f32`)

| Index | Meaning |
|---:|---|
| 0..3 | `r,g,b,a` |

### Line stream (`cmd_f32`)

Starts at `cmd_f32[4]`. Each line is 8 floats:

`x1,y1,x2,y2,r,g,b,a`

### Sprite stream (`cmd_i32`)

Starts at `cmd_i32[32]`. Each sprite is 7 ints:

`handle,x,y,w,h,rot_degrees,a255`

### Text streams (`cmd_i32`, `cmd_f32`, `cmd_u8`)

Text is split to avoid copying/packing floats into integers:

- `cmd_i32` metadata: `(font_handle, byte_off, byte_len)` per text command
- `cmd_f32` payload: `(x, y, r, g, b, a)` per text command
- `cmd_u8` bytes: UTF-8 text bytes with a trailing NUL at `byte_off + byte_len`

In the runtime, the base offsets are currently:

- `text_i32_base = 32 + (max_sprites * 7)`
- `text_f32_base = 4 + (max_lines * 8)`

The current prototype uses fixed maxima (see `runtime/stasis_graphics.c`):

- `max_lines = MAX_LINES`
- `max_sprites = 4096`
- `max_text = 2048`
- `max_text_bytes = 65536`

## How submission executes

`stasis_gfx_submit_v1` executes streams in layout order:

1. Calls `stasis_begin_frame()`
2. If `(flags & 1) != 0`: calls `stasis_clear(r,g,b,a)`
3. If `line_count > 0`: calls `stasis_draw_lines_f32(...)`
4. If `sprite_count > 0`: loops and calls `stasis_gfx_draw_sprite(...)`
5. If `cmd_u8 != NULL` and text present: loops and calls `stasis_draw_text(...)`
6. If `(flags & 2) != 0`: calls `stasis_end_frame()` (present)

This "present bit" exists so benchmarks can exclude swap/vsync while still exercising queue building and submission.

## Practical guidance (buffer writers)

- Treat the header as authoritative: counts are clamped by the runtime.
- Keep the layout stable and versioned (bump `version` on any incompatible change).
- Avoid per-draw-size sprite rerasterization in the host; draw sizes can fluctuate by 1px frame-to-frame and overflow the atlas.
- Prefer a single `submit()` per tick; if you need multi-pass, that should become multiple streams/passes in the layout (still deterministic).

## Benchmarks

- `samples/render_command_buffer_bench_submit.stasis` compares:
  - per-call `draw_line` (many host calls)
  - batched `draw_lines_f32` (1 host call)
  - `gfx_cmd_submit_*` (1 host call; can measure build vs prebuilt)
- `samples/render_heavy_submit_bench.stasis` compares the same submission styles with both lines and sprites, so the signal is large enough to see on native.

For more stable timing than `get_time_ms()`, use `get_time_us()` (added to the runtime bindings).
