# Bounded collections and generics

<!-- tags: collections, generics, capacity, storage, overflow -->

Use `Type[N]` with an explicit count for a fixed roster. A generic struct can
own `T[N]`; functions bind `T` and `N` through their first struct parameter.
`Type[]` borrows storage and cannot be retained in struct fields. See the
[example](examples/src/feature_reference.stasis) and
[capacity test](examples/tests/feature_reference.test.stasis).

Compiler-owned families are `pool`, `stable_pool`, `queue`, `ring_buffer`,
`map`, `set`, `priority_queue`, `grid`, and `bitset`. Current direct payloads
are scalar lanes, with integer map/set keys. Check the selected toolchain
before using another payload type.

Choose overflow behavior at the call site:

```stasis
function enqueue_score(value: i32): bool {
    if (recent_scores.can_push()) {
        recent_scores.push(value);
        return true;
    }
    return false;
}
```

This queue drops the newest value when full. Queue/ring overwrite is an
explicit operation; state-dependent actions require their matching `can_*`
guard. Choose pool identity/reuse rules deliberately. `priority_queue` selects
the smallest priority and breaks ties by insertion order.

Use `stasis inspect` before raising capacities. Test empty, full, removal, and
reuse boundaries; see [bounded storage](a-little-stasis/05-bounded-storage-is-policy.md).
