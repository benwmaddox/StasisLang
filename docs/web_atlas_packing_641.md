# Web atlas packing measurement (#641)

Measured against `runtime/web/game.js` at `c7d430ee` (2026-09-22). This is an
investigation; the renderer is unchanged. The committed browser fixture runs
that production script with a deterministic guest frame and real WebGL2. The
Node test runs the same script with a fake WebGL2 context for exact call counts.

## Conclusion

Separate images that fit within a 2048x2048 RGBA texture do not necessarily
share one page. The allocator starts each new page at 512x512, grows a new page
only when an individual image requires it, and never enlarges an existing page.
The first two pixels of each page are reserved for a white solid texel. Each
image receives two pixels of extruded padding per side. Eighteen 252x252 images
therefore occupy six 512x512 pages (three 256x256 padded images per page),
although all 18 padded images cover only 1,179,648 of 4,194,304 pixels in a
2048 page. A constructive single-page layout places six padded 256x256 images
per row for three rows (1536x768); the 2px solid-texel reservation still fits
at the start of the first row. Alternating two images on different pages
produces 133 transitions
and 134 draws for 146 sprite instances. An equivalent authored 6x3 sheet uses
one 2048 page and one draw. The sheet reserves 16 MiB instead of 6 MiB, a
10 MiB cost. Preallocating a 2048 page for every game would waste memory for
small sprite sets; the measured gap justifies a conditional policy follow-up.

## Reproduce

From the repository root on a machine with Node and a WebGL2 browser:

```powershell
$env:STASIS_ATLAS_REPORT = '1'
node --test --test-name-pattern 'atlas efficiency measurement' runtime/web/tests/render_pipeline.test.mjs
python -m http.server 8765 --bind 127.0.0.1
```

Open these fixture URLs in separate browser loads, then inspect
`document.body.dataset` or press F3 for the runtime HUD:

- `http://127.0.0.1:8765/runtime/web/tests/fixtures/atlas_efficiency.html?case=separate`
- `http://127.0.0.1:8765/runtime/web/tests/fixtures/atlas_efficiency.html?case=sheet`
- `http://127.0.0.1:8765/runtime/web/tests/fixtures/atlas_efficiency.html?case=representative`

The separate and sheet modes create identical 18 translucent tile pictures.
One mode loads 18 individual 252x252 PNG data URLs; the other loads one
1512x756 PNG sheet and selects its cells with the ordinary sprite crop fields.
Both submit 146 instances in the same order: 18 unique tiles, then 128 draws
alternating the first and last tile. The representative mode loads the actual
in-repo `arena_background.png` (1672x941) and `smoke.png` (64x64) through the
production image preparation path, then draws the background and 32 moving
translucent smoke sprites. All measurements use RGBA8 page bytes and the
current two-pixel extruded padding; mipmaps are not allocated.

For a repeatable timing sample, evaluate this in the browser after `ready=true`:

```js
(async () => {
  const host = [], frame = [];
  for (let i = 0; i < 60; i++) {
    await new Promise(requestAnimationFrame);
    host.push(Number(document.body.dataset.hostReplayMs));
    frame.push(Number(document.body.dataset.frameWorkMs));
  }
  host.sort((a, b) => a - b);
  frame.sort((a, b) => a - b);
  return { hostP50: host[30], hostP95: host[57],
    frameP50: frame[30], frameP95: frame[57] };
})()
```

## Measurements

The exact counts below were observed in both the Node fixture and a real
Chromium WebGL2 run. Pixel upload counts include one 2x2 white upload per page;
the pixel upload bytes count includes its 16 bytes. Instance records are 64
bytes each. Upload counters are cumulative; draw/bind counters are per frame.

| Case | Padded pixels / allocated pixels | Pages / GPU bytes | Pixel uploads / bytes | Instance upload | Draws / binds / transitions |
| --- | ---: | ---: | ---: | ---: | ---: |
| 18 separate 252x252 | 1,179,648 / 1,572,864 (75.0%) | 6 x 512 / 6,291,456 B | 24 / 4,718,688 B | 9,344 B | 134 / 134 / 133 |
| Equivalent 1512x756 sheet | 1,152,160 / 4,194,304 (27.5%) | 1 x 2048 / 16,777,216 B | 2 / 4,608,656 B | 9,344 B | 1 / 1 / 0 |
| Mixed 6x252, 4x120x240, 4x64x96, 4x32x48 | 548,928 / 786,432 (69.8%) | 3 x 512 / 3,145,728 B | 21 / 2,195,760 B | 9,344 B | 131 / 131 / 130 |

The separate fixture has 1,143,072 content pixels and 36,576 padding pixels;
the sheet has the same content pixels and 9,088 padding pixels. The 6x3 layout
proves the separate image set fits one 2048 page with padding and the solid
texel. The mixed-size case establishes low padded area but does not assert a
specific one-page placement for that ordering.
The 18-image order includes a first-pass page traversal (18 images) before
the 128 alternating draws, which explains 134 rather than 128 draws.

Browser timing on this workstation varied with WebGL driver load: the two
60-frame separate-image samples had host replay p50 of 16.3 and 50.9 ms
(p95 17.9 and 52.3 ms); the sheet samples had p50 0.7 and 0.6 ms (p95 1.0
and 0.8 ms). Paired total frame-work p50 was 50.9 ms separate and 0.6 ms
sheet in the second sample. Guest tick/render work was negligible in this
fixture. `hostReplayMs` includes JavaScript command replay and WebGL calls,
not GPU completion or presentation; these figures are an observed local
comparison, not a portable frame-rate guarantee. The exact draw/bind gap is
the stronger result.

The real in-repo asset pair needs 1,588,444 padded pixels, or 37.9% of a 2048
page. On a cold browser load, smoke finished preparation first: it claimed a
512 page, and the later 1672x941 background required a 2048 page. The result
was two pages, 17,825,792 allocated bytes, four pixel uploads, two draws and
one transition. On two warm reloads, the background arrived first; smoke fit
beside it on one 2048 page, yielding 16,777,216 bytes, three uploads, one
draw and zero transitions. Both cases uploaded 6,353,776 sprite/padding bytes,
plus 16 white-texel bytes per page, and submitted 8,192 instance bytes per
frame. This is a load-order effect, not a leak. Both
visible results were inspected.

## Residency, ordering, and limits

The Node fixture rendered 25 additional frames of both cases without another
atlas pixel upload. A separate animation check changed both sprite position
and crop UV between frames: the 64-byte instance record changed, while atlas
upload count stayed at two. The live representative fixture moves smoke every
frame and keeps its loaded atlas entries. Full and raw-source crop variants can
both occupy atlas space when source and prepared drawables differ; the existing
`configured residency accounts for full and source atlas variants` test covers
the budget consequence. A density refresh or context restore can legitimately
upload pixels again.

Page sharing alone cannot eliminate every split. `game.js` flushes on page
changes, execution-domain changes, clip/scissor changes, and the 4,096-sprite
capacity. The existing ordered clipping and translucent `A-B-C-A-C-B` tests
verify that painter order and scissor boundaries are preserved. The comparison
holds the draw order and 146 instances fixed; it changes only asset packaging.
Compiler hot-render `grouping_key` and `atlas_eligible` metadata are not read
by `runtime/web/game.js`; Web placement follows actual resource readiness and
dimensions. `web.atlas_budget_bytes` can cap allocation and must also be
considered by any future policy.

Visual evidence inspected: [separate tiles](web_atlas_efficiency_641_separate.png)
and [the equivalent sheet](web_atlas_efficiency_641_sheet.png) show the same
translucent order with their different HUD counts. The in-repo game assets
rendered correctly in [cold two-page](web_atlas_efficiency_641_two_pages.png)
and [warm one-page](web_atlas_efficiency_641_one_page.png) captures.

## Narrow follow-up recommendation

Add an optional, bounded Web atlas page-sizing/prewarm policy for games whose
known active raster set can fit within one 2048 page under the two-pixel
padding rule. Keep the default 512-page behavior for small/unknown sets and
honor `MAX_TEXTURE_SIZE` and `web.atlas_budget_bytes`. Acceptance should
demonstrate deterministic co-residency for the 18-image and mixed fixtures,
independent of asset completion order; record page bytes and at most one
page-driven draw for the alternating fixture; retain exact transparency order,
clip/capacity barriers, resource refresh rollback, and unchanged-asset
residency. Measure memory and frame-time tradeoffs against this baseline before
enabling the policy broadly. A compiler metadata path is unnecessary unless
it supplies reliable sizes for this specific decision.

Theory gained: the first ready resource determines whether later large images
can share its page, because pages are never enlarged. A future policy that
reserves a suitable page before asynchronous preparation completes should
remove this load-order difference while still permitting small games to use
small pages.
