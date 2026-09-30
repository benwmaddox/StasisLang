# Language quick reference

<!-- tags: syntax, types, views, generics, effects, imports -->

This is a project-local orientation to the supported Stasis surface, not a
replacement for compiler diagnostics. Start with the [compiled example](examples/src/feature_reference.stasis)
and its [test](examples/tests/feature_reference.test.stasis). The source and
compiler shipped with this vendor snapshot define the exact available forms.

## Storage and types

`i32`, `u8`, `u16`, `u32`, `f32`, `f64`, and `bool` are scalar types; `void` is a
return type. `Type[N]`, `ascii[N]`, and `utf8[N]` own bounded storage. `Type[]`,
`ascii[]`, and `utf8[]` are views of caller-owned storage. A fixed array's
capacity does not imply a live element count: store and check that count
explicitly. Struct fields cannot retain borrowed views. A named struct or an
array of named structs cannot be returned by value; pass a destination to a
function instead. `string` is a UTF-8 string view; use owned `utf8[N]` when
state must retain bytes. Function-local primitive scalar arrays are
zero-initialized each time their declaration executes.

The example owns its values in a global generic struct:

```stasis
struct ScoreBuffer<T: type, N: i32> {
    count: i32;
    values: T[N];
}
```

Type and value parameters are declared on the struct. A function may infer
them from its first concrete struct parameter. There is no runtime type value,
heap allocation, or function-owned `function f<T>` syntax. See
[bounded collections](bounded-collections.md) before choosing a container.

## Expressions, calls, and control flow

Arithmetic, comparisons, and assignment use infix operators. `&&` and `||`
short-circuit. Calls may be written as `buffer.capacity()` or
`capacity(buffer)` when the receiver resolves to the same function. `let`
introduces a local, `global` owns persistent simulation state, and `const`
names a compile-time value. Use `if`/`else`, complete three-part `for` loops,
`foreach` where the source type supports it, `continue`, and `return`.

Imports resolve from the current module or from a project-root path such as
`/vendor/stasis/stdlib/graphics.stasis`. Use the latter for public vendor APIs.
Put tests in `.test.stasis` files. The [testing guide](testing.md) shows
parameterless test declarations and narrow `@effects(...)` contracts.

Conversions are explicit. `to_*` helpers return values; `from_*` helpers mutate
the destination. Unsigned arithmetic wraps within its width. Ordinary floating
point arithmetic is not promised bit-identical across architectures; use the
fixed-point intrinsics when strict cross-target replay math is required.

Run `stasis fmt --check`, `stasis check`, and `stasis test` on the project after
changing source. `check` catches unsupported combinations rather than silently
providing a substitute implementation.
