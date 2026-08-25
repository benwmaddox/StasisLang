# GPU instancing for large Stasis scenes

Status: investigation only. This report does not change the v5 graphics command
ABI or renderer behavior. It records current paths, a measured Web baseline,
and a staged design for adjacent-compatible batching.

## Executive recommendation

Use instancing first for long runs of rectangles or sprites that are already
adjacent in the v5 order stream and have the same GPU state. Keep source order
and source-over blending semantics; do not sort transparent instances in the
first implementation. A batch key includes material, atlas page, sampler and
filter, blend mode, clip/scissor, and shader variant. A different key, or any
intervening command, ends the run.

The first production slice should add a private host-side GPU path behind the
existing command interpreter. It accepts current typed lanes, builds a
transient instance buffer, and falls back per run. The v5 layout remains the
source of truth and no guest-to-GPU pointer ownership is introduced. Add an
explicit opaque mode only in a later ABI version after it has a separate order
contract; never infer opacity from alpha alone.

## What exists today

The v5 command stream in `src/stdlib/internal/gfx_cmd.stasis` has a 32-i32
header, separate typed arrays, and an order array at i32 18,464. Lines and
rectangles share an 8-f32 geometry stride; sprites use three i32 values
(`handle`, rotation degrees, alpha) and eight f32 values (`x`, `y`, `w`, `h`,
normalized `u0`, `v0`, `u1`, `v1`). An empty order stream means category order
line, rectangle, sprite, text. Otherwise the order array is authoritative.
The order capacity is 16,144, shared line+rectangle geometry capacity is 10,000,
and sprite capacity is 4,096.

The current implementations map as follows:

| Path | Current batching and ordering | Instancing status | Fallback/resource boundary |
| --- | --- | --- | --- |
| Web (`runtime/web/game.js`) | Consecutive rectangle entries become one run; a non-rectangle flushes it. Ordered runs use order indices. | WebGL2 rectangle batcher only; one `drawArraysInstanced` call plus one Canvas `drawImage` composite per eligible run. | Canvas 2D for short runs, unavailable WebGL2, initialization failure, or a thrown draw error. Context loss is not detected today and is a recovery gap. Sprites and text replay individually in Canvas 2D. |
| Android GLES (`StasisPreviewRenderer.java`) | Adjacent same-kind entries with consecutive source indices are grouped. Rectangles expand to triangles in chunks; sprites group while texture and filter match. | No hardware instancing; expanded vertex batches and ordinary GLES draws. | Texture provider owns decoded textures and atlas-independent handles; invalid UV data follows existing append/flush behavior and is not described as a clean skip here. |
| Native GL (`runtime/stasis_graphics.c`) | Ordered rectangles call `stasis_fill_rect` individually. Lines flush category-local geometry. Sprites append six vertices and flush on atlas page or capacity changes. | No hardware instancing. Sprite atlas pages are already a useful material boundary. | GL uses atlas pages; SDL uses SDL texture calls and does not use the GL atlas path. |
| SDL/native fallback | Category-local calls remain in command order; no reorder or global sort. | No instancing assumption. | Preserve existing SDL renderer behavior and per-texture state changes. |

The native and Android descriptions are source-derived current behavior, not
the older comment that described Android rectangles as purely individual
replays. Android currently groups adjacent rectangle entries into expanded
triangle draws, but it does not use hardware instancing.

## Measured Web baseline

### Method and context

I packaged `samples/swarm_field` as a development Web package, served the
resulting `dist/swarm_field-web` directory with Python's local HTTP server, and
opened it in real Chromium through Playwright. The package was warmed until
frame 90, then 120 consecutive `requestAnimationFrame` samples were collected
from the development HUD's `document.body.dataset` values. Dataset metrics are
serialized with `toFixed(3)`; the tables round those values to 0.1 ms for
readability. All reported quantiles use the non-interpolated upper convention
`Q(q) = x[floor(q * N)]`; they are labeled upper p50/upper p95 and are not
conventional interpolated median/nearest-rank p95 values. This is a single
local run, not a cross-machine benchmark or a claim about GPU execution time.

Context: Windows 10 Pro 2009, Intel64 Family 6 Model 170 (22 logical
processors reported by the environment), Chrome 151.0.7922.174, headless
Chromium launched with `--no-sandbox` by Playwright, 1280x720 viewport, ANGLE
WebGL2 renderer `ANGLE (Intel, Intel(R) Arc(TM) Graphics (0x00007D55) Direct3D11
vs_5_0 ps_5_0, D3D11)`. The host reported `Canvas2D + WebGL2`.

`swarm_field` emits 8,192 1.8x1.8 alpha-0.94 rectangle commands plus four
setup/crosshair commands. All agent entries are one adjacent rectangle run,
so this measures the existing eligible fast path rather than a mixed-alpha or
atlas scene.

| Metric (120 post-warmup frames) | Upper p50 | Upper p95 | Maximum |
| --- | ---: | ---: | ---: |
| Guest render (Wasm) | 0.0 ms | 0.1 ms | 0.2 ms |
| Browser replay | 0.2 ms | 0.3 ms | 0.4 ms |
| Total frame work | 0.3 ms | 0.4 ms | 0.6 ms |
| Instances | 8,196 | 8,196 | 8,196 |
| Batches | 1 | 1 | 1 |
| Instrumented instanced draws | 1 | 1 | 1 |
| Uploaded bytes | 262,272 B | 262,272 B | 262,272 B |
| Backend | Canvas2D + WebGL2 | Canvas2D + WebGL2 | Canvas2D + WebGL2 |

The 262,272 B figure is 8,196 instances * 8 f32 * 4 bytes. The current Web
rect batcher uploads all eight floats per instance with `bufferSubData`, then
draws a four-vertex triangle strip. It clears a transparent offscreen WebGL2
canvas and composites that canvas once at the run position. `renderPrepMs`,
`gpuSubmitMs`, and `gpuExecutionMs` were unavailable; browser replay is
host-side work and does not mean the GPU completed within that time.
The runtime's `drawCalls` counter records the one instanced WebGL draw but not
the following Canvas `drawImage`, so the physical work is one GPU draw plus one
composite for this run. This counter is not directly comparable to the Canvas
sprite count without reporting the composite separately.

### Sprite and mixed-order measurements

To cover paths absent from Swarm Field, I used two disposable development
packages importing the existing v5 graphics library, then deleted them after
measurement. The first emitted 4,096 adjacent sprites (the v5 sprite limit),
with per-sprite rotation and constant alpha 180. The second emitted 2,048 sprites and 2,048
rectangles in alternating source order (4,096 order entries, 2,048 of each
primitive), varied alpha and rotation, and used two normalized UV rectangles.
Both used one 4x4 SVG asset. This is an atlas-like UV workload, but it does not
measure atlas page allocation: the current Web runtime has no atlas/material
allocation or page metric, and its sprite path uses Canvas `drawImage`.

The exact method and Chromium context are the same as the Swarm measurement:
warm to frame 90, sample 120 animation frames, and report upper p50/upper
p95/maximum
for the HUD dataset. The current Web sprite path cannot expose GPU instance,
batch, or upload counters, so `instances` and `batches` are `-1`. The reported
zero uploaded bytes is only the current rectangle-instancing upload counter;
it does not claim zero texture upload, driver upload, or memory traffic. Those
allocation and transfer metrics are unavailable on this Canvas path.

| Fixture and source workload | Backend | Guest render upper p50/upper p95/max | Browser replay upper p50/upper p95/max | Total frame work upper p50/upper p95/max | Commands / sprites / rectangles | Draws | Instances / batches / uploaded |
| --- | --- | --- | --- | --- | --- | ---: | --- |
| 4,096 adjacent sprites; rotation varied, constant alpha 180 | Canvas2D | 0.0 / 0.1 / 0.2 ms | 16.5 / 17.5 / 28.5 ms | 16.5 / 17.6 / 28.5 ms | 4,096 / 4,096 / 0 | 4,096 | -1 / -1 / 0 B |
| 2,048 sprites + 2,048 alternating rectangles; rotation, alpha, and two UV rectangles varied | Canvas2D | 0.0 / 0.1 / 0.2 ms | 20.7 / 24.8 / 44.1 ms | 20.8 / 24.9 / 44.2 ms | 4,096 / 2,048 / 2,048 | 4,096 | -1 / -1 / 0 B |

For a compatible/page-local native GL run of 4,096 sprites, the current
expanded-geometry path would upload one buffer containing 24,576 vertices
(4,096 * 6), subject to its page and capacity flushes; this is not a hardware
instancing measurement.

The four-page atlas case remains a source-derived projection: current native GL
flushes on atlas page and other paths split on texture/filter, but this Web
runtime does not expose atlas allocation/page metrics. A future atlas fixture
must record page count, material-key runs, allocation bytes, and peak staging
and GPU-buffer memory before that case can become a measured gate. Current v5
limits are 4,096 sprites, 10,000 shared line+rectangle geometry entries, and
16,144 order entries. 16,000 mixed order entries are therefore realizable
under v5 when primitive capacities and the shared geometry limit are respected,
but there is no 16k single-primitive sprite or rectangle v5 fixture. Any 16k
single-primitive scale case below is explicitly future synthetic/ABI work, not
a current benchmark.

## Compatible batch rules

An instance may join the preceding instance only when all of these are equal:

1. The entries are adjacent in the v5 order stream and are the same primitive
   family (rectangle or sprite).
2. Material/shader variant, blend equation/factors, atlas texture page,
   sampler/filter, clip/scissor, and render target are equal.
3. Coverage and color semantics are representable by the active shader.
   Rotation, UV rectangle, and alpha are per-instance data, not reasons to
   split a batch. Tint is white under v5 and becomes per-instance data only
   with an explicit future ABI/material extension.
4. The run remains within transient-buffer and API attribute limits.

The key is deliberately stricter than "same texture": a material system must
not merge different blend, clip, color-space, or sampling state. A clear, line,
text command, or sprite between two rectangles flushes the rectangle run even
when the two rectangles otherwise match. The same rule applies to sprites. An
opaque-only optimization may sort by key in a future explicit mode, but
source-over content remains adjacent-only.

## Candidate instance layouts

These are host-private GPU records initially. They do not replace the v5 ABI.
The byte counts are explicit so upload and bandwidth budgets can be measured.

| Record | Fields | Raw size | Recommended stride |
| --- | --- | ---: | ---: |
| Current rectangle lane | `x,y,w,h,r,g,b,a` (8 f32) | 32 B | 32 B |
| Current sprite lanes | `handle,rotation,alpha` (3 i32) plus `x,y,w,h,u0,v0,u1,v1` (8 f32) | 44 B | split typed lanes; no GPU record today |
| Candidate rectangle instance | `rect` vec4 plus `tint` vec4 | 32 B | 32 B |
| Candidate sprite instance | `rect` vec4, `uv` vec4, `color_rgba` vec4, `rotation_sin,rotation_cos,page_or_flags,material_id` | 64 B | 64 B |

The v5 sprite has no tint field. The 64 B private record is intentionally
padded and stable: its initial color lane is `(1.0,1.0,1.0,v5_alpha)` (white
RGB with the v5 per-sprite alpha), and its final lane can carry a material/page
selector and flags without changing attribute offsets. Varying RGB tint belongs
to an explicit future ABI/material extension, not an interpretation of the
current v5 payload.
The first shader can derive the rotated quad around its center from sin/cos.
If arbitrary affine transforms become required, reserve the next version for a
six-float transform and retain a 64 B or 80 B aligned stride after measuring
the target API. Never reinterpret the existing 44 B split-lane guest payload
as a packed ABI record without an ABI version and validation.

## Atlases, materials, transparency, and ordering

Atlas allocation should be deterministic and retain padding/extrusion at sprite
edges. The current native GL loader uses 2,048x2,048 pages with two-pixel
padding and tracks a page index; this is a natural first material key. Web and
Android may keep current per-image/texture ownership in fallback while an
atlas-backed path is introduced incrementally.

The complete key should be `(shader_variant, atlas_page, sampler_filter,
blend_mode, clip_rect, render_target, color_space)`. A material table can map a
stable material ID to this tuple, but the key must still be resolved at flush
time so a failed/reloaded resource cannot use stale GPU state. Atlas page
changes flush. UVs remain normalized and are clamped/rejected using the same
validation policy as v5.

Source-over alpha is order-dependent. Preserve the order stream and batch only
adjacent compatible entries; do not depth-sort, atlas-sort, or globally sort
transparent sprites. An opaque mode can permit key sorting only after a future
command contract explicitly marks the run opaque and acceptance tests prove
parity. Alpha 0 is still ordered until such an optimization is specified.

## Culling and overdraw

Cull instances against the logical viewport plus a conservative rotated-sprite
margin before copying them into the GPU buffer. Culling must preserve the
source-order position of survivors and must count a dropped/cull diagnostic
separately from a malformed command. Scissor/clip state belongs in the batch
key, not in per-instance data.

For dense scenes, culling reduces vertex work and upload bytes but cannot fix
overdraw among visible transparent sprites. Use an optional coarse spatial
index owned by the host only when profiling shows command decoding is the
bottleneck; do not make guest pointers or host-owned entity state part of the
ABI. Measure fill-rate separately with a deliberately overlapping alpha scene.
Do not cull a rotated quad using its unrotated rectangle unless the margin is
conservative.

## Buffer update and draw-call strategy

Start with one reusable per-context instance buffer, growing geometrically to a
bounded maximum. Each run writes a contiguous host array, then uses
`bufferSubData` (or orphan/map-and-write where the backend proves it is safe)
and one instanced draw. Reuse capacity across frames; do not allocate a GPU
buffer per command. Count bytes copied, bytes uploaded, batches, and draws in
existing development metrics.

Flush on key changes, order interruptions, detected target/context loss, or
buffer capacity. The current Web path does not listen for
`webglcontextlost`, call `isContextLost`, or inspect GL errors, so it cannot
promise Canvas replay after a silent context loss; detection and recovery are
required implementation work. A failed upload, shader compile, detected
context loss, or resize must discard the attempted run and replay it using the
existing path. Fallback is transactional: use per-run fallback only when
failure is detected before framebuffer mutation; after a potentially partial
draw, abandon/rebuild the offscreen target or fail the frame rather than
replaying and double-blending translucent content. A fallback cannot silently
reorder commands. WebGL2 requires `drawArraysInstanced` and
`vertexAttribDivisor`; WebGL1/WebGL without those features uses Canvas 2D.
Android GLES2 and SDL use their existing expanded-geometry/per-call paths until
an independently validated instanced backend is available.

## Platform and ABI implications

The v5 ABI remains unchanged for the first stages: the guest writes the same
arrays, and each host translates them to private records. This keeps Web,
Android, SDL, native GL, JIT, AOT, and replay tools on one contract. A future
ABI version may add explicit material IDs, opaque-mode declarations, or a
validated packed instance stream, but it must specify capacity, alignment,
version rejection, and fallback behavior together. Existing v5 frames must
continue to render identically.

No guest-to-GPU direct ownership is allowed. Wasm linear memory remains guest
owned; the host copies validated values into transient GPU memory. Resource
handles remain generation-checked and atlas/material replacement is published
only after successful creation. This avoids stale texture pointers and leaves
an ownership model in which explicit context-loss recovery can be added; it is
not evidence that the current Web rectangle batcher already recovers.

## Benchmark matrix and acceptance gates

Before each stage, run the same warmup/sample protocol on Web and a matching
native/Android fixture where available. Record backend, logical/display size,
instance count, compatible runs, batches, draws, uploaded bytes, guest render,
host replay, total frame work, and GPU timestamp availability.

| Gate | Fixture | Required evidence | Initial quantitative target |
| --- | --- | --- | --- |
| Baseline | Swarm Field, 8,192 rectangles | Real browser sample and current fallback sample | WebGL2 remains one batch, with one instanced draw plus one reported composite; no >10% upper-p95 total-frame-work regression. |
| Data parity | Rotated/UV/alpha sprites; future tint extension separately | Pixel/hash comparison against v5 replay | 100% sampled pixel/hash parity for source-over order; no dropped/invalid increase. |
| Split correctness | Interleaved rectangle/sprite/text and four atlas pages | Trace of flushes plus visual comparison | Every key/order transition creates expected boundary; zero cross-material merges. |
| Sprite scale | 1k and 4k sprites (v5 max 4,096) | WebGL2, Canvas, Android GLES, native GL/SDL measurements | Instanced GPU draws are <= compatible runs; offscreen composites and total render submissions are reported separately; host replay upper p95 is >=25% lower than per-call fallback at 4k compatible sprites. |
| Geometry scale | 1k, 4k, and 8k rectangles/lines with shared total <=10,000 | Same backend matrix | No >10% upper-p95 total-work regression; upload/copy bytes and draw reduction are reported. |
| Order scale | Mixed primitives up to 16,144 order entries | Same backend matrix | Every source-order boundary is retained; no cross-key merge. |
| Future synthetic scale | 16k single-primitive sprites or rectangles only after a versioned capacity/ABI extension | Explicitly not a v5 fixture; 16k mixed order entries remain a valid v5 order-scale case | Must be labeled synthetic and cannot gate v5 shipping. |
| Recovery | Context/shader/resize/resource failure injection | Explicit loss/error detection, fallback render, and next-frame recovery | No crash, blank batch, stale texture, or order change; recovery within one frame after resource availability. |
| Shipping | Development and release packages | Package tests and static inspection | v5 ABI unchanged; no generated artifacts; existing exports/assets behavior retained. |

The 25% and 10% thresholds are acceptance targets, not measured results. GPU
time becomes a gate only when delayed timestamp queries are available without a
per-frame fence; otherwise use host replay and total frame work.

Current metrics cover commands, primitive counts, WebGL2 instances/batches,
instrumented instanced draws, uploaded bytes, and phase timings; they omit the
Canvas composite after each WebGL2 run. Every future benchmark must also
record host allocation count/bytes, peak CPU staging-buffer bytes, peak GPU
instance-buffer bytes, CPU copy time and upload time when separable, upper
p50/upper p95 guest and host phases, small-scene (<64 instance)
total-work regression, composites, total render submissions, and draw
reduction against the per-call fallback. Atlas experiments additionally
record page count, material-key runs, texture allocation/replacement events,
and peak decoded/atlas memory. Missing metrics are unavailable, never zero.

## Staged rollout and stop conditions

1. **Document and instrument.** Keep the current Web rectangle batcher. Add
   per-run keys and metrics in a private experiment, plus deterministic tests
   for adjacent-only ordering, UV/rotation/alpha values, white-RGB parity, and
   fallback. Stop if
   the metric path changes v5 output or adds allocations per command.
2. **Web rectangles.** Generalize the existing WebGL2 rectangle buffer to the
   candidate key, add explicit context-loss/error detection with transactional
   recovery, and keep Canvas 2D fallback. Ship only if parity, recovery, and
   baseline gates pass on WebGL2 and no-WebGL2 environments.
3. **Web sprites/atlas.** Add a private atlas/material cache and 64 B sprite
   records. Keep per-image Canvas 2D fallback and invalidate cache entries on
   resource/context generation changes. Stop if atlas seams, color-space,
   filtering, or alpha parity fail.
4. **Native GL/Android experiment.** Reuse the same validated run planner but
   retain backend-specific expanded geometry until measured instancing wins.
   Android must support its minimum GLES profile; SDL remains the compatibility
   fallback. Stop if shader availability or device variance makes fallback
   less predictable than today.
5. **Optional ABI extension.** Only after two shipping cycles and benchmark
   evidence consider a versioned opaque/material stream. Require explicit
   version rejection, replay fixtures, and rollback to v5 before adoption.

Risks include transparent-order regressions, atlas bleeding, filter/color-space
drift, context loss, buffer stalls, shader/compiler variance, mobile driver
bugs, and measurements that conflate CPU submission with GPU execution. The
stop condition for any stage is a pixel/order mismatch, stale resource after
reload, crash on fallback, >10% baseline regression, or failure to meet the
recovery gate. Retain the existing path until the failing condition is fixed.

## Evidence and validation notes

Source evidence is current `gfx_cmd.stasis`, Web `game.js` and render pipeline
tests, native `stasis_graphics.c`, and Android
`StasisPreviewRenderer.java`. The measured package was produced locally with
the published Stasis executable after the repository Cargo runner hit its
machine-specific signing-certificate error; package output and browser
profiles are intentionally not committed. The focused render pipeline test
remains the deterministic regression check for 64-instance uploads, fallback,
and interleaved source order.

## Appendix: disposable benchmark reproduction

The following is the exact compact source pattern used for the two measured
fixtures. Start from a copy of `samples/swarm_field` so its vendored
`stdlib/graphics.stasis` and `stdlib/internal/gfx_cmd.stasis` are unchanged;
replace the manifest entry with one of the two files below and remove the
copied sample's original `src/main.stasis`. The manifest essentials are:

```json
{
  "manifest_version": 1,
  "name": "swarm_field",
  "entry": "src/sprite_bench.stasis",
  "tests": "tests",
  "output": "build",
  "vendor": {
    "stasis": {
      "release_id": "development",
      "sha256": "ae83fa61f536b13463ecded28234ba60ba49084ab2805f997074b6e61803cf72"
    }
  }
}
```

Use the same manifest with `entry` set to `src/mixed_bench.stasis` for the
second run. Copy this exact asset to `assets/smoke.svg` (the Stasis load call
requests a 4x4 raster from this SVG):

```svg
<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64" viewBox="0 0 64 64">
  <rect width="64" height="64" rx="12" fill="#31d17c"/>
  <path d="M16 33l10 10 22-25" fill="none" stroke="#102238" stroke-width="8"/>
</svg>
```

`src/sprite_bench.stasis`:

```text
import "/vendor/stasis/stdlib/graphics.stasis";
const SPRITE_COUNT: i32 = 4096;
global sprite: Sprite;
function main(): i32 {
    init_window(640, 360, "GPU sprite benchmark");
    if (!sprite.load_sprite_from("assets/smoke.svg", 4, 4)) {
        return 1;
    }
    return 0;
}
function tick(): i32 {
    return 0;
}
function render(): i32 {
    begin_frame();
    clear(0.015, 0.025, 0.055, 1.0);
    for (let i: i32 = 0; i < SPRITE_COUNT; i = i + 1) {
        let column: i32 = i % 128;
        let row: i32 = i / 128;
        let x: f32 = 0.0;
        let y: f32 = 0.0;
        x = i32_to_f32(column) * 5.0;
        y = i32_to_f32(row) * 5.0;
        sprite.draw(x, y, 180, i % 360);
    }
    end_frame();
    return 0;
}
```

From the copied project directory, package and serve each entry. The recorded
sprite run used the default output and port 8766 (the published executable path
is the one used for this run):

```powershell
& "D:\code\StasisLang\bin\stasis.exe" package --target web --development-build
Start-Process -WindowStyle Hidden -FilePath python -ArgumentList '-m','http.server','8766','--bind','127.0.0.1' -WorkingDirectory (Join-Path (Get-Location) 'dist\swarm_field-web')
```

The recorded mixed run used a separate output and port 8767:

```powershell
& "D:\code\StasisLang\bin\stasis.exe" package --target web --development-build --out mixed-dist
Start-Process -WindowStyle Hidden -FilePath python -ArgumentList '-m','http.server','8767','--bind','127.0.0.1' -WorkingDirectory (Join-Path (Get-Location) 'mixed-dist')
```

The
Playwright CLI skill's cached package was used through the same Chromium
binary. The collection algorithm is exact: wait until `frames >= 90` and the
expected sprite count is present, then await 120 successive
`requestAnimationFrame` callbacks and read the HUD dataset on each callback.
For each metric, sort the 120 values and use the report's non-interpolated
upper-quantile convention `Q(q) = x[floor(q * N)]`: upper p50 is index
`floor(.5 * 120)` (60), upper p95 is index `floor(.95 * 120)` (114), and
maximum is the final sorted value. These are deliberately not conventional
interpolated median/nearest-rank p95 values.

The equivalent one-line collector command is:

```powershell
$env:NODE_PATH = 'C:\Users\Ben\AppData\Local\npm-cache\_npx\31e32ef8478fbf80\node_modules'
node -e "const {chromium}=require('playwright'); (async()=>{const url=process.argv[1],expected=+process.argv[2],b=await chromium.launch({headless:true,executablePath:'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe',args:['--no-sandbox']}),p=await b.newPage({viewport:{width:1280,height:720}});await p.goto(url,{waitUntil:'networkidle'});const rows=await p.evaluate(async({expected})=>{while(Number(document.body.dataset.frames||0)<90||Number(document.body.dataset.sprites||0)!==expected)await new Promise(r=>requestAnimationFrame(r));const o=[];for(let i=0;i<120;i++){await new Promise(r=>requestAnimationFrame(r));const d=document.body.dataset;o.push({guest:+d.wasmRenderMs,browser:+d.browserReplayMs,total:+d.frameWorkMs,instances:+d.instances,batches:+d.batches,draws:+d.drawCalls,uploaded:+d.uploadedBytes,commands:+d.commands,sprites:+d.sprites,rectangles:+d.rectangles,backend:d.backend});}return o;},{expected});for(const k of ['guest','browser','total','instances','batches','draws','uploaded','commands']){const a=rows.map(x=>x[k]).filter(Number.isFinite).sort((x,y)=>x-y);console.log(k,a[60],a[114],a[119]);}console.log(rows[0].backend,rows[0].sprites,rows[0].rectangles);await b.close()})().catch(e=>{console.error(e);process.exit(1)})" http://127.0.0.1:8766/ 4096
```

Run the same command against `http://127.0.0.1:8767/` with expected sprite
count `2048`. The collector's `instances`, `batches`, and `uploaded` fields
are the current Web rectangle-instancing counters, so `-1`, `-1`, and `0` for
Canvas sprites mean unavailable/no rectangle-instance upload, not zero texture
upload, driver traffic, or memory traffic.

`src/mixed_bench.stasis`:

```text
import "/vendor/stasis/stdlib/graphics.stasis";
const SPRITE_COUNT: i32 = 2048;
global sprite: Sprite;
function main(): i32 {
    init_window(640, 360, "GPU mixed-order benchmark");
    if (!sprite.load_sprite_from("assets/smoke.svg", 4, 4)) {
        return 1;
    }
    return 0;
}
function tick(): i32 {
    return 0;
}
function render(): i32 {
    begin_frame();
    clear(0.015, 0.025, 0.055, 1.0);
    for (let i: i32 = 0; i < SPRITE_COUNT; i = i + 1) {
        let column: i32 = i % 64;
        let row: i32 = i / 64;
        let x: f32 = 0.0;
        let y: f32 = 0.0;
        x = i32_to_f32(column) * 10.0;
        y = i32_to_f32(row) * 10.0;
        let alpha: i32 = 96;
        if (i % 2 == 0) { alpha = 220; }
        sprite.draw(x, y, alpha, (i * 17) % 360);
        if (i % 2 == 0) {
            gfx_cmd_set_last_sprite_uv(0.0, 0.0, 0.5, 1.0);
        }
        fill_rect(x + 2.0, y + 2.0, 5.0, 5.0, 0.2, 0.8, 0.9, 0.35);
    }
    end_frame();
    return 0;
}
```
