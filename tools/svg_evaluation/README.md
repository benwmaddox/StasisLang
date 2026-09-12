# Post-SVGO evaluation (task 338)

Keep the pinned SVGO 3.3.2 precision-2 baseline. No additional pass is promoted
to the game-asset pipeline. The experiments here are offline research, not a
general-purpose SVG optimizer. Renderer replacement is outside this task.

## Reproduce

From the repository root, with Node 24, Python 3.12, Pillow 12.3.0, NumPy 2.5.2,
CMake and a native C++ toolchain:

```powershell
# Use an existing SVGO 3.3.2 package directory, or install locally:
npm install --prefix output/svg-evaluation-deps --no-save --ignore-scripts svgo@3.3.2
node tools/svg_evaluation/generate.cjs output/svg-evaluation-deps/node_modules/svgo output/svg-evaluation
python tools/svg_evaluation/build_probe.py
python -m unittest discover -s tools/svg_evaluation -p test_evaluate.py
python tools/svg_evaluation/evaluate.py --probe output/svg-evaluation-build/Release/stasis_svg_bench.exe --output output/svg-evaluation
git diff --check
```

For single-configuration generators, the probe is directly under the build
directory. The recorded Windows run used the already installed package at
`D:/code/SiteRefresh/node_modules/svgo`, and a fresh build at
`output/svg-evaluation-clean`. The build helper normalizes Windows environment
key casing to avoid MSBuild's duplicate `Path`/`PATH` failure. Each native command
has a 900-second timeout; each render subprocess has a 120-second timeout.

The generator pins and asserts the SVGO version, master SHA-256 values and two
identical independent optimization results. It writes only derivatives under
the specified output directory. Raw bytes, gzip level 9 with zero mtime, and
Brotli text mode quality 11 use Node's built-in codecs. `sizes.json` records Node,
zlib and Brotli versions; compressed sizes should be compared under those same
versions. `renders.json` contains all target/background gates and timing samples.
The checked-in `evidence` directory retains the measured reports, baselines and
review PNGs; the full candidate derivatives are reproducible ignored output.

## Corpus and provenance

These are existing LIVE-produced vectors, copied byte-for-byte, without running
vectorization or touching their original locations. They are low-resolution
LIVE checkpoint artwork, not a new claim of fidelity to an original raster.

| Master | Paths | Intrinsic size | SHA-256 |
| --- | ---: | --- | --- |
| `corpus/character.svg` | 300 | 128x128 | `7a43e3fe96a45e3a53f199d0f12904b36bcc82f332ab780a89646328f86e31cc` |
| `corpus/screen.svg` | 1500 | 128x277 | `5c7dcf1b4a0292f5b8057130b116f70498c586f6bbda26332f5417ae58fc7528` |

Character source: `D:/code/external/LIVE-Layerwise-Image-Vectorization/local_output/live-runs/178758545903_puddle-500path-sweep/output-svg/1-4-10-15-20-50-100-100.svg`.
Its preserved run config identifies the schedule, 100 iterations and LIVE input.
The original run also retains `optimizer-proof/300-raw.png` and
`300-optimized.png`; neither was used as evidence for the fresh baseline.

Screen source: `D:/code/output/io-game-concepts/proofs/live-screen-review/puddle-pop/live-1500-raw.svg`.
The preserved historical `screen-metrics.json`, `screen-build-report.py` and
`screen-config.yaml` link the checkpoint to LIVE run
`178758846319_live-screen-review-puddle-1500`. Its original
`output-svg/45-45-60-150-300-300-600.svg` was independently hashed and matches.
The historical report script is provenance only; do not execute it as part of
this evaluation (it describes earlier raster/vectorization work).

## Baseline and candidates

Baseline: one `preset-default` pass, `floatPrecision: 2`, `multipass: false`,
`removeViewBox: false`. Keep intrinsic dimensions, viewBox and canvas background.
The preset already performs most structural/numeric/group/path cleanup. Reuse
its implementations for those experiments rather than maintain duplicate code.

| Candidate | Experiment and rationale |
| --- | --- |
| structural | Empty attributes/containers, group collapse, attribute sorting after baseline. |
| numeric | Numeric, path and transform normalization again at precision 2. Tests whether a fixed-point pass helps. |
| palette | Custom deterministic classes factor repeated exact hex fills; never group opacity. |
| reuse | SVGO `reusePaths`; tests exact duplicate geometry without inventing approximate matches. |
| consolidation | Inherited attribute grouping, group collapse and safe `mergePaths`; no forced overlap merge. |
| structural_numeric | Cleanup followed by numeric normalization, to expose possible normalization interactions. |
| palette_reuse | Fill factoring followed by reuse, to test whether shared style exposes reuse opportunities. |
| subpixel | Trial removal of direct-root paths with a conservative control-point hull below 0.25 pixels at the maximum scale; unknown commands, transforms, strokes and viewBox are ineligible. No path qualifies in this corpus. |
| precision1 | Deliberately aggressive negative control; reduces numeric precision and must face exactly the same pixel gate. Never eligible for promotion because precision 2 remains required. |

Custom candidates are confined to this verified corpus. In particular, CSS and
`use` support must not be inferred for NanoSVG or another renderer from these
results. Structural auditing rejects raster payloads, untested element types,
external references and active content, and checks dimensions and path counts.
There are no duplicate geometries eligible for reuse and no new `use` elements
in the measured outputs. Subpixel size alone is never proof of invisibility:
any future eligible deletion still must pass every rendered gate.

## Render and timing contract

Use the repository's pinned ThorVG 1.2.0 and the unmodified
`stasis_svg_rasterize_file` bridge. It contains the artwork while preserving
aspect ratio, rounds contained dimensions upward, centers it, and leaves
transparent padding. Thus 1080x2400 and 2160x4800 are render targets, not source
dimensions. The screen's 128:277 aspect ratio differs from 9:20; padding is
expected. Desktop landscape targets test that same contain behavior.

Character targets: 64, 128, 256, 512 and 1024 square; 1080x2400, 2160x4800 and
3840x2160. Screen targets: 360x800, 720x1600, 1080x2400, 2160x4800, 1920x1080 and
3840x2160. These cover actual character display, enlarged inspection, 9:20 phone
and 4K desktop, including the required large screen targets.

Every candidate is compared over every output pixel against the precision-2
baseline: straight RGBA (including alpha), then composites over black and white.
The gate is exact equality, intentionally stricter than a perceptual threshold.
Any failing candidate is rejected regardless of aggregate similarity or savings.
The command also enforces the reviewed candidate/target/background acceptance
matrix in `evidence/renders.json`. Unexpected passes (including the precision-1
negative control), unexpected failures, missing or extra comparisons, and
inconsistent aggregate results exit nonzero after writing diagnostic artifacts.
Timings and the magnitudes of nonzero differences are not fixed oracles. A
deliberate change to the expected matrix requires review of the recorded evidence;
rerunning the evaluator does not overwrite that checked-in oracle.
Repeated native renders must also be byte-identical. Byte-identical SVG
candidates share their render/timing observations, explicitly keyed by SHA-256.

The native probe records three end-to-end load/raster calls per unique SVG and
target: first cold, then two warm calls. Timing includes file load, parsing,
contain sizing, allocation, draw and synchronization. ThorVG's asynchronous load
submission is not a completed parse measurement; the probe therefore reports
the synchronous bridge boundary instead of mislabeling submission latency.
These measurements are diagnostic, not a statistically powered performance
claim. There is no runtime code or renderer change.

## Recommendation

Precision 2 supplies the useful reduction. Structural cleanup, reuse,
consolidation and subpixel trials find no additional savings. Palette classes
increase compressed size. A numeric repeat saves only a handful of bytes and
changes rendered pixels at some targets. No measured result justifies another
production pass. Keep masters immutable, generate precision-2 derivatives, and
rerun this matrix before revisiting any custom pass on a different corpus.

## Recorded results (2026-09-09)

| Asset / variant | Raw bytes | gzip bytes | Brotli bytes |
| --- | ---: | ---: | ---: |
| Character master | 200186 | 66300 | 57199 |
| Character precision-2 baseline | 49207 | 18782 | 16621 |
| Character numeric repeat | 49204 | 18785 | 16622 |
| Character palette | 49176 | 18954 | 16719 |
| Character precision-1 control | 40324 | 14227 | 12644 |
| Screen master | 1004102 | 330035 | 261897 |
| Screen precision-2 baseline | 246083 | 92178 | 83212 |
| Screen numeric repeat | 246078 | 92164 | 83201 |
| Screen palette | 246364 | 92528 | 83460 |
| Screen precision-1 control | 202821 | 69285 | 62562 |

Structural, reuse, consolidation and subpixel output hashes equal baseline.
Neither baseline contains transforms, so transform normalization has no eligible
work; these results do not establish correctness for a transformed asset corpus.
Structural+numeric equals numeric; palette+reuse equals palette. An independent
second generation reproduced every SVG hash and all three size measurements.

All 420 candidate/target/background comparisons completed (including baseline
self-checks). Palette outputs match baseline pixels at all targets but increase
compressed size. Numeric repeats fail at three character targets and four screen
targets. For example, screen 2160x4800 changes 221 RGBA pixels, including 200
black-background pixels and 204 white-background pixels (maximum composited
channel delta 3). The large straight-RGB delta of 127 occurs at low alpha; it is
not a claim of a large visible difference. Even this small regression is not
justified by 5 raw bytes saved. Precision 1 fails at every target.

Representative baseline load+raster timings, milliseconds (cold, warm, warm):

| Asset / target | Samples |
| --- | --- |
| Character 256x256 | 2.409, 1.809, 1.841 |
| Screen 1080x2400 | 22.613, 20.992, 20.364 |
| Screen 2160x4800 | 74.874, 78.642, 76.149 |
| Screen 3840x2160 | 29.636, 28.444, 30.725 |

Native build: MSVC 19.44.35222, Release, fresh pinned ThorVG source compilation.
Both CTest tests and all four Python gate tests pass. Native repeat comparisons
pass for every rendered unique SVG/target. The current renderer is the scope of
this result; it establishes neither NanoSVG compatibility nor cross-platform
bitwise raster equivalence.

Visual evidence: inspected `evidence/review.png` (baseline/numeric/precision-1
contact sheet) and `evidence/screen/baseline-1080x2400.png` (full target render
and contain padding). The preserved character PNG is the contact sheet's
256x256 baseline source. Automated full-resolution gates cover enlarged sizes
and backgrounds; the contact sheet is not their substitute. The rough LIVE
checkpoint details are pre-existing artwork, not newly introduced optimization
artifacts. No animation or interactive behavior changes, so MP4 is inapplicable.

Theory gained: a second path normalization can slightly move raster coverage
even at unchanged precision; actual renderer gates found this at larger targets.
Adjacent prediction: future normalization changes must repeat the envelope test,
even when source-level numeric precision and dimensions are unchanged.
