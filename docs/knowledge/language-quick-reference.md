# Language quick reference

<!-- tags: syntax, types, views, generics, effects, imports -->

The [example](examples/src/feature_reference.stasis) and
[test](examples/tests/feature_reference.test.stasis) compile with this bundle.

## Storage

- Scalars: `i32`, `u8`, `u16`, `u32`, `f32`, `f64`, `bool`; return type: `void`.
- `Type[N]`, `ascii[N]`, and `utf8[N]` own bounded storage. Track live array
  occupancy with an explicit count.
- `Type[]`, `ascii[]`, `utf8[]`, and `string` are caller-backed views. Struct
  fields cannot retain views; use owned buffers for persistent text.
- Pass a destination for named-struct results; named structs and arrays of
  named structs cannot be returned by value.
- Local primitive scalar arrays are zero-initialized when their declaration
  executes. `global` owns persistent state; `let` is local; `const` is constant.

Generic parameters belong to structs; functions infer them from their first
struct parameter. See [bounded collections](bounded-collections.md).

## Expressions and calls

Use infix arithmetic, comparison, and assignment. `&&` and `||` short-circuit.
Receiver calls such as `buffer.capacity()` and `capacity(buffer)` select the
same function when resolution agrees. Use `if`/`else`, complete three-part
`for` loops, supported `foreach` sources, `continue`, and `return`.

`to_*` conversions return values; `from_*` conversions mutate the destination.
Unsigned arithmetic wraps. For strict cross-target deterministic math, use
fixed-point intrinsics; ordinary floats have no cross-architecture bit guarantee.

Import project-local modules relatively and public vendor APIs with paths such
as `/vendor/stasis/stdlib/graphics.stasis`. Keep tests in `.test.stasis` files;
see [testing](testing.md) for results and `@effects(...)`. Validate source with
`stasis fmt --check`, `stasis check`, and `stasis test`.
