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

This is research code only. `crates/stasis_dynload/src/cross_atlas_research.rs`
is compiled only by the default-off `cross-atlas-research` Cargo feature. The
module contains plain owned/value data and deterministic planning: no loaded
library, JIT function pointer, renderer handle, or platform API. `stasis_dynload`
also ships as `rlib`, `cdylib`, and `staticlib`, so this is an incubation seam for
one future standard renderer-core planner shared by JIT, linked AOT, Android, and
Web hosts. No production wiring, render ABI version, capacity, game rule, or art
changed.

This Rust placement resolves PR #628's language-ownership review: portable
runtime policy belongs in Rust and only unavoidable backend GPU calls belong in
C/platform shims. The measurement example uses `std::time::Instant::elapsed`,
then bounds the `u128` nanosecond result before conversion. It never multiplies
an absolute Windows QPC counter by one billion, resolving the overflow review.

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

`CrossAtlasInstance` uses `#[repr(C)]` and is compile-time asserted to exactly
80 bytes:

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

## Atlas construction policy

Choose co-residency primarily by interleaving affinity, not by asset name, load
order, or dimensions alone. The implementable policy is a deterministic weighted
graph followed by capacity-constrained clustering:

1. Create one node per compatible realized sprite. Reject edges across format,
   sampler, color-space, backend, or other binding-domain constraints.
2. Weight each edge by predicted plus observed adjacent transitions between its
   two sprites. Add a smaller deterministic co-occurrence weight when both appear
   in the same frame but are not adjacent. Predicted evidence comes from the
   compiler's ordered-flow analysis; observed evidence may be a bounded,
   versioned runtime histogram. Task #335 v3 currently provides stable groups and
   conservative transition evidence; a future per-pair table can refine edges
   without changing this policy.
3. Process edges by descending weight, with stable resource identity as the tie
   break. Merge clusters only when the realized members still fit one concrete
   atlas/array binding domain. Treat the highest-affinity clusters as protected:
   a lower-weight merge cannot evict a member, force an extra page, or cut a
   heavier transition edge.
4. After the protected clusters are placed, opportunistically fill reasonable
   spare capacity with format/sampler-compatible sprites. Fill candidates use
   descending residual affinity, then stable identity. A filler is accepted only
   when it uses already allocated capacity and does not displace, repack, or split
   a protected cluster. Otherwise it remains standalone or starts its own domain
   only when its independent evidence justifies that allocation.
5. Publish the complete assignment transactionally for one renderer generation.
   Record total transition weight, weight cut by domain boundaries, occupancy,
   spill count, allocation bytes, and migration overlap. A failed construction or
   migration retains the prior complete assignment.

This is intentionally a policy, not an atlas-packer implementation. Rectangle
placement still needs a deterministic padding-aware packer, and texture arrays
still require compatible allocated layer dimensions. The existing research
planner begins after assignment: it consumes realized `binding_domain_id` values
and proves the ordered draw consequences. Its fixtures show the objective. An
interleaved eight-resource group drops from 4,096 binds to one when co-resident;
asset-major order starts at only eight binds; deterministic two-domain spill cuts
the same high-affinity sequence into 1,024 ordered domain runs. Therefore scarce
capacity should protect interleaved affinity first, while opportunistic fill is a
secondary occupancy optimization.

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

## Reproducible fixtures and measurements

Each fixture has 4,096 sprites and eight resources. The interleaved fixture is
`A B C D E F G H` repeated inside binding domain 1. The asset-major fixture has
512 consecutive uses of each resource in the same domain. The spill fixture
retains interleaved order but resources A-D realize in domain 1 and E-H in domain
2. Geometry, UV, scale, tint, and order otherwise match.

| Fixture | Baseline binds/draws | Bindless | Mega/array | WebGL2 array (1,024 capacity) | Conventional adjacent |
| --- | ---: | ---: | ---: | ---: | ---: |
| Interleaved, one domain | 4,096 / 4,096 | 1 / 1 | 1 / 1 | 1 / 4 | 4,096 / 4,096 |
| Asset-major, one domain | 8 / 4,096 | 1 / 1 | 1 / 1 | 1 / 4 | 8 / 8 |
| Interleaved, two-domain spill | 4,096 / 4,096 | 1 / 1 | 1,024 / 1,024 | 1,024 / 1,024 | 4,096 / 4,096 |

These are exact planner counters, not GPU measurements. They make the Task #335
selector concrete: repeated distinct transitions create the largest avoidable
bind/draw surface; asset-major order is already cheap for adjacent conventional
batching; deterministic page spill reintroduces every ordered domain transition.
Bindless remains one domain-independent run, but no current backend claim follows
from that model.

The Rust release example recorded 31 samples of 1,000 plans per fixture/profile.
Representative mega-atlas p50/p95 planner times were 38.173/38.633 us
(interleaved), 38.670/39.226 us (asset-major), and 39.299/40.376 us (spill).
Raw invocation-order samples and the complete exact modeled matrix are checked in
at `docs/evidence/task-341-cross-atlas-submission/raw_measurements.json`.
No Android device, browser GPU, queue timestamp, GPU memory, frame-time, energy,
or mobile thermal measurement was collected. `null` means unavailable.

Reproduce with repository-owned Cargo caching:

```text
python tools/cargo_cache.py run -- cargo test -p stasis_dynload --features cross-atlas-research
python tools/cargo_cache.py run -- cargo run -p stasis_dynload --features cross-atlas-research --example cross_atlas_benchmark --release
```

Capture stdout and compare fixture/profile identities and exact modeled counters.
CPU timing samples are expected to vary; recompute quantiles by the documented
method rather than requiring byte-identical times.

## Correctness evidence

The deterministic Rust contract covers exact size and key offsets; transparent
overlap with intentionally non-monotonic order; crop, rotation, pivot,
scale/flip, tint/alpha preservation; clip/material/blend/filter/pass boundaries;
alternating resources in one domain; array and mega-atlas domain changes;
bindless cross-domain behavior; capacity overflow; unsupported-feature fallback;
injected upload/device failure; insufficient output; and the safe maximum count.
The fallback tests require zero exposed prototype runs and baseline-equivalent
counters, preventing double draw and prefix reorder.
Compile-level trait checks require the input/profile/run/counter records to be
`Copy + Send + Sync + 'static`, and a handle-free plan is constructed from owned
numeric identities. This is the portability boundary for JIT, linked AOT,
Android bridge, and future Web consumers.

## Staged adoption recommendation

1. Keep the default-off feature as incubation only. After production ABI design
   review, promote the record and planner into the standard shared renderer-core
   path consumed by both JIT and linked AOT hosts; do not fork planner semantics.
2. Consume merged Task #335 v3 group keys, transition evidence, and sizing data to
   prototype the affinity-first clustering policy and mega-atlas placement behind
   a runtime capability flag. Fill spare capacity only after high-affinity groups
   are protected.
   Assign a runtime binding-domain ID per realized page/array object; never treat
   the compiler group key as proof that one draw can bind every spilled page. Add
   frame hashes, seam/filter/color-space tests, allocation/peak-memory counters,
   and forced-loss recovery.
3. Wire AOT only after its host can populate the same 80-byte values and resolved
   domain IDs without dynamic-library handles. Require compile/link checks for
   `rlib`/`staticlib`, identical JIT/AOT plan traces, generation-safe resource
   publication, and complete transactional fallback before changing any ABI.
4. Prototype Android GLES and WebGL2 texture arrays with queried limits. Gate on
   exact-order traces, no visual mismatch, bounded peak memory, and measured
   device/browser p95 improvement. Several ordered draws are an acceptable result.
5. Keep SDL and Canvas conventional. Consider a new desktop GPU backend only if
   device measurements justify bindless/descriptor complexity.
6. Promote backend-specific GPU shims independently. Never make one backend's literal-one-draw
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
