# Product Requirements Document (PRD)

## Project

Stasis Immediate Box Layout

## 1. Purpose

Provide a small, shared layout foundation for Stasis games and tools using explicit bounding boxes and deterministic placement rules.

The system computes a child rectangle from:

- an available parent rectangle
- the child's measured or fixed width and height
- independent horizontal and vertical placement
- explicit x and y offsets

Rendering, hit testing, and debug visualization consume the same computed rectangles. Layout is recalculated from current state when needed and does not create or retain a host-side UI tree.

This PRD defines the first reusable layer beneath buttons, menus, HUDs, cards, and other UI compositions. It builds on the existing `src/stdlib/flex_layout.stasis`, graphics command buffers, cached text measurement, host display snapshots, and plain-array conventions.

## 2. Goals

The first version must:

1. Establish one canonical rectangle representation for shared layout code.
2. Place fixed-size or measured content within a parent rectangle using start, center, or end placement on each axis.
3. Support explicit offsets without adding specialized constraint types for common cases.
4. Calculate top-left text draw coordinates from cached text width and an explicit line-box height.
5. Keep layout computation independent from painting and interaction behavior.
6. Allow paint bounds, content bounds, and hit bounds to be derived from the same allocated box.
7. Compose with existing row, column, and grid layout helpers.
8. Operate without heap allocation, callbacks, closures, or retained component trees.
9. Produce deterministic results suitable for Stasis tests and Cranelift JIT/AOT execution.
10. Make layout errors and computed bounds easy to inspect.

## 3. Non-Goals

Version 1 does not aim to provide:

- a retained-mode component or widget tree
- a CSS-compatible flexbox implementation
- a general constraint solver
- automatic text wrapping, ellipsis, or font scaling
- scroll containers
- clipping or scissor support
- automatic overlap avoidance
- automatic portrait/landscape screen redesign
- animation ownership
- keyboard, gamepad, or accessibility focus management
- a complete button interaction lifecycle
- percentage, intrinsic, min/max, or weighted size resolution
- persistence of computed rectangles across frames

These features may build on the box primitives later, but they are not prerequisites for a useful first version.

## 4. Product Principles

### 4.1 Explicit data over hidden layout state

Layout functions receive ordinary values and arrays and write ordinary rectangle data. They do not create hidden nodes, register components, or depend on call history.

### 4.2 Immediate computation

The expected frame model is:

```text
current game and display state
 -> compute available boxes
 -> place child boxes
 -> emit draw commands from those boxes
 -> test input against those boxes
 -> discard temporary box data
```

Persistent state is reserved for behavior that genuinely crosses frames, such as pointer capture, focus, selection, scrolling, or animation progress.

### 4.3 One box, multiple consumers

An allocated box is authoritative. Paint, content placement, hit-bound derivation, overflow checks, and debug drawing must not independently reconstruct its coordinates.

### 4.4 Layout is not painting

Core layout functions do not emit graphics commands. Text placement computes coordinates and bounds; a separate call draws the cached run.

### 4.5 Independent, strongly typed axes

Horizontal and vertical placement use separate enum types:

- horizontal: `Left`, `Center`, or `Right`
- vertical: `Top`, `Center`, or `Bottom`

This creates nine common placements without special constants such as `TOP_RIGHT` or `CENTER_XY`. Separate types make call sites readable and let the compiler reject accidentally swapped horizontal and vertical arguments.

### 4.6 Offsets use screen-coordinate signs

Offsets are always added after placement:

- positive x moves right
- negative x moves left
- positive y moves down
- negative y moves up

For example, content placed at `UiHorizontal.Right` with `offset_x = -16.0` is shifted 16 logical units left from the parent's right edge.

### 4.7 Coordinate-space policy is explicit

The box primitives operate in whichever coordinate space the caller supplies. They do not silently scale between logical, native, drawable, safe-viewport, or design-canvas coordinates.

Screen-space safe layout and fixed-design canvas fitting are separate policies layered above the box primitives.

## 5. User and Developer Scenarios

### 5.1 Centered title

A game measures a cached title run, supplies an explicit line height, and places its line box at horizontal `Center` and vertical `Top` within a header box.

### 5.2 Button label

A button painter derives a padded content box from the button's allocated box, centers a cached label within it, and draws at the returned top-left coordinate.

### 5.3 Right-aligned HUD counter

A HUD derives an inset safe-content box and places a measured counter at horizontal `Right`, vertical `Top` with no specialized right-edge API.

### 5.4 Corner icon

A fixed-size icon is placed at `UiHorizontal.Right` and `UiVertical.Bottom` within a card's content box. Its result is passed directly to sprite drawing.

### 5.5 Flex-composed footer

`flex_row` allocates button boxes across a footer. The Msplacement primitive centers each icon or text run within its allocated button box.

### 5.6 Larger touch target

A visual icon box is expanded into a separate hit box using explicit edge expansion. Hit testing uses the expanded box while debug mode can display both bounds.

## 6. Canonical Data Model

### 6.1 Rectangle representation

Shared layout uses the existing plain-array representation:

```stasis
// [x, y, width, height]
const UI_RECT_STRIDE: i32 = 4;
const UI_RECT_X: i32 = 0;
const UI_RECT_Y: i32 = 1;
const UI_RECT_W: i32 = 2;
const UI_RECT_H: i32 = 3;
```

The final prefix and module name may be selected during implementation, but the repository must converge on one shared set of rectangle constants and helpers. New duplicate `FLEX_RECT_*`, `HUD_RECT_*`, and `UI_RECT_*` vocabularies must not remain as independent public contracts.

Rectangles use top-left coordinates, matching Stasis sprite and text placement.

### 6.2 Rectangle invariants

Public geometry and placement functions must:

- preserve finite input values when valid
- normalize computed negative width and height to zero where an operation can shrink a box
- define edge inclusion consistently for hit testing
- accept rectangles stored individually or at a fixed stride in a larger array where applicable
- avoid writes outside the documented output rectangle

The v1 hit-test edge policy is:

```text
x >= left and x < right
y >= top  and y < bottom
```

Half-open bounds avoid two adjacent controls claiming a shared edge.

### 6.3 Placement enums

```stasis
enum UiHorizontal {
    Left,
    Center,
    Right
}

enum UiVertical {
    Top,
    Center,
    Bottom
}
```

Placement is a closed semantic choice, so the public API uses Stasis enums rather than unrelated integer constants. This lets the compiler reject invalid values and prevents callers from accidentally passing a flex justification, alignment value, sprite handle, or arbitrary integer as a placement.

The axes intentionally use different enum types. A call that supplies `UiVertical.Top` in the horizontal argument is a compile-time type error.

The implementation should compare enum members directly and should not convert them to `i32` unless an actual integer ABI boundary requires it. Any such boundary must use the explicit `enum_to_i32` conversion described by the language spec.

## 7. Realistic Usage Examples

These examples show the intended call style. Cached text handles and sprite handles are loaded during initialization; layout and drawing happen from current frame state.

The examples use the proposed names from this PRD. Exact module imports may change during implementation.

### 7.1 Initialization of reusable resources

Text used every frame is cached once rather than rebuilt or measured from an uncached string in the render path.

```stasis
global ui_font: i32;
global menu_title_run: i32;
global play_label_run: i32;
global button_sprite: i32;

function menu_load_ui(): void {
    ui_font = load_font("assets/ui.ttf", 28);
    menu_title_run = gfx_cache_text(ui_font, "BRICKOUT REVENGE");
    play_label_run = gfx_cache_text(ui_font, "PLAY");
    button_sprite = gfx_load_sprite("assets/button.png", 256, 96);
}
```

### 7.2 Menu title centered horizontally

This places a title at the horizontal center of a header box while keeping its top edge 24 logical units below the header's top edge.

```stasis
function menu_draw_title(screen: f32[]): void {
    let header: f32[4];
    let title: f32[4];

    // Reserve the top 96 units of the supplied screen or safe-area box.
    ui_rect_set(
        header,
        screen[UI_RECT_X],
        screen[UI_RECT_Y],
        screen[UI_RECT_W],
        96.0
    );

    let fits: bool = ui_place_text_run(
        title,
        header,
        menu_title_run,
        32.0,
        UiHorizontal.Center,
        UiVertical.Top,
        0.0,
        24.0
    );

    draw_text_cached(
        ui_font,
        menu_title_run,
        title[UI_RECT_X],
        title[UI_RECT_Y],
        1.0,
        0.9,
        0.7,
        1.0
    );

    if (!fits) {
        ui_debug_draw_overflow(title);
    }
}
```

The important inputs are independent:

- `UiHorizontal.Center` centers the measured text width horizontally.
- `UiVertical.Top` starts from the header's top edge vertically.
- `offset_y = 24.0` moves the title down from that edge.

No caller needs to calculate the title's x coordinate.

### 7.3 Button anchored to the top-left with an offset

This places a fixed-size button 20 units from the left and 140 units from the top of the supplied screen box.

```stasis
function menu_place_play_button(
    out_button: f32[],
    screen: f32[]
): void {
    ui_place_in_rect(
        out_button,
        screen,
        180.0,
        56.0,
        UiHorizontal.Left,
        UiVertical.Top,
        20.0,
        140.0
    );
}
```

Because offsets always use screen-coordinate signs, positive x moves the button right and positive y moves it down. The same function can anchor a button to the top-right:

```stasis
ui_place_in_rect(
    button,
    screen,
    180.0,
    56.0,
    UiHorizontal.Right,
    UiVertical.Top,
    -20.0,
    140.0
);
```

Here `offset_x = -20.0` moves the right-aligned button 20 units left from the screen's right edge.

### 7.4 Text centered inside a button

The button's allocated rectangle is authoritative. Its background consumes the button rectangle, while its label is centered in a derived content rectangle.

```stasis
function menu_draw_play_button(button: f32[]): void {
    let content: f32[4];
    let label: f32[4];

    gfx_draw_sprite(
        button_sprite,
        f32_to_i32(button[UI_RECT_X]),
        f32_to_i32(button[UI_RECT_Y]),
        f32_to_i32(button[UI_RECT_W]),
        f32_to_i32(button[UI_RECT_H]),
        0,
        255
    );

    ui_rect_inset_edges(
        content,
        button,
        16.0,
        8.0,
        16.0,
        8.0
    );

    let fits: bool = ui_place_text_run(
        label,
        content,
        play_label_run,
        28.0,
        UiHorizontal.Center,
        UiVertical.Center,
        0.0,
        1.0
    );

    draw_text_cached(
        ui_font,
        play_label_run,
        label[UI_RECT_X],
        label[UI_RECT_Y],
        1.0,
        1.0,
        1.0,
        1.0
    );

    if (!fits) {
        ui_debug_draw_overflow(label);
    }
}
```

The one-unit y offset is an explicit optical adjustment. The actual line box remains centered before that adjustment; no button-specific `0.55 * height` heuristic is hidden in the painter.

### 7.5 Drawing and hit testing the same button

This complete menu fragment places the button once, draws from that rectangle, derives a larger touch target, and tests the current pointer snapshot against it.

```stasis
function menu_draw_and_hit_test(screen: f32[]): bool {
    let button: f32[4];
    let hit: f32[4];

    menu_place_play_button(button, screen);
    menu_draw_play_button(button);

    // Increase the touch target without changing the painted button.
    ui_rect_expand_edges(
        hit,
        button,
        6.0,
        6.0,
        6.0,
        6.0
    );

    if (input_pointer_count() > 0 && input_pointer_went_up(0)) {
        return ui_rect_contains(
            hit,
            input_pointer_x_px(0),
            input_pointer_y_px(0)
        );
    }

    return false;
}
```

This example demonstrates geometric reuse, not the final click lifecycle. A later interaction layer must capture a stable button ID on pointer-down and define whether pointer-up outside the captured button cancels activation.

### 7.6 Full menu rendering from the safe viewport

An adaptive menu begins in safe screen-space coordinates. It does not automatically fit a fixed design canvas.

```stasis
function menu_render(): void {
    let safe: f32[4];

    ui_rect_set(
        safe,
        gfx_safe_viewport_x().to_f32(),
        gfx_safe_viewport_y().to_f32(),
        gfx_safe_viewport_width().to_f32(),
        gfx_safe_viewport_height().to_f32()
    );

    menu_draw_title(safe);

    if (menu_draw_and_hit_test(safe)) {
        game_start();
    }
}
```

If the implementation provides `ui_safe_viewport_rect(out_rect)`, the four-value setup above becomes:

```stasis
let safe: f32[4];
ui_safe_viewport_rect(safe);
```

The explicit version remains useful documentation of which coordinate space the menu uses.

### 7.7 Centered labels in a flex button row

Flex distributes sibling boxes; anchored placement positions each label within its assigned box.

```stasis
function menu_draw_footer(footer: f32[]): void {
    let buttons: f32[8];
    let widths: f32[2];
    let heights: f32[2];

    widths[0] = 140.0;
    widths[1] = 140.0;
    heights[0] = 52.0;
    heights[1] = 52.0;

    flex_row(
        buttons,
        footer,
        widths,
        heights,
        2,
        16.0,
        FLEX_JUSTIFY_CENTER,
        FLEX_ALIGN_CENTER
    );

    menu_draw_button_at(buttons, 0, back_label_run);
    menu_draw_button_at(buttons, 1, play_label_run);
}
```

`menu_draw_button_at` reads the rectangle at `index * UI_RECT_STRIDE`, derives its content box, and calls `ui_place_text_run` with center/center placement. Row distribution and content alignment therefore remain separate, composable operations.

## 8. Required API Behavior

Exact naming may change during implementation, but v1 must provide the following capabilities.

### 8.1 Basic rectangle construction

```stasis
function ui_rect_set(
    out_rect: f32[],
    x: f32,
    y: f32,
    width: f32,
    height: f32
): void;
```

### 8.2 Edge inset

```stasis
function ui_rect_inset_edges(
    out_rect: f32[],
    rect: f32[],
    left: f32,
    top: f32,
    right: f32,
    bottom: f32
): void;
```

Result:

```text
x = rect.x + left
y = rect.y + top
w = max(0, rect.w - left - right)
h = max(0, rect.h - top - bottom)
```

In-place operation, where `out_rect` and `rect` refer to the same array, must either be supported and tested or rejected explicitly in documentation. Supporting it is preferred.

### 8.3 Edge expansion

```stasis
function ui_rect_expand_edges(
    out_rect: f32[],
    rect: f32[],
    left: f32,
    top: f32,
    right: f32,
    bottom: f32
): void;
```

This supports touch bounds larger than paint bounds without changing the visual layout.

### 8.4 Point containment

```stasis
function ui_rect_contains(
    rect: f32[],
    x: f32,
    y: f32
): bool;
```

Containment follows the half-open edge policy in section 6.2.

### 8.5 Child placement

```stasis
function ui_place_in_rect(
    out_rect: f32[],
    parent: f32[],
    width: f32,
    height: f32,
    horizontal: UiHorizontal,
    vertical: UiVertical,
    offset_x: f32,
    offset_y: f32
): void;
```

Placement formula per axis:

```text
Left/Top:     parent_start
Center:       parent_start + (parent_size - child_size) * 0.5
Right/Bottom: parent_start + parent_size - child_size

result = selected_position + offset
```

The function does not automatically clamp the child to the parent. A child larger than its parent remains aligned according to the selected rule and can be detected as overflow.

### 8.6 Cached text placement

```stasis
function ui_place_text_run(
    out_rect: f32[],
    parent: f32[],
    run_handle: i32,
    line_height: f32,
    horizontal: UiHorizontal,
    vertical: UiVertical,
    offset_x: f32,
    offset_y: f32
): bool;
```

Behavior:

1. Read width with `gfx_measure_text_cached(run_handle)`.
2. Use the explicit `line_height` as the text line-box height.
3. Place that measured line box using `ui_place_in_rect`.
4. Return whether the entire line box fits within the parent rectangle before offsets move it outside.

The result rectangle's x and y are suitable for `draw_text_cached` because Stasis text drawing uses a top-left origin.

The function does not draw, wrap, truncate, shrink, cache, or allocate text.

### 8.7 Rectangle fit check

The implementation must provide or internally use a deterministic whole-box containment check so non-text content can report overflow using the same policy as text.

## 9. Composition with Existing Flex Layout

The existing `flex_row`, `flex_column`, and `flex_grid` remain responsible for distributing sibling boxes.

The placement primitive is responsible for positioning content inside each distributed box.

```text
parent box
 -> flex row/column/grid
 -> allocated child boxes
 -> inset each child box
 -> place measured content in each content box
 -> draw and hit-test from the results
```

V1 must not add a competing row, column, or grid algorithm solely to obtain anchored content placement.

As part of implementation, `flex_layout.stasis` should consume the canonical rectangle module or otherwise migrate toward its constants and helpers without breaking supported samples unnecessarily.

## 10. Coordinate Spaces and Viewports

### 10.1 Default screen-space layout

Adaptive UI should normally begin with a rectangle derived from the host safe viewport:

```stasis
gfx_safe_viewport_x()
gfx_safe_viewport_y()
gfx_safe_viewport_width()
gfx_safe_viewport_height()
```

The safe-viewport helper converts these snapshot values into an `f32[4]` rectangle. It performs no additional scale or letterbox policy.

### 10.2 Fixed-design canvas

Games may separately fit a fixed logical canvas into a safe or full viewport while preserving aspect ratio. This policy may expose scale and offset data similar to Brickout's existing play layout.

Fixed-canvas fitting is not embedded into `ui_place_in_rect`. The caller must deliberately choose and consistently use a coordinate space for layout, drawing, and input conversion.

### 10.3 Pixel snapping

Core layout remains in floating-point coordinates. Pixel snapping belongs at the final coordinate transform or paint boundary so repeated composition does not accumulate rounding error.

## 11. Text Model

### 11.1 Measurement

V1 uses cached text runs for per-frame UI placement. Text runs are created during initialization or resource updates, not during the rendering hot path.

### 11.2 Height

Until the runtime exposes font ascent, descent, baseline, and measured height, callers supply an explicit line-box height.

This height represents layout space, not glyph ink bounds and not a baseline coordinate.

### 11.3 Optical adjustment

Art-directed vertical adjustment is represented by ordinary `offset_y`. The core layout layer does not contain font-specific optical correction tables.

Reusable game-level text styles may hold line height, color, font handle, and optical offsets, but style storage is outside the v1 geometry contract.

### 11.4 Overflow

V1 detects fit and returns the result to the caller. It does not decide whether the caller should:

- draw overflowing content
- substitute shorter copy
- choose a separately cached smaller-font run
- skip drawing
- render debug diagnostics

## 12. Rendering and Interaction Boundaries

### 12.1 Rendering

Painting functions receive resolved rectangles. They must not independently recalculate layout from unrelated screen constants.

A button painter may:

1. draw a sprite or corrected nine-slice into the button box
2. derive a padded content box
3. place a cached text run within that box
4. draw at the returned x and y

### 12.2 Hit testing

Hit testing receives the authoritative allocated or explicitly expanded hit rectangle.

V1 geometry does not define click timing, pointer capture, multi-pointer arbitration, focus, or disabled-state transitions. Those behaviors require persistent interaction identity and belong in a later interaction slice.

### 12.3 Stable identity

Any later immediate-mode widget behavior must use explicit stable integer IDs. Rectangle equality or call order must not be the sole persistent identity of a control.

## 13. Debuggability

The first implementation must make pure layout results testable without a renderer.

A debug paint helper should also be provided or demonstrated that can distinguish at least:

- allocated bounds
- inset content bounds
- expanded hit bounds
- overflowing content
- safe viewport bounds

Debug drawing must consume computed boxes and must not change layout results.

## 14. Error and Edge Policies

V1 must define and test:

- zero-size parent rectangles
- zero-size children
- children larger than parents
- excessive inset values
- negative offsets
- fractional coordinates and odd sizes
- compile-time rejection when a caller supplies a non-placement value
- compile-time rejection when a caller swaps `UiHorizontal` and `UiVertical` arguments
- points exactly on every edge
- invalid or zero cached-run handles according to existing graphics runtime behavior

The core API does not silently reposition overflowing children to make them fit. Overflow is data that the caller may detect and handle.

## 15. Performance Requirements

- Core geometry and placement perform no heap allocation.
- Placement is constant time.
- No host call is introduced on the per-frame layout path beyond the existing cached text width behavior. If cached width currently crosses the host boundary, a follow-up should move the width into Stasis-owned cached metadata rather than growing additional hot-path calls.
- Row, column, and grid behavior remains bounded by explicit item counts and fixed arrays.
- The same functions and semantics are used in diagnostic and normal builds.
- JIT and AOT produce equivalent placement results within documented floating-point tolerance.

## 16. Testing Requirements

### 16.1 Pure geometry tests

Tests must cover all nine combinations of horizontal and vertical placement.

For each axis, verify:

- start placement
- center placement with even and odd dimensions
- end placement
- positive and negative offsets
- a child larger than its parent

### 16.2 Rectangle operation tests

Tests must cover:

- symmetric and asymmetric inset
- inset larger than available size
- expansion
- in-place inset if supported
- half-open containment on all four edges
- adjacent boxes sharing an edge

### 16.3 Text tests

Tests must cover:

- short cached run that fits
- exact-width fit
- width overflow
- line-height overflow
- horizontal centering invariant
- vertical line-box centering invariant
- optical adjustment through offsets

Representative invariants include:

```text
child_center_x == parent_center_x + offset_x
child_center_y == parent_center_y + offset_y
end_child_right == parent_right + offset_x
```

### 16.4 Composition tests

At least one test must:

1. allocate multiple boxes using the existing flex row or column path
2. place content within each allocated box
3. verify the computed content and hit bounds

### 16.5 End-to-end compiler gate

The slice must include at least one representative `.stasis` sample or test program that:

1. imports the shared layout modules
2. computes a safe or fixed parent box
3. uses flex and anchored placement
4. reaches Cranelift IR
5. is built into an executable
6. runs with asserted placement behavior

All test commands must remain bounded to 300 seconds, with lingering test/compiler processes checked after each run.

## 17. Recommended Delivery Slices

### Slice 1: Canonical geometry

- introduce the shared rectangle constants and helpers
- add inset, expansion, containment, and fit behavior
- add deterministic Stasis tests
- migrate only the minimum existing code needed to validate reuse

### Slice 2: Anchored placement

- add independent-axis placement
- cover all nine placement combinations
- verify overflow and offset behavior
- compose placement with an existing flex row or column

### Slice 3: Cached text line boxes

- add cached-run placement using explicit line height
- return fit status
- replace one real screen's hardcoded text-start calculation
- visually verify debug bounds and text positioning

### Slice 4: Paint integration

- update one sprite-backed or nine-slice button path to consume resolved boxes
- correct undersized nine-slice behavior before treating it as a shared primitive
- derive paint, content, and hit bounds from one allocated button box

Interaction identity and pointer capture should be proposed as a separate PRD or follow-up slice after the geometry proves useful on real screens.

## 18. Acceptance Criteria

V1 is complete when:

1. One canonical rectangle representation is used by the new shared APIs.
2. A caller can place a known-size child in any of nine parent positions with explicit offsets.
3. A caller can obtain correct top-left coordinates for a cached text run using measured width and explicit line height.
4. Layout functions emit no drawing commands and retain no component state.
5. Existing flex output can be used directly as placement input.
6. Insets and expansions allow content and hit bounds to derive from one allocated box.
7. Overflow is detectable and does not silently change placement.
8. Safe screen-space layout does not implicitly force fixed-design scaling.
9. Tests establish half-open edge behavior and all placement invariants.
10. One representative UI path compiles and runs end to end through Cranelift with asserted results.
11. Debug output can display the authoritative allocated and derived boxes.
12. Touched code passes a simplicity review and leaves no duplicate or obsolete rectangle helpers without an explicit migration reason.
13. Placement parameters use distinct `UiHorizontal` and `UiVertical` enums end to end, with explicit conversion only at a demonstrated integer ABI boundary.

## 19. Future Extensions

Potential follow-up work includes:

- parent-anchor to child-anchor attachment for tooltips, badges, and world labels
- fixed/fill/weight size resolution before flex placement
- multiline text metrics and wrapping
- clipping and scroll containers
- stable-ID interaction state with pointer capture
- focus navigation and accessibility metadata
- reusable style tables after multiple screens establish stable needs
- richer overflow diagnostics
- baseline-aware text placement when runtime font metrics are available

Each extension should remain a deterministic transformation or explicit persistent behavior layer. It must not require a parallel retained rendering pipeline.

## 20. Theory of Operation

### Mapping

Real UI composition is represented as nested available rectangles. Layout transforms a parent rectangle and explicit placement inputs into child rectangles. Rendering projects those rectangles into graphics commands, while input compares pointer snapshots against the same geometry.

The model deliberately excludes semantic component trees, event dispatch ownership, and automatic reflow policy.

### Rationale

Stasis already renders immediately from current state into command buffers and uses fixed arrays across compiler backends. Plain rectangle transforms match that execution model, remain inspectable, and compose with the existing flex helpers.

The nearest tempting alternative is a retained component hierarchy with automatic measurement and callbacks. That would introduce hidden lifecycle, identity, allocation, and compiler-surface requirements before the underlying placement needs are proven.

### Extension point

A future layout behavior should fit as either:

- a pure transformation that produces rectangles from rectangles and explicit size rules, or
- an explicit persistent interaction structure keyed by stable IDs

For example, a tooltip can add parent-anchor to child-anchor attachment without changing rendering ownership or creating a UI tree.

### Prediction

If the model is sound, adding a new container such as an equal-cell toolbar will require only a deterministic box-allocation function. Existing text placement, painting, hit testing, and debug visualization will consume its output unchanged.
