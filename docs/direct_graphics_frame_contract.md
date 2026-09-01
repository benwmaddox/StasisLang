# Direct graphics frame contract

Version 7 makes the canonical graphics frame the only guest-owned sprite
representation. A guest reserves one bounded sprite run, writes semantic
instances directly into the compiler/runtime-owned `gfx_cmd_i32` and
`gfx_cmd_f32` lanes, and finalizes the run once. Finalization publishes one run
header and one order entry. Reservation, cancellation, and failed finalization
do not change published frame counts or order.

## Ownership boundary

Guest data describes what to draw: a logical image handle, logical source crop,
destination geometry, pivot, scale/flip, rotation, tint, feature flags, and
shared material/blend/filter/pass intent. Physical atlas pages, normalized
packed UVs, binding domains, resource generations, GPU handles and records,
batch construction, and fallback policy remain host-private. Hosts may merge
adjacent compatible semantic runs, but must preserve painter order.

## Version 7 layout

All lanes use native `i32`/`f32` alignment. A sprite instance is exactly 64
bytes: three i32 fields followed by thirteen f32 fields.

| Lane | Offset | Meaning |
| --- | ---: | --- |
| i32 | 0 | logical image handle (nonzero; signed encodings are valid) |
| i32 | 1 | packed tint RGBA8 (`0xRRGGBBAA`) |
| i32 | 2 | negotiated instance/feature flags |
| f32 | 0..3 | destination x, y, width, height |
| f32 | 4..7 | logical-image source x, y, width, height |
| f32 | 8..9 | explicit destination-local pivot x, y |
| f32 | 10..11 | scale x, y; negative values express flips |
| f32 | 12 | clockwise rotation in degrees |

The canonical capacity is 4,096 instances. Source width and height may be zero
only as the documented full-logical-image sentinel used by legacy single-sprite
helpers; otherwise source and destination dimensions are finite and positive.
Pivot, scale, rotation, source coordinates, and destination coordinates must be
finite. Scale components must be non-zero. Tint and feature flags are semantic
values and never encode a physical resource.

A run header is eight i32 fields (32 bytes): first instance, count, clip id,
material, blend, filter, pass, and run flags. There are at most 4,096 runs. Clip
id `-1` means the ordered clip stack owns clipping. These fields are negotiated;
version 7 accepts only zero, meaning normal material, source-over blend, normal
filtering, default pass, and order-dependent transparent replay. Hosts reject
reserved nonzero values transactionally until enum-specific behavior is
versioned. Instance feature flags follow the same rule. Each sprite order entry
references one run header, never an individual instance.

The v7 canonical capacities and bases are:

| Item | Value |
| --- | ---: |
| i32 count | 67,888 |
| f32 count | 146,564 |
| u8 count | 65,536 |
| sprite i32 base/stride | 32 / 3 |
| sprite f32 base/stride | 80,004 / 13 |
| text i32 base/stride | 12,320 / 3 |
| sprite-run i32 base/stride | 18,464 / 8 |
| order i32 base | 51,232 |
| text f32 base/stride | 133,252 / 6 |
| clip f32 base/stride | 145,540 / 4 |

## Writer lifecycle

Only one unfinished sprite writer may exist. `reserve` checks instance, run,
and order capacity together before returning a generation-scoped token. A
failed reservation changes no published frame state. Writes are checked,
sequential direct stores; an out-of-range or stale-token write fails without
advancing the writer. `finalize(actual_count)` accepts only
`0 <= actual_count <= written_count`, publishes one bounded run and order entry,
and closes the token. `cancel` closes it without publishing. A frame begin or
hot-generation change invalidates an outstanding token. Stale tokens,
double-finalize, malformed counts, and overflow fail deterministically.

Writer values are frame-local capabilities: applications must not place them in
globals or persistent structs, return them, or retain them across render/code
swap generations. Runtime generation checks remain authoritative even where a
compiler cannot prove non-escape. Writer helpers carry the `graphics` effect;
that effect composes normally with explicitly declared application render-state
effects.

## Compatibility and migration

`gfx_cmd_sprite` and the public `Sprite`/`SpriteSheet` helpers are implemented
through the writer. Sprite sheets emit logical pixel crops, not normalized UVs.
The scratch-array `gfx_cmd_sprites_from`/`gfx_draw_sprites` path and raw sprite
count/set APIs are removed in v7: callers reserve, write, and finalize instead.
There is no second authoritative sprite representation and no guest
scratch-to-frame copy. Hosts may still repack semantic instances into private
GPU records when atlas placement or the graphics API requires it.

Image decode, atlas placement, and the private white-texel upload are resource
preparation work performed on load or when the resource, density, or graphics
context generation changes. They are not guest frame-construction work. A
dynamic render still writes its semantic instances into the canonical frame on
each render; moving transforms therefore pay direct stores plus the host's
private record repack/upload. The benchmark's JavaScript typed-array allocation
is measurement scaffolding and does not model the runtime's persistent global
canonical lanes. A game may cache its own immutable semantic inputs, but v7 does
not expose a sealed persistent frame/list or a safe patch-in-place slot API.
One-time static-list construction and transform-only patch/replay remain the
non-forgeable persistent-reference work tracked for #399; no such optimization
is assumed by this contract or its evidence.

Hosts validate an entire frame before replay. Invalid run spans, order
references, handles, flags, shared state, or non-finite/invalid geometry reject
the frame transactionally; a consumer must retain its prior valid frame rather
than partially adopting malformed data.

The order stream is declarative correctness data, not an application batching
schedule. A run may contain A-B-A-B logical images when host-private placement
puts them in one compatible binding domain; logical handle transitions alone do
not split it. Capable hosts also translate solid rectangles into their private
ordered quad representation and merge them with adjacent compatible sprites.
The WebGL2 host reserves a private white texel per atlas binding domain and uses
the existing 64-byte quad record with per-instance tint; this consumes no guest
handle and exposes no atlas identity. Canvas, SDL, and unsupported paths replay
the identical semantic order conventionally. Hosts still split on real clip,
blend, pass, material/shader, binding-domain, or bounded-capacity differences,
and never globally sort transparent work.
