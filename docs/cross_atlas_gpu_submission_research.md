# Cross-atlas GPU submission research

## Decision

Stasis can make every scene-active image GPU-resident and can build one ordered
80-byte-per-sprite frame upload. It cannot portably issue one literal draw across
arbitrary textures on every current backend. Adopt the portable record and
instrumentation first, then prototype texture arrays on Android GLES and WebGL2.
Keep native SDL and Canvas on conventional adjacent batching. A modern native GPU
backend may later use bindless descriptors; the legacy GL 2.1 conformance path
must not define that contract.

"One cross-atlas submission" must therefore mean one CPU frame upload and one
queue submission where supported, with one or more ordered draws. It means one
literal draw only when all instances share pass, clip, material, blend/filter,
capacity, and a binding domain visible to one shader invocation.

This is research code only. `runtime/stasis_cross_atlas_prototype.*` is linked
only into its opt-in test and measurement targets. No production renderer, game
rule, render ABI version, capacity, or art changed.

## Current behavior and prior work

The shipping desktop/mobile path is SDL. Each SDL sprite call applies modulation
and calls `SDL_RenderTextureRotated`; SDL exposes no portable cross-texture
instance binding. The legacy native GL adapter expands every sprite to six
vertices, flushes on sprite-atlas page or vertex capacity, and binds one page for
each run. Android expands adjacent compatible sprites to GLES triangles. Current
Web code packs adjacent compatible sprites into 64-byte WebGL2 records when their
resources resolve to the same private atlas page, then issues one instanced draw
and one Canvas composite per page-local run. Page changes split; short, oversize,
atlas-full, initialization-error, draw-error, and context-loss cases replay in
ordered Canvas `drawImage` calls.

Task #271 (`docs/gpu_instancing_report.md`) established the key invariants and
records the state at its measurement date:
preserve source order for source-over alpha, batch adjacent compatible entries,
put rotation/UV/alpha in instance data, and split on material, page, sampler,
blend, clip, pass, or capacity. Its measured Web rectangle path handled 8,196
instances with one 262,272-byte upload and one instanced draw plus one Canvas
composite. Its 4,096-sprite Canvas sample reported 4,096 draws and 16.5/17.5/28.5
ms browser replay p50/p95/max. Sprite instance/batch/upload metrics on that
historical Canvas path were unavailable, not zero. The current Web test contract
now additionally proves same-page cross-handle batching, page-boundary splits,
64-byte sprite uploads, and whole-run Canvas fallback without partial GPU
submission.

Merged Task #335 (`docs/hot_render_metadata.md`, v3) addresses a different stage.
Compiler snapshots and AOT manifests publish a stable group key, conservative
render-frequency and distinct-transition evidence, aggregate logical pixel area,
maximum group extents, and backend constraints. The runtime owns decoded/device
dimensions, placement, pages, formats, sampler compatibility, and transactional
fallback. Equal realized dimensions are not required for one atlas page: mixed
sizes may share it when the v3 group plus runtime format/sampler/backend constraints
match. Page limits, capacity, and fragmentation can still spill one stable group
across multiple page objects. The merged policy can reduce `A B A B` binds when
compatible images land on one page, but it leaves SDL and Android one texture per
image and does not itself reduce sprite submission count. This proposal consumes
the merged grouping/sizing evidence; it does not duplicate compiler analysis.

## Portable frame record

`StasisCrossAtlasInstance` is compile-time asserted to exactly 80 bytes:

| Bytes | Field | Meaning |
| ---: | --- | --- |
| 0..15 | `destination[4]` | x, y, width, height |
| 16..31 | `uv_crop[4]` | normalized crop rectangle |
| 32..39 | `pivot[2]` | rotation pivot |
| 40..47 | `scale[2]` | scale; negative values preserve flips |
| 48..51 | `rotation` | backend-neutral angle value |
| 52..63 | tint, resource, order | packed RGBA, stable resource identity, source order |
| 64..71 | clip, binding domain, material | clip ID; atlas/array object ID; material ID |
| 72..75 | blend, filter, pass, flags | compact state selectors |
| 76..79 | `feature_flags` | explicitly negotiated feature requirements |

The prototype never sorts or rewrites records. Its trace hashes source order and
resource identity, and every run is a contiguous `(first_instance,count)` span.
Destination, crop, pivot, scale/flip, rotation, tint/alpha, binding-domain/resource,
clip, material, and state survive byte-for-byte. `resource_id` selects the logical
image or layer. `binding_domain_id` identifies the concrete texture-array or
mega-atlas object that one draw binds; it is not merely the compiler group key.

## Run and fallback rules

Adjacent instances split, in deterministic precedence order, on capacity, pass,
clip, material, blend/filter, then binding compatibility. Conventional profiles
split when either texture resource or binding-domain identity changes. Mega-atlas
and texture-array profiles may cross resource identity only while
`binding_domain_id` stays equal; distinct atlas textures or array objects split.
Bindless profiles alone may cross binding-domain identity because the shader-visible
descriptor is per instance. Pass and clip changes remain draws: they alter render
target or fixed-function state and cannot be hidden in instance data without
changing semantics.

The planner rejects counts above `UINT32_MAX / 80`, zero draw capacity, missing
buffers, unsupported feature bits, injected upload failure, and insufficient run
output capacity. Unsupported features and upload/device failure transactionally
select the complete conventional baseline before any prototype run is exposed.
The fallback has zero prototype runs, so a caller cannot submit a prefix and then
redraw it or reorder the remainder. Context-loss integration must follow
`docs/renderer_resource_lifecycle.md`: retain CPU sources, rebuild every active
generation, withhold partial frames, and publish only a complete successful
generation.

## Residency and memory lifecycle

"All scene-active" must be an explicit bounded working set, not every project
asset. Decode and stage the selected set, allocate compatible atlas/array groups,
upload them, and atomically publish stable resource/binding-domain IDs for a renderer
generation. Eviction cannot occur during an accepted frame. A budget miss, format
or dimension mismatch, texture-layer/page limit, allocation failure, upload
failure, or generation change keeps the old complete generation or chooses the
conventional frame. Peak memory must include retained CPU source bytes, decoded
staging, destination GPU storage, and migration overlap.

Array layers require equal allocated dimensions and compatible format/mip/sampler
rules. A mega-atlas allows mixed image dimensions but needs padding and UV inset
to prevent filtering bleed. Bindless keeps separate allocations but requires
descriptor residency and a backend beyond the current legacy native GL path.

## Backend capability matrix

| Backend profile | Resident cross-image binding | Achievable submission | Unavoidable qualification |
| --- | --- | --- | --- |
| Modern desktop native (future) | Bindless/descriptor array, or mega-atlas | Literal one draw for bindless or one mega-atlas domain; several draws for state/domain/capacity | Current SDL and legacy GL 2.1 do not provide this portable contract; add only in a new backend after capability query |
| Legacy native GL | Existing atlas page | One upload plus several page/state draws | Each page object is a binding domain; page, clip/pass/material, and capacity split |
| SDL Renderer | Backend-owned individual textures | Conventional adjacent batching | No portable shader-visible array/bindless resource selector |
| Android GLES 3 | `sampler2DArray` for compatible layers, or mega-atlas | Literal one draw for one binding-domain/state run; several draws across objects/capacity | Runtime layer/size limits, equal array-layer allocation, format/sampler, domain, clip/pass |
| WebGL2 | `sampler2DArray` for compatible layers, or mega-atlas | One upload plus one or several domain/state draws; one browser/frame submission is possible | `MAX_ARRAY_TEXTURE_LAYERS`, array/atlas object changes, buffer limits, state; multi-draw is not a core assumption |
| Canvas 2D | Individual image sources | Conventional ordered `drawImage` calls | No GPU instance buffer, shader resource index, or literal cross-image draw |

Multi-draw can reduce API calls where an optional backend extension exists, but
it remains multiple draws and must not be called one literal draw. No current
production backend is changed or claimed to support it here.

## Reproducible fixture and measurements

The fixture constructs 4,096 sprites in exact source order, alternating eight
resource IDs that are layers/regions inside binding domain 1. Geometry, UV,
scale, tint, and order are populated. Four
profiles use identical input: desktop bindless (capacity 65,535), Android array
(4,096), WebGL2 array (1,024), and Canvas conventional (4,096). Baseline and
prototype call/byte/bind/draw/submission counters are exact planner-model outputs.
They are not driver counters. The baseline models one record preparation/upload
call and one draw per sprite; it is a comparison model, not a claim that SDL
uploads an 80-byte hardware record today.

On the recorded Windows x64 MSVC host, 31 samples each ran 1,000 planner calls.
The reported sample is integer nanoseconds per call. Samples are sorted only for
nearest-rank quantiles: p50 index 15 and p95 index 29. Raw invocation-order
samples are checked in at
`docs/evidence/task-341-cross-atlas-submission/raw_measurements.json`.

| Profile | Planner p50 / p95 | Prototype upload | Binds | Draws | Queue submissions | GPU/frame time |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| Desktop bindless model | 43.394 / 43.968 us | 1 / 327,680 B | 1 | 1 | 1 | unavailable |
| Android array model | 44.630 / 45.040 us | 1 / 327,680 B | 1 | 1 | 1 | unavailable |
| WebGL2 array model | 44.686 / 45.242 us | 1 / 327,680 B | 1 | 4 | 1 | unavailable |
| Canvas conventional model | 45.483 / 46.020 us | 4,096 / 327,680 B | 4,096 | 4,096 | unavailable | unavailable |

The profile names are modeled capability configurations executed by the same
Windows CPU binary. No Android device, browser GPU, queue timestamp, GPU memory,
frame-time, energy, or mobile thermal measurement was collected. `null` means
unavailable; none of those values is inferred as zero.

Reproduce from a Visual Studio developer shell:

```text
cmake -S runtime -B .build/task-341-cross-atlas -G Ninja \
  -DSTASIS_CROSS_ATLAS_RESEARCH_TESTS=ON \
  -DSTASIS_GRAPHICS_BUILD_SHARED=OFF -DSTASIS_GRAPHICS_BUILD_STATIC=OFF \
  -DSTASIS_BUILD_RUNNER=OFF -DSTASIS_BUILD_SYS=OFF \
  -DSTASIS_BUILD_MOBILE_RUNTIME=OFF
cmake --build .build/task-341-cross-atlas
ctest --test-dir .build/task-341-cross-atlas --output-on-failure
.build/task-341-cross-atlas/stasis_cross_atlas_measurement_fixture.exe
```

Capture stdout and compare fixture/profile identities and exact modeled counters.
CPU timing samples are expected to vary; recompute quantiles by the documented
method rather than requiring byte-identical times.

## Correctness evidence

The deterministic C contract covers exact size and key offsets; transparent
overlap with intentionally non-monotonic order; crop, rotation, pivot,
scale/flip, tint/alpha preservation; clip/material/blend/filter/pass boundaries;
alternating resources in one domain; array and mega-atlas domain changes;
bindless cross-domain behavior; capacity overflow; unsupported-feature fallback;
injected upload/device failure; insufficient output; and the safe maximum count.
The fallback tests require zero exposed prototype runs and baseline-equivalent
counters, preventing double draw and prefix reorder.

## Staged adoption recommendation

1. Land only instrumentation and the versioned portable frame record after a
   production ABI design review; keep the prototype isolated until then.
2. Consume merged Task #335 v3 group keys, transition evidence, and sizing data to
   prototype residency and mega-atlas placement behind a runtime capability flag.
   Assign a runtime binding-domain ID per realized page/array object; never treat
   the compiler group key as proof that one draw can bind every spilled page. Add
   frame hashes, seam/filter/color-space tests, allocation/peak-memory counters,
   and forced-loss recovery.
3. Prototype Android GLES and WebGL2 texture arrays with queried limits. Gate on
   exact-order traces, no visual mismatch, bounded peak memory, and measured
   device/browser p95 improvement. Several ordered draws are an acceptable result.
4. Keep SDL and Canvas conventional. Consider a new desktop GPU backend only if
   device measurements justify bindless/descriptor complexity.
5. Promote a backend independently. Never make one backend's literal-one-draw
   ability a portable semantic promise.

The stop conditions are any transparent-order change, atlas seam or color-space
change, incomplete context-loss frame, unbounded active-set memory, small-scene
regression above the agreed budget, or device p95 without material improvement.

Visual evidence: not applicable (research-only code and documentation).

Theory gained: cross-atlas batching is two independent problems: resource
residency chooses which resource identities a shader can see, while the ordered
frame planner chooses contiguous state-compatible runs. Task #335 informs the
first; Task #271 constrains the second. The adjacent prediction is that a future
material variant adds a run boundary without changing instance ordering or
residency publication.

Good: one fixed input record and deterministic split trace make backend claims
comparable without pretending modeled counters are GPU measurements.

Bad: this worker had no mobile/browser GPU timing or memory telemetry, so the
economics of array padding and migration overlap remain unknown.

Adjustment: collect device-side p50/p95, peak GPU/staging memory, context-loss,
and seam/color-space evidence before enabling any production backend.
