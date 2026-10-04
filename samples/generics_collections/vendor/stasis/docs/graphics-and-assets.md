# Graphics and asset ownership

<!-- tags: graphics, assets, fonts, audio, render, ownership -->

Import `/vendor/stasis/stdlib/graphics.stasis` for drawing and
`/vendor/stasis/stdlib/audio.stasis` for audio. See the compiled
[display](examples/src/display_spaces.stasis) and
[loading](examples/src/loading_screen.stasis) examples.

## Drawing

The host resets, validates, and publishes each `render()` construction. Draw
in painter order; authored code must not call `begin_frame` or `end_frame`.
Use `clear` when replacing the background. Coordinates and hit tests use the
same logical units; see [display spaces](display-and-coordinate-spaces.md).

Use `PresentationList` for reusable sprite/rectangle input, `LineBatch` for
lines, or `SpriteRunWriter` for streaming sprites. Finalize or cancel a writer
in the same frame. Its source rectangle uses logical image pixels, while its
explicit pivot is in destination-local units; see [Rig2D attachments](../rig2d.md#solve-then-render-attachments)
for a complete joint-to-pivot example. Keep simulation rules in `tick()`.

In the display example, `button_rect` is resolved during `tick()` and
`button_presentation` is a persistent `PresentationList`. Rebuild its draw
input each frame, then replay it into the host's construction:

```stasis
function render(): i32 {
    button_presentation.reset_presentation();
    button_presentation.append_solid_rect(button_rect.x, button_rect.y, button_rect.w, button_rect.h, 0.2, 0.5, 0.9, 1.0);
    button_presentation.replay();
    return 0;
}
```

## Resources

- Request assets after a loading frame, poll ready/failed states, and release
  owned resources on replacement or failure. Follow [loading screens](loading-screens.md).
- Sprites use typed `SpriteRef` values. Use `Sprite.valid()` and `poll_reload()`.
  `AudioVoice` playback state is separate from its `AudioAsset`.
- Each successful `load_font` needs one `release_font`, even when repeated loads
  return the same handle. Reuse owned fonts; acquire a replacement before
  releasing the old one. Final release invalidates cached text runs.
- Check `font_status` before publishing text layout. Web font loading and
  metric calibration can be asynchronous.
