# Hot-render image metadata

Stasis compiler snapshots publish versioned `hot_render_images` records for
statically identifiable `Sprite` and `SpriteSheet` loads. The AOT engine bundle
contains the same deterministic records as JIT `ProgramSnapshot`; neither form
contains pixels, textures, page coordinates, or backend object identifiers.

Each record contains the stable receiver identity, logical asset path and
geometry, `max_renders_per_render`, an optional unknown cause, eligibility, a
compatibility key, backend constraints, and an inclusion or exclusion reason. A
JSON `null` maximum means unknown. Numeric zero and one remain exact and are
standalone by default. A finite maximum greater than one is only a candidate: the
compiler's deterministic profitability screen also requires multiple compatible
images, aggregate reuse, bounded logical geometry, and enough aggregate logical
area. The reason records the exact decision.

The analysis starts at every reachable `render()` and follows shared HIR calls.
Sequential sites add, exclusive branches take the maximum, fixed loops multiply,
and fixed-capacity `foreach` loops use their declared capacity. Checked `u64` and
loop-bound arithmetic prevents wrapping. Recursion, overflow, dynamic loops,
dynamic image identity, ambiguous helper resolution, and other unprovable flow
produce unknown rather than an optimistic estimate. A recursive render-reachable
call poisons all declared images conservatively, including globals not passed as
parameters.

Loader discovery is reachability constrained, resolves compiler string/integer
constants, and accepts only stable paths, positive logical geometry, and stable
receivers. Sprite-sheet records store checked `columns * cell_width` and
`rows * cell_height` logical extents. These are not final device raster sizes.

At program publication, JIT snapshots and AOT manifests atomically replace the
runtime policy table. Both synchronous sprite loads and asynchronous `ImageAsset`
requests resolve that table by logical path and requested logical dimensions.
The asynchronous request copies the resolved policy into its task before decode,
so concurrent requests cannot consume one another's policy. A missing record,
unknown maximum, unsupported contract version, stale path/geometry match, or an
older runtime without the policy-aware request export is standalone-safe. The desktop GL loader
computes decoded/device-scaled raster dimensions after display-scale and device
limit checks, then groups by those realized dimensions plus format, sampler, and
backend constraints. Thus the same logical sheet can realize at 2048x2048,
4096x4096, or another supported extent without changing compiler metadata.

Cold images use standalone textures. Page creation, texture-limit, allocation,
or atlas upload failure falls back transactionally to a standalone texture; page
capacity naturally creates multiple compatible pages. SDL and Android retain
their existing one-texture-per-image behavior. Handles, source regions, tint,
alpha, filtering, rotation, clip/order semantics, reload publication, and
generation checks are unchanged.

Replacing metadata affects the policy lookup atomically. A cached sprite is not
duplicated merely because policy changed: its next acquisition or ordinary
reload rerasterizes the same cache entry under the accepted policy, retaining the
handle. If migration fails, the old texture and policy remain valid and the load
reports failure. Metadata publication itself never touches GPU objects.

## Representative modeled benchmark

The deterministic load model below uses eight compatible 512x512 RGBA8 images,
each rendered 64 times in asset-major order, and a 2048x2048 atlas page. It is an
exact arithmetic model, not a GPU measurement.

| Metric | Standalone baseline | Atlas candidate | Change |
| --- | ---: | ---: | ---: |
| Logical image pixels | 2,097,152 | 2,097,152 | 0 |
| GPU allocation bytes | 8,388,608 | 16,777,216 | +8,388,608 |
| Atlas occupancy | n/a | 50.0% | n/a |
| Sprite submissions | 512 | 512 | 0 |
| Worst-case texture batches/binds | 512 | 1 | -511 (99.8%) |

On this Windows worker, the reproducible ignored microbenchmarks reported:

| Measurement | Standalone fixture | Atlas fixture |
| --- | ---: | ---: |
| Median debug compiler wall time (7 fresh AOT processes) | 6.379 ms | 86.176 ms |
| Serialized metadata | 3,289 bytes | 3,585 bytes |
| Realized-load planner (10,000 iterations) | n/a | 4.246 us/plan |

Run the ignored `hot_render_compiler_microbenchmark` and
`hot_render_planner_microbenchmark` tests through `tools/cargo_cache.py` with
`-- --ignored --nocapture` to reproduce them. The compiler comparison deliberately
uses one versus 64 explicit draws per image, so it measures the representative
programs rather than isolating metadata-pass overhead. Load allocation bytes,
occupancy, and bind counts in the first table are exact modeled values.

At a 4096 maximum texture extent, realized 4096x4096 images are placed only on
4096-compatible pages and spill deterministically to additional pages. At a 2048
device limit the same 4096 realization is standalone fallback. Focused planner
tests assert both outcomes and deterministic ordering. The model makes the cost
tradeoff explicit: the compiler rejects a single hot image and sparse/small groups
because one full page can cost more memory than standalone textures.

For a GPU-capable benchmark host, use a release build and capture three runs after
warmup with `STASIS_HOST_PERFORMANCE_METRICS=1`. Record JIT/AOT compile wall time,
manifest bytes, image-load time, allocated/upload bytes, occupancy, binds, draw
batches, and median/p95 frame time. Repeat without metadata and with a deliberately
low page limit; semantic output and frame hashes must match.

This worker compiled the Rust contracts, ran the standalone C policy seam, and
syntax-checked the C renderer, but
Windows Device Guard blocked freshly linked test executables and no usable native
GL runtime build was available. Compile/load/frame-time GPU samples are therefore
reported as unavailable rather than inferred from the model.

Visual evidence: not applicable (no user-visible behavior change).

Theory gained: render frequency and logical sheet geometry are compiler facts;
realized dimensions, placement, and texture limits are runtime facts. A versioned
policy record connects them without moving GPU ownership into the compiler. The
adjacent prediction is that color-space variants can extend backend constraints
without changing the count lattice.

Good: the same HIR and snapshot record feed JIT, AOT, and runtime policy.

Bad: dynamic receiver identity deliberately loses optimization even when a human
can infer the target.

Adjustment: extend typed alias/value-flow facts before adding syntax-pattern
detectors for dynamic identities.
