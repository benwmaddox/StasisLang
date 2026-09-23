# Physical text raster audit (#639)

Audit base: `9e0d06b3`, 2026-09-23. This PR repairs confirmed Web gaps and is
partial progress on #639. It does not establish every-platform acceptance.
Parent #547 retains its Rootbeer consumer migration and desktop lifecycle work.

## Backend inventory

| Production path | Implementation and transform | Current evidence / remaining work |
| --- | --- | --- |
| Desktop JIT and AOT, SDL render drivers | `runtime/CMakeLists.txt` includes `stasis_graphics.c`. `stasis_sync_display_metrics` reads SDL render-output dimensions and uses logical letterboxing. `stasis_build_font_atlas` bakes glyphs with the backing helper in `stasis_display_scale.h`. Direct text and immutable/replaceable runs consume this atlas. | Source audit only in this PR. Fonts have minimum 2x sampling, an 8x preparation cap, and a 4096 atlas limit. Audit >8x handling, rounded viewport coverage, and division of rounded raster metrics by the unrounded `pixel_scale`; fresh native PNGs and resize/restore tests remain required. |
| Packaged Android AOT SDL shell | `mobile/shells/android/app/src/main/cpp/CMakeLists.txt` adds the shared runtime. | Same native implementation; actual device output metrics, renderer restoration, limits and font behavior remain unqualified. No independent per-game repair. |
| Apple packaged AOT | `mobile/shells/ios/StasisMobile.xcodeproj/project.pbxproj` includes the shared `stasis_graphics.c`. Desktop Apple uses the native build. | Shared-source inventory, not an Apple build or device pass. Xcode/Apple device qualification is unavailable on this Windows host. |
| Android Workshop GLES preview | `StasisPreviewRenderer.fitViewport` fits the surface and publishes `rasterScale`; `WorkshopTextureProvider` uses it in both cached text and `rasterText`, including fallback typefaces. Surface/renderer generations gate texture reuse. | Source audit only. The fitted scalar uses the smaller rounded viewport axis and clamps at 8x. Bitmap-to-logical rounding and scale changes within its 0.001 tolerance need qualification. No emulator or physical-phone acceptance was run for this Web-only repair. |
| Web WebGL2 | `runtime/web/game.js` uses Canvas2D for text preparation and WebGL2 atlas quads. Direct commands, cached static/dynamic runs and fallback handle zero converge on `preparedTextResource`. | Repaired and tested here. Real Chrome host-renderer PNGs inspected; packaged-game, other browser/GPU and phone qualification remain open. |

The older task comment describing an uncapped native glyph helper is stale:
current `stasis_display_font_scaled_extent_for_backing` calls the capped
`stasis_display_preparation_scale`. The cap must not be mistaken for completed
physical-output acceptance.

## Web repair and boundaries

The September 17 change already rasterized text at a density tier. However,
that tier came from the smaller backing axis and was capped at 8x. Text could
still be magnified by a larger axis or a fitted scale above 8x.

Text now selects the larger actual backing ratio, rounds up through the existing
tiers where possible, and uses the actual ratio above the highest tier. Integer
Canvas extents round upward. The preparation transform uses the rounded Canvas
extent divided by the logical extent on each axis; this exactly matches the
inverse texture-to-quad mapping, including fractional baseline placement.
Logical measurement remains independent of sampling resolution.

A changed text sampling scale releases prepared entries and their atlas resources.
An unchanged scale reuses them, independently of sprite tier changes. Text keys
include the actual text scale. Physical-byte cache accounting, transient oversized
entries, WebGL maximum-texture checks, font readiness/release, stale completion
handling and context restoration retain their existing paths. Exceeding device
texture limits fails visibly rather than silently reducing text resolution.

## Validation and visual evidence

- `node --test runtime/web/tests/*.test.mjs`: 210 passed.
- Three new host-renderer regression tests cover nonuniform axes, fractional
  extents, >8x, unchanged-scale reuse, fallback direct text, dynamic replacement,
  failure-safe replacement, and context restoration. Existing tests cover 1x/2x,
  fractional tiers, font lifecycle, physical cache bytes and texture-limit errors.
- `node tools/run_text_raster_browser_acceptance.mjs target/text-raster-browser 9e0d06b3`:
  passed with Chrome 153 and real Canvas2D/WebGL2 via SwiftShader. The fixture
  supplies the guest ABI in JavaScript, so this is a host integration test, not
  a compiler/package or hardware-GPU test. Set `STASIS_BROWSER_EXECUTABLE` to
  select a locally installed Chromium executable. Node must provide `WebSocket`.
- Before/after capture uses a 640x360 logical canvas in a 1280x360 backing, with
  12px and 30px Basic Regular text. The two logical quads and baseline inputs are
  identical; physical rasters grow from 128x12 / 147x31 to 256x24 / 294x62.
- Visual evidence: inspected [before.png](../evidence/task639-web-text/before.png)
  and [after.png](../evidence/task639-web-text/after.png). Text edges are sharper,
  with matching placement and no visible descender clipping in these strings.
  This deliberately nonuniform fixture stretches letters horizontally in both
  images; it demonstrates the host's sampling coverage, not a preferred layout.
  [receipt.json](../evidence/task639-web-text/receipt.json) records actual Canvas
  dimensions, submitted quads, browser version and runtime source hashes.
- `git diff --check` and Node syntax checks passed.
- Required baseline `tools/validate_repo.sh` stopped before implementation on an
  existing unsafe-boundary violation in
  `crates/stasis_compiler/tests/sprite_run_writer_public_seam.rs`. Earlier ABI and
  host-contract gates passed. This unrelated file is not changed here.

Remaining acceptance: repair/verify native and Workshop limits and rounding,
then use fresh native/mobile artifacts for 1x/2x/fractional portrait/landscape,
font fallback, resize/display changes and restoration captures. Apple and
physical-phone qualification remain blocked on those platform lanes. #639 stays
Active; this PR must not be treated as completion of its cross-platform scope.

Theory gained: a host's scalar asset-density tier need not bound both actual text
output axes. The failing renderer tests and browser captures demonstrate that
text needs its own sampling identity; a change confined to the smaller axis can
then reuse the text raster while rebuilding sprite resources independently.
