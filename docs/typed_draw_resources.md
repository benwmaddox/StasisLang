# Typed drawable resources

## Status

Implemented typed-only graphics-standard-library surface. Legacy raw sprite and prepared-text functions are removed from Stasis source.

## Problem

The removed sprite API represented a loaded image as an untyped `i32` handle:

```stasis
let aura: i32 = gfx_load_sprite("aura.svg", 64, 64);
gfx_draw_sprite(aura, x, y, 136.0, 136.0, 0, 255);
```

Loading and drawing independently declare the same painted dimensions. A scalable SVG, or a high-resolution raster deliberately reduced during loading, can therefore be baked into a small physical cache and silently enlarged later. The caller still appears to use vector or high-resolution source art, but the framebuffer exposes the reduced cache pixels.

Prepared text has the same structural weakness: an `i32` handle loses the logical measurements and resource kind that distinguish it from every other integer.

## Mapping

A drawable resource represents three distinct things:

- Stasis-visible logical painted dimensions used for layout and ordinary drawing;
- an opaque native handle used to find renderer-owned data;
- a physical raster cache derived by the host from logical dimensions and current display density.

The first slice models only the stable Stasis-visible portion. Renderer generation, physical pixel dimensions, source provenance, anchors, scaling policies, and automatic re-rasterization remain runtime or future-design concerns.

```stasis
struct Sprite {
    handle: i32;
    width: i32;
    height: i32;
}

struct TextRun {
    font: i32;
    handle: i32;
    width: f32;
    height: f32;
}
```

`width` and `height` are logical painted dimensions. They are not source pixels, atlas pixels, native drawable pixels, layout occupancy, or collision bounds.

## Initial API

Stasis receiver-form functions are the primary API. They mutate an existing global-backed struct view and avoid depending on temporary struct return materialization.

```stasis
function load_sprite_from(self: Sprite, path: string, width: i32, height: i32): bool;
function draw(self: Sprite, x: f32, y: f32, alpha: i32, rotation: i32): void;
function release(self: Sprite): void;

function load_text_from(self: TextRun, font: i32, text: string): bool;
function draw(self: TextRun, x: f32, y: f32, r: f32, g: f32, b: f32, a: f32): void;
```

Representative use:

```stasis
global state {
    aura: Sprite;
    title: TextRun;
}

function main(): i32 {
    if (!state.aura.load_sprite_from("assets/aura.svg", 136, 136)) { return 1; }
    if (!state.title.load_text_from(title_font, "SELECT ASSAULT")) { return 1; }
    return 0;
}

function tick(): void {
    state.aura.draw(38.0, 228.0, 255, 0);
    state.title.draw(92.0, 184.0, 1.0, 0.9, 0.7, 1.0);
}
```

Receiver calls may target nested fields directly. Prefer one root application
state global per entry file and keep drawable resources beneath it; separate
globals remain appropriate only for fixed host ABI surfaces.

The first implementation intentionally provides only canonical-size `draw`. It does not provide anchors, general scaling, fit modes, stretching, upscaling exceptions, or a scene-object abstraction.

Receiver-scoped resolution distinguishes the two `draw` functions by parameter 0 type before matching the remaining arity and types. `Sprite.draw` and `TextRun.draw` may therefore use their natural different arities; the generic spec sentence requiring same-name declarations to share one arity does not describe the implemented receiver-scoped rule and should be clarified in the canonical specification.

## Loading invariants

`load_sprite_from` must:

1. reject non-positive logical dimensions without calling the host;
2. attempt the native load using the supplied logical dimensions;
3. on success, assign the handle, width, and height together;
4. on failure, leave the receiver unchanged (which keeps a fresh zero-valued receiver invalid);
5. return `true` only when the resulting `Sprite` is valid.

`load_text_from` follows the same transactional-result rule. Its width and height are the cached run's logical measured bounds. If the current host surface exposes only cached width, the implementation must add or derive a real logical height rather than inventing a placeholder contract.

Replacing an already valid sprite releases the old handle only after its replacement loads successfully. Failed replacement preserves the old valid resource. Prepared text remains owned by the host cache, which currently has no individual run-release operation; this API does not invent a second ownership policy.

`Sprite.release()` is idempotent. It calls the native release bridge only when the
receiver has a nonzero handle, then clears `handle`, `width`, and `height` even
when the handle is stale or already released. A copied `Sprite` value is not an
extra host acquisition: manually copying its integer fields and releasing both
copies is invalid ownership. Code that needs two owners must load the resource
twice (the host may return the same native handle and balances both references).

Native desktop and mobile-AOT handles contain a slot generation. Release makes
the old generation invalid, and a later slot reuse receives a different
generation; stale handles are omitted from renderer restoration. The JIT and
mobile AOT replacement paths publish the newly acquired receiver state before
releasing the previous ownership, including when both acquisitions return the
same handle.

Android Workshop uses stable manifest handles as content identities, not GPU
allocation IDs. Typed loads maintain a bounded per-project reference table. A
zero-reference release is queued for the GL thread and is canceled if the same
stable handle is acquired before the queue is drained. Workshop may map several
manifest handles to one decoded texture by content hash; the GLES texture is
deleted only after its last mapping is removed. The queue and table retain
bounded deterministic limits, and overflow is surfaced as a resource error.
Raw/direct manifest handles that were never acquired through typed loading are
not owned by this release table. Surface restoration rebuilds only live entries;
released entries and pending canceled releases cannot reappear.

## Drawing invariants

`Sprite.draw` reads width and height from the receiver and emits the existing sprite command at exactly those logical dimensions. It rejects or ignores an invalid receiver according to the existing graphics-command failure policy; it never substitutes caller-provided dimensions.

`TextRun.draw` reads the cached text handle from its receiver. Font ownership is explicit because the existing prepared-text command requires both font and run handles; it must not recover the font through a detector, global side table keyed only by source position, or fake fallback.

Frequently changing labels use the bounded caller-owned replacement contract in
[dynamic_text_runs.md](dynamic_text_runs.md); immutable `load_text_from` behavior remains unchanged.

Physical density remains transparent to Stasis code:

```text
physical raster width = logical painted width * active raster scale
```

The host remains responsible for rounding, caps, atlas placement, density-generation changes, and backend-specific texture ownership.

## Host boundary and migration

`load_sprite_from` and `load_text_from` are direct host-bound receiver functions. A struct view crosses the ABI as `(base, index, len)`, allowing the host to publish the handle and logical measurements transactionally into either a global struct or an array element. No raw loader, cache, measurement, release, or legacy draw wrapper is declared in Stasis source.

The native renderer's lower-level ABI remains an implementation detail used by the receiver host functions. Bundled samples and standard-library helpers use `Sprite` and `TextRun`; command-buffer primitives remain available to renderer-building code but are not resource-loading APIs.

## Verification requirements

The first implementation slice must cover:

- receiver-form mutation of a global `Sprite` and `TextRun`;
- successful load metadata assignment;
- atomic clearing on invalid dimensions or native load failure;
- idempotent `Sprite.release()` clearing all receiver metadata;
- canonical sprite command width and height sourced from the receiver;
- cached-text drawing through the receiver;
- JIT and AOT lowering for the representative program where applicable;
- one end-to-end executable sample with asserted behavior;
- bounded repository validation with no lingering test/compiler processes.

The implementation includes `samples/typed_sprite` as the executable contract fixture and
`samples/typed_drawable_visual` as a typed-only deterministic framebuffer fixture. Run them with:

```powershell
powershell -ExecutionPolicy Bypass -File tools/verify-typed-drawables.ps1
powershell -ExecutionPolicy Bypass -File tools/verify-typed-drawable-visual.ps1
powershell -ExecutionPolicy Bypass -File tools/verify-typed-drawable-migration.ps1
```

The visual verifier renders the typed path twice through the real Stasis framebuffer and requires
the PNG bytes to be identical. The migration verifier rejects removed API names, compiles ten
representative bundled entries, and proves a legacy `gfx_load_sprite` call is a compile error. The implementation also adds JIT and linked-AOT regression coverage for
same-name receiver methods whose receiver types and natural arities differ.

The final reviewed captures are physically 360x240 pixels while the renderer reports an 800x600
logical canvas. Raw and typed captures are byte-identical with SHA-256
`D4EEA1A4838D60DA95F40517DF00687A2F5C462FAAEFFC37F6BAED8575DAFC11`.
Independent review found no typed-path clipping, alignment, scaling, color, or raster-quality
regression. The deterministic parity font renders the lower glyph row as thin bars; that shared
fixture appearance is accepted because this test asserts rendering equivalence rather than UI copy
legibility.

Final visual defect log:

- `TDRAW-01` (blocker if present): repeated typed framebuffer mismatch; closed, none observed.
- `TDRAW-02` (minor): physical capture is 360x240 rather than logical 800x600; accepted and
  documented because both paths share the same output surface.
- `TDRAW-03` (major for standalone copy, out of scope for the deterministic fixture): deterministic
  test glyphs are not human-readable; accepted because this fixture tests resource rendering rather
  than UI copy.
- `TDRAW-04` (minor if present): clipping or poor edge rasterization; closed, none observed.
- `TDRAW-05` (major if present): typed-path displacement, bounds, color, or scale regression;
  closed, none observed.

## Rationale

The nearest tempting alternative is to retain raw handles and diagnose mismatched load/draw dimensions through compiler data-flow analysis. That preserves two declarations of the same fact and cannot cover dynamic handle flow without runtime metadata. Typed resources instead make canonical drawing correct by construction and leave future resizing as an explicit extension.

The receiver is a mutable view into global-backed state, matching Stasis's existing struct-view ABI and preferred receiver-call syntax. A factory returning a temporary struct would add no semantic value to this slice and would depend on broader temporary struct materialization.

## Extension point

Future work may add explicit downscale, fit, or intentionally upscaled operations. Those APIs must remain distinct from canonical `draw` and must validate against the resource's logical painted envelope. Asset-manifest IDs may eventually supply the logical size so paths and dimensions also have one declaration.

TextRun and font ownership intentionally remain unchanged in this slice. The
existing host text cache retains font bytes and prepared runs through surface
restore; a future text/font release extension must define cache ownership
separately rather than reusing sprite release semantics.
`SpriteSheet` ownership is likewise unchanged; adding its explicit release
operation remains a separate API extension from this `Sprite.release()` slice.

Theory gained: a drawable's logical painted envelope belongs to the loaded resource, not to each draw command. The existing graphics path proves that physical raster density is already a host concern while Stasis draw commands still duplicate logical size. This predicts that receiver-owned logical dimensions can remove silent enlargement from ordinary drawing without changing the renderer or command-buffer architecture.

## Sprite sheets and deterministic clips

`Sprite.draw` remains a full-texture draw. `SpriteSheet.load_sprite_sheet_from(path, columns, rows, cell_width, cell_height)` loads one asset transactionally and records a validated uniform grid; `draw_frame` selects a row-major cell while preserving its logical painted dimensions. `draw_frame_scaled` selects the same validated cell while accepting caller-selected width and height, keeping geometry customization in the stdlib without exposing UV command-buffer patching to game code. A manifest sprite may additionally declare `format.layout` with the same `columns` and `rows`; Stasis validates that the declared image dimensions divide evenly into those cells and preserves the metadata through preparation. Prepared PNG atlases resize whole cells before rebuilding the image, so `SpriteSheet.load_sprite_sheet_from` can continue to use exact cell dimensions. The sheet abstraction owns normalized source-region details, so ordinary game code cannot accidentally issue malformed UV commands.

`AnimationClip` is a pure helper for authored frame mappings. Its `first_frame`, `frame_count`, `ticks_per_frame`, and integer playback mode are data-driven; the standard constants are `ANIMATION_PLAYBACK_ONCE`, `ANIMATION_PLAYBACK_LOOP`, and `ANIMATION_PLAYBACK_PING_PONG`. `frame_at(elapsed_ticks)` clamps negative time and wraps or reflects deterministically, while `finished(elapsed_ticks)` reports completion for once-only clips. Games should bind frame mapping and timing from JSON; StasisLang owns UV sampling and playback mechanics.
The executable reference is `samples/sprite_sheet_animation`. It loads one 2x2 PNG, draws all four row-major cells, and packages the single source image for both JIT and AOT builds. The fixture verifier and language tests are:

```powershell
python tools/verify_sprite_sheet_fixture.py
stasis --workspace samples/sprite_sheet_animation check
stasis --workspace samples/sprite_sheet_animation build
```

Good: one texture and one sprite command represent every cell, so animation frames do not duplicate GPU resources or asset-manifest records. Bad: nested resource structs and arbitrary annotated wrapper calls are not yet supported by the current JIT call boundary. Adjustment: `SpriteSheet` uses a flat resource layout and one compiler-recognized loader while keeping clip timing independent and data-driven. Theory gained: a generic renderer needs only validated source regions plus deterministic frame selection; game-specific states, pacing, and clip mappings remain configuration data rather than engine policy.
