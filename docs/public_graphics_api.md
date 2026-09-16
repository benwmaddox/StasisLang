# Public graphics API

Application Stasis code imports `stdlib/graphics.stasis`. The supported frame path is:

Lifecycle 1 applies only to a package with a zero-argument authored `render()`
entry. Tick-only or render-less packages use lifecycle 0/absent direct or
manual construction and must not attach lifecycle 1 to `main` or `tick`.

1. Enter `render()`; a lifecycle-v1 host resets the command builder exactly once before calling guest code. Call optional `clear(...)` when the frame requests background replacement.
2. Immediate `draw_line`, `fill_rect`, typed `draw_sprite(SpriteRef, ...)`, `draw_text`, drawable methods, or caller-owned `PresentationList`, `SpriteRunWriter`, and `LineBatch` values.
3. Call `end_frame()` to request publication. Repeated calls are idempotent, and a later `clear(...)` preserves the request.

Returning without `end_frame()` discards the working construction. A nonzero render result, malformed frame, or unfinished `SpriteRunWriter` also aborts it, leaving the last accepted frame independently owned by the consumer. A no-clear publication requests no background replacement and does not promise retained framebuffer pixels. `begin_frame()` remains only for legacy packages and explicit manual/tick-only builders; negotiated `render()` implementations must not call it.

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
| `stasis_runner` | May parse and verify state/launch sidecar metadata, then invokes the already-exported render entry; it never wraps render or adds another reset/finish. | Invokes the legacy direct render entry. |
| Authored guest render | Draws and requests publication with `end_frame()`; a manual `begin_frame()` is invalid during host-owned construction. | Legacy/manual/tick-only code may use explicit `begin_frame()`. |

If lifecycle 1 guest code calls `begin_frame()` while the host-owned render is
active, the nested `gfx_cmd_begin()` marks the construction invalid and finish
aborts it. This is a construction transaction failure, not a second render
pass. Neither lifecycle version changes logical coordinates, viewport or safe
viewport rules, drawable resolution, or any resolution cap; those remain
host-owned display policy.

After a new nightly is installed, regenerate each consumer's vendored stdlib,
generated bridge/bindings, and package metadata together. Consumers carrying a
temporary compatibility bridge (for example, a conditional manual begin in
`render()`) should verify the lifecycle-1 generated entry first, then remove
that bridge and keep only the authored drawing/publication contract. Run the
consumer's focused render and ABI checks and confirm that no manual begin
remains in negotiated `render()`. Lifecycle-0/absent packages stay on direct
render until rebuilt with lifecycle 1; consumers without a production bridge
need only the coordinated vendor/metadata refresh. This migration has no
viewport or resolution-cap effect.

`LineBatch` owns storage for 512 typed `Line` values and its bounded count. Use `reset_lines`, `append` or `append_line`, then `draw`. Failed appends return `false`; drawing clamps a corrupted count to owned storage. Lines still enter the canonical command stream one at a time, preserving painter order, the shared line/rectangle capacity, and deterministic drop accounting.

`SpriteRef` is the compiler-owned nominal identity returned by `Sprite.reference()` and carried by `Sprite`, `SpriteSheet`, `ImageAsset`, `SpriteRunWriter`, and `PresentationCommand`. It keeps the existing 32-bit host ABI lane, but integers cannot be assigned or passed as sprite references and application modules cannot redefine the type. Use `Sprite.valid()` and `Sprite.poll_reload()` instead of inspecting a raw handle.

`PresentationList` owns 256 typed, painter-ordered sprite or solid-rectangle slots. Build it with `append_sprite` and `append_solid_rect`, update a sprite slot with `patch_sprite`, then `replay`; replay clamps a corrupted count to owned storage and preserves exact insertion order. This is persistent logical input, not an atlas page, backend record, or view of the frame arrays. The host may privately coalesce compatible A/B/A/B sprites and rectangles after validating the frame, but application code must not sort transparent work or encode batching/atlas decisions.

`SpriteRunWriter` remains the bounded streaming option. Reserve, write typed `SpriteRef` instances, and finalize or cancel it in the same frame. Its token is not a public command-buffer offset.

`load_font(path, size)` returns an opaque, generation-safe font handle. Call
`release_font(handle)` when that logical size is superseded. Releasing zero or an
already-released handle is harmless. A released handle never aliases a later
font, and a failed replacement load leaves the previous handle usable until the
caller explicitly releases it. Graphics runtime ABI 4 makes the release symbol
mandatory across JIT, AOT, desktop, Android, and Web hosts.

The command arrays, `GFX_*` layout constants, and `gfx_cmd_*` helpers belong to `stdlib/internal/gfx_cmd.stasis`. The compiler rejects their import, use, or redeclaration outside the canonical graphics implementation and explicit `tests/stasis` ABI seams. It also rejects aliases of privileged graphics extern symbols, so spelling a different Stasis function name cannot bypass the module boundary. Renderer fallback entry points such as the C `stasis_draw_lines_f32` symbol remain runtime implementation details.

The compiler recognizes graphics implementation modules only when both their normalized module identity and their complete source content match the compiler-owned `graphics.stasis`, `asset_tasks.stasis`, or `internal/gfx_cmd.stasis` module. A project file that merely adopts a canonical `src/stdlib`, toolchain `.stasis_cache/toolchain/src/stdlib`, or `vendor/stasis[/src]/stdlib` path is not trusted when its content differs. Vendor manifest hashes additionally protect checked-in snapshot integrity. Raw `tests/stasis` ABI seams are enabled only for compiler unit builds or when the configured project root resolves to the Stasis repository itself; an ordinary project cannot claim the exception by copying its path spelling.

Migration is staged by consumer: StasisLang's supported API now requires `SpriteRef` and no longer supports `Sprite.handle` or `draw_sprite(i32, ...)`. Current in-repository StasisLang graphics samples, the packaged Android Workshop projects, and their active vendored stdlib snapshots use `reference`, `valid`, typed writers, or `PresentationList`; remaining `.handle` fields belong to unrelated audio or cached-text types. ChessTD migration is sibling task #480 and must update before adopting this toolchain revision; no ChessTD source is part of this change.
