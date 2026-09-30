# Bounded collections and generics

<!-- tags: collections, generics, capacity, storage, overflow -->

Stasis storage has explicit maximum sizes. Choose the representation based on
the identity and overflow rule your game needs. The
[compiled example](examples/src/feature_reference.stasis) and
[test](examples/tests/feature_reference.test.stasis) show a generic owned array
and a queue.

## Owned array and generic policy

Use `Type[N]` with an explicit count for a simple fixed roster. A generic
struct can parameterize its element type and compile-time capacity; each
concrete application has fixed storage. A function gets `T` and `N` through
its first struct parameter, not through explicit call-site type arguments:

```stasis
function capacity(self: ScoreBuffer<T, N>): i32 {
    return N;
}
```

`Type[]` is a borrowed view. Generic struct fields can own `T[N]`, but cannot
retain borrowed views. Fixed arrays do not gain an automatic live `.length`;
keep a count in the owner and check it before indexing. See
[bounded storage](a-little-stasis/05-bounded-storage-is-policy.md) for a manual
slot-reuse policy.

## Compiler-owned containers

The supported type families include `pool`, `stable_pool`, `queue`,
`ring_buffer`, `map`, `set`, `priority_queue`, `grid`, and `bitset`. These are
fixed-layout compiler-owned types, not heap containers. Their supported
payloads and operations differ; use `stasis check` against the selected
toolchain. Current direct executable payloads are scalar lanes, with integer
map/set keys. Do not assume an arbitrary struct, string, or view can be stored.

A queue makes a full-capacity decision visible at the call site:

```stasis
function enqueue_score(value: i32): bool {
    if (recent_scores.can_push()) {
        recent_scores.push(value);
        return true;
    }
    return false;
}
```

The `false` branch is this game's drop-newest policy. Queue/ring overwrite is
available as an explicit operation; overflow policy is not inferred from the
type. Other state-dependent actions likewise require their matching `can_*`
guard. A pool's returned slot and a stable pool's occupied slot have different
reuse implications: choose based on whether external references must remain
stable. `priority_queue` selects the smallest priority, breaking ties by
insertion order.

Use `stasis inspect` to see total direct storage and the largest pools before
raising capacities. `stasis inspect --capacity state.enemies=512` projects a
change without editing source. Test full, empty, and removal/reuse boundaries.
