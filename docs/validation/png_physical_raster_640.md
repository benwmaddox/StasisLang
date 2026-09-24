# Physical PNG raster audit (#640)

Audit base: `3bb32f5f`, updated 2026-09-24. The original Web repair is now
followed by a focused native SDL and Android Workshop repair. Existing artwork
and the asset schema are unchanged.

## Backend inventory and open acceptance

| Production path | Source preparation and draw path | Remaining acceptance |
| --- | --- | --- |
| Desktop JIT/AOT SDL and GL | Shared native preparation covers the larger exact fitted backing axis plus the maximum per-frame crop/instance sampling requirement. It retains source/required/prepared dimensions and decoded/prepared bytes, reports `source-underprovisioned`, and preserves handles through resize and renderer restoration. | Windows SDL software rendering and lifecycle seams are qualified below. Hardware GL and non-Windows native devices were not available. |
| Android packaged AOT | The SDL shell builds the repaired shared runtime, including the same PNG preparation, source diagnostic, cache and restoration logic. | Both JNI ABIs were rebuilt and verified, but the packaged SDL APK was not rendered on a device in this run. |
| Apple shared SDL | iOS project sources include shared `stasis_graphics.c`; desktop uses native runtime. | Xcode, Apple builds and physical devices are unavailable on this Windows host. No Apple qualification is claimed. |
| Android Workshop GLES | Frame collection combines every use of a handle before planning. The larger fitted axis is uncapped. Adequate raster sources decode directly to the required preparation; insufficient sources decode at their real size before the display fallback is prepared. Cache state and acceptance receipts distinguish source, decoded, prepared, upload and texture bytes and count underprovisioned sprites. | JVM contracts and a real API 35 APK build are qualified below. The emulator render preflight did not reach a game frame, and no physical phone was available. |
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
GREEN after the focused tests and visual review. At that Web-only checkpoint,
umbrella #640 remained incomplete for the platform and source-detail requirements
listed above; the following section records the subsequent native and Workshop
repair and its explicit platform limits.

## Native SDL and Android Workshop repair

Native sprite preparation now uses an exact rational ratio for the larger
rounded fitted viewport axis. Each accepted frame first combines full-image and
sheet-crop requirements for every shared handle, including fractional,
nonuniform and negative scale. The cache keeps the highest successfully
prepared instance requirement for that handle. A failed larger preparation
keeps the last valid texture, records the failed physical dimensions and does
not retry the same impossible allocation on every draw; a changed requirement,
source reload or lifecycle rebuild can try again. Density changes, async decode
completion, atlas staging and renderer restoration all use the same physical
key. Logical crop conversion still divides by the logical sheet extent.

Raster decode records the real PNG width, height and decoded bytes before
preparation. If contain-fit would enlarge the source, the published entry is
`source-underprovisioned`; preparation remains available for logical continuity
but interpolation is not counted as physical-detail compliance. Opt-in native
receipts expose source file bytes separately from decoded and prepared RGBA
bytes. Vector SVG entries do not report raster-source insufficiency.

Workshop uses the same contract at its GLES boundary. `AndroidRasterPlan`
combines frame requirements and computes uncapped exact physical dimensions.
For PNG/JPEG/WebP, the provider validates manifest dimensions against decoded
bounds. An insufficient raster is first decoded at its actual source size, then
scaled only for the visible fallback; a structured `sprite_source_resolution`
warning and acceptance snapshot expose the insufficiency. Shared cache identity
still includes canonical path, content hash, prepared dimensions, density and
surface/renderer generations. The current manifest schema has no density-source
family, so this repair deliberately selects only its canonical path/hash and
does not invent filename conventions.

## Native and Workshop validation (2026-09-24)

- Native focused build and `ctest -R
  "stasis_(display_scale|asset_tasks)_contract"`: 2/2 passed. The contracts
  cover 1x, fractional, nonuniform and >8x planning; an adequate 64x64 PNG;
  explicit 2x2 underprovision; logical sheet crops; negative scale and rotation;
  aggregation for repeated handles; unchanged-size reuse; stale async decode
  across a density change; shared-handle resize; renderer reset/restoration;
  physical byte accounting and generation-safe release.
- `python tools/cargo_cache.py run -- cargo test -p stasis
  desktop_runtime_uses_physical_drawable_pixels_at_monitor_density --lib`:
  passed (1 test). This includes the exact native preparation receipt oracle.
- Fresh release Rust JNI bridges were built for `arm64-v8a` and `x86_64`.
  `rust_bridge_provenance.ps1 -Mode Verify -Profile release` passed for both.
- `gradle -p mobile/android :app:testWorkshopDebugUnitTest -x
  :app:verifyWorkshopRustBridge`: passed. The skipped Gradle task was run
  directly immediately beforehand because this workstation's Gradle child
  PowerShell could not autoload `Get-FileHash`. Tests cover combined crop and
  transform sampling, limits, exact identities, source sufficiency, distinct
  decoded/prepared bytes, diagnostics and uncapped fitted-axis rounding.
- `gradle -p mobile/android --project-prop=stasis.renderAcceptance=true
  :app:assembleWorkshopDebug -x :app:verifyWorkshopRustBridge`: passed against
  the same directly verified fresh JNI inputs.
- Native rendered evidence: the asset contract produced
  `task640-native-physical-sheet.png` at 64x64 after a 1.5x density transition
  and renderer reset. ImageMagick inspection found exactly four opaque colors;
  quadrant samples were red, green, blue and yellow, and both sides of the
  horizontal/vertical crop seams matched the expected neighboring cells. This
  confirms logical sheet selection and no atlas-gutter contamination in the
  rendered output.
- API 35 emulator evidence was attempted twice on the existing emulator and
  once after a clean emulator restart. All three runs stopped in the existing
  Workshop IT-027 preflight with `GLES token timeout`; the renderer surface was
  created only afterward and the render-parity gate never observed a game
  frame. Those failed captures are not accepted as rendering evidence. No
  physical Android phone was available.

Apple qualification remains explicitly unavailable: this Windows host has no
Xcode toolchain, Apple simulator or physical iOS/macOS device. The shared source
was compiled through the Windows native target, but this run makes no Apple
build, launch, GPU, lifecycle or rendered-output claim.

Visual evidence: the deterministic native 64x64 sheet capture was inspected by
quadrant and seam pixels as described above. Android and Apple visual evidence
is not claimed because their platform gates were unavailable or failed before a
game frame.

Good: source insufficiency is now observable without changing logical layout or
resource handles. Bad: interpolation remains necessary to display an
underprovided canonical source, and the emulator preflight prevented GLES
visual qualification. Adjustment: keep interpolation labeled as fallback,
retain separate source/prepared accounting, and require a green IT-027 run or
physical-device capture before claiming Android rendered parity. Theory gained:
physical preparation is a frame-wide demand on a logical resource, while source
sufficiency is a property of the selected canonical asset and must never be
inferred from the size of an interpolated texture.
