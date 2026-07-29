# Typed drawable resources

## Status

Implemented initial graphics-standard-library slice, with the raw handle API retained for compatibility.

## Problem

The current sprite API represents a loaded image as an untyped `i32` handle:

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

The first implementation intentionally provides only canonical-size `draw`. It does not provide anchors, general scaling, fit modes, stretching, upscaling exceptions, or a scene-object abstraction.

Receiver-scoped resolution distinguishes the two `draw` functions by parameter 0 type before matching the remaining arity and types. `Sprite.draw` and `TextRun.draw` may therefore use their natural different arities; the generic spec sentence requiring same-name declarations to share one arity does not describe the implemented receiver-scoped rule and should be clarified in the canonical specification.

## Loading invariants

`load_sprite_from` must:

1. reject non-positive logical dimensions without calling the host;
2. attempt the native load using the supplied logical dimensions;
3. on success, assign the handle, width, and height together;
4. on failure, clear the handle and dimensions together;
5. return `true` only when the resulting `Sprite` is valid.

`load_text_from` follows the same atomic-result rule. Its width and height are the cached run's logical measured bounds. If the current host surface exposes only cached width, the implementation must add or derive a real logical height rather than inventing a placeholder contract.

Replacing an already valid resource raises a lifecycle question. The first implementation must inspect existing renderer ownership before choosing whether to release the old handle. It must not release an old valid resource before a replacement has loaded successfully, and it must not silently create an unbounded leak path.

## Drawing invariants

`Sprite.draw` reads width and height from the receiver and emits the existing sprite command at exactly those logical dimensions. It rejects or ignores an invalid receiver according to the existing graphics-command failure policy; it never substitutes caller-provided dimensions.

`TextRun.draw` reads the cached text handle from its receiver. Font ownership is explicit because the existing prepared-text command requires both font and run handles; it must not recover the font through a detector, global side table keyed only by source position, or fake fallback.

Physical density remains transparent to Stasis code:

```text
physical raster width = logical painted width * active raster scale
```

The host remains responsible for rounding, caps, atlas placement, density-generation changes, and backend-specific texture ownership.

## Compatibility and migration

The initial slice adds the typed API alongside the existing raw functions. It migrates one representative end-to-end sample and adds deterministic coverage before considering removal or deprecation of raw handle APIs.

The typed API must work through the same command buffer and native implementation as existing drawing. It is a source-level ownership improvement, not a second rendering pipeline.

## Verification requirements

The first implementation slice must cover:

- receiver-form mutation of a global `Sprite` and `TextRun`;
- successful load metadata assignment;
- atomic clearing on invalid dimensions or native load failure;
- canonical sprite command width and height sourced from the receiver;
- cached-text drawing through the receiver;
- JIT and AOT lowering for the representative program where applicable;
- one end-to-end executable sample with asserted behavior;
- bounded repository validation with no lingering test/compiler processes.

The implementation includes `samples/typed_sprite` as the executable contract fixture and
`samples/typed_drawable_visual` as a raw-versus-typed framebuffer parity fixture. Run them with:

```powershell
powershell -ExecutionPolicy Bypass -File tools/verify-typed-drawables.ps1
powershell -ExecutionPolicy Bypass -File tools/verify-typed-drawable-visual.ps1
```

The visual verifier renders both paths through the real Stasis framebuffer and requires the PNG
bytes to be identical. The implementation also adds JIT and linked-AOT regression coverage for
same-name receiver methods whose receiver types and natural arities differ.

The final reviewed captures are physically 360x240 pixels while the renderer reports an 800x600
logical canvas. Raw and typed captures are byte-identical with SHA-256
`D4EEA1A4838D60DA95F40517DF00687A2F5C462FAAEFFC37F6BAED8575DAFC11`.
Independent review found no typed-path clipping, alignment, scaling, color, or raster-quality
regression. The deterministic parity font renders the lower glyph row as thin bars; that shared
fixture appearance is accepted because this test asserts rendering equivalence rather than UI copy
legibility.

Final visual defect log:

- `TDRAW-01` (blocker if present): raw/typed pixel mismatch; closed, none observed.
- `TDRAW-02` (minor): physical capture is 360x240 rather than logical 800x600; accepted and
  documented because both paths share the same output surface.
- `TDRAW-03` (major for standalone copy, out of scope for parity): deterministic test glyphs are
  not human-readable; accepted because both paths render the exact same cached text pixels.
- `TDRAW-04` (minor if present): clipping or poor edge rasterization; closed, none observed.
- `TDRAW-05` (major if present): typed-path displacement, bounds, color, or scale regression;
  closed, none observed.

## Rationale

The nearest tempting alternative is to retain raw handles and diagnose mismatched load/draw dimensions through compiler data-flow analysis. That preserves two declarations of the same fact and cannot cover dynamic handle flow without runtime metadata. Typed resources instead make canonical drawing correct by construction and leave future resizing as an explicit extension.

The receiver is a mutable view into global-backed state, matching Stasis's existing struct-view ABI and preferred receiver-call syntax. A factory returning a temporary struct would add no semantic value to this slice and would depend on broader temporary struct materialization.

## Extension point

Future work may add explicit downscale, fit, or intentionally upscaled operations. Those APIs must remain distinct from canonical `draw` and must validate against the resource's logical painted envelope. Asset-manifest IDs may eventually supply the logical size so paths and dimensions also have one declaration.

Theory gained: a drawable's logical painted envelope belongs to the loaded resource, not to each draw command. The existing graphics path proves that physical raster density is already a host concern while Stasis draw commands still duplicate logical size. This predicts that receiver-owned logical dimensions can remove silent enlargement from ordinary drawing without changing the renderer or command-buffer architecture.
