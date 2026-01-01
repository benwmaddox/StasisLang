# Button Layout With Sprite Caps + Text (Stasis)

This note documents a Stasis-friendly pattern for variable-width buttons that
fill available horizontal space while preserving icon/text aspect ratios. The
button background is built from three sprites (left cap, center, right cap) so
only the center stretches in X while corners stay crisp. Text is drawn with
fonts, not baked into the SVG.

## Pattern summary

- Background: three sprites (left cap, center tile, right cap).
- Size: fixed height; width calculated from available row width and count.
- Icon: scaled by height to preserve aspect ratio.
- Text: rendered via `draw_text` and centered with `measure_text`.

## Layout math

- `button_w = (available_w - gap * (count - 1)) / count`
- `button_h = fixed_h`
- `cap_w = button_h * cap_aspect`
- `center_w = max(0, button_w - 2 * cap_w)`
- `icon_h = button_h * 0.6`
- `icon_w = icon_h * icon_aspect`

## Stasis shared module

Shared helpers live in `src/stdlib/ui_button_9slice.stasis`. Import and use these from apps:

```stasis
import "../src/stdlib/stdlib.stasis";
import "../src/stdlib/ui_button_9slice.stasis";
```

## 9-slice PNG assets

Sample 9-slice assets (dark blue fill with off-white rounded border) are in:

```
docs/assets/ui/button_9slice_dark/
```

Files include:
- `btn_9slice_dark_full.png` (full source)
- `btn_9slice_dark_00.png` .. `btn_9slice_dark_22.png` (3x3 slices, each 32x32)
- `icon_play.png`, `icon_star.png` (small icons)

Slice sizes for these assets:
- `slice_w = 32.0`
- `slice_h = 32.0`

## Stasis sketch

```stasis
// Load slices and icons once (paths below reference docs/assets).
let button_top_left: i32 = gfx_load_sprite("docs/assets/ui/button_9slice_dark/btn_9slice_dark_00.png", 32, 32);
let button_top: i32 = gfx_load_sprite("docs/assets/ui/button_9slice_dark/btn_9slice_dark_01.png", 32, 32);
let button_top_right: i32 = gfx_load_sprite("docs/assets/ui/button_9slice_dark/btn_9slice_dark_02.png", 32, 32);
let button_left: i32 = gfx_load_sprite("docs/assets/ui/button_9slice_dark/btn_9slice_dark_10.png", 32, 32);
let button_center: i32 = gfx_load_sprite("docs/assets/ui/button_9slice_dark/btn_9slice_dark_11.png", 32, 32);
let button_right: i32 = gfx_load_sprite("docs/assets/ui/button_9slice_dark/btn_9slice_dark_12.png", 32, 32);
let button_bottom_left: i32 = gfx_load_sprite("docs/assets/ui/button_9slice_dark/btn_9slice_dark_20.png", 32, 32);
let button_bottom: i32 = gfx_load_sprite("docs/assets/ui/button_9slice_dark/btn_9slice_dark_21.png", 32, 32);
let button_bottom_right: i32 = gfx_load_sprite("docs/assets/ui/button_9slice_dark/btn_9slice_dark_22.png", 32, 32);
let button_slice_w: f32 = 32.0;
let button_slice_h: f32 = 32.0;

let icon_play: i32 = gfx_load_sprite("docs/assets/ui/button_9slice_dark/icon_play.png", 64, 64);
let icon_star: i32 = gfx_load_sprite("docs/assets/ui/button_9slice_dark/icon_star.png", 64, 64);

// Row layout helper: compute button width and x offsets for a row.
let w: f32 = ui_row_button_width(720.0, 3, 16.0);
let h: f32 = 56.0;
let y0: f32 = 60.0;
let x0: f32 = ui_row_button_x(40.0, 0, w, 16.0);
let x1: f32 = ui_row_button_x(40.0, 1, w, 16.0);
let x2: f32 = ui_row_button_x(40.0, 2, w, 16.0);

// Example usage with icons + text.
ui_draw_button_9slice(
    button_top_left, button_top, button_top_right,
    button_left, button_center, button_right,
    button_bottom_left, button_bottom, button_bottom_right,
    button_slice_w, button_slice_h,
    x0, y0, w, h, "Play", font_handle,
    0.96, 0.96, 0.96, 1.0, icon_play, 1.0, 16.0, 16.0);
ui_draw_button_9slice(
    button_top_left, button_top, button_top_right,
    button_left, button_center, button_right,
    button_bottom_left, button_bottom, button_bottom_right,
    button_slice_w, button_slice_h,
    x1, y0, w, h, "Favorites", font_handle,
    0.96, 0.96, 0.96, 1.0, icon_star, 1.0, 16.0, 16.0);
ui_draw_button_9slice(
    button_top_left, button_top, button_top_right,
    button_left, button_center, button_right,
    button_bottom_left, button_bottom, button_bottom_right,
    button_slice_w, button_slice_h,
    x2, y0, w, h, "No Icon", font_handle,
    0.96, 0.96, 0.96, 1.0, 0, 1.0, 16.0, 16.0);
```

Example app:

```
examples/ui_button_9slice_example.stasis
```

## Notes

- Keep the SVG art sized to a known pixel size when loading sprites so scale
  calculations are stable.
- Use a small `gap` between buttons; the math above already accounts for it.
- If you need hit regions, store the computed `x/y/w/h` per button for input.
