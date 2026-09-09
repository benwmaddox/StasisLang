# Stasis UI Gallery

This sample is a small teaching game for the single-pass UI module. It draws
the layout recipe next to the result so the code and the interaction remain
easy to inspect.

The same `gallery_layout` recipe is replayed with `UiPass.Input` during the
tick and `UiPass.Paint` during rendering. Primary pointer data is snapshotted
once with `ui_input_begin_primary(ui_gallery_host_frame)`; widgets use the
current rectangle rather than repeating six input fields and four geometry
fields at every call site.

Run from the repository root:

```powershell
stasis --workspace samples/ui_gallery check
stasis --workspace samples/ui_gallery test
stasis --workspace samples/ui_gallery run --ticks 600
stasis --workspace samples/ui_gallery record main.stasis --width 1100 --height 720 --fps 60 --frames 240 --input-script record_input.json --output artifacts/ui_gallery
```

The tabs demonstrate:

- **Stacks** — horizontal and nested vertical stacks, spacing, reverse flow,
  and a final `rest` child.
- **Anchors** — all nine combinations of the typed horizontal and vertical
  anchor enums, with safe-area inset.
- **Scroll** — a fixed-row list drawn between `ui_scroll_clip_begin` and
  `ui_scroll_end`. `ui_scroll_drag_current` uses the screen layer, so a modal
  cancels its capture. Otherwise, release outside still completes the drag.
- **Viewport** — a live moving world, viewport-local HUD, and a button cycling
  Stretch, Contain, Cover, and IntegerScale. Press inside the fitted viewport
  to map the pointer to 320x180 world coordinates and move the marker.
- **Combined** — a playable viewport beside an overlay panel. Open the modal to
  see a higher input layer capture the pointer while the world remains visible.

The layout module is included in `vendor/stasis/stdlib` so the sample remains
self-contained and version-pinned like a generated Stasis project. Its source
is synchronized with the canonical
[`src/stdlib/ui_single_pass.stasis`](../../src/stdlib/ui_single_pass.stasis).

The bundled Press Start 2P font is distributed under the SIL Open Font License;
see [`assets/OFL.txt`](assets/OFL.txt).
