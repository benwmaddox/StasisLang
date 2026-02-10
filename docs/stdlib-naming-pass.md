# Stdlib Naming Pass (Receiver-Scoped APIs)

This pass makes stdlib call sites shorter and removes legacy alias names.

## Why this is scoped

Stasis now disallows arity-based overloading for the same function name. That means very generic names like `clear(...)` can collide across modules if they use different parameter counts.

Because of that, this pass focuses on:

- concise names that are still namespace-safe
- first-parameter overloads where the receiver type differs
- migration to a single canonical name per operation

## New preferred names

### `src/stdlib/stdlib.stasis`

- `ascii_push(dst: ascii[], b: u8): i32`
- `ascii_push_i32(dst: ascii[], value: i32): void`
- `length_bytes(s: utf8[]): i32`
- `length_chars(s: utf8[]): i32`
- `utf8_from_ascii(dst: utf8[], src: ascii[], dst_max: i32): i32`

### `src/stdlib/game_math.stasis`

- `game_abs(x: i32): i32`
- `game_abs(x: f32): f32`
- `game_min(a: i32, b: i32): i32`
- `game_min(a: f32, b: f32): f32`
- `game_max(a: i32, b: i32): i32`
- `game_max(a: f32, b: f32): f32`
- `game_clamp(x: i32, lo: i32, hi: i32): i32`
- `game_clamp(x: f32, lo: f32, hi: f32): f32`
- `game_lerp(a: f32, b: f32, t: f32): f32`

Legacy names (`*_i32`, `*_f32`) are removed.

## Call style guidance

Prefer receiver form for receiver-centric operations:

- write `receiver.doAction(action)` over `doAction(receiver, action)`.

```stasis
name_buf.ascii_clear();
name_buf.ascii_append("HP=");
name_buf.ascii_push_i32(42);

out_text.utf8_from_ascii(name_buf, out_text.length);
```

Function form is still supported and can be clearer for utility/math helpers:

```stasis
let clamped: f32 = game_clamp(v, 0.0, 1.0);
let eased: f32 = game_lerp(0.0, 1.0, t);
```
