# Generics and compile-time value parameters

Generics are supported for bounded fixed-storage programs. A generic parameter
is resolved during compilation and produces an ordinary concrete Stasis type or
function specialization; it is not a runtime type value and it does not add a
heap or dynamic-dispatch path.

The language contract is also summarized in [the specification](spec.md#422-generic-types-and-compile-time-value-parameters). The executable reference is
[`samples/generics_collections`](../samples/generics_collections/README.md).

## Supported contract

Only structs declare type parameters (`T: type`) and compile-time signed 32-bit
value parameters (`N: i32`). A function receives those names from the concrete
generic struct application in its first parameter:

```stasis
struct Buffer<T: type, N: i32> {
    count: i32;
    values: T[N];
}

function capacity(buffer: Buffer<T, N>): i32 {
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

The first argument is the only source of function generic bindings. Free and
receiver calls are equivalent and select the same specialization:

```stasis
let free_result: i32 = capacity(samples);
let method_result: i32 = samples.capacity();
```

Function-owned declarations such as `function capacity<T: type>(...)` and
explicit calls such as `capacity<T>(...)` or `capacity::<T>(...)` are migration
errors. A placeholder used only by a later parameter, return type, body, or raw
view is also rejected. Introduce a nominal generic struct in parameter one when
a reusable compile-time policy is required.

Specializations have nominal identity based on the defining declaration and
the canonical, ordered arguments. Equal constant expressions identify the same
specialization; different capacities or element types do not. Generic structs
use the existing concrete layout machinery, including fixed-array headers,
alignment, SoA field paths, and state-inspection metadata.

Generated symbols use the reserved `__stasis_type_`, `__stasis_function_`, and
`__stasis_const_` namespaces. Source structs, enums, globals, global blocks,
constants, externs, and functions may not use those prefixes. Other
compiler-owned families, including test wrappers, remain distinct. The
compiler retains the full canonical specialization key while elaborating and
treats a generated-symbol reuse for a different key as a deterministic
collision. Identity uses the canonical project-relative source path with `/`
separators, the defining declaration, and canonical ordered type/value
arguments; file insertion order, formatting, call spelling, and equivalent
free/receiver qualification do not change it. Generic placeholder names are
alpha-renamed by their ordered declaration slots: renaming `T` to `U` (and
updating its receiver-bound uses) does not create a new concrete function or
struct specialization. Equivalent project-relative and qualified generic- or
ordinary-struct type spellings resolve to the same defining declaration before
specialization identity is computed.

The existing ownership and view rules remain authoritative:

- `T[N]` owns its statically declared storage; it has no runtime length,
  allocation, resize, or implicit copy operation.
- `T[]` is a view and can accept fixed arrays with different capacities after
  `T` has been bound by parameter one's nominal generic struct. A raw `T[]`
  parameter cannot introduce a function generic.
- A struct or element parameter is a view of caller-backed storage. Generic
  syntax never creates a hidden owning temporary or deep copy.
- Struct fields may own fixed-capacity storage such as `T[N]`, but may not
  store borrowed views such as `T[]` or `string`. This is checked on ordinary
  declarations and again after generic substitution.
- Functions and externs may not return named structs or arrays of named
  structs. Pass the destination as a non-owning struct parameter and return a
  scalar or `void`; the same rule applies after generic substitution.
- Operations on `T` are checked after substitution. Generic code cannot invent
  arithmetic, conversions, memcpy, or field access that the concrete type does
  not already support.
- Generic declarations do not add array `.length`; explicit `count` fields
  remain the logical occupancy for bounded collections.

## Resource ceilings and counting

Generic elaboration constructs all rewritten sources before publishing any of
them. A reusable raw `Compiler` retains the newly authored candidate text after
a failed check but never publishes a partially expanded source set. JIT, native
AOT, and Wasm retain their last accepted program snapshot and artifacts when
elaboration fails; the development hot-swap backend also leaves its prepared
candidate and commit queue unchanged. A corrected candidate can be retried on
the same process.

The supported ceilings are inclusive:

| Resource | Ceiling | Counting convention |
| --- | ---: | --- |
| Project source | 16 MiB | Sum of original UTF-8 source bytes presented to the shared frontend. The guard is applied before specialization, so it is also the compiler's source-input ceiling for a project with no generic declarations. |
| Specializations | 4,096 | Distinct concrete generic struct specializations plus distinct receiver-bound concrete function specializations. Reuses of the same canonical key count once. Standalone/function-owned generic roots are rejected and never consume this budget. |
| Instantiation depth | 128 | A directly requested specialization is depth 0; each specialization first discovered while materializing it is parent depth plus 1. Depth 128 is accepted and 129 is rejected. |
| Constant evaluation | 10,000 steps | Each parsed primary or unary prefix consumes one step. A top-level constant dependency graph shares one counter for the compilation; an independently evaluated generic argument starts its own counter. Checked binary arithmetic does not add a second step beyond its operands. |
| Constant dependencies | 128 active definitions | The active dependency chain is bounded before recursion. Self and mutual cycles fail with a stable cycle diagnostic. |
| Type IDs | 65,536 entries | IDs are unsigned 16-bit values `0..=65535`, including builtins. The next distinct type is rejected without mutating the table. |
| Static layout | `u32` bytes | Header, payload multiplication, and final addition are checked independently. Zero-capacity fixed arrays are valid; any byte-size overflow is rejected. |
| Expanded source | 16 MiB | Sum of UTF-8 bytes after concrete declarations and receiver-bound functions are emitted. Every substitution and replacement pass preflights or incrementally checks its output before allocation, and materialized function bodies are charged before they are retained. |
| Elaboration work | 32 Mi work units | `input bytes + retained materialized function bytes + final expanded bytes + 1,024 * specialization count`, using checked arithmetic. This deterministic proxy bounds simultaneous source rewriting and specialization fan-out. |

The exact neighbor fixtures exercise specialization counts 4,095/4,096/4,097,
depths 127/128/129, and both standalone and shared-compilation evaluation
counts 9,999/10,000/10,001. Separate fixtures cover TypeId exhaustion, the last
fitting and first overflowing `u32` layout, zero capacity, constant cycles,
finite/mutual/expanding recursion, generated-name spoofing, injected hash
collisions, large-body multiplication, the combined 32 Mi work formula, and
cross-backend plus prepared-hot-swap rollback/retry.

## Backend and packaging contract

The compiler specializes once and sends the concrete result through the normal
backend paths. JIT, native AOT, and Wasm support bounded named-struct-array
views for internal calls, including fixed-to-view conversion, receiver-owned
fields, forwarded views, indexed element calls, field mutation, `foreach`, and
`max_length`. The sample uses the hash-checked `vendor/stasis` snapshot for
every packaged target. Web, Android, and native packaging all compile the
canonical `src/main.stasis` entry and use the same renderer lifecycle.

| Surface | Executable coverage | Result required for a green run |
| --- | --- | --- |
| Syntax, receiver binding, free/dot calls, scalar storage, nesting, bounds | `generics_collections_jit_aot_wasm::generic_collection_sample_tests_pass_in_the_production_jit_shape`; the two sample tests | Both tests return true. |
| Generic expansion and graphics provenance | `generics_collections_jit_aot_wasm::generic_collection_aot_accepts_vendor_graphics_after_expansion` | Generic constant rewriting does not invalidate the compiler-owned vendor graphics module. |
| Negative diagnostics | `generics_collections_jit_aot_wasm::negative_generic_collection_fixtures_keep_expected_diagnostics` | Runtime capacities, unresolved values, layout overflow, ambiguous receivers, and unsupported composite copies fail with stable diagnostics. |
| JIT, AOT, and Wasm parity | `generics_collections_jit_aot_wasm::backend_neutral_generics_oracle_matches_jit_aot_and_wasm` | JIT and Node/Wasm observe the independent digest; Windows also links and executes the AOT object. Their program snapshots agree on concrete function identities, capacities, alignments, and SoA field planes. |
| Receiver and forwarded named-struct views | `generics_collections_jit_aot_wasm::indexed_named_struct_elements_resolve_local_receiver_and_qualified_calls`; Wasm backend receiver-view tests | Internal indexed calls, whole-element field-wise copies, owner isolation, `foreach`, and `max_length` execute against caller-backed storage. |
| Incremental identity and code selection | `backend::jit::tests::generic_body_edits_rejit_specializations_and_callers_but_reuse_unrelated_code`; `generic_constant_and_helper_edits_invalidate_exact_dependency_closure`; `generic_signature_and_overload_edits_have_exact_identity_effects`; `removed_and_new_generic_specializations_preserve_unaffected_identity`; `equivalent_generic_argument_spelling_reuses_specialization_and_callers`; `alpha_renamed_qualified_receiver_generic_reuses_identity_and_layout` | Body, referenced constant/helper, signature, overload-set, and specialization-set edits rebuild exactly their affected closure. Equivalent constants, alpha-renamed receiver slots, qualified struct spellings, and unrelated artifacts retain identity and code. |
| Nested and SoA state transactions | `generic_hot_swap_state` | Two differently sized owners plus a nested owner preserve every scalar and field plane through compatible growth; compile, type, effect, layout, and hook rejection preserve the active executable and state before a valid retry. Evidence includes artifact counters and deterministic state digests. |
| Filesystem and editor transactions | `tests::notify_watch_service_reloads_imported_generic_source_through_real_jit_commit`; `live_workspace::tests::imported_generic_dirty_buffer_preview_apply_and_rejection_are_transactional` | A physical imported-file event and editor preview/apply both stage a real JIT candidate. Rejected editor candidates leave disk, code, and state unchanged. |
| Packaged Web | `apps/stasis/tests/generics_collections_package.rs`; `tools/run_generics_collections_browser_acceptance.mjs` | The package selects `src/main.stasis`; Node observes the full workload result/digest and bounds traps; a real browser presents the authored frame without runtime errors. |
| Android AOT link | The `android-package-link` slow CI lane and the generated `android` Gradle project | The generic arm64 package links `libmain.so`; its bundle and link map agree. The x86_64 development APK is the emulator lane and must present valid frames. |
| Host coverage | `generics-cross-platform` CI matrix on Ubuntu, Windows, and macOS; Windows bootstrap additionally runs the native AOT seam | The shared JIT/Wasm/diagnostic contract passes on all three hosts. |

Hot swap is a development-JIT transaction only. Native AOT, Web, Android, and
iOS release packages remain immutable; parity for those packages means they
consume the same canonical specialization identities and concrete layouts at
build time.

## Deliberate boundaries

The following are not part of the current supported contract: function-owned
generic declarations, explicit generic function calls, generic enums, aliases,
defaults, variadic or higher-kinded parameters, traits, runtime value arguments,
non-`i32` value parameters, symbolic equation solving, or arbitrary
composite-element copying. Lifecycle and host entries remain concrete.

Named-struct returns and stored views are rejected by the shared frontend, so
JIT, native AOT, Wasm, lifecycle declarations, host externs, and generated
wrappers observe one contract and one diagnostic category. Web packaging has
no alternate entry: the internal Wasm named-struct-array view representation
executes the full sample directly.

## Acceptance matrix

| Dimension | Concrete fixture or rule | Green evidence |
| --- | --- | --- |
| Type/value kind and arity | `Buffer<T, N>`, first-parameter binding, and the negative kind/arity fixtures | Parser and semantic diagnostics in the generic harness. |
| Constant evaluation | Equal expressions, negative offsets, overflow, zero division, and extent checks | `negative_generic_collection_fixtures_keep_expected_diagnostics`. |
| Identity and layout | `Buffer<i32, 4>`, `Buffer<i32, 8>`, `Buffer<f32, 3>`, `Nested<i32, 2>`, and `Rig2D<24>/<64>` | Published AOT symbols/layout plus the JIT test workload. |
| Resource neighbors | 4,095/4,096/4,097 specializations, depth 127/128/129, 9,999/10,000/10,001 evaluation steps, TypeId and layout endpoints | Non-ignored `frontend::generics::tests` and `frontend::types::tests` fixtures enforce every boundary in normal compiler CI. |
| Transactional failure | Expanding recursion, large receiver-body multiplication, and combined-work overflow | Expanding recursion has matching check/JIT/AOT/Wasm diagnostics and successful retry; large-body and combined-work fixtures prove all-or-nothing expanded-source publication; the prepared JIT path preserves its accepted snapshot/package and queues no commit. |
| Binding and views | Free/dot receiver binding, nominal generic first parameters, and rejection of raw-view-only generics | Production JIT sample test and ambiguous/unresolved negatives. |
| Bounds and occupancy | Full/empty append, ring wrap/drop, pool release, explicit counts, and fixed capacity | The two `.test.stasis` tests and native AOT/Wasm entry results. |
| Effects and name lookup | Definition-site imports and existing receiver/view effect checking after substitution | Generic compilation uses the ordinary semantic checker; failures are not erased by specialization. |
| Incremental/swap | Helper edit preserving live state; capacity/layout change migration and rejected-candidate retry | `live_edit_helper.stasis` sample test and `development_swap` unit test. |
| Tooling and distribution | Vendor status/hash, formatter, `check`, `test`, desktop package, Web package, Android arm64/x86_64 package, and emulator launch | Sample README commands, packaged-Web integration test, Android slow lane, and recorded local package/emulator run. |

`cargo fmt --all -- --check`, the focused generic tests, the sample formatter,
and the repository's applicable ABI/CI policy checks are required before a
generic release change is published.

Visual evidence: not applicable to this compiler and packaging contract.

## Tooling contract

The CLI, live workspace, and Android Workshop bridge expose the same
compiler-owned generic metadata. Source-item responses include the generic
struct or first-parameter-bound function's `generic_parameters`, stable
`symbol_id`, defining `file`, signature, source spans, and source hash.
Reference responses retain the referenced symbol, containing item, defining
file, and the exact `source_span`; Android adds no transport-side source
reparsing. The live `:symbols` and `:references` commands use the same
project-relative paths and byte offsets as the CLI and Workshop JSON APIs.

Compiler failures carry a stable `code`, primary `path`/`start`/`end`/`symbol`,
and any related template or call-site locations. The Android native v1
diagnostic envelope projects those fields as additive `primary` and `related`
objects while retaining the legacy file and line markers. Existing consumers
may continue to read the legacy fields; new consumers should use the typed
locations.

The supported tooling surface is deliberately narrow: generic structs with
`T: type` and/or `N: i32`, and functions whose first parameter binds those
parameters through the nominal struct. Standalone function generic
declarations, method-only generic declarations, and explicit generic calls
are not supported. Both `capacity<T>(value)` and turbofish forms such as
`capacity::<T>(value)` (including qualified calls) produce the exact
`stasis.explicitGenericCall` diagnostic at the callee name and include the
generic template declaration as a related location. Completion and hover are
also intentionally suppressed inside an explicit generic call.

Semantic updates require the source hash returned by source-item discovery
when a caller supplies an expected hash. A stale hash is rejected before any
file is written; if later validation or receipt creation fails, the bridge
and live workspace restore the prior sources and runtime candidate. Correcting
the source or hash allows the same request to be retried. These parity tests
run on the host and do not require a connected Android device.

## Migration from function-owned generics

Move each function's generic list to its receiver struct and remove explicit
call arguments:

```stasis
// Before
function append<T: type, N: i32>(buffer: Buffer<T, N>, value: T): bool { ... }
append::<i32, 8>(items, value);

// Current
function append(buffer: Buffer<T, N>, value: T): bool { ... }
append(items, value);
```

If the old function had no nominal generic first parameter, introduce a small
policy struct only when the compile-time distinction is part of the domain.
Otherwise make the function concrete. The compatibility parser recognizes old
declarations and calls only to report this migration; it never executes them.
