# Stasis Language Specification (Rewrite V1)

This spec is aligned to the in-process, tick-based live compilation and hot-swap PRD.

## 1. Overview

Stasis is a statically allocated language for deterministic simulation and game logic.

Core direction for Rewrite V1:
- Single process runtime.
- In-process Cranelift JIT.
- File-level incremental compilation.
- Hot swap only between ticks.
- Rust host wrapper with Stasis-owned compiler logic.

## 2. Core Principles

- Static global memory only.
- AoS source syntax lowered to SoA storage.
- Deterministic behavior and layout.
- Explicit side effects and assignments.
- Receiver-form callable style preferred.
- Tick-based simulation semantics.

## 3. Operators

Arithmetic and comparison are infix only:
- `+ - * / %`
- `< <= > >= == !=`

Assignment is infix:
- `=`
- `+= -= *= /= %=`

Method-style arithmetic/comparison forms are removed from the language surface.

Numeric conversions should use receiver-form conversion helpers.

Example:
- `f32Value.from_i32(i32Value)`

Conversion semantics:
- `from_*` operations write into the receiver target.
- They are assignment-like operations with side effects.
- They must not be treated as pure value-returning conversion calls.

Example:

```stasis
let ticks_i32: i32;
let alpha: f32;

ticks_i32.from_u32(DebugUI.swapFlashTicks);
alpha.from_i32(ticks_i32);
alpha /= 180.0;
```

## 4. Functions and Call Style

Functions are receiver-scoped by parameter 0 type.

Example declarations:

```stasis
function damage(self: Enemy, amount: i32): void {
    self.hp -= amount;
}

function damage(self: Hero, amount: i32): void {
    self.hp -= amount;
}
```

Preferred call style is receiver form:

```stasis
enemy.damage(5);
hero.damage(5);
```

Function-form calls remain supported when needed:

```stasis
damage(enemy, 5);
```

Resolution key is `(function_name, parameter0_type)`.

## 5. Memory Model

- All persistent data is global.
- No dynamic allocation.
- No hidden copies.
- Struct arrays are lowered to SoA backing storage.
- Layout is deterministic and compile-time known.

## 6. Tick Policy

Stasis-level lifecycle counters are tick-based.

- Simulation logic should not depend on `dt`.
- Engine defines `TICKS_PER_SECOND`.
- State progression should be expressed in ticks.

Example:

```stasis
global DebugUI {
    swapFlashTicks: u32;
}

function draw_debug_ui(): void {
    if (DebugUI.swapFlashTicks > 0) {
        let ticks_i32: i32;
        let alpha: f32;
        ticks_i32.from_u32(DebugUI.swapFlashTicks);
        alpha.from_i32(ticks_i32);
        alpha /= 180.0;
        draw_swap_icon(alpha);
        DebugUI.swapFlashTicks -= 1;
    }
}
```

## 7. Hot Swap Model

Two-phase model:

1. Background compile:
   - Re-lex, parse, index, and semantic-check changed file.
   - Compute per-function semantic hashes.
   - Compile changed functions.
2. Commit between ticks:
   - Run `on_code_swap()` if present.
   - Atomically update function pointer table.
   - Retire previous code generation.

Swap is rejected if:
- Global layout changes.
- Signature compatibility changes.
- `on_code_swap()` fails.

On rejection, old code and old data remain active.

## 8. Swap Hook

Optional hook:

```stasis
function on_code_swap(): void {
    // adjust invariants or transient state
}
```

Rules:
- Runs once per successful swap attempt.
- Runs between ticks.
- Runs before new code executes.
- May mutate global data.
- Must not invoke gameplay entrypoints.

## 9. Incremental Compilation Rules

Granularity:
- Invalidation unit: file.
- Correctness unit: file.
- Emission unit: function.

Per-function hashes:
- `fnSigHash` for ABI/signature-relevant shape.
- `fnBodyHash` for behavior.

Gating:
- Unchanged `fnBodyHash` can reuse generated machine code.
- Layout-affecting change forces conservative rebuild for the file.

## 10. Runtime Boundary

Compiled code calls a stable host API for:
- logging
- input state
- rendering commands
- audio events
- entity/system helpers

All host ABI parameters should use stable primitive representations.

## 11. Development Target for Rewrite V1

- Primary backend: in-process Cranelift JIT.
- Host runtime: Rust (`winit + glutin + glow`).
- C usage: only where unavoidable for platform bindings.
- Compiler logic orchestration: implemented in `.stasis` source.

## 12. Current Status Note

This document defines Rewrite V1 direction. Legacy bootstrap/tooling details from prior repository generations are intentionally excluded from this spec.
