# Public graphics API

Application Stasis code imports `stdlib/graphics.stasis`. The supported frame path is:

1. `begin_frame()` and `clear(...)`.
2. Immediate `draw_line`, `fill_rect`, typed `draw_sprite(SpriteRef, ...)`, `draw_text`, drawable methods, or caller-owned `PresentationList`, `SpriteRunWriter`, and `LineBatch` values.
3. `end_frame()`.

`LineBatch` owns storage for 512 typed `Line` values and its bounded count. Use `reset_lines`, `append` or `append_line`, then `draw`. Failed appends return `false`; drawing clamps a corrupted count to owned storage. Lines still enter the canonical command stream one at a time, preserving painter order, the shared line/rectangle capacity, and deterministic drop accounting.

`SpriteRef` is the compiler-owned nominal identity returned by `Sprite.reference()` and carried by `Sprite`, `SpriteSheet`, `ImageAsset`, `SpriteRunWriter`, and `PresentationCommand`. It keeps the existing 32-bit host ABI lane, but integers cannot be assigned or passed as sprite references and application modules cannot redefine the type. Use `Sprite.valid()` and `Sprite.poll_reload()` instead of inspecting a raw handle.

`PresentationList` owns 256 typed, painter-ordered sprite or solid-rectangle slots. Build it with `append_sprite` and `append_solid_rect`, update a sprite slot with `patch_sprite`, then `replay`; replay clamps a corrupted count to owned storage and preserves exact insertion order. This is persistent logical input, not an atlas page, backend record, or view of the frame arrays. The host may privately coalesce compatible A/B/A/B sprites and rectangles after validating the frame, but application code must not sort transparent work or encode batching/atlas decisions.

`SpriteRunWriter` remains the bounded streaming option. Reserve, write typed `SpriteRef` instances, and finalize or cancel it in the same frame. Its token is not a public command-buffer offset.

The command arrays, `GFX_*` layout constants, and `gfx_cmd_*` helpers belong to `stdlib/internal/gfx_cmd.stasis`. The compiler rejects their import, use, or redeclaration outside the canonical graphics implementation and explicit `tests/stasis` ABI seams. It also rejects aliases of privileged graphics extern symbols, so spelling a different Stasis function name cannot bypass the module boundary. Renderer fallback entry points such as the C `stasis_draw_lines_f32` symbol remain runtime implementation details.

The compiler recognizes graphics implementation modules only when both their normalized module identity and their complete source content match the compiler-owned `graphics.stasis`, `asset_tasks.stasis`, or `internal/gfx_cmd.stasis` module. A project file that merely adopts a canonical `src/stdlib`, toolchain `.stasis_cache/toolchain/src/stdlib`, or `vendor/stasis[/src]/stdlib` path is not trusted when its content differs. Vendor manifest hashes additionally protect checked-in snapshot integrity. Raw `tests/stasis` ABI seams are enabled only for compiler unit builds or when the configured project root resolves to the Stasis repository itself; an ordinary project cannot claim the exception by copying its path spelling.

Migration is staged by consumer: StasisLang's supported API now requires `SpriteRef` and no longer supports `Sprite.handle` or `draw_sprite(i32, ...)`. Current in-repository StasisLang graphics samples, the packaged Android Workshop projects, and their active vendored stdlib snapshots use `reference`, `valid`, typed writers, or `PresentationList`; remaining `.handle` fields belong to unrelated audio or cached-text types. ChessTD migration is sibling task #480 and must update before adopting this toolchain revision; no ChessTD source is part of this change.
