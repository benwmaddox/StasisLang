# Maddox #655: Wasm bounds-check evidence

## Change

The Wasm backend reuses an index proof only for a direct indexed source that resolves through the fixed `context.struct_collections` collection path, with no field suffix. That exact branch emits an unsigned bounds check against the collection's fixed length before constructing the view. The check proves the source index is nonnegative and below the length, so the copy loop can load every field without repeating the same signed range checks.

The proof helper mirrors `encode_struct_view_expr` resolution. Local named-struct-array bindings and receiver-derived collections are excluded: their guards validate a relative index, while the view start can be dynamic and `start + index` can overflow or refer outside the underlying collection. Identifier and forwarded views, suffixed paths, and other mutable aliases also retain field-level checks. Fixed global struct collections use separate field planes, but the selected collection binding supplies the shared, fixed element index domain; no generalized proof is claimed for dynamic or aliased views.

Source evaluation and its original bounds guard remain before the first copy store. The target bounds guard remains before stores. The tests cover self-copy aliasing, negative and high source indices, zero-length sources, and negative/high target indices; after every trap, the destination fields remain unchanged.

## Binaryen comparison

The production-shaped release probe used a three-field named struct collection and a fixed `i32[16]` loop. `main`, `render(index)`, and `tick()` were emitted roots, so the dynamic source read and copy were reachable. The before and after modules were passed through pinned Binaryen v132 with `wasm-opt -Oz`, then disassembled with `wasm-dis`.

The wasm-dis guard count includes `i32.ge_u`, `i32.lt_s`, and `i32.ge_s` comparisons across the complete module:

| Stage | Before | After |
|---|---:|---:|
| Raw Wasm guard comparisons | 30 | 24 |
| `wasm-opt -Oz` guard comparisons | 18 | 12 |

The compiler removes six repeated comparisons, two for each of the three copied fields. Binaryen leaves those six checks in the pre-change module.

The pre-change optimized WAT repeated these signed guards before each source-field load:

    (if (i32.lt_s (local.get $0) (i32.const 0)) (then (unreachable)))
    (if (i32.ge_s (local.get $0) (i32.const 3)) (then (unreachable)))
    (i32.load ...)

After the change, the original unsigned source-index guard remains, while the repeated field guards are gone:

    (if (i32.lt_s (local.get $0) (i32.const 0)) (then (unreachable)))
    (i32.load ...)

The `i32.lt_s` shown in the after snippet is the scalar-versus-array discriminator; it is not one of the removed source-view range guards. The initial unsigned source-index check remains in both outputs.

| Module | Before raw | After raw | Before optimized | After optimized |
|---|---:|---:|---:|---:|
| Wasm bytes | 2,478 | 2,424 | 1,997 | 1,942 |
| Raw DEFLATE bytes, level 9 | 978 | 972 | 853 | 843 |
| ZIP archive bytes, `game.wasm`, DEFLATE level 9 | 1,094 | 1,088 | 969 | 959 |

Sizes use the raw module and a one-file archive. Binaryen was invoked as:

    D:\code\Tools\binaryen-version_132\bin\wasm-opt.exe -Oz input.wasm -o output.wasm

The temporary size-probe WAT showed that the reductions remained after optimization; the small byte reductions are consistent with that disassembly. Compression is included as a packaging-size observation, not a runtime metric.

## Runtime sample

A bounded Node/V8 sample used the before and after optimized modules generated from the focused three-field copy fixture. Each module was warmed with 200,000 `render(1)` calls; then five alternating samples ran 2,000,000 calls each in the same Node process. Each call returned the same digest, which the harness accumulated and checked.

| Module | Samples (ms) | Median (ms) |
|---|---|---:|
| Before | 12.31, 11.72, 14.86, 14.10, 16.26 | 14.10 |
| After | 14.45, 11.40, 15.34, 13.35, 12.97 | 13.35 |

The after median was 5.3% lower in this run. The sample ranges overlap substantially and vary by roughly 20%; this is directional evidence only, not a stable performance guarantee or a benchmark across hosts. The byte and WAT evidence is more repeatable than these short timing samples.

## Execution checks

The focused Node oracle verifies:

- A valid source copy updates all three fields, and self-copy remains valid.
- Source indices `-1`, `3`, `4`, `INT_MAX`, and `INT_MIN` trap.
- The zero-length source branch traps before writes.
- Negative and high destination indices trap before writes.
- The destination remains unchanged after every trap.

The optimized probe was also executed before and after `wasm-opt -Oz`; both produced:

    44,120,60,120,true,60,true,60,true,60

Existing zero-extent view tests pass, including the JIT/AOT/Wasm parity test. The runtime sample used only successful fixed-global copies; trap behavior is covered by the separate oracle above.

## Other backend audit

Cranelift was inspected read-only. The shared emitter already carries a `bounds_proven` property on struct views and suppresses repeated field checks after a checked array-element view is formed. JIT and AOT tests cover checked struct views and out-of-bounds literals. No Cranelift changes were needed. The Wasm change does not alter local, receiver, identifier, or forwarded-view checks.

## Work summary

- Visual evidence: not applicable; this is compiler-generated Wasm with no user-visible rendering change.
- Theory gained: a fixed global source's unsigned element check proves its index for each field load in that one copy.
- Good: reuse of a local proof removes code without adding an interprocedural or range-analysis pass.
- Bad: the first probe's roots were incorrect, and dead-code elimination made its output unsuitable; the final size probe used reachable roots.
- Adjustment: performance probes must verify exports and reachability after compilation before interpreting optimized output.
