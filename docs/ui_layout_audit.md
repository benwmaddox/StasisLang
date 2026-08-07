# Machine-readable UI layout audits

`src/stdlib/ui_layout_audit.stasis` complements framebuffer review with exact
logical geometry. It reports the final rectangles that drawing and hit testing
consume, including runtime `TextRun.width` and `TextRun.height`, then evaluates
declared relationships such as centering, containment, padding, baselines, and
overlap.

The audit is allocation-free. Numeric output uses signed thousandths of one
logical layout unit (`*_milli`) so tools receive deterministic integers without
losing subpixel placement. Audit calls belong in development or QA paths; they
do not alter placement or rendering.

## Example

The complete executable example is
`samples/immediate_axis_layout/`. After the normal layout function has produced
the rectangles used for drawing and hit testing, it emits the same geometry:

```stasis
import "../../src/stdlib/ui_layout_audit.stasis";

ui_audit_rect("play_button", "button", button_x, button_y, button_w, button_h);
ui_audit_text("play_label", label_x, label_y, play.width, play.height, 24);

ui_audit_center_check(
    "x",
    "play_label",
    "play_button",
    ui_audit_center_delta_x(button_x, button_w, label_x, play.width),
    0.5
);
ui_audit_center_check(
    "y",
    "play_label",
    "play_button",
    ui_audit_center_delta_y(button_y, button_h, label_y, play.height),
    1.0
);
ui_audit_padding_axis("x", "play_label", "play_button", button_x, button_w, label_x, play.width, 0.5);
ui_audit_padding_axis("y", "play_label", "play_button", button_y, button_h, label_y, play.height, 0.5);
```

Representative output is one record per line:

```text
UI_LAYOUT|rect|id=play_button|role=button|x_milli=90000|y_milli=504000|w_milli=180000|h_milli=56000
UI_LAYOUT|text|id=play_label|x_milli=151000|y_milli=520000|w_milli=58000|h_milli=24000|font_size=24
UI_LAYOUT|check|kind=center_x|child=play_label|parent=play_button|delta_milli=0|tolerance_milli=500|pass=1
UI_LAYOUT|check|kind=center_y|child=play_label|parent=play_button|delta_milli=0|tolerance_milli=1000|pass=1
UI_LAYOUT|padding|axis=x|id=play_label|parent=play_button|start_milli=61000|end_milli=61000|contained=1
```

Values in documentation illustrate the schema; a real run reports the loaded
font's measured bounds and the current safe viewport.

## AI review workflow

1. Emit every visible panel, button, icon, and text box once after layout.
2. Give stable IDs to corresponding draw and hit-test elements.
3. Add checks for the design intent: shared centers, repeated baselines, equal
   gaps, parent containment, safe-edge clearance, and expected/non-expected
   overlap.
4. Capture stdout to a text artifact and provide it to the reviewer with the
   intended logical resolution and tolerances.
5. Ask the reviewer to report every `pass=0`, compare repeated rows/columns,
   identify asymmetric padding, and flag rectangles that are present without a
   declared relationship.
6. Keep framebuffer review. Geometry cannot judge contrast, rasterization,
   optical weight, imagery, or whether a mathematically valid composition feels
   crowded.

Suggested review prompt:

```text
Review this UI_LAYOUT trace at 360x720. Report failed checks first. Then compare
repeated controls for equal dimensions, centers, baselines, gaps, padding, edge
clearance, containment, and unexpected overlap. Distinguish exact geometric
defects from visual questions that still require the framebuffer.
```

For intentional optical nudges, audit both the mathematical placement and the
final draw rectangle. Give the nudge a named ID or role so a reviewer does not
mistake it for unexplained drift.
