# Graphics and asset ownership

<!-- tags: graphics, assets, fonts, audio, render, ownership -->

Import `/vendor/stasis/stdlib/graphics.stasis` for drawing and
`/vendor/stasis/stdlib/audio.stasis` for audio. The
[loading example](examples/src/loading_screen.stasis) and
[display example](examples/src/display_spaces.stasis) compile with this bundle.

## One render construction

The host resets graphics construction on entry to `render()` and validates and
publishes it after the function returns. Do not call the removed public
`begin_frame` or `end_frame` wrappers. Draw in painter order with `clear`,
`draw_line`, `fill_rect`, `draw_text`, typed `draw_sprite`, or a bounded
`PresentationList`. A frame without `clear` does not promise that prior
framebuffer pixels persist. Keep game rules in `tick()` and derive drawing from
the resulting state.

The display example uses the same resolved logical rectangle for drawing and
pointer hit testing:

```stasis
function render(): i32 {
    button_presentation.reset_presentation();
    button_presentation.append_solid_rect(button_rect.x, button_rect.y, button_rect.w, button_rect.h, 0.2, 0.5, 0.9, 1.0);
    button_presentation.replay();
    return 0;
}
```

Draw and hit-test positions use logical canvas units. The host owns density and
window-to-canvas conversion; see [display spaces](display-and-coordinate-spaces.md).

## Acquire, wait, use, release

The loading example requests an `ImageAsset` and `AudioAsset` after a loading
frame, polls their ready/failed states, and releases them on its bounded
failure path. `ImageAsset`/`Sprite` references are typed `SpriteRef` values;
integers are not sprite references. Use `Sprite.valid()` and `poll_reload()`
instead of reading a raw handle. An `AudioVoice` owns playback state separately
from its `AudioAsset`.

`load_font(path, size)` acquires one font ownership reference. Balance each
successful acquisition with `release_font`, even if repeated loads returned
the same handle. Reuse an already owned font when its size is unchanged.
Native loads can be ready immediately; Web fonts may report `Pending` or
`Loading` until metrics are calibrated. Check `font_status` before publishing
dependent text layout. Obtain a replacement before releasing an old font;
the final release invalidates its cached text runs.

Use a bounded `PresentationList` for reusable painter-ordered sprite/rectangle
input, `LineBatch` for bounded line input, and `SpriteRunWriter` for streaming
sprites. Finalize or cancel a writer within the same frame. Do not sort
transparent commands to imitate host batching.

For slow content work and first-frame ordering, follow
[loading screens](loading-screens.md). For a visual regression, use a fresh
`stasis record` PNG or MP4 capture and inspect the result.
