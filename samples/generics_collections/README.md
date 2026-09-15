# Generic bounded collections

This sample is a small, allocation-free consumer of Stasis compile-time
generic parameters. Run it from a checkout with:

```text
stasis --workspace samples/generics_collections vendor status
stasis --workspace samples/generics_collections fmt --check
stasis --workspace samples/generics_collections check
stasis --workspace samples/generics_collections test
stasis --workspace samples/generics_collections run --headless --ticks 1
stasis --workspace samples/generics_collections package --target desktop --development-build
stasis --workspace samples/generics_collections package --target web --development-build
stasis --workspace samples/generics_collections package-mobile --target android-arm64 --out build/android-arm64 --development-build
```

The production compiler seam checks the canonical `src/main.stasis` entry
through JIT, linked AOT, raw Wasm, and the packaged Web runtime. All targets
execute the same generic collection workload and expose the same deterministic
post-run state digest through `tick()`. The Web acceptance also verifies the
real browser's WebGL2 frame and the fixed-array bounds trap. Android uses this
same full entry and valid frame lifecycle.
`vendor/stasis` is the recorded, hash-checked graphics/runtime snapshot used
by every packaged target.

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

Scalar `i32` and `f32` elements support assignment and return through
receiver-bound helpers. `Entity` is accessed through indexed field paths and an
`Entity[]` view; the sample deliberately does not claim a general deep-copy
operation for arbitrary composite `T`. The concrete `first_value(i32[])`
demonstrates the existing fixed-array-to-view rule without introducing a raw
view-only generic.

`Nested<T, N>` demonstrates a legal generic container containing another
generic container. The two integer buffers use capacities 4 and 8 and are
mutated independently. `CompileValue<N>` binds the compile-time value used by
`apply_offset` and `apply_mode`, demonstrating that an `i32` generic value is
not inherently a capacity. `buffer_capacity` is exercised in both free and
receiver-style call paths; function calls never carry explicit generic
arguments.

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
