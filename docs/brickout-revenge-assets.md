# Brickout Revenge - Graphics Asset Pipeline (Dev-First)

This document defines a simple, dev-friendly way to author and hot-reload visuals for `samples/brickout_revenge/brickout_revenge.stasis` without adding CLI flags.

Design reference:
- See `docs/brickout-revenge-brainstorm.md` for the high-level game goals (layout, level editor, economy, and monetization assumptions).

Goals:
- Author visuals as small vector-like source files.
- Bake to textures at startup or first use (runtime-managed).
- Keep Stasis code static-memory-friendly (store `i32` handles only).
- Support hot reload during development by swapping backing textures while keeping handles stable.

Non-goals (initial version):
- Full SVG feature parity (masks, blend modes, filters, SVG text, SMIL animation, shape morphs).
- A general-purpose retained-mode scene graph in Stasis.

## Directory Layout

- Source (editable): `samples/brickout_revenge/assets/*.svg`
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

Notes:
- Stasis stores the returned `i32` handles in globals/struct fields (static memory friendly).
- The runtime owns all allocations and GL resources; Stasis never sees pointers or variable-sized arrays.

## Sprite Source Format: SVG (current)

- Author sprites as standard SVG with explicit `width`/`height` or `viewBox` so rasterization is deterministic.
- Keep shapes simple (rects/paths/lines) for predictable baking; gradients/opacity are OK.
- Do not rely on SVG filters or SMIL animation tags; the runtime bakes a single frame. Animate by layering sprites and varying transforms/alpha in Stasis code.
- Legacy `.stv` has been removed; author sprites directly in SVG.

Rasterization:
- SVG is rasterized to RGBA8 (with the same supersampling/downsample step we used for `.stv`) then packed into the atlas with mipmaps.

## Hot Reload Model

In dev watch mode, sprite source SVGs are watched and hot-reloaded automatically (no explicit polling).

## Breakout Revenge - Sprite Set

Canonical sources (SVG):
- `samples/brickout_revenge/assets/paddle.svg`
- `samples/brickout_revenge/assets/ball.svg`
- `samples/brickout_revenge/assets/brick_basic.svg` (base)
- `samples/brickout_revenge/assets/brick_basic_turret.svg` (layer)
- `samples/brickout_revenge/assets/brick_basic_fx.svg` (layer)
- `samples/brickout_revenge/assets/brick_armored.svg` (base)
- `samples/brickout_revenge/assets/brick_armored_turret.svg` (layer)
- `samples/brickout_revenge/assets/brick_armored_fx.svg` (layer)
- `samples/brickout_revenge/assets/brick_reflector.svg` (base)
- `samples/brickout_revenge/assets/brick_reflector_fx.svg` (layer)

## Next Steps (Later)

- Add a cache format and on-disk cache in `assets_cache/` keyed by `(source-hash, scale, bake-version)`.
- Add a `gfx_load_sprite_or_embedded(path, bytes_ptr, bytes_len)` path for shipping without sources.
- Remove any lingering references to `.stv` in tooling/tests if discovered.
- Add animation metadata (timeline tweens) separate from the baked pixels, so most edits only update transforms, not textures.
