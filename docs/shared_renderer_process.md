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

Schema v1 does not allow games to interleave command categories. Flexible
cross-category ordering is tracked separately and requires a schema bump.

## Platform boundary

Shipping CMake builds set `STASIS_GRAPHICS_SDL_ONLY=ON`. Android and iOS shells
add only lifecycle, asset-root, input/surface, and package glue. Windows CI
builds this same SDL-only target and runs the portable trace contract test.

The Android Workshop and its bundled Published preview flavor retain an embedded
GLES development adapter. They are tested on-device for preview correctness, but
they do not yet share the SDL resource lifecycle and are not evidence of pixel
parity with a shipping package. Published game artifacts use
`stasis package-mobile` and the SDL runtime. The old desktop GL adapter is
available only when CMake is explicitly configured with
`STASIS_GRAPHICS_SDL_ONLY=OFF`; it is not packaged or exercised as the canonical
process.
