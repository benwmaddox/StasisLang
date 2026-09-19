# Public graphics API

Application Stasis code imports `stdlib/graphics.stasis`. The supported frame path is:

Lifecycle 1 applies only to a package with a zero-argument authored `render()`
entry. Tick-only or render-less packages without an exported render entry may
use lifecycle 0/absent direct or manual construction. Packaged render entries
used by `stasis_runner` must publish lifecycle 1; lifecycle 1 must not attach to
`main` or `tick` as a substitute for a render entry.

1. Enter `render()`; a lifecycle-v1 host resets the command builder exactly once before calling guest code. Call optional `clear(...)` when the frame requests background replacement.
2. Immediate `draw_line`, `fill_rect`, typed `draw_sprite(SpriteRef, ...)`, `draw_text`, drawable methods, or caller-owned `PresentationList`, `SpriteRunWriter`, and `LineBatch` values.
3. Return from `render()`; the host calls `gfx_cmd_construction_finish(result)` to validate and publish the construction.

An unsuccessful render result, malformed frame, or unfinished `SpriteRunWriter` aborts the construction, leaving the last accepted frame independently owned by the consumer. A no-clear construction requests no background replacement and does not promise retained framebuffer pixels. Authored source must not call the removed public `begin_frame` or `end_frame` wrappers; the frontend rejects those calls. Native `stasis_begin_frame` and `stasis_end_frame` remain host-private device and submission operations.

## Render-entry ownership and migration

The construction lifecycle is negotiated in package metadata and is separate
from graphics-device setup, viewport policy, and presentation. The owner matrix
is:

| Owner / package shape | Lifecycle 1 | Lifecycle 0 or absent |
| --- | --- | --- |
| Non-monolithic generated bridge | Calls `gfx_cmd_construction_reset()` -> authored `render()` -> `gfx_cmd_construction_finish(result)` exactly once. | Calls the authored render entry directly; no construction helpers are added. |
| Windows monolithic generated bindings | Generated AOT binding calls reset -> authored render -> finish exactly once. | Calls the authored render entry directly. |
| Android generated AOT bindings | Shared mobile AOT entry calls `gfx_cmd_construction_reset()` -> authored `render()` -> `gfx_cmd_construction_finish(result)` exactly once. | Generated mobile entry calls the authored render entry directly. |
| iOS generated AOT bindings | Shared mobile AOT entry calls `gfx_cmd_construction_reset()` -> authored `render()` -> `gfx_cmd_construction_finish(result)` exactly once. | Generated mobile entry calls the authored render entry directly. |
| `stasis_runner` | May parse and verify state/launch sidecar metadata, then invokes the already-exported render entry; it never wraps render or adds another reset/finish. | Rejects an exported render entry as obsolete; tick-only and render-less packages remain direct/manual. |
| Authored guest render | Draws only; host finish validates and publishes the construction. | A package without an exported render entry remains under its negotiated compatibility contract. |

If source authored for the host-owned contract contains either removed public
frame wrapper, frontend validation rejects the package before rendering. This is
a construction contract failure, not a second render pass. Neither lifecycle
version changes logical coordinates, viewport or safe viewport rules, drawable
resolution, or any resolution cap; those remain host-owned display policy.

After a new nightly is installed, regenerate each consumer's vendored stdlib,
generated bridge/bindings, and package metadata together. Consumers carrying a
temporary compatibility bridge should verify the lifecycle-1 generated entry
first, then remove both public frame-wrapper calls and keep only authored
drawing. Run the consumer's focused render and ABI checks and confirm that no
guest frame wrappers remain in negotiated `render()`. A package with an
exported render entry must publish lifecycle 1 before use with `stasis_runner`;
a tick-only or render-less package may remain lifecycle 0/absent direct/manual.
Consumers without a production bridge need only the coordinated vendor and
metadata refresh. This migration has no viewport or resolution-cap effect.

`LineBatch` owns storage for 512 typed `Line` values and its bounded count. Use `reset_lines`, `append` or `append_line`, then `draw`. Failed appends return `false`; drawing clamps a corrupted count to owned storage. Lines still enter the canonical command stream one at a time, preserving painter order, the shared line/rectangle capacity, and deterministic drop accounting.

`SpriteRef` is the compiler-owned nominal identity returned by `Sprite.reference()` and carried by `Sprite`, `SpriteSheet`, `ImageAsset`, `SpriteRunWriter`, and `PresentationCommand`. It keeps the existing 32-bit host ABI lane, but integers cannot be assigned or passed as sprite references and application modules cannot redefine the type. Use `Sprite.valid()` and `Sprite.poll_reload()` instead of inspecting a raw handle.

`PresentationList` owns 256 typed, painter-ordered sprite or solid-rectangle slots. Build it with `append_sprite` and `append_solid_rect`, update a sprite slot with `patch_sprite`, then `replay`; replay clamps a corrupted count to owned storage and preserves exact insertion order. This is persistent logical input, not an atlas page, backend record, or view of the frame arrays. The host may privately coalesce compatible A/B/A/B sprites and rectangles after validating the frame, but application code must not sort transparent work or encode batching/atlas decisions.

`SpriteRunWriter` remains the bounded streaming option. Reserve, write typed `SpriteRef` instances, and finalize or cancel it in the same frame. Its token is not a public command-buffer offset.

`load_font(path, size)` returns an opaque, generation-safe font handle. Call
`release_font(handle)` when that logical size is superseded. Each successful
`load_font` acquires one ownership reference and must be balanced by one release.
A host may return the same handle for repeated loads of the same font; balance
the acquisitions even when their handle values match. Reuse an already-owned
font when its required size has not changed instead of loading it again.

`font_status(handle)` reports the existing `AssetState` values for a font. A valid
status-capable native handle is `Loaded` as soon as `load_font` returns; a Web handle is `Pending`
or `Loading` until its `FontFace` has loaded and Canvas metrics have been calibrated.
Web failures settle as `Failed`. A released or invalid handle reports `None` on a
status-capable host.
When a legacy native runtime lacks the optional status export, a positive handle
reports `Failed` so a consumer cannot wait forever; existing synchronous loading
and release calls remain available.
Native `Loaded` describes a live font owner and source data. Renderer restoration
can still rebuild its atlas after a density or context transition, and the host
withholds frames until that device-local work is ready.
Consumers that publish text layout after `main` should wait for `Loaded`; pending
text may carry compatibility fallback dimensions until calibration replaces them.
On Web, `Loaded` covers the font source and logical metric calibration. The first
draw then prepares a Canvas and atlas entry at the active physical density tier;
the reported metrics and submitted text quad stay in logical units. A density-tier
change evicts that prepared resource and rebuilds it on the next draw, while a
same-tier scale change reuses it. Device or WebGL extent failures remain visible
renderer failures and never lower the raster tier to preserve publication.

Cached text does not acquire an additional font ownership reference. It remains
usable while the font has an owner; the final release invalidates that font and
its cached text runs. Rebuild those runs for the replacement font. Releasing zero
or an invalidated handle is harmless, but releasing a live shared handle twice
can consume another owner's reference. An invalidated handle never aliases a
later font. Acquire a usable replacement before releasing the previous font; a
failed replacement load does not release the previous ownership reference.
Graphics runtime ABI 4 makes the release symbol mandatory across JIT, AOT,
desktop, Android, and Web hosts.

The command arrays, `GFX_*` layout constants, and `gfx_cmd_*` helpers belong to `stdlib/internal/gfx_cmd.stasis`. The compiler rejects their import, use, or redeclaration outside the canonical graphics implementation and explicit `tests/stasis` ABI seams. It also rejects aliases of privileged graphics extern symbols, so spelling a different Stasis function name cannot bypass the module boundary. Renderer fallback entry points such as the C `stasis_draw_lines_f32` symbol remain runtime implementation details.

The compiler recognizes graphics implementation modules only when both their normalized module identity and their complete source content match the compiler-owned `graphics.stasis`, `asset_tasks.stasis`, or `internal/gfx_cmd.stasis` module. A project file that merely adopts a canonical `src/stdlib`, toolchain `.stasis_cache/toolchain/src/stdlib`, or `vendor/stasis[/src]/stdlib` path is not trusted when its content differs. Vendor manifest hashes additionally protect checked-in snapshot integrity. Raw `tests/stasis` ABI seams are enabled only for compiler unit builds or when the configured project root resolves to the Stasis repository itself; an ordinary project cannot claim the exception by copying its path spelling.

Migration is staged by consumer: StasisLang's supported API now requires `SpriteRef` and no longer supports `Sprite.handle` or `draw_sprite(i32, ...)`. Current in-repository StasisLang graphics samples, the packaged Android Workshop projects, and their active vendored stdlib snapshots use `reference`, `valid`, typed writers, or `PresentationList`; remaining `.handle` fields belong to unrelated audio or cached-text types. ChessTD migration is sibling task #480 and must update before adopting this toolchain revision; no ChessTD source is part of this change.
