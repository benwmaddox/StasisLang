# Hot-render image metadata

Stasis compiler snapshots publish versioned `hot_render_images` records for
statically identifiable `Sprite` and `SpriteSheet` loads. The AOT engine bundle
contains the same deterministic records as JIT `ProgramSnapshot`; neither form
contains pixels, textures, page coordinates, or backend object identifiers.

Each record contains the stable receiver identity, logical asset path and
geometry, `max_renders_per_render`, an optional unknown cause, eligibility, a
stable group key, group member count, aggregate logical pixel area, maximum
group extents, backend constraints, and an inclusion or exclusion reason. A
JSON `null` maximum means unknown. Numeric zero and one remain exact and are
standalone by default. A finite maximum greater than one is only a candidate: the
compiler's deterministic profitability screen also requires multiple compatible
images, repeated distinct-image transitions, and bounded logical geometry.
Aggregate logical area is page-sizing evidence rather than an eligibility gate,
so small interleaved sprites remain valuable candidates. Raw frequency is only a gate: `A A A A` already batches
as one standalone texture, while `A B A B` has avoidable texture transitions and
can benefit from one atlas. The reason records the exact decision.

The analysis starts at every reachable `render()` and follows shared HIR calls.
Its ordered flow summary preserves conservative counts, possible first/last
identities, empty paths, and weighted transitions. Sequential sites connect
possible endpoints, branches take conservative maxima and union endpoints, fixed
loops multiply internal transitions and add last-to-first iteration edges, and
fixed-capacity `foreach` loops use their declared capacity. Checked `u64` and
loop-bound arithmetic prevents wrapping. Recursion, overflow, dynamic loops,
unresolved/type-erased image identity, ambiguous helper resolution, and other
unprovable flow produce unknown rather than an optimistic estimate. A recursive render-reachable
call poisons all declared images conservatively, including globals not passed as
parameters.

Constant indexed receivers remain one identity. A dynamic indexed receiver such
as `pieces[kind]` resolves to every statically loaded `pieces[N]` element with the
same suffix. Every candidate receives the full conservative site and loop bound.
ChessTD-style `pieces[kind]` and `enemies[kind]` sites therefore form one useful
batch group when their preserved order predicts repeated transitions. An empty
finite set remains unknown and standalone-safe.

Loader discovery is reachability constrained, resolves compiler string/integer
constants, and accepts only stable paths, positive logical geometry, and stable
receivers. Sprite-sheet records store checked `columns * cell_width` and
`rows * cell_height` logical extents. These are not final device raster sizes.

At program publication, JIT snapshots and AOT manifests atomically replace the
runtime policy table. Both synchronous sprite loads and asynchronous `ImageAsset`
requests resolve that table by logical path and requested logical dimensions.
The optional v3 native exports carry eligibility, a deterministic nonzero 64-bit
group ID, member count, aggregate logical pixel area, and maximum logical group
extents. The asynchronous request copies the complete policy into its task before
decode, so interleaved requests cannot consume one another's policy. The legacy
boolean exports and a runtime without the v3 exports are standalone-safe. A
missing record, unknown maximum, unsupported contract version, or stale
path/geometry match is also standalone-safe.

The desktop GL loader computes decoded/device-scaled raster dimensions after
display-scale and device-limit checks. Page compatibility is the v3 group ID plus
the runtime-owned format, sampler, and backend constraints; equal dimensions are
not required. A page therefore packs mixed realized sizes from the same compiler
group, while distinct groups never share a page. For each new page, the runtime
scales the group sizing evidence from the current member and selects the smallest
deterministic page extent that covers the estimated group occupancy. Configured
page extents and the device texture limit are ceilings rather than allocation
requests. When the estimate exceeds those ceilings, members spill deterministically
to additional compatible pages.

Cold images use standalone textures. Page creation, texture-limit, allocation,
or atlas upload failure falls back transactionally to a standalone texture; page
capacity and fragmentation naturally create additional compatible pages. SDL and Android retain
their existing one-texture-per-image behavior. Handles, source regions, tint,
alpha, filtering, rotation, clip/order semantics, reload publication, and
generation checks are unchanged.

Replacing metadata affects the policy lookup atomically. A cached sprite is not
duplicated merely because policy changed: its next acquisition or ordinary
reload rerasterizes the same cache entry under the accepted policy, retaining the
handle. The cache migration snapshots the complete old v3 policy and reraster
state. If migration fails, the old texture, policy, and pending state remain
valid and the load reports failure. Metadata publication itself never touches
GPU objects.

## Representative modeled benchmark

The deterministic load model below uses eight compatible 512x512 RGBA8 images,
rendered 64 times in interleaved order, and a 2048x2048 atlas page. It is an
exact arithmetic model, not a GPU measurement.

| Metric | Standalone baseline | Atlas candidate | Change |
| --- | ---: | ---: | ---: |
| Logical image pixels | 2,097,152 | 2,097,152 | 0 |
| GPU allocation bytes | 8,388,608 | 16,777,216 | +8,388,608 |
| Atlas occupancy | n/a | 50.0% | n/a |
| Sprite submissions | 512 | 512 | 0 |
| Predicted texture transitions/binds | 512 | 1 | -511 (99.8%) |

On this Windows worker, the reproducible ignored microbenchmarks reported:

| Measurement | Standalone fixture | Atlas fixture |
| --- | ---: | ---: |
| Median debug compiler wall time (7 fresh AOT processes) | 92.241 ms | 85.886 ms |
| Serialized metadata | 4,361 bytes | 5,273 bytes |
| Realized-load planner (10,000 iterations) | n/a | 4.048 us/plan |

Run the ignored `hot_render_compiler_microbenchmark` and
`hot_render_planner_microbenchmark` tests through `tools/cargo_cache.py` with
`-- --ignored --nocapture` to reproduce them. The compiler comparison deliberately
uses contiguous and interleaved explicit draw sequences, so it measures the
representative programs rather than isolating metadata-pass overhead. Load allocation bytes,
occupancy, and bind counts in the first table are exact modeled values.

At a 4096 maximum texture extent, members from one group may have different
realized dimensions and still share a page when they fit. At a 2048 device limit,
a member whose padded extent exceeds the limit uses standalone fallback. Focused
planner and native policy tests assert grouping, deterministic adaptive page
sizing, capping, spill behavior, and standalone fallback. The model makes the
cost tradeoff explicit: the compiler rejects a single hot image or a group without
repeated distinct-image transitions, while runtime area evidence avoids oversized
pages for small candidates.

For a GPU-capable benchmark host, use a release build and capture three runs after
warmup with `STASIS_HOST_PERFORMANCE_METRICS=1`. Record JIT/AOT compile wall time,
manifest bytes, image-load time, allocated/upload bytes, occupancy, binds, draw
batches, and median/p95 frame time. Repeat without metadata and with a deliberately
low page limit; semantic output and frame hashes must match.

Native policy and async request tests exercise the v3 evidence, legacy-safe
fallback, request isolation, and cache migration without requiring a GPU. The GL
renderer is compiled in the desktop configuration, and the built Windows DLL is
checked for both v3 exports. Compile/load/frame-time GPU samples remain unavailable
rather than inferred from the arithmetic model.

Visual evidence: not applicable (no user-visible behavior change).

Theory gained: render frequency, stable group identity, and group sizing evidence
are compiler facts; realized dimensions, placement, page size, and texture limits
are runtime facts. A versioned policy record connects them without moving GPU
ownership into the compiler. The adjacent prediction is that color-space variants
can extend the group identity/backend constraints without changing the count
lattice.

Good: the same HIR and snapshot record feed JIT, AOT, and runtime policy.

Bad: type-erased receivers and dynamic indices without a finite statically loaded
candidate set deliberately lose optimization.

Adjustment: extend typed alias/value-flow facts if future collection views obscure
the current collection-path/suffix identity set.
