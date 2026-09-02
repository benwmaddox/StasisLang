# Public graphics API

Application Stasis code imports `stdlib/graphics.stasis`. The supported frame path is:

1. `begin_frame()` and `clear(...)`.
2. Immediate `draw_line`, `fill_rect`, `draw_sprite`, `draw_text`, typed drawable methods, or caller-owned `SpriteRunWriter` and `LineBatch` values.
3. `end_frame()`.

`LineBatch` owns storage for 512 typed `Line` values and its bounded count. Use `reset_lines`, `append` or `append_line`, then `draw`. Failed appends return `false`; drawing clamps a corrupted count to owned storage. Lines still enter the canonical command stream one at a time, preserving painter order, the shared line/rectangle capacity, and deterministic drop accounting.

`SpriteRunWriter` remains caller-owned. Reserve, write, and finalize or cancel it in the same frame. Its token is not a public command-buffer offset.

The command arrays, `GFX_*` layout constants, and `gfx_cmd_*` helpers belong to `stdlib/internal/gfx_cmd.stasis`. The compiler rejects their import, use, or redeclaration outside the canonical graphics implementation and explicit `tests/stasis` ABI seams. Renderer fallback entry points such as the C `stasis_draw_lines_f32` symbol remain runtime implementation details.

The compiler recognizes graphics implementation modules by their canonical `src/stdlib`, toolchain `.stasis_cache/toolchain/src/stdlib`, or project `vendor/stasis[/src]/stdlib` module identity. This path identity is the source boundary; it is not a claim of cryptographic provenance. Vendor snapshot hashes remain the integrity check for checked-in snapshots.
