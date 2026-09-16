# Shared renderer process

Stasis shipping packages use one renderer process on desktop, Android, and iOS:

1. JIT or AOT game code writes the stable `gfx_cmd` family buffers (canonical schema v8; v7 is accepted only as a legacy input).
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

Schema v8 keeps clear and present as frame boundaries and records each line,
filled rectangle, sprite, direct-text, or cached-text submission in one bounded
cross-category order stream; the renderer accepts schema v7 only through the
documented legacy path.
order stream. It also records bounded logical top-origin clip descriptors and
ordered clip-push/clip-pop entries in that same stream. Payloads remain in typed category arrays;
each order entry names its category and payload index. The trace mixes an explicit kind marker and every
consumed value in requested order. Clip state is nested, restored by pop, and reset at each frame
boundary. Counts are clamped to the contract capacities;
invalid text ranges contribute metadata but never read outside the byte buffer.
JIT and AOT traces must match exactly for the representative conformance frame.

Current-schema frames use one declarative order stream. Sprite entries reference
bounded semantic run headers; each run owns a contiguous span of canonical
instances. Calls to `gfx_cmd_line`, `gfx_cmd_rect`, `gfx_cmd_sprite`, and
`gfx_cmd_text` append order entries automatically, while the direct sprite-run
writer publishes one entry at finalization. Games do not need a layer API or
batching-driven reordering. Hosts reject invalid or out-of-range references
transactionally.

## Construction lifecycle boundary

The shared renderer consumes the frame after construction. Package metadata
selects exactly one construction owner; the renderer and device lifecycle do not
add a second one:

| Owner / package shape | Lifecycle 1 behavior | Lifecycle 0 or absent behavior |
| --- | --- | --- |
| Non-monolithic generated bridge | Reset -> authored render -> finish exactly once. | Direct authored render; no generated reset/finish. |
| Windows monolithic generated bindings | Generated AOT binding performs reset -> authored render -> finish exactly once. | Direct authored render. |
| Android generated AOT bindings | Shared mobile AOT entry performs `gfx_cmd_construction_reset()` -> authored `render()` -> `gfx_cmd_construction_finish(result)` exactly once. | Generated mobile entry calls authored render directly. |
| iOS generated AOT bindings | Shared mobile AOT entry performs `gfx_cmd_construction_reset()` -> authored `render()` -> `gfx_cmd_construction_finish(result)` exactly once. | Generated mobile entry calls authored render directly. |
| `stasis_runner` | Verifies state/launch sidecar metadata when present and invokes the exported render entry; it never wraps again. | Invokes the legacy direct render entry. |

Lifecycle 1 is a render-entry contract and therefore requires a zero-argument
authored `render()` export. A tick-only or render-less package must stay on
lifecycle 0/absent direct/manual construction; lifecycle 1 must not be attached
to `main` or `tick` as a substitute for a render entry.

The generated entry calls `gfx_cmd_construction_reset()` and
`gfx_cmd_construction_finish(result)`; `stasis_begin_frame()` and
`stasis_end_frame()` remain host-private device/submission operations. A guest
`begin_frame()` nested inside lifecycle-1 authored render invalidates the active
construction and causes finish to abort. Construction lifecycle negotiation has
no implication for logical coordinates, viewport/safe viewport, drawable
resolution, or a resolution cap. See the [full owner matrix and migration
rules](begin_frame_design.md#exact-owner-matrix).

Once a new nightly is installed, consumers should regenerate vendor snapshots,
generated bindings, and package metadata together, verify the lifecycle-1 entry,
then remove any temporary manual-begin compatibility bridge from authored
`render()` code. Lifecycle-0/absent consumers remain direct-render packages
until rebuilt; this migration does not alter display limits.

Coordinates are logical top-left pixels. Clip rectangles use the same logical
top-origin coordinates; native GL/GLES converts them to drawable bottom-origin
scissors while Canvas and SDL apply the equivalent top-origin clip. Colors and alpha are straight alpha;
SDL uses source-alpha over destination. Sprite alpha is clamped to `0..255`,
linear filtering is used for normal sprite textures, rotation is clockwise
around the explicit pivot (center by default), and an invalid sprite handle resolves to the
procedural magenta checker. Text and SVG rasterization,
cache keys, and resource replacement live in `stasis_graphics.c`, so platform
shells cannot redefine them.

Logical, native, drawable, safe-viewport, input-transform, and resource-density
semantics are defined in `display_metrics.md`. Reserved gfx_cmd v8 header slots
carry host display metadata to embedded previews but do not participate in the
backend-independent command trace.

Lines grow forward and filled rectangles grow backward in one 10,000-record
geometry arena. This preserves the fixed command-buffer size and the historical
10,000-line capacity while preventing the two payload types from overlapping.
The order stream is bounded by the sum of category capacities, so successful
typed command submission cannot overflow it before its payload category.

## Platform boundary

Shipping CMake builds one native renderer: SDL_Renderer. Android and iOS shells
add only lifecycle, asset-root, input/surface, and package glue. Windows CI
builds this same target and runs the portable trace contract test.

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
full-frame copy. There is no alternate native renderer or runtime renderer
selector.

## Frame pacing

Rendering and presentation do not define simulation time. Desktop `stasis play`
paces the tick boundary against an absolute monotonic deadline. Work performed by
input collection, `tick`, `render`, and `stasis_gfx_submit_u8`--including a
blocking vsynced present--counts toward the configured interval. The host waits
only for the remaining budget, adds no delay after an overrun, and resets its
deadline after a whole-interval pause so suspended or stalled sessions do not
run a catch-up burst.

The Android and iOS shell applies the same deadline invariant through the shared
mobile frame pacer. Display synchronization can contribute to pacing on every
platform, but it never authorizes an extra simulation step. Physical-display
verification at 60, 90, and 120 Hz remains part of platform acceptance because
drivers can differ in whether and how long presentation blocks.
