# Shared renderer process

Stasis shipping packages use one renderer process on desktop, Android, and iOS:

1. JIT or AOT game code writes the `gfx_cmd` v4 guest buffers.
2. `stasis_gfx_submit_u8` validates and interprets that versioned buffer.
3. `stasis_graphics.c` owns frame order, resources, blending, filtering,
   clipping state, fallback sprites, and renderer shutdown.
4. SDL owns platform surface creation, texture upload/draw, input adaptation,
   and present.

`runtime/stasis_render_contract.h` is the C source of truth for magic, version,
flags, capacities, offsets, and the backend-independent trace. The single
Stasis ABI implementation is `src/stdlib/internal/gfx_cmd.stasis`; public
application code reaches it through `src/stdlib/graphics.stasis`. Unsupported
magic or versions are rejected without drawing.

## Command contract

Schema v4 keeps clear and present as frame boundaries and records each line,
filled rectangle, sprite, direct-text, or cached-text submission in one bounded cross-category
order stream. Payloads remain in typed category arrays; each order entry names
its category and payload index. The trace mixes an explicit kind marker and every
consumed value in requested order. Counts are clamped to the contract capacities;
invalid text ranges contribute metadata but never read outside the byte buffer.
JIT and AOT traces must match exactly for the representative conformance frame.

For compatibility, schema v2 and schema v3 frames with an empty order stream
use the prior line -> sprite -> text order. Schema v4's empty-order fallback is
line -> filled rectangle -> sprite -> text. This supports games that prebuild
persistent category buffers with `gfx_cmd_set_*_at` and count setters. New calls
to `gfx_cmd_line`, `gfx_cmd_rect`, `gfx_cmd_sprite`, `gfx_cmd_text`, their cached/bulk variants,
append order entries automatically; games do not need a separate layer API.
Invalid or out-of-range order references are skipped deterministically.

Coordinates are logical top-left pixels. Colors and alpha are straight alpha;
SDL uses source-alpha over destination. Sprite alpha is clamped to `0..255`,
linear filtering is used for normal sprite textures, rotation is clockwise
around the destination center, and an invalid sprite handle resolves to the
procedural magenta checker. Schema v4 has no clip command; the interpreter
resets the SDL clip rectangle at each frame boundary. Text and SVG rasterization,
cache keys, and resource replacement live in `stasis_graphics.c`, so platform
shells cannot redefine them.

Logical, native, drawable, safe-viewport, input-transform, and resource-density
semantics are defined in `display_metrics.md`. Reserved gfx_cmd v4 header slots
carry host display metadata to embedded previews but do not participate in the
backend-independent command trace.

Lines grow forward and filled rectangles grow backward in one 10,000-record
geometry arena. This preserves the fixed command-buffer size and the historical
10,000-line capacity while preventing the two payload types from overlapping.
The order stream is bounded by the sum of category capacities, so successful
typed command submission cannot overflow it before its payload category.

## Platform boundary

Shipping CMake builds set `STASIS_GRAPHICS_SDL_ONLY=ON`. Android and iOS shells
add only lifecycle, asset-root, input/surface, and package glue. Windows CI
builds this same SDL-only target and runs the portable trace contract test.

The Android Workshop menus remain native Android UI. Its embedded game canvas
cannot use SDL's single Android window without handing the editor activity and
surface lifecycle to SDL, so Workshop and the generated release shell
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

Shipping artifacts use `stasis package-mobile` and the SDL runtime.
The preview adapter is therefore an embedded-editor boundary, not a competing
shipping renderer. It performs no per-command JNI calls and adds no additional
full-frame copy. The old desktop GL adapter is available only when CMake is
explicitly configured with `STASIS_GRAPHICS_SDL_ONLY=OFF`; it is not packaged or
exercised as the canonical process.
