# Physical PNG raster audit (#640)

Audit base: `3bb32f5f`, 2026-09-23. This is a bounded Web repair, not completion
of every-platform task #640. Existing artwork is unchanged.

## Backend inventory and open acceptance

| Production path | Source preparation and draw path | Remaining acceptance |
| --- | --- | --- |
| Desktop JIT/AOT SDL and GL | `runtime/CMakeLists.txt` includes `stasis_graphics.c`; `bake_raster_to_rgba_sized` decodes with SDL_image, contains in the requested raster, nearest-samples even when that enlarges the original, then premultiplies alpha. Shared sprite preparation feeds native draw/atlas paths. | Original detail can be insufficient without an explicit underprovision outcome. Audit physical transform limits, crops, cache/atlas lifetime and capture fresh native 1x/2x/fractional/fitted output. |
| Android packaged AOT | The SDL shell builds the shared runtime, including the same PNG preparation. | Same source-detail gap; fresh package, actual output metrics and device restoration remain unqualified. |
| Apple shared SDL | iOS project sources include shared `stasis_graphics.c`; desktop uses native runtime. | Xcode, Apple builds and physical devices are unavailable on this Windows host. No Apple qualification is claimed. |
| Android Workshop GLES | `WorkshopTextureProvider.decode` validates decoded bounds against manifest dimensions, then calls `ImageDecoder.setTargetSize` or density-scaled BitmapFactory and `createScaledBitmap`. The provider uploads into `WorkshopSpriteAtlas`. | Undersized sources can be enlarged; source-family policy, fitted transform/rounding, region semantics and real emulator/device evidence remain open. No Workshop emulator or physical-phone acceptance was run for this Web-only slice. |
| Web WebGL2 | `spriteRasterRequest` plans physical pixels; `rasterSprite` decodes/prepares PNGs, stages atlas variants and publishes cache ownership. Full images and logical sheet regions share resource lifetime. | This repair covers framebuffer sampling and logical crop mapping. Per-instance enlargement beyond the loaded logical extent, packaging/source-family selection and other browser/device lanes remain open. |

No density source-family selection was found in these inspected preparation
paths. This PR uses real detail from the supplied original; it does not invent a
file naming convention or claim that interpolation restores missing information.
Web retains `source-underprovisioned` in `data-asset-fallback`, with original and
decoded dimensions/bytes. Its displayed fallback may be enlarged to preserve
logical geometry; that outcome is explicitly **not** physical-detail compliance.
Raster dimension/byte caps and GPU limits likewise remain detectable outcomes.

## Web repair

Sprite sampling now covers the larger actual framebuffer-to-logical axis.
Existing density tiers round upward, and scales above the highest tier use the
actual ratio. Integer raster extents round upward subject to existing limits.
Sprite cache identity and invalidation use that sampling scale, independently of
the smaller-axis display tier. Changes to the smaller axis alone reuse an
already sufficient sprite resource. Font/display ABI behavior is unchanged.

Sheet crop coordinates are normalized by the resource's logical source extent,
not its physical pixel extent. Visual inspection found this second defect: a
2x preparation previously changed which part of the PNG was displayed. The
existing source variant, alpha handling, atlas gutters, submission order,
physical-byte accounting, rollback and context restoration paths are retained.

## Validation

- `node --test runtime/web/tests/*.test.mjs`: 213 passed. Three new tests cover
  1x/2x/fractional/nonuniform/>8x sampling, physical bytes, atlas gutters, resize,
  reuse, restoration and logical sheet UV extents. Existing tests cover
  insufficient sources, cache release, stale preparation, limits and rollback.
- `node tools/run_png_raster_browser_acceptance.mjs target/task640-browser-final 3bb32f5f`:
  passed in Chrome 153 with real Canvas2D/WebGL2 via SwiftShader. This supplies
  the guest ABI in JavaScript; it is not compiler/package or hardware-GPU proof.
  It uses the unchanged `samples/asset_breakout/assets/arena_background.png`.
- The fitted/nonuniform before/after pair keeps a 640x360 logical canvas and
  1280x360 backing. Prepared source grows from 400x225 to 800x450 with identical
  full and sheet destination bounds. Separate 1x, 2x and 1.25x captures follow
  real resize events. `WEBGL_lose_context` restoration produces a PNG identical
  byte for byte to the pre-loss fractional capture. The fixture records both
  batched and split-page draw submissions.
- `node --check` for runtime and browser acceptance, plus `git diff --check`:
  passed. Source-format hooks installed. No lingering test executables remain.
- Required `tools/validate_repo.sh` passed its pre-Cargo gates and built fresh
  workspace artifacts, then failed `desktop_watch_frames_never_mix_tick_and_render_generations`
  in `desktop_hot_swap_generation_seam`: the CLI refuses installed runtime
  startup because it has no verified build fingerprint. This native tooling
  prerequisite is unrelated to the Web changes. Full repository validation is
  not claimed. Reproduced with `python tools/cargo_cache.py run -- cargo test -p stasis --test desktop_hot_swap_generation_seam -- --test-threads=1` (3 passed, 1 failed).
  The Workshop render-parity emulator gate remains unrun.

Visual evidence: inspected [before](../evidence/task640-web-png/before.png),
[after](../evidence/task640-web-png/after.png), [1x](../evidence/task640-web-png/one.png),
[2x](../evidence/task640-web-png/two.png), [fractional](../evidence/task640-web-png/fractional.png)
and [restored](../evidence/task640-web-png/restored.png). Fine lines retain more
detail, full-image placement and the central sheet region remain consistent,
and restored pixels match. The nonuniform pair intentionally stretches the
image horizontally to exercise both axes. [Receipt](../evidence/task640-web-png/receipt.json)
records browser version, runtime hashes, physical resource sizes and draw records.

Theory gained: resource sampling follows physical output, but crop coordinates
belong to the logical source. The before/after images exposed a crop error that
matching destination bounds alone could not detect. This predicts that future
source-family selection must preserve logical source extents while changing
physical decoded dimensions.

Reviewer personas for this bounded Web slice: Language Designer, Compiler
Architect, Runtime Engineer, Code Expert, Performance Expert and Human Advocate
GREEN after the focused tests and visual review. Umbrella #640 remains incomplete
for the platform and source-detail requirements listed above.
