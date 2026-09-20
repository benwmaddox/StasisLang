# Typed fixed-capacity simulation collections

This document freezes the compiler contract for the nine bounded simulation
collection kinds in issue #147. A typed collection is a compiler-recognized type
application with a fixed storage descriptor. It is not a source-defined generic
struct with an array and a count, and it never allocates or resizes at runtime.

Implementation status: the compiler parses, validates, interns, fingerprints,
and reports descriptor layouts for all nine collection kinds. The production
state layout and executable operation surface currently support persistent
global `pool<i32, N, error|drop_newest>` and
`queue<i32, N, error|drop_newest|overwrite_oldest>` and
`ring_buffer<i32, N, error|drop_newest|overwrite_oldest>` values. Other
collection kinds and payload types are rejected at the production layout
boundary instead of falling back to nominal scalar storage. Pool, queue, and
ring-buffer operations lower through the same direct-storage emitter for JIT
and native AOT, and their metadata/payload lanes are exposed through program
snapshots and state inspection.

The remaining operation descriptions in this document are the normative target
contract for task #147. Stable-pool, map, set, priority-queue, grid, and bitset
operations are not executable yet. Structured overflow telemetry and migration
between changed descriptors also remain pending. The current executable slice
includes:

```stasis
global actors: pool<i32, 2, error>;

function tick(): i32 {
    let actor_slot: i32 = pool_push(actors, 10);
    return actor_slot + pool_count(actors);
}

global events: queue<i32, 4, overwrite_oldest>;

function enqueue_event(value: i32): bool {
    return queue_push(events, value);
}
```

The parser retains the ordinary call and indexed-path syntax. Current pool,
queue, and ring-buffer lowering and later collection slices resolve
compiler-owned operations by the collection descriptor. `src/stdlib` may
expose thin declarations for future operations, but it does not define another
backing container, generic push helper, aggregate return type, or implicit
`foreach` implementation.

The persistent inventory follows direct named-struct fields from global roots
and global-block fields. Generic-instantiated struct roots, fixed-array wrappers
around structs, locals, and receiver-owned lifetimes require the shared
elaborated inventory integration in a later phase; they are not silently treated
as executable typed collections by the current slice. Typed collections cannot
be passed, returned, assigned, indexed directly, or used as `foreach` sources;
compiler-owned operations are the only executable access boundary.

## Type applications and policies

The supported forms are:

| Kind | Type form | Policies | Fixed lanes |
| --- | --- | --- | --- |
| `pool` | `pool<T, N, P>` | `error`, `drop_newest` | `count: i32`, `values: T[N]` |
| `stable_pool` | `stable_pool<T, N, P>` | `error`, `drop_newest` | `count: i32`, `occupied: u8[N]`, `values: T[N]` |
| `queue` | `queue<T, N, P>` | `error`, `drop_newest`, `overwrite_oldest` | `count: i32`, `head: i32`, `values: T[N]` |
| `ring_buffer` | `ring_buffer<T, N, P>` | `error`, `drop_newest`, `overwrite_oldest` | `count: i32`, `head: i32`, `values: T[N]` |
| `map` | `map<K, V, N, P>` | `error`, `drop_newest` | `count: i32`, `occupied: u8[N]`, `keys: K[N]`, `values: V[N]` |
| `set` | `set<K, N, P>` | `error`, `drop_newest` | `count: i32`, `occupied: u8[N]`, `keys: K[N]` |
| `priority_queue` | `priority_queue<T, N, P>` | `error`, `drop_newest` | `count: i32`, `next_order: u32`, `priority: i32[N]`, `order: u32[N]`, `values: T[N]` |
| `grid` | `grid<T, W, H, P>` | `error` | `values: T[W*H]` |
| `bitset` | `bitset<N, P>` | `error` | `words: u32[ceil(N/32)]` |

`N`, `W`, and `H` are nonnegative decimal compile-time constants after generic
elaboration. Zero is valid and reserves metadata but no payload slots. Values
above `i32::MAX`, negative values, unresolved names, malformed applications, and
checked dimension or byte arithmetic overflow are diagnostics. The compiler
does not clamp, truncate, or substitute a fallback capacity.

The policy is part of the type identity and layout hash. `error` rejects a full
insertion without writing state and records a structured diagnostic.
`drop_newest` returns the operation's failure result, leaves all existing lanes
unchanged, and records bounded nonfatal telemetry. Only queues and ring buffers
support `overwrite_oldest`; a full push removes the FIFO-oldest item and appends
the new value atomically.

`priority_queue` is a binary min-heap ordered by `(priority, insertion_order)`.
Insertion order is a compiler-owned `u32` lane. Clearing resets it to zero;
attempting an insertion when it is `u32::MAX` is a runtime error with no write,
never a compile-time rejection and never a wrapping sequence.

## Payload and operation contract

The first slice accepts `i32`, `f32`, `f64`, `bool`, `u8`, `u16`, and `u32`
payload lanes. Map and set keys are integer lanes (`i32`, `u8`, `u16`, or
`u32`) with bitwise equality. Float and NaN keys are rejected. A flat
scalar-field struct may be added when the existing explicit field-wise copy
path can copy every field; this type contract introduces no aggregate copy or
aggregate return ABI. Nested arrays, views, strings, recursive structs, opaque
handles, and arbitrary generic composite copies remain unsupported.

The pool operations are `pool_push(collection, value) -> i32`,
`pool_remove(collection, index) -> bool`, `pool_count`, `pool_capacity`, and
`pool_clear`. `pool_push` returns a physical slot or `-1` for a dropped or
rejected insertion. Removal is swap-removal: the removed index and any index to
the moved last value become invalid.

`stable_pool_insert` and `stable_pool_remove` have the same result shape. A
stable index remains valid while its `occupied` lane is set; removal leaves a
tombstone and later insertion reuses the lowest free slot.

Queue and ring operations are `<kind>_push -> bool`, `<kind>_pop -> bool`,
`<kind>_peek -> T` for scalar payloads, `<kind>_physical_index -> i32`,
`<kind>_count`, `<kind>_capacity`, and `<kind>_clear`. FIFO logical index `j`
maps to `(head + j) mod N`; `head` is normalized to zero when empty. A
physical or logical index is valid only for the current count. The physical
index helper returns `-1` when empty or out of range. An empty scalar peek
returns the scalar zero value and records a bounded empty-read diagnostic; it
does not read payload storage.

Map operations are `map_put`, `map_get`, `map_contains`, and `map_remove`.
Set operations are `set_add`, `set_contains`, and `set_remove`. Both scan
integer keys linearly, iterate occupied slots in ascending physical order, and
reuse the lowest free slot. Grid access is `grid_get(x, y)`, `grid_set(x, y,
value)`, or `grid_at(x, y)` in row-major `y*W+x` order. Out-of-range grid
access is write-free. Bit index zero is the least-significant bit of word zero;
unused tail bits are always masked to zero.

Every `clear` is in place at the current simulation tick. It preserves the
collection path, descriptor identity, capacity, and policy while zeroing
metadata, payload, keys, priorities, order, occupancy, and bit words. Removal
and pop also zero the released storage lane. There are no hidden generation or
handle lanes in this version; stale slot indexes must be revalidated by the
caller.

## Exact memory and identity

The compiler lays out lanes in the listed order. Each lane is aligned to its
ABI scalar alignment, and the total size is rounded to the greatest lane
alignment. The native state widths are 1 byte for `u8`, 2 for `u16`, 4 for
`i32`, `f32`, `bool`, and `u32`, and 8 for `f64`. Thus
`pool<i32, 2, error>` costs 12 bytes, `queue<i32, 2, error>` costs 16 bytes,
`stable_pool<u16, 3, error>` costs 16 bytes, and `bitset<33, error>` costs 8
bytes. Zero-length payload lanes contribute zero bytes but do not permit
modulo-by-zero arithmetic.

The canonical identity includes state path, kind, policy, element/key/value
types, dimensions, lane schema, and capacity. Typed collection symbols use a
compiler-owned namespace distinct from ordinary scalar and fixed-array symbols.
Changing any identity component is incompatible for state migration. Growing a
descriptor copies active lanes and zero-initializes the tail. Shrinking with
live elements, or changing kind, policy, dimensions, key type, or value type,
rejects before writing the active state.

## Diagnostics and execution parity

Each lowered operation carries a compiler-generated site record containing its
operation id, state path, policy, function and symbol identity, source file,
range, and debug source offset. Source text cannot provide a caller label.
Overflow and empty-read records use the additive
`stasis.collection.overflow.v1` envelope with state path, collection kind,
operation, policy, capacity, simulation tick, caller identity, peak usage,
requested capacity, current memory bytes, suggested memory bytes, and suggested
increase. The suggestion is the smallest representable capacity above the
observed peak, or the first valid grid dimension increase; no suggestion is
reported when the type's checked limit is exhausted.

Telemetry is fixed-size compiler-owned data outside simulation state. It may be
shown by inspection, but it does not affect replay hashes or snapshots. The
simulation execution boundary supplies an explicit `u64` tick to both JIT and
linked native AOT and clears it after the tick. Wall time, network time, and
host labels are not used.

JIT and linked AOT consume the same descriptor, lane order, bounds, comparator,
overflow result, snapshot order, and diagnostic site table. A green acceptance
run must compare independent expected values, every metadata and payload lane,
state hashes, snapshots, and structured diagnostics. A compiler test that only
checks that a type parses is insufficient.
