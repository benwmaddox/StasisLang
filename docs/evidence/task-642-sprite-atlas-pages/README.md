# Task #642 — configurable sprite atlas pages

These captures use the production `runtime/web/game.js` renderer in Chromium
with WebGL2. The fixture creates 18 distinct 252×252 sprite images and replays
146 ordered sprite instances (the 18 source sprites followed by 128 repeats).
The 2px extrusion on each side makes each packed rectangle 256×256. The
browser reported `MAX_TEXTURE_SIZE = 16384`.

| Setting | Atlas dimensions | Allocated bytes | Live entries | Draws | Texture binds | Page transitions | Uploads |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 512px baseline | six 512×512 pages | 6,291,456 (6 MiB) | 18 | 134 | 134 | 133 | 24 |
| 2048px configured | one 2048×2048 page | 16,777,216 (16 MiB) | 18 | 1 | 1 | 0 | 19 |

The larger page spends 10 MiB more reserved texture memory for this workload,
while reducing draw submissions by 133 and eliminating page transitions.
Instance uploads are unchanged at 9,344 bytes. WebGL-reported frame and replay
times vary with the browser/device and are deliberately not treated as a
controlled performance result.

## Visual evidence

- Before: [512px baseline capture](atlas-pages-before-512.png)
- After: [2048px configured capture](atlas-pages-after-2048.png)
- [Machine-readable metrics](metrics.json)

## Theory gained

The browser capture also exposed a real-device issue missed by the fake-WebGL
tests: the sprite fragment shader's `mediump` varying precision rendered the
shared 1024px/2048px atlas blank in this Chromium WebGL2 implementation. Using
`highp` preserves the normalized UV precision required by larger pages. The
512px control renders before and after the change; the 2048px image is the
verified, successful single-draw output. WebGL2 guarantees fragment `highp`.

The focused Node atlas-measurement test additionally measures the same
workload through the renderer's deterministic fake-WebGL path. That harness
reports pixel-upload bytes and median host replay duration; those figures are
kept separate in `metrics.json` and are not GPU timings.
