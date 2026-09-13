# Generic bounded collections

This sample is a small, allocation-free consumer of Stasis compile-time
generic parameters. Run it from a checkout with:

```text
stasis --workspace samples/generics_collections prepare
stasis --workspace samples/generics_collections fmt --check
stasis --workspace samples/generics_collections check
stasis --workspace samples/generics_collections test
stasis --workspace samples/generics_collections run --headless --ticks 1
```

The production compiler seam also checks the entry point through JIT, AOT
object generation, and the scalar `wasm_entry.stasis` through WebAssembly
execution. The full entry intentionally keeps receiver-owned struct-array
coverage in the JIT/AOT path because the current Wasm contract rejects that
backend-specific shape; the standalone Wasm fixture keeps the scalar generic
contract covered without hiding that boundary.

```text
python tools/cargo_cache.py run -- cargo test -p stasis_compiler --test generics_collections_jit_aot_wasm -- --nocapture
```

## Storage contract

`Buffer<T, N>`, `Ring<T, N>`, `SampleWindow<N>`, `EntityPool<N>`, and
`Rig2D<N>` own a fixed `T[N]` or struct-array field. `N` is a non-negative,
compile-time `i32`; zero-capacity arrays have no usable element slots and
follow the existing fixed-array extent policy. The full bounded layout is
reserved even when `count` is zero. None of these helpers allocates, resizes,
or exposes a runtime length.

Counts are explicit and are the only logical occupancy. `buffer_clear`,
`window_clear`, `ring_clear`, and `entity_pool_clear` reset reuse state without
erasing the backing layout. Appends and pushes reject full collections without
changing the count. `Ring` wraps `head` and its write slot modulo `N` and
rejects empty drops. `EntityPool` reuses the last released slot and only copies
scalar fields, which is the supported struct-view shape in this sample.

Scalar `i32` and `f32` elements support assignment and return through generic
helpers. `Entity` is accessed through indexed field paths and an `Entity[]`
view; the sample deliberately does not claim a general deep-copy operation for
arbitrary composite `T`. `first_value<T>(T[])` accepts fixed arrays of
different capacities through the existing fixed-array-to-view rule.

`Nested<T, N>` demonstrates a legal generic container containing another
generic container. The two integer buffers use capacities 4 and 8 and are
mutated independently. `apply_offset<N>` and `apply_mode<N>` demonstrate that
an `i32` generic value is a compile-time parameter rather than inherently a
capacity. `buffer_capacity` is exercised with both inferred parameters and the
explicit `::<f32, 3>` form.

`live_edit_helper.stasis` is intentionally tiny: `live_edit_tick` increments
`live_edit_state` before calling the helper. During a live session, change only
the helper return expression and re-run the live-edit command; the next result
changes while the incremented `live_edit_state` remains. The generic collection
layouts are not changed by that helper edit.

The intentionally failing sources are under `negative/`, outside the normal
project `tests/` directory. They are compiled by the focused Rust harness and
cover runtime capacity arguments, unsupported composite element copies,
unresolved fixed-capacity inference, checked layout overflow, and ambiguous
generic receivers. They must not be included in the green sample test run.
