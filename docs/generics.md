# Generics and compile-time value parameters

Generics are supported for bounded fixed-storage programs. A generic parameter
is resolved during compilation and produces an ordinary concrete Stasis type or
function specialization; it is not a runtime type value and it does not add a
heap or dynamic-dispatch path.

The language contract is also summarized in [the specification](spec.md#422-generic-types-and-compile-time-value-parameters). The executable reference is
[`samples/generics_collections`](../samples/generics_collections/README.md).

## Supported contract

Generic structs and functions may declare type parameters (`T: type`) and
compile-time signed 32-bit value parameters (`N: i32`):

```stasis
struct Buffer<T: type, N: i32> {
    count: i32;
    values: T[N];
}

function capacity<T: type, N: i32>(self: Buffer<T, N>): i32 {
    return N;
}

global samples: Buffer<f32, 128>;
```

Type arguments must be concrete supported types. Value arguments are constant
expressions made from literals, named constants, enclosing generic values,
parentheses, unary signs, and checked `+`, `-`, `*`, `/`, and `%` operations.
They are evaluated as signed `i32`; overflow, division by zero, and invalid
array extents are diagnostics. A value parameter is not inherently a capacity:
negative values are valid when used as offsets or modes, while a substituted
array extent must still satisfy the ordinary fixed-array policy.

Inferred calls are preferred:

```stasis
let result: i32 = samples.capacity();
```

When inference is not possible, a complete explicit argument list uses the
unambiguous `::<...>` marker:

```stasis
let result: i32 = capacity::<f32, 128>(samples);
```

Specializations have nominal identity based on the defining declaration and
the canonical, ordered arguments. Equal constant expressions identify the same
specialization; different capacities or element types do not. Generic structs
use the existing concrete layout machinery, including fixed-array headers,
alignment, SoA field paths, and state-inspection metadata.

The existing ownership and view rules remain authoritative:

- `T[N]` owns its statically declared storage; it has no runtime length,
  allocation, resize, or implicit copy operation.
- `T[]` is a view and can accept fixed arrays with different capacities, but it
  cannot infer a capacity parameter.
- A struct or element parameter is a view of caller-backed storage. Generic
  syntax never creates a hidden owning temporary or deep copy.
- Operations on `T` are checked after substitution. Generic code cannot invent
  arithmetic, conversions, memcpy, or field access that the concrete type does
  not already support.
- Generic declarations do not add array `.length`; explicit `count` fields
  remain the logical occupancy for bounded collections.

## Backend and packaging contract

The compiler specializes once and sends the concrete result through the normal
backend paths. The full collection workload is intentionally kept as a native
JIT/AOT fixture because the current Wasm backend does not support the
receiver-owned `Entity[]` view used by the entity-pool portion. The sample uses
the hash-checked `vendor/stasis` snapshot for native and mobile packaging. Its
`stasis.json` selects `src/wasm_entry.stasis` for Web packaging; that scalar
fixture exercises the same generic collection contract and executes in the
produced `game.wasm`. Android uses the same full entry and renderer lifecycle
as native packaging.

| Surface | Executable coverage | Result required for a green run |
| --- | --- | --- |
| Syntax, inference, explicit calls, scalar storage, nesting, bounds | `generics_collections_jit_aot_wasm::generic_collection_sample_tests_pass_in_the_production_jit_shape`; the two sample tests | Both tests return true. |
| Generic expansion and graphics provenance | `generics_collections_jit_aot_wasm::generic_collection_aot_accepts_vendor_graphics_after_expansion` | Generic constant rewriting does not invalidate the compiler-owned vendor graphics module. |
| Negative diagnostics | `generics_collections_jit_aot_wasm::negative_generic_collection_fixtures_keep_expected_diagnostics` | Runtime capacities, unresolved values, layout overflow, ambiguous receivers, and unsupported composite copies fail with stable diagnostics. |
| JIT and Wasm execution | `generics_collections_jit_aot_wasm::generic_collection_scalar_fixture_executes_in_wasm` | The module has a valid Wasm header and Node observes `main() == 0`. |
| Linked native AOT | `generics_collections_aot_seam::generic_collection_sample_links_and_runs_in_aot` on Windows | The linked executable observes `main() == 0`; the existing unsigned-host policy may skip only when Application Control returns 4551 and signing is not required. |
| Incremental state and swap | `development_swap::tests::generic_collection_capacity_swap_migrates_state_and_allows_retry_after_rejection` | Compatible state migrates and a rejected candidate leaves the active state retryable. |
| Packaged Web | `apps/stasis/tests/generics_collections_package.rs` | `package --target web --development-build` emits `game.wasm`, and Node executes its scalar entry with result 0. |
| Android AOT link | The `android-package-link` slow CI lane and the generated `android` Gradle project | The generic arm64 package links `libmain.so`; its bundle and link map agree. The x86_64 development APK is the emulator lane and must present valid frames. |
| Host coverage | `generics-cross-platform` CI matrix on Ubuntu, Windows, and macOS; Windows bootstrap additionally runs the native AOT seam | The shared JIT/Wasm/diagnostic contract passes on all three hosts. |

## Deliberate boundaries

The following are not part of the current supported contract: generic enums,
aliases, defaults, variadic or higher-kinded parameters, traits, runtime value
arguments, non-`i32` value parameters, symbolic equation solving for inference,
or arbitrary composite-element copying. Generic functions must be concrete at
lifecycle and host boundaries; use a concrete wrapper when one is required.

Web packaging must use the sample's scalar `wasm_entry.stasis` until the
receiver-owned struct-array view is supported by that backend. This is a
backend boundary, not a fallback execution mode: unsupported generic shapes
must remain compile-time errors.

## Acceptance matrix

| Dimension | Concrete fixture or rule | Green evidence |
| --- | --- | --- |
| Type/value kind and arity | `Buffer<T, N>`, explicit `::<f32, 3>`, and the negative kind/arity fixtures | Parser and semantic diagnostics in the generic harness. |
| Constant evaluation | Equal expressions, negative offsets, overflow, zero division, and extent checks | `negative_generic_collection_fixtures_keep_expected_diagnostics`. |
| Identity and layout | `Buffer<i32, 4>`, `Buffer<i32, 8>`, `Buffer<f32, 3>`, `Nested<i32, 2>`, and `Rig2D<24>/<64>` | Published AOT symbols/layout plus the JIT test workload. |
| Inference and views | Receiver inference, explicit calls, `first_value<T>(T[])`, and capacity non-inference from views | Production JIT sample test and ambiguous/unresolved negatives. |
| Bounds and occupancy | Full/empty append, ring wrap/drop, pool release, explicit counts, and fixed capacity | The two `.test.stasis` tests and native AOT/Wasm entry results. |
| Effects and name lookup | Definition-site imports and existing receiver/view effect checking after substitution | Generic compilation uses the ordinary semantic checker; failures are not erased by specialization. |
| Incremental/swap | Helper edit preserving live state; capacity/layout change migration and rejected-candidate retry | `live_edit_helper.stasis` sample test and `development_swap` unit test. |
| Tooling and distribution | Vendor status/hash, formatter, `check`, `test`, desktop package, Web package, Android arm64/x86_64 package, and emulator launch | Sample README commands, packaged-Web integration test, Android slow lane, and recorded local package/emulator run. |

`cargo fmt --all -- --check`, the focused generic tests, the sample formatter,
and the repository's applicable ABI/CI policy checks are required before a
generic release change is published.

Visual evidence: not applicable to this compiler and packaging contract.
