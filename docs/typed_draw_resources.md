# Typed drawable resources

## Status

Proposed initial graphics-standard-library slice. This document defines the intended source contract before implementation and migration.

## Problem

The current sprite API represents a loaded image as an untyped `i32` handle:

```stasis
let aura: i32 = gfx_load_sprite("aura.svg", 64, 64);
gfx_draw_sprite(aura, x, y, 136.0, 136.0, 0, 255);
```

Loading and drawing independently declare the same painted dimensions. A scalable SVG, or a high-resolution raster deliberately reduced during loading, can therefore be baked into a small physical cache and silently enlarged later. The caller still appears to use vector or high-resolution source art, but the framebuffer exposes the reduced cache pixels.

Cached text has the same structural weakness: an `i32` handle loses the logical measurements and resource kind that distinguish it from every other integer.

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

struct CachedText {
    handle: i32;
    width: i32;
    height: i32;
}
```

`width` and `height` are logical painted dimensions. They are not source pixels, atlas pixels, native drawable pixels, layout occupancy, or collision bounds.

## Initial API

Stasis receiver-form functions are the primary API. They mutate an existing global-backed struct view and avoid depending on temporary struct return materialization.

```stasis
function load_sprite_from(self: Sprite, path: string, width: i32, height: i32): bool;
function draw(self: Sprite, x: f32, y: f32, alpha: i32, rotation: i32): void;

function load_text_from(self: CachedText, font: i32, text: string): bool;
function draw(self: CachedText, x: f32, y: f32, r: f32, g: f32, b: f32, a: f32): void;
```

Representative use:

```stasis
global state {
    aura: Sprite;
    title: CachedText;
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

The first slice intentionally provides only canonical-size `draw`. It does not provide anchors, general scaling, fit modes, stretching, upscaling exceptions, or a scene-object abstraction.

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

`CachedText.draw` reads the cached text handle from its receiver. Font ownership must be explicit in the final struct or runtime handle contract because the existing cached-text command requires both font and run handles. The implementation may add `font: i32` to `CachedText` if the current host contract requires it; it must not recover the font through a detector, global side table keyed only by source position, or fake fallback.

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

- receiver-form mutation of a global `Sprite` and `CachedText`;
- successful load metadata assignment;
- atomic clearing on invalid dimensions or native load failure;
- canonical sprite command width and height sourced from the receiver;
- cached-text drawing through the receiver;
- JIT and AOT lowering for the representative program where applicable;
- one end-to-end executable sample with asserted behavior;
- bounded repository validation with no lingering test/compiler processes.

## Rationale

The nearest tempting alternative is to retain raw handles and diagnose mismatched load/draw dimensions through compiler data-flow analysis. That preserves two declarations of the same fact and cannot cover dynamic handle flow without runtime metadata. Typed resources instead make canonical drawing correct by construction and leave future resizing as an explicit extension.

The receiver is a mutable view into global-backed state, matching Stasis's existing struct-view ABI and preferred receiver-call syntax. A factory returning a temporary struct would add no semantic value to this slice and would depend on broader temporary struct materialization.

## Extension point

Future work may add explicit downscale, fit, or intentionally upscaled operations. Those APIs must remain distinct from canonical `draw` and must validate against the resource's logical painted envelope. Asset-manifest IDs may eventually supply the logical size so paths and dimensions also have one declaration.

Theory gained: a drawable's logical painted envelope belongs to the loaded resource, not to each draw command. The existing graphics path proves that physical raster density is already a host concern while Stasis draw commands still duplicate logical size. This predicts that receiver-owned logical dimensions can remove silent enlargement from ordinary drawing without changing the renderer or command-buffer architecture.
