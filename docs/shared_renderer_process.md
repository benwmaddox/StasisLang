# Shared renderer process

Stasis shipping packages use one renderer process on desktop, Android, and iOS:

1. JIT or AOT game code writes the `gfx_cmd` v1 guest buffers.
2. `stasis_gfx_submit_u8` validates and interprets that versioned buffer.
3. `stasis_graphics.c` owns frame order, resources, blending, filtering,
   clipping state, fallback sprites, and renderer shutdown.
4. SDL owns platform surface creation, texture upload/draw, input adaptation,
   and present.

`runtime/stasis_render_contract.h` is the C source of truth for magic, version,
flags, capacities, offsets, and the backend-independent trace. The Stasis
modules `src/runtime/gfx_cmd.stasis` and `src/stdlib/gfx_cmd.stasis` use the same
layout. Unsupported magic or versions are rejected without drawing.

## Command contract

Schema v1 has one fixed order: optional clear, lines, sprites, text or cached
text, then optional present. The trace mixes an explicit kind marker and every
consumed value in that order. Counts are clamped to the contract capacities;
invalid text ranges contribute metadata but never read outside the byte buffer.
JIT and AOT traces must match exactly for the representative conformance frame.

Coordinates are logical top-left pixels. Colors and alpha are straight alpha;
SDL uses source-alpha over destination. Sprite alpha is clamped to `0..255`,
linear filtering is used for normal sprite textures, rotation is clockwise
around the destination center, and an invalid sprite handle resolves to the
procedural magenta checker. Schema v1 has no clip command; the interpreter
resets the SDL clip rectangle at each frame boundary. Text and SVG rasterization,
cache keys, and resource replacement live in `stasis_graphics.c`, so platform
shells cannot redefine them.

Logical, native, drawable, safe-viewport, input-transform, and resource-density
semantics are defined in `display_metrics.md`. Reserved gfx_cmd v1 header slots
carry host display metadata to embedded previews but do not participate in the
backend-independent command trace.

Schema v1 does not allow games to interleave command categories. Flexible
cross-category ordering is tracked separately and requires a schema bump.

## Platform boundary

Shipping CMake builds set `STASIS_GRAPHICS_SDL_ONLY=ON`. Android and iOS shells
add only lifecycle, asset-root, input/surface, and package glue. Windows CI
builds this same SDL-only target and runs the portable trace contract test.

The Android Workshop menus remain native Android UI. Its embedded game canvas
cannot use SDL's single Android window without handing the editor activity and
surface lifecycle to SDL, so Workshop and the bundled Published preview flavor
share one thin `StasisPreviewRenderer` GLES adapter instead. Both flavors use the
same command interpreter, batching, clipping, rotation, alpha, filtering, and
fallback behavior; only their texture sources differ. The steady-state draw loop
uses fixed command arrays and direct vertex buffers. Texture uploads happen on
first use or an explicit Workshop asset change, and framebuffer allocations
happen only for an explicit screenshot capture.

Surface/context loss, resize, orientation, background/resume, and renderer reset
follow the generation-based state machine in `renderer_resource_lifecycle.md`.
Both adapters retain CPU source metadata, reject stale GPU generations, and restore
through their normal resource providers before accepting the next valid frame.

Published shipping artifacts use `stasis package-mobile` and the SDL runtime.
The preview adapter is therefore an embedded-editor boundary, not a competing
shipping renderer. It performs no per-command JNI calls and adds no additional
full-frame copy. The old desktop GL adapter is available only when CMake is
explicitly configured with `STASIS_GRAPHICS_SDL_ONLY=OFF`; it is not packaged or
exercised as the canonical process.
