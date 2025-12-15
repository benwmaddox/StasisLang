# Brickout Revenge - Graphics Asset Pipeline (Dev-First)

This document defines a simple, dev-friendly way to author and hot-reload visuals for `samples/brickout_revenge.stasis` without adding CLI flags.

Goals:
- Author visuals as small vector-like source files.
- Bake to textures at startup or first use (runtime-managed).
- Keep Stasis code static-memory-friendly (store `i32` handles only).
- Support hot reload during development by swapping backing textures while keeping handles stable.

Non-goals (initial version):
- Full SVG/Flash feature parity (masks, blend modes, filters, shape morphs).
- A general-purpose retained-mode scene graph in Stasis.

## Directory Layout

- Source (editable): `assets_src/brickout-revenge/*.stv`
- Cache (generated): `assets_cache/brickout-revenge/*.bin` (optional; future)

The runtime always attempts to load from the source path you pass. If you later want to ship only baked assets, you can keep the same Stasis code and have the runtime fall back to cached/embedded bytes (not implemented yet).

## Stasis Surface API (Built-ins)

These functions are treated as built-ins by the compiler and lowered to external calls provided by the graphics runtime library.

- `gfx_load_sprite(path: string[N]) -> i32`
  - Loads a sprite source file from disk, bakes it to an atlas texture, and returns a stable handle.
  - If the same path is loaded multiple times, the runtime returns the same handle.

- `gfx_draw_sprite(handle: i32, x: f32, y: f32, sx: f32, sy: f32, rot: f32, r: f32, g: f32, b: f32, a: f32) -> void`
  - Draws the baked sprite at `(x, y)` in screen coordinates.
  - `(sx, sy)` scales around the sprite's center.
  - `rot` is radians, rotating around the sprite's center.
  - `(r,g,b,a)` multiplies/tints the sprite output.

- `gfx_poll_reload(handle: i32) -> bool`
  - Checks whether the sprite source changed on disk and, if so, rebuilds it and updates the atlas region.
  - Handle remains stable; subsequent draws use the new pixels.
  - Intended to be called once per frame for a small set of sprites.

Notes:
- Stasis stores the returned `i32` handles in globals/struct fields (static memory friendly).
- The runtime owns all allocations and GL resources; Stasis never sees pointers or variable-sized arrays.

## Sprite Source Format: `.stv` (Stasis Tiny Vector)

This is a minimal, ASCII-only, line-oriented format that is easy to parse and rasterize.

Header:
- First non-empty line must be: `stv 1`

Commands:
- `size <w> <h>`
  - Required. Output sprite size in pixels.

- `rgba <r> <g> <b> <a>`
  - Sets current drawing color. Each component is a float in `[0..1]`.

- `rect <x> <y> <w> <h>`
  - Filled axis-aligned rectangle.

- `circle <cx> <cy> <radius>`
  - Filled circle.

- `line <x1> <y1> <x2> <y2> <thickness>`
  - Filled thick line (capsule).

Parsing rules:
- Whitespace-separated tokens.
- Lines starting with `#` are comments.
- Unknown commands are ignored (future-proofing).

Rasterization:
- CPU rasterization into an RGBA8 bitmap.
- The runtime may use fixed supersampling (e.g. 2x) then downsample for simple antialiasing.
- Baked sprites are packed into an atlas texture with mipmaps enabled for stable downscaling.

## Hot Reload Model

Hot reload is always enabled for now (no flags):
- `gfx_poll_reload(handle)` checks the on-disk modified time.
- If modified, the runtime reloads and rebakes the sprite and updates the atlas region via `glTexSubImage2D`.
- Mipmaps are regenerated for the atlas after updates (acceptable for dev; can be optimized later).

Recommended usage pattern:
- Load once during initialization (store handles in globals).
- Call `gfx_poll_reload` once per frame for the small set of sprites you are actively using.

## Breakout Revenge - Initial Sprite Set

Suggested initial set (remade assets):
- `assets_src/brickout-revenge/paddle.stv`
- `assets_src/brickout-revenge/ball.stv`
- `assets_src/brickout-revenge/brick_basic.stv`
- `assets_src/brickout-revenge/brick_armored.stv`
- `assets_src/brickout-revenge/brick_reflector.stv`

## Next Steps (Later)

- Add a cache format and on-disk cache in `assets_cache/` keyed by `(source-hash, scale, bake-version)`.
- Add a `gfx_load_sprite_or_embedded(path, bytes_ptr, bytes_len)` path for shipping without sources.
- Expand `.stv` to support strokes, gradients, and multi-layer compositing.
- Add animation metadata (timeline tweens) separate from the baked pixels, so most edits only update transforms, not textures.

