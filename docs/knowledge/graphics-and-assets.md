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
in the same frame. Keep simulation rules in `tick()`.

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
