# Product Requirements Document (PRD)

## Project

Stasis Immediate Axis Placement

## 1. Purpose

Provide a small, shared way to calculate the starting x or y coordinate of fixed-size or measured UI content within an available region.

The v1 system is deliberately scalar. Each placement function receives:

- the parent start coordinate
- the parent size
- the child size
- a strongly typed placement choice

It returns one `f32` coordinate. The caller uses those returned coordinates for text, sprites, buttons, hit testing, or as inputs to another layout calculation.

This design matches the Stasis language surface implemented today. It uses ordinary scalar locals and scalar returns. It does not require function-local arrays, local struct materialization, array returns, hidden allocation, or a retained UI tree.

`f32` is the canonical type for presentation geometry across the proposed public path. Positions, sizes, padding, pointer coordinates, text measurement, single-pass rectangle results, and sprite geometry remain `f32` until a platform renderer explicitly rasterizes them.

Stasis is a pre-1.0 language. This work chooses a clear canonical API and removes superseded compatibility aliases in the same change. It does not preserve ambiguous names, duplicate wrappers, deprecated stubs, or old HostFrame fields solely for source or ABI compatibility.

## 2. Problem

Stasis games currently repeat placement calculations such as:

```stasis
let title_x: f32 = safe_x + (safe_w - title_w) * 0.5;
let label_y: f32 = button_y + (button_h - line_h) * 0.5 + 1.0;
let icon_x: f32 = panel_x + panel_w - icon_w - 12.0;
```

These calculations are individually simple, but repeated versions can diverge between screens, drawing, and hit testing. The single-pass rectangle flow distributes sibling rectangles, while typed axis placement provides the small primitive for positioning one item within each resulting rectangle.

The shared system should capture that missing operation without introducing a larger UI framework.

## 3. Goals

V1 must:

1. Calculate a child x coordinate from horizontal placement inputs.
2. Calculate a child y coordinate from vertical placement inputs.
3. Use different enum types for horizontal and vertical placement.
4. Express margins and padding by adjusting the available region before placement.
5. Work entirely with supported scalar parameters, locals, and returns.
6. Support cached text placement using width measured and stored during resource initialization plus an explicit line height.
7. Support buttons, icons, HUD labels, menu titles, and similar immediate-mode drawing.
8. Compose with single-pass horizontal and vertical stacks plus game-owned grid placement.
9. Perform no allocation and no host-side UI registration.
10. Produce deterministic JIT and AOT behavior.
11. Remove avoidable `i32`/`f32` conversions from the public presentation path.
12. Expose layout-facing viewport and sprite geometry as `f32`, updating host/runtime definitions where required.
13. Remove ambiguous pre-1.0 display and input compatibility APIs in favor of explicit logical-coordinate and physical-pixel APIs.

## 4. Non-Goals

V1 does not provide:

- a rectangle value type
- local `f32[4]` rectangle storage
- functions that write rectangles through `f32[]` output parameters
- a retained component or widget tree
- a general constraint solver
- CSS-compatible multi-pass layout
- automatic text wrapping, ellipsis, or font scaling
- clipping or scrolling
- automatic sibling distribution beyond the single-pass stack helpers
- pointer capture, focus, or click lifecycle ownership
- automatic portrait/landscape redesign
- automatic overflow resolution
- style ownership
- animation ownership
- replacing integer handles, counts, indices, enum representations, or physical device metadata with floats

Flat arrays remain valid for game-owned global-backed bulk storage. They are not required by either the v1 single-item placement API or the single-pass current-rectangle flow.

## 5. Product Principles

### 5.1 One axis, one scalar result

Horizontal placement returns x. Vertical placement returns y. The API does not manufacture a rectangle when the caller only needs a draw origin.

### 5.2 Strongly typed axes

Horizontal and vertical choices use different enums so the compiler rejects swapped arguments.

```stasis
enum UiHorizontal {
    Left,
    Center,
    Right,
}

enum UiVertical {
    Top,
    Center,
    Bottom,
}
```

The implementation compares enum members directly. It does not convert them to `i32` unless an actual integer ABI boundary requires it, in which case it uses explicit `enum_to_i32` conversion.

### 5.3 The available region owns margins and padding

Placement functions do not accept an offset. Callers express margins and padding by deriving the region in which content is allowed to appear:

```stasis
let content_x: f32 = parent_x + left_padding;
let content_width: f32 = parent_width - left_padding - right_padding;
```

Alignment then has one stable meaning within that adjusted region. This avoids negative right/bottom offsets and avoids an ambiguous direction for center offsets.

An art-directed visual nudge is a separate operation applied explicitly after placement:

```stasis
let label_y: f32 = ui_place_y(content_y, content_h, label_h, UiVertical.Center);
label_y += label_optical_y;
```

### 5.4 Layout is pure calculation

Placement functions do not draw, measure text, inspect input, retain state, or change host state.

### 5.5 Coordinate space belongs to the caller

The functions operate in whatever coordinate space the caller supplies. They do not silently translate among safe viewport, window, native, drawable, fixed-design, or world coordinates.

### 5.6 Current language behavior is the boundary

V1 examples must compile using implemented Stasis features. In particular, examples must not declare function-local fixed arrays such as:

```stasis
// Not part of the currently supported v1 contract.
let rect: f32[4];
```

`Type[]` parameters are supported Stasis views, but this placement API does not need them.

### 5.7 Presentation geometry uses `f32`

The canonical public presentation types are:

| Value | Type |
|---|---|
| Layout x/y | `f32` |
| Layout width/height | `f32` |
| Layout margins, padding, and optical nudges | `f32` |
| Pointer x/y used for hit testing | `f32` |
| Measured text width and line-box height | `f32` |
| Line endpoints | `f32` |
| Sprite x/y/width/height | `f32` |
| Resource handles | `i32` |
| Counts and array indices | `i32` |
| Physical pixel counts retained only as host metadata | `i32` |

Raw platform dimensions may originate as integers because physical pixels are discrete. The host snapshot may continue storing those raw values as `i32`. The layout-facing Stasis API converts them once and exposes `f32`; callers must not repeat that conversion at every use.

### 5.8 Sprite geometry remains floating point through submission

The current sprite API and command stream use `i32` geometry. The implementation associated with this PRD should migrate sprite x, y, width, and height to `f32` rather than hiding four conversions inside a convenience wrapper:

```stasis
function gfx_draw_sprite(
    handle: i32,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    rot_deg: i32,
    alpha: i32
): void;
```

Sprite handles, rotation policy, and alpha remain unchanged. The graphics command-buffer version and host decoder must change together so sprite geometry is carried in the floating-point command stream or an equivalently typed representation.

Rounding and rasterization then occur in one platform rendering layer. Stasis layout and widget code do not choose an integer rounding policy independently.

### 5.9 Pre-1.0 HostFrame replacement policy

This is an intentional breaking cleanup. The scalar display and input query
functions are removed rather than deprecated. `graphics.stasis` transitively
provides the public `HostFrame` type, callers own one global snapshot, and
`refresh(self: HostFrame): void` updates it once per tick or frame.

Layout reads `host_frame.display.logical_width`, `logical_height`, `safe_x`,
`safe_y`, `safe_width`, and `safe_height`. Physical metadata is available as
the `native_*_px` and `drawable_*_px` fields. Pointer records expose logical
deltas and positions plus normalized positions without per-field host calls.

This public-surface cleanup does not change the raw HostFrame wire layout, so
its version remains 4. Compatibility fields such as `HOST_I_WINDOW_*` and
`HOST_I_VIEWPORT_*` were already removed from that active contract rather than
being populated indefinitely as aliases. Removed source APIs fail with normal
deterministic unknown-function diagnostics; no compatibility implementation
remains.

## 6. Required API

### 6.1 Horizontal placement

```stasis
function ui_place_x(
    parent_x: f32,
    parent_width: f32,
    child_width: f32,
    horizontal: UiHorizontal
): f32;
```

Behavior:

```text
Left:   parent_x
Center: parent_x + (parent_width - child_width) * 0.5
Right:  parent_x + parent_width - child_width
```

### 6.2 Vertical placement

```stasis
function ui_place_y(
    parent_y: f32,
    parent_height: f32,
    child_height: f32,
    vertical: UiVertical
): f32;
```

Behavior:

```text
Top:    parent_y
Center: parent_y + (parent_height - child_height) * 0.5
Bottom: parent_y + parent_height - child_height
```

### 6.3 Oversized children

The functions do not clamp or resize children larger than their parents. They apply the selected formula consistently:

- left/top alignment preserves the parent's starting edge
- center alignment overflows equally on both sides
- right/bottom alignment preserves the parent's ending edge

The caller decides whether overflow is acceptable.

### 6.4 Invalid placement values

The enum-typed signatures prevent ordinary callers from supplying arbitrary integers or swapping horizontal and vertical values. No runtime fallback for an unknown placement integer is required.

## 7. Realistic Usage Examples

### 7.1 Cache reusable UI resources during initialization

```stasis
global ui_font: i32;
global menu_title_run: i32;
global menu_title_width: f32;
global play_label_run: i32;
global play_label_width: f32;
global button_sprite: i32;
global host_frame: HostFrame;

function menu_load_ui(): void {
    ui_font = load_font("assets/ui.ttf", 28);
    menu_title_run = gfx_cache_text(ui_font, "BRICKOUT REVENGE");
    menu_title_width = gfx_measure_text_cached(menu_title_run);
    play_label_run = gfx_cache_text(ui_font, "PLAY");
    play_label_width = gfx_measure_text_cached(play_label_run);
    button_sprite = gfx_load_sprite("assets/button.png", 256, 96);
}
```

The host call that measures a cached run occurs during initialization or an explicit resource update. Stasis-owned width scalars are stored alongside the run handles. Per-frame code reads those scalars and makes no text-measurement host call.

### 7.2 Menu title centered horizontally

The title is centered within the safe viewport and placed 24 units below its top edge.

```stasis
function menu_draw_title(): void {
    let safe_x: f32 = host_frame.display.safe_x;
    let safe_y: f32 = host_frame.display.safe_y;
    let safe_w: f32 = host_frame.display.safe_width;

    let title_w: f32 = menu_title_width;
    let title_h: f32 = 32.0;

    let title_x: f32 = ui_place_x(
        safe_x,
        safe_w,
        title_w,
        UiHorizontal.Center
    );

    let title_area_y: f32 = safe_y + 24.0;
    let title_area_h: f32 = 96.0 - 24.0;
    let title_y: f32 = ui_place_y(
        title_area_y,
        title_area_h,
        title_h,
        UiVertical.Top
    );

    draw_text_cached(
        ui_font,
        menu_title_run,
        title_x,
        title_y,
        1.0,
        0.9,
        0.7,
        1.0
    );
}
```

The caller never manually calculates the centered x coordinate.

### 7.3 Button anchored top-left within an adjusted region

This creates scalar button bounds 20 units from the safe viewport's left edge and 140 units below its top edge.

```stasis
function menu_draw_play_button(): void {
    let safe_x: f32 = host_frame.display.safe_x;
    let safe_y: f32 = host_frame.display.safe_y;
    let safe_w: f32 = host_frame.display.safe_width;
    let safe_h: f32 = host_frame.display.safe_height;

    let button_w: f32 = 180.0;
    let button_h: f32 = 56.0;

    let button_area_x: f32 = safe_x + 20.0;
    let button_area_y: f32 = safe_y + 140.0;
    let button_area_w: f32 = safe_w - 20.0;
    let button_area_h: f32 = safe_h - 140.0;

    let button_x: f32 = ui_place_x(
        button_area_x,
        button_area_w,
        button_w,
        UiHorizontal.Left
    );

    let button_y: f32 = ui_place_y(
        button_area_y,
        button_area_h,
        button_h,
        UiVertical.Top
    );

    gfx_draw_sprite(
        button_sprite,
        button_x,
        button_y,
        button_w,
        button_h,
        0,
        255
    );
}
```

For a 20-unit margin on both horizontal edges, define the available region once and select either edge:

```stasis
let button_area_x: f32 = safe_x + 20.0;
let button_area_w: f32 = safe_w - 40.0;

let button_x: f32 = ui_place_x(
    button_area_x,
    button_area_w,
    button_w,
    UiHorizontal.Right
);
```

### 7.4 Text centered inside a button

The button has scalar bounds. Padding derives a scalar content region, and the label is centered within it.

```stasis
let pad_x: f32 = 16.0;
let pad_y: f32 = 8.0;
let content_x: f32 = button_x + pad_x;
let content_y: f32 = button_y + pad_y;
let content_w: f32 = button_w - pad_x * 2.0;
let content_h: f32 = button_h - pad_y * 2.0;

let label_w: f32 = play_label_width;
let label_h: f32 = 28.0;

let label_x: f32 = ui_place_x(
    content_x,
    content_w,
    label_w,
    UiHorizontal.Center
);

let label_y: f32 = ui_place_y(
    content_y,
    content_h,
    label_h,
    UiVertical.Center
);
label_y += 1.0;

draw_text_cached(
    ui_font,
    play_label_run,
    label_x,
    label_y,
    1.0,
    1.0,
    1.0,
    1.0
);
```

The one-unit y change is an explicit optical nudge after placement. The available region still represents button padding, and the shared placement function does not hide font- or button-specific heuristics.

### 7.5 Fit detection with scalars

V1 does not need a rectangle helper to detect whether the label fits:

```stasis
let label_fits: bool =
    label_w <= content_w &&
    label_h <= content_h;
```

The caller may draw normally, use alternate cached copy, or display a debug warning.

### 7.6 Hit testing with the same scalar button bounds

The button draw and input paths use the same `button_x`, `button_y`, `button_w`, and `button_h` values:

```stasis
function ui_point_in_box(
    point_x: f32,
    point_y: f32,
    box_x: f32,
    box_y: f32,
    box_w: f32,
    box_h: f32
): bool {
    return
        point_x >= box_x &&
        point_x < box_x + box_w &&
        point_y >= box_y &&
        point_y < box_y + box_h;
}
```

```stasis
let hit_pad: f32 = 6.0;
let hit_x: f32 = button_x - hit_pad;
let hit_y: f32 = button_y - hit_pad;
let hit_w: f32 = button_w + hit_pad * 2.0;
let hit_h: f32 = button_h + hit_pad * 2.0;

if (host_frame.pointer_count > 0 && host_frame.pointers[0].went_up) {
    let clicked: bool = ui_point_in_box(
        host_frame.pointers[0].x_logical,
        host_frame.pointers[0].y_logical,
        hit_x,
        hit_y,
        hit_w,
        hit_h
    );
}
```

This demonstrates geometric reuse, not a final click lifecycle. Pointer capture and stable control IDs remain separate future interaction work.

### 7.7 Using placement with the single-pass rectangle flow

The single-pass layout owns one ephemeral current rectangle. A horizontal scope advances through sibling boxes without retaining a parallel rectangle array:

```stasis
function menu_layout_footer(): void {
    let row_w: f32 = 296.0;
    let row_h: f32 = 52.0;
    let row_x: f32 = ui_place_x(20.0, 320.0, row_w, UiHorizontal.Center);
    let row_y: f32 = ui_place_y(620.0, 56.0, row_h, UiVertical.Center);
    ui_hstack_begin(row_x, row_y, row_w, row_h, 16.0, UiStackDirection.Forward);

    ui_hstack_next_fixed(140.0);
    draw_footer_button(ui_current_x(), ui_current_y(), ui_current_width(), ui_current_height());

    ui_hstack_next_fixed(140.0);
    draw_footer_button(ui_current_x(), ui_current_y(), ui_current_width(), ui_current_height());

    ui_hstack_end();
}
```

Content placement reads the current rectangle immediately and uses the same typed axis functions:

```stasis
let button_x: f32 = ui_current_x();
let button_y: f32 = ui_current_y();
let button_w: f32 = ui_current_width();
let button_h: f32 = ui_current_height();

let label_x: f32 = ui_place_x(
    button_x,
    button_w,
    label_w,
    UiHorizontal.Center
);

let label_y: f32 = ui_place_y(
    button_y,
    button_h,
    label_h,
    UiVertical.Center
);
label_y += 1.0;
```

Typed axis placement therefore composes directly with the single-pass current-rectangle flow. Callers that need geometry later copy only the scalar values they own; the shared layout layer does not retain per-control rectangles.

## 8. Text Model

### 8.1 Width

Text caching and width measurement occur together during initialization or an explicit resource update:

```stasis
title_run = gfx_cache_text(ui_font, "BRICKOUT REVENGE");
title_width = gfx_measure_text_cached(title_run);
```

Per-frame UI uses the Stasis-owned `title_width` scalar. It must not call `gfx_measure_text_cached`, because that binding crosses the host boundary and the graphics stdlib excludes host calls from rendering hot paths.

Each reusable text resource therefore has two distinct values:

- cached run handle: `i32`, used for drawing
- cached measured width: `f32`, used for layout

### 8.2 Height

Until the runtime exposes ascent, descent, baseline, and measured text height, callers provide an explicit line-box height.

The height represents layout space, not glyph ink bounds or a baseline coordinate.

### 8.3 Optical adjustment

Art-directed correction is applied explicitly after placement as a signed nudge at the call site or from a game-owned style value. It is not part of the placement API.

### 8.4 Overflow

V1 exposes enough scalar information for the caller to detect fit. It does not wrap, shrink, clip, reject, or otherwise resolve overflow.

## 9. Coordinate Spaces

### 9.1 Adaptive safe-area UI

Adaptive menus and HUDs can read safe viewport scalars directly:

```stasis
host_frame.refresh();
let safe_x: f32 = host_frame.display.safe_x;
let safe_y: f32 = host_frame.display.safe_y;
let safe_w: f32 = host_frame.display.safe_width;
let safe_h: f32 = host_frame.display.safe_height;
```

The logical, safe viewport, and available presentation fields are `f32`.
Physical native and drawable pixel counts remain `i32` metadata. Window
creation and resize requests may continue accepting integer logical dimensions
because they establish a discrete requested canvas configuration. Hit testing
reads `x_logical` and `y_logical` from a present pointer in the same refreshed
snapshot.

### 9.2 Fixed-design canvas

A game may separately calculate a fixed logical canvas scale and offset, similar to Brickout's existing play layout. The caller then passes coordinates from that chosen space into `ui_place_x` and `ui_place_y`.

The axis functions do not silently choose letterboxing or convert input coordinates.

### 9.3 Pixel snapping

Placement and graphics submission remain in `f32`. Conversion or snapping occurs inside the platform renderer at rasterization so nested calculations and Stasis command construction do not accumulate rounding error.

If a backend requires integer raster bounds, it must use one documented edge-based policy. It should derive width from snapped right and left edges rather than independently truncating x and width, which can introduce seams.

## 10. Rendering and Interaction Boundaries

### 10.1 Rendering

Drawing consumes the calculated scalar coordinates. Placement functions emit no graphics commands.

### 10.2 Hit testing

Hit testing consumes the same scalar bounds used for drawing, or explicitly expanded scalar bounds for a larger touch target.

V1 uses half-open bounds:

```text
x >= left and x < left + width
y >= top  and y < top + height
```

This prevents adjacent controls from both claiming a shared edge.

### 10.3 Interaction state

V1 does not define click timing or pointer capture. A later interaction layer should use stable integer control IDs and explicitly define press, capture, release, cancellation, focus, and multi-pointer behavior.

## 11. Performance Requirements

- Each axis function is constant time.
- Placement performs no allocation.
- Placement makes no host calls.
- Cached width measurement remains the only text-width dependency.
- Viewport integers are converted to `f32` once at the host/stdlib boundary, not once per caller.
- Sprite submission does not convert x, y, width, or height back to `i32` in Stasis code.
- Diagnostic behavior uses the same placement path.
- JIT and AOT results are equivalent within documented floating-point tolerance.

## 12. Testing Requirements

### 12.1 Axis tests

Horizontal tests cover:

- left, center, and right placement
- zero-size parent and child
- child larger than parent
- fractional values and odd sizes

Vertical tests cover the equivalent top, center, and bottom behavior.

### 12.2 Type-safety tests

Compiler tests verify that:

- `UiHorizontal` is accepted by `ui_place_x`
- `UiVertical` is accepted by `ui_place_y`
- arbitrary `i32` values are rejected
- passing `UiVertical` to `ui_place_x` is rejected
- passing `UiHorizontal` to `ui_place_y` is rejected

### 12.3 Placement invariants

Representative assertions include:

```text
centered_child_center == parent_center
right_child_edge == parent_right
bottom_child_edge == parent_bottom
```

### 12.4 Realistic composition

At least one test or sample must:

1. read or define scalar parent bounds
2. use a width cached during initialization or an explicit test width
3. center text horizontally
4. derive an inset available region and place a button at its top-left
5. center a label inside that button
6. verify the calculated values

### 12.5 Graphics boundary tests

Tests must verify that:

- logical canvas and safe viewport snapshot fields are `f32`
- removed compatibility display and input APIs are absent from the stdlib surface
- logical pointer snapshot fields are `f32` in the same coordinate space as logical and safe layout bounds
- pointer, text, line, single-pass layout, placement, and sprite geometry share the `f32` presentation path
- sprite command encoding and host decoding agree on the new geometry representation
- fractional sprite positions and sizes survive command submission until renderer rasterization
- odd and adjacent sprite bounds follow the documented backend snapping policy without gaps introduced by independent truncation
- all repository callers are migrated in the same checklist-selected group; removed external source calls receive deterministic unknown-function diagnostics
- the HostFrame version is bumped and active hosts no longer populate compatibility window or viewport alias fields

### 12.6 End-to-end compiler gate

Completed implementation work must include a representative `.stasis` program that reaches Cranelift IR, builds into an executable, runs, and verifies behavior with assertions.

All test commands remain bounded to 900 seconds, with lingering processes checked after each run.

## 13. Implementation Areas

`docs/build_checklist.md` is the authoritative source for implementation selection, grouping, and ordering. This PRD defines required implementation areas only. When this work is selected, the checklist must sequence these areas according to repository priorities and migration constraints.

### Float presentation boundary

- expose logical canvas and safe viewport geometry as `f32` HostFrame fields
- replace pointer compatibility queries with explicit logical-coordinate fields
- remove superseded scalar queries and HostFrame alias fields
- expose native and drawable metadata as explicit `_px` fields
- preserve the HostFrame v4 wire layout while migrating callers to the typed snapshot atomically
- migrate sprite x/y/width/height parameters and command storage to `f32`
- update existing sprite callers and parity fixtures
- verify fractional geometry through a real renderer path

### Typed scalar axis placement

- add `UiHorizontal` and `UiVertical`
- add `ui_place_x` and `ui_place_y`
- add deterministic axis and type-safety tests
- run an end-to-end executable fixture

### One real menu adoption

- replace one menu title's manual centering calculation
- anchor one button using the shared axis functions
- center one cached label inside that button
- use the same button bounds for drawing and hit testing
- add visual or command-buffer verification where practical

No further layout abstraction is required before these implementation areas prove the API. Their order in this document is descriptive, not authoritative.

## 14. Acceptance Criteria

V1 is complete when:

1. `ui_place_x` returns correct left, center, and right positions.
2. `ui_place_y` returns correct top, center, and bottom positions.
3. Horizontal and vertical choices use distinct enum types end to end.
4. Margins and padding adjust the available region before placement; placement accepts no offset.
5. Oversized children follow the documented alignment formulas without implicit clamping.
6. Realistic examples use supported scalar locals and returns.
7. No v1 example or API requires function-local fixed arrays or local struct materialization.
8. Cached text width and explicit line height are sufficient to calculate draw origins.
9. Single-pass current rectangles feed scalar placement without retained geometry.
10. One real menu title, button, and centered button label demonstrate the API.
11. Drawing and hit testing reuse the same scalar button bounds.
12. Tests execute the representative path through Cranelift and verify results.
13. Canonical logical and safe viewport HostFrame fields are `f32` without caller-side conversion.
14. Sprite x/y/width/height remain `f32` through Stasis command submission.
15. The graphics command-buffer version and all host decoders agree on the migrated sprite representation.
16. Raw physical pixel counts, handles, counts, and indices remain `i32` where integer semantics are intrinsic.
17. Superseded scalar display and pointer queries are removed rather than deprecated.
18. Canonical logical pointer fields share the coordinate space used by the logical and safe display fields.
19. Native and drawable integer HostFrame fields use explicit `_px` names.
20. The HostFrame v4 contract contains no active compatibility aliases for removed window or viewport fields.

## 15. Deferred Extensions

Add later only when a demonstrated caller requires them:

- a first-class `UiRect` value after local struct values and ergonomic struct returns are implemented
- rectangle inset and expansion helpers over a supported value model
- a combined nine-position anchor for stored configuration
- parent-anchor to child-anchor attachment for tooltips and badges
- fixed/fill/weight size resolution
- multiline text metrics and wrapping
- clipping and scroll containers
- stable-ID interaction state with pointer capture
- focus navigation and accessibility metadata

The scalar axis functions remain useful beneath these extensions.

## 16. Theory of Operation

### Mapping

The real behavior being represented is one-dimensional placement: choose where a child of known size begins within an available span. Two independent applications of that rule produce a two-dimensional draw origin.

Text width comes from cached measurement, text height from an explicit line box, and button or sprite sizes from game-owned values. Drawing and input consume the resulting scalar bounds.

### Rationale

Stasis currently supports scalar locals and returns cleanly, while `Type[]` is a view over existing fixed storage rather than local temporary array construction. A scalar-return API expresses the needed calculation without pretending that local rectangle values are available.

Using `f32` throughout presentation geometry also matches cached text measurement, pointer input, line commands, single-pass layout, and scaled canvases. Keeping sprite geometry or layout-facing viewport accessors as `i32` would insert conversion and rounding decisions into ordinary UI code without providing a meaningful performance benefit.

The nearest tempting alternative is an output `f32[]` rectangle API. Although array views are supported, that design requires pre-existing storage and makes a simple calculation appear to create a local rectangle. It also encourages unnecessary global scratch layout state.

### Extension point

A future rectangle type can call the same axis functions internally. The single-pass layout already exposes current scalar x, y, width, and height values that feed directly into the axis functions.

### Prediction

If this model is sufficient, menu titles, button labels, corner icons, HUD counters, and content inside stack-generated boxes will share `ui_place_x` and `ui_place_y` without needing a general rectangle framework. A request for rectangle values should arise only when multiple downstream consumers genuinely need to pass or store the complete box as one value.

Machine-readable review is a separate concern from placement. The debug-facing
`src/stdlib/testing/ui_layout_audit.stasis` module consumes the final scalar rectangles and measured
text bounds without changing them, then emits deterministic geometry and
relationship checks for automated or AI review. See
`docs/ui_layout_audit.md` and `samples/immediate_axis_layout/audit.stasis`.
