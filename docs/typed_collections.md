# Typed fixed-capacity simulation collections

This document freezes the compiler contract for the nine bounded simulation
collection kinds in issue #147. A typed collection is a compiler-recognized type
application with a fixed storage descriptor. It is not a source-defined generic
struct with an array and a count, and it never allocates or resizes at runtime.

Descriptor parsing, validation, interning, fingerprinting, and layout reporting
cover all nine collection kinds. Executable direct storage covers persistent
`i32` pool, stable-pool, queue, ring-buffer, map, set, priority-queue, and grid
paths plus bitsets. The grammar is policy-free, state-dependent actions use fused
caller-owned guards, and queue/ring overwrite is an explicit operation.
Unsupported payload types fail at the production layout boundary instead of
falling back to nominal scalar storage.

The target receiver surface is illustrated here:

```stasis
global actors: pool<i32, 2>;

@requires(actors.can_push())
function add_actor(value: i32): i32 {
    return actors.push(value);
}

function tick(): i32 {
    if (actors.can_push()) {
        let actor_slot: i32 = add_actor(10);
        return actor_slot + actors.count();
    } else {
        return -1;
    }
}

global events: queue<i32, 4>;

@requires(events.can_push())
function push_event(value: i32): void {
    events.push(value);
}

function enqueue_event(value: i32): bool {
    if (events.can_push()) {
        push_event(value);
        return true;
    } else {
        return false;
    }
}

function overwrite_event(value: i32): bool {
    events.overwrite_oldest(value);
    return true;
}
```

The false arm in `enqueue_event` is the explicit drop-newest behavior. There is
no implicit overflow choice attached to the type. The parser retains ordinary
call and indexed-path syntax; compiler-owned receiver operations resolve from
the exact collection descriptor. `src/stdlib` may expose thin declarations for
future operations, but it does not define another backing container, generic
push helper, aggregate return type, or implicit `foreach` implementation.

The persistent inventory follows direct named-struct fields from global roots
and global-block fields. Generic-instantiated struct roots, fixed-array wrappers
around structs, locals, and receiver-owned lifetimes require the shared
elaborated inventory integration in a later phase; they are not silently treated
as executable typed collections by the current slice. Typed collections cannot
be passed, returned, assigned, indexed directly, or used as `foreach` sources;
compiler-owned operations are the only executable access boundary.

## Type applications and fixed layouts

The nine canonical forms are:

| Kind | Type form | Fixed lanes |
| --- | --- | --- |
| `pool` | `pool<T, N>` | `count: i32`, `values: T[N]` |
| `stable_pool` | `stable_pool<T, N>` | `count: i32`, `occupied: u8[N]`, `values: T[N]` |
| `queue` | `queue<T, N>` | `count: i32`, `head: i32`, `values: T[N]` |
| `ring_buffer` | `ring_buffer<T, N>` | `count: i32`, `head: i32`, `values: T[N]` |
| `map` | `map<K, V, N>` | `count: i32`, `occupied: u8[N]`, `keys: K[N]`, `values: V[N]` |
| `set` | `set<K, N>` | `count: i32`, `occupied: u8[N]`, `keys: K[N]` |
| `priority_queue` | `priority_queue<T, N>` | `count: i32`, `next_order: u32`, `priority: i32[N]`, `order: u32[N]`, `values: T[N]` |
| `grid` | `grid<T, W, H>` | `values: T[W*H]` |
| `bitset` | `bitset<N>` | `words: u32[ceil(N/32)]` |

There is no overflow-policy type argument. `N`, `W`, and `H` are nonnegative
decimal compile-time constants after generic elaboration. Zero is valid and
reserves metadata but no payload slots. A zero-capacity queue or ring buffer is
still a valid type; only a call to its `overwrite_oldest(value)` operation is a
compile-time error because that operation requires at least one payload slot.
Values above `i32::MAX`, negative values, unresolved names, malformed
applications, and checked dimension or byte arithmetic overflow are diagnostics.
The compiler does not clamp, truncate, or substitute a fallback capacity.

The type contains no hidden overflow mode. The side-effect-free `can_*`
preflight reports whether an expected action is currently possible. In source,
the caller chooses the consequence in the false arm of the required positive
guard. An empty false arm is the explicit drop-newest behavior; it is not a
second type identity or a hidden runtime branch. The action itself has no
second success/failure result.

`priority_queue` is a binary min-heap ordered by `(priority, insertion_order)`.
Insertion order is a compiler-owned `u32` lane. Clearing resets it to zero;
`can_push()` is false when another insertion would advance `u32::MAX`, so the
sequence never wraps.

## Payload and operation contract

The first slice accepts `i32`, `f32`, `f64`, `bool`, `u8`, `u16`, and `u32`
payload lanes. Map and set keys are integer lanes (`i32`, `u8`, `u16`, or
`u32`) with bitwise equality. Float and NaN keys are rejected. A flat
scalar-field struct may be added when the existing explicit field-wise copy
path can copy every field; this type contract introduces no aggregate copy or
aggregate return ABI. Nested arrays, views, strings, recursive structs, opaque
handles, and arbitrary generic composite copies remain unsupported.

Pool operations are `collection.push(value) -> i32`,
`collection.remove(index) -> void`, `collection.count()`,
`collection.capacity()`, and `collection.clear()`. Under its required positive
`can_push()` proof, `push` returns the new physical slot. It has no failure
sentinel. Removal is swap-removal: the removed index and any index to the moved
last value become invalid.

`collection.insert(value) -> i32` on a stable pool returns the lowest free slot
under its required positive `can_insert()` proof. It has no failure sentinel.
`collection.remove(index) -> void` leaves a tombstone without compacting any
other value. A stable index remains valid
while its `occupied` lane is set; later insertion reuses the lowest free slot.
Stable pools also expose `count()`, `capacity()`, and `clear()`.

Queue and ring operations are `push(value) -> void`, `pop() -> void`,
`peek(logical_index) -> T` for scalar payloads,
`physical_index(logical_index) -> i32`, `count()`, `capacity()`, and `clear()`.
`push`, `pop`, `peek`, and `physical_index` require their matching positive
guards. `overwrite_oldest(value)` is a separate, unguarded operation on
queue and ring-buffer receivers: with room it appends; when full it evicts the
FIFO-oldest item and appends the new value. It does not call `can_push()` and
does not require a caller-side check. It is statically rejected only for
`queue<T, 0>` and `ring_buffer<T, 0>`; those zero-capacity types remain valid for
their other operations.

FIFO logical index `j` maps to `(head + j) mod N`; `head` is normalized to zero
when empty. A physical or logical index is valid only for the current count.
The physical index helper and scalar peek are not executable for an empty or
out-of-range logical index because `can_peek(logical_index)` is false and the
required positive arm is not entered. Neither operation has an invalid-input
sentinel.

Priority-queue operations are `push(priority, value) -> void`, `pop() -> void`,
`peek() -> i32`, `peek_priority() -> i32`, `count()`, `capacity()`, and
`clear()` on the priority-queue receiver. `push` requires `can_push()`; `pop`,
`peek`, and `peek_priority` require `can_pop()` or `can_peek()` as appropriate.
Lower priorities are returned first; equal priorities retain insertion order.

The executable i32 map operations are `put(key, value) -> void`,
`get(key) -> i32`, `contains(key) -> bool`, and `remove(key) -> void` on the map
receiver. Updating an existing key remains possible when the map is full.
`get` requires `can_get(key)` and therefore has no missing-key sentinel;
`contains` is the unguarded membership query. The executable i32 set operations
are `add(key) -> void`, `contains(key) -> bool`, and `remove(key) -> void` on the
set receiver. Adding an existing key is idempotent. Both scan keys linearly,
iterate occupied slots in ascending physical order, and reuse the lowest free
slot. Removal zeros the released key and map value. Corrupt count/occupancy or
duplicate-key metadata is a fatal game error, not an ordinary false result.

Grid receiver operations are `can_access(x, y)`, `get(x, y)`,
`set(x, y, value)`, `capacity()`, and `clear()`. Grid coordinates use row-major
`y*W+x` order. `get` and `set` require the exact positive `can_access(x, y)`
proof, so an out-of-range access stays in the source-visible false arm and does
not touch storage. Bitset receiver operations are `can_access(index)`,
`test(index)`, `set(index, value)`, `capacity()`, and `clear()`. `test` and
`set` require the exact positive proof. Bit index zero is the least-significant
bit of word zero; unused tail bits are always masked to zero.

## Caller-owned proofs and fused lowering

A state-dependent operation is guard-required. The caller owns the proof and
must write the exact positive structural form, with the same receiver path in
both calls:

```stasis
@requires(actors.can_push())
function create_actor(value: i32): i32 {
    return actors.push(value);
}

if (actors.can_push()) {
    let created: i32 = create_actor(value);
    use(created);
}
```

`@requires(...)` is a function precondition satisfied by the caller-owned proof
in the enclosing direct positive `if`. It is compile-time metadata and emits no
runtime code. Required functions are initially internal, non-recursive,
non-exported, and compile-time-expanded at guarded call sites. The create-style
result (`created`) is an ordinary source value and remains usable afterward in
the same positive arm. For an operation named `insert`, `put`, or `add`, the
annotation names its matching `can_*` proof. `overwrite_oldest(value)` is the
queue/ring exception and has no proof or guard requirement.

In this first contract form, a required function body must be exactly one direct
collection action expression, or a return of that action. This keeps mandatory
expansion and proof matching explicit and entirely compile-time.

The condition must be the direct `if (receiver.can_*())` call. A cached boolean,
an alias or helper wrapper, a negated condition, a comparison with `true`, a
compound condition, or a check in a different branch does not establish the
proof. For example, `let ok = receiver.can_push(); if (ok) ...` and
`if (!receiver.can_push()) ...` are not inferred. The action must use that exact
receiver and must not be preceded by a possible mutation of it; otherwise the
compiler reports a proof error. The `@requires` proof is caller-owned; no local
helper guard is synthesized or inferred.

The proof and create/action lower as one compile-time-expanded operation. The
compiler emits one predicate or key scan and retains any free slot, existing key
slot, validated index, or queue position only as an SSA value at that call site.
There is no hidden runtime ABI argument or proof object. The proof is consumed
by the expanded create/action, which uses the SSA values for straight-line
mutation. The source-visible positive branch remains the one branch. The
lowering emits no runtime helper call, duplicate scan, second `can_*`
evaluation, hidden success/failure branch, action success value, or extra action
branch. If a required
function cannot be expanded, compilation fails. The false arm remains the
source-visible, write-free path.

Every guarded operation has a side-effect-free preflight with the same
immediate checks: pools expose `can_push()` and `can_remove(index)`; stable
pools expose `can_insert()` and `can_remove(index)`; queues and ring buffers
expose `can_push()`, `can_pop()`, and `can_peek(logical_index)`; maps expose
`can_put(key)`, `can_get(key)`, and `can_remove(key)`; sets expose `can_add(key)` and
`can_remove(key)`; priority queues expose `can_push()`, `can_pop()`, and
`can_peek()`; grids and bitsets expose `can_access(...)`. A preflight reads only
the state needed to decide the action and writes no lane. When paired with its
guarded action, it is fused rather than emitted as a separate runtime call.

Every `clear` is in place at the current simulation tick. It preserves the
collection path, descriptor identity, and capacity while zeroing metadata,
payload, keys, priorities, order, occupancy, and bit words. Removal and pop
also zero the released storage lane. There are no hidden generation or handle
lanes in this version; stale slot indexes must be revalidated by the caller.

## Exact memory and identity

The compiler lays out lanes in the listed order. Each lane is aligned to its
ABI scalar alignment, and the total size is rounded to the greatest lane
alignment. The native state widths are 1 byte for `u8`, 2 for `u16`, 4 for
`i32`, `f32`, `bool`, and `u32`, and 8 for `f64`. Thus
`pool<i32, 2>` costs 12 bytes, `queue<i32, 2>` costs 16 bytes,
`stable_pool<u16, 3>` costs 16 bytes, and `bitset<33>` costs 8 bytes. Zero-length
payload lanes contribute zero bytes but do not permit modulo-by-zero arithmetic.

The canonical identity includes state path, kind, element/key/value types,
dimensions, lane schema, and capacity. It has no overflow-policy component.
Typed collection symbols use a compiler-owned namespace distinct from ordinary
scalar and fixed-array symbols. Changing any identity component is currently a
reset-required, non-migratable layout change. Snapshot, inspection, and memory
reporting preserve each lane's physical element count, including packed bitset
word counts, without confusing it with the collection's logical capacity.

## Diagnostics and execution parity

Guard failures are ordinary source control flow, not collection errors or hidden
telemetry events. Stasis has no recoverable exception path for collection
operations. The compiler diagnoses missing, mismatched, reused, or
non-structural proofs before code generation. After a positive proof, the action
completes without another success/failure channel. Corrupt metadata, an
impossible post-proof index, or any other invariant violation is a fatal game
execution error: it terminates execution and is never translated into `false`,
`0`, `-1`, a catchable exception, or a partial write. Runtime collection code
contains only the source-visible condition, fused action, and fatal invariant
checks; it introduces no recovery branch or helper call.

JIT and linked AOT consume the same descriptor, lane order, bounds, comparator,
operation result, snapshot order, and diagnostic site table. A green acceptance
run must compare independent expected values, every metadata and payload lane,
state hashes, snapshots, and structured diagnostics. A compiler test that only
checks that a type parses is insufficient.
