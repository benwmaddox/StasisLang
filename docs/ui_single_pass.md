# Single-pass UI

Stasis UI screens can be written as a small immediate recipe. The recipe is
replayed from the current safe rectangle each frame; resolved positions are
scratch values and are never game state.

## The small vocabulary

- `ui_vstack_begin` and `ui_hstack_begin` carve fixed main-axis extents with
  explicit spacing. `UiStackDirection.Reverse` starts at the far edge.
- `ui_*stack_next_fixed` consumes one known extent. At most one
  `ui_*stack_next_rest` child is allowed, and it should be the final child.
- `ui_anchor` places a known-size child with the existing `UiHorizontal` and
  `UiVertical` enums. Use `ui_inset` first for safe-area padding.
- `ui_scroll_begin` stores only a keyed offset and drag pointer. Rows can use
  `ui_scroll_item_y` and `ui_scroll_item_visible` without retaining rectangles.
  In the render pass, pair `ui_scroll_clip_begin` with `ui_scroll_end` so the
  same viewport becomes an ordered renderer clip.
- `ui_viewport_fit` derives `Stretch`, `Contain`, `Cover`, or `IntegerScale`
  geometry. `ui_viewport_map_x/y` convert screen coordinates to content space.
- `ui_button` uses a stable integer ID. Active ID, pointer capture, hot ID, and
  modal layer are interaction state; button rectangles remain frame-local.

`ui_current_x`, `ui_current_y`, `ui_current_width`, and
`ui_current_height` expose the one current rectangle so drawing and hit testing
can use exactly the same geometry.

## The single-pass rule

Every child must provide a known main-axis extent when encountered. This keeps
layout a forward cursor operation with bounded work and no lookahead. A screen
may call the same recipe once while routing input and once while painting. Each
invocation is one forward layout pass: there is no measurement pass, no
multi-pass negotiation, and no geometry read from the previous frame. Recompute
the fitted viewport before mapping input, then recompute it again before drawing.

Supported patterns include fixed-row lists, nested stacks, corner HUDs,
split-screen viewports, and modal overlays. A `Scroll` state may persist an
offset, but it must not persist row rectangles or parent-relative positions.

## Deliberate limits

The module does not provide automatic wrapping, content-sized parent
negotiation, multiple weighted/rest children, masonry, or a retained widget
tree. Text metrics must be known or cached by the caller. If a stack overflows,
receives a negative extent, or repeats `rest`, it sets a deterministic status
bit via `ui_status` instead of allocating or silently changing the recipe.

See [`samples/ui_gallery`](../samples/ui_gallery/README.md) for a runnable
gallery and interaction examples.
