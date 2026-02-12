# Stasis Language Specification (Rewrite V1)

This document is the language-level specification for Rewrite V1.
It is aligned with:
- `docs/live-compilation-prd.md`
- `docs/rewrite_v1_checklist.md`

The focus is deterministic simulation/game logic with static memory, in-process incremental compilation, and safe hot swap.

## 1. Overview

Stasis is a statically allocated language with explicit behavior.

Core direction for Rewrite V1:
- Single process runtime.
- In-process Cranelift JIT for development.
- Cranelift AOT for production builds.
- File-level incremental compilation.
- Hot swap only between ticks.
- Rust host wrapper with Stasis-owned compiler orchestration.

## 2. Core Principles

- Static global memory only.
- No hidden allocation and no hidden copies.
- AoS source syntax lowered to SoA storage.
- Deterministic behavior and deterministic layout.
- Explicit side effects and assignment.
- Receiver-form callable style preferred.
- Tick-based simulation semantics.

## 3. Lexical Structure

### 3.1 Identifiers

```text
[_a-zA-Z][_a-zA-Z0-9_]*
```

### 3.2 Literals

- Integer literal (base 10): `123`
- Float literal: `123.0`, `0.5`
- Boolean literal: `true`, `false`
- String literal: `"text"`
- Backtick literal (test names): `` `name` ``

### 3.3 Keywords

```text
struct enum global function extern test return let if else for foreach in import
```

Additional reserved keywords may be introduced later.

## 4. Types

### 4.1 Primitive Types

```text
u8 u16 u32 i32 f32 f64 bool void
```

### 4.2 Composite Types

- Fixed-size arrays: `Type[N]`
- Structs: `struct Name { ... }`
- Enums: `enum Name { ... }`
- Strings:
- `utf8[N]`
- `ascii[N]`
- `string` (alias for UTF-8 string type in Rewrite V1 runtime conventions)
- `string[N]` (alias form for compatibility)

### 4.2.1 String Layout and Invariants

String-like storage is fixed-layout and deterministic.

`ascii[N]` layout:
- header `byte_length: i32`
- payload `bytes[N]`

`utf8[N]` layout:
- header `byte_length: i32`
- header `char_length: i32`
- payload `bytes[N]`

Rules:
- `ascii[N]` payload bytes must be valid single-byte ASCII.
- `utf8[N]` payload bytes must be valid UTF-8.
- For `utf8[N]`, `char_length` must match decoded character count.
- `ascii[N]` allows direct payload byte writes; written bytes must remain in ASCII range (`0..127`), and header values must remain consistent.
- `utf8[N]` payload mutation must go through checked helper APIs (direct raw-byte mutation is not allowed in source-level semantics).
- Mutations must keep header values synchronized with payload contents.
- Invalid updates that break these invariants are compile-time errors (when statically known) or runtime errors through checked runtime helpers.

### 4.3 Numeric Conversion Semantics

Numeric conversion helpers use receiver-form methods in two categories:

`from_*` conversions (mutating target):
- Assignment-like operations that write into receiver target.
- Statement-style side-effect operations.
- Example: `f32Value.from_i32(i32Value);`

`to_*` conversions (pure value):
- Pure operations on basic numeric types.
- Expression-safe and can be used in declarations and initializers.
- Example: `let alpha: f32 = ticks_i32.to_f32();`

Example:

```stasis
let ticks_i32: i32;
let alpha: f32;

ticks_i32.from_u32(DebugUI.swapFlashTicks);
alpha.from_i32(ticks_i32);
alpha /= 180.0;
```

Equivalent initializer style:

```stasis
let ticks_i32: i32 = DebugUI.swapFlashTicks.to_i32();
let alpha: f32 = ticks_i32.to_f32();
alpha /= 180.0;
```

### 4.4 Local Type Inference

Local `let` declarations may omit explicit type annotations when type can be inferred from initializer/context.

Examples:

```stasis
let count = 0;      // inferred as i32
let alpha = 0.5;    // inferred as f32
let hp: u8 = 0;     // explicit narrow type remains supported/required when needed
let enemy = state.enemies[0]; // inferred from indexed expression when unambiguous
```

Rules:
- If type annotation is omitted, initializer is required.
- If type annotation is provided, it is authoritative.
- Narrow integer types should be explicitly annotated when required by layout/ABI (`u8`, `u16`, etc).
- Struct/array element expressions can infer local type when source type is uniquely known.
- Example: `let enemy = state.enemies[0];` infers `Enemy` element view/reference type.
- A binding inferred from an element expression aliases that element; it is not an implicit copy.
- For primitive locals, `let b = a;` copies the primitive value.
- For struct/element references, `let b = a;` binds another alias to the same referenced element.
- If inference has multiple valid candidate types, declaration is rejected with an ambiguity diagnostic and requires explicit annotation.
- Numeric literal defaults are deterministic:
- integer literals infer as `i32` by default
- floating literals infer as `f32` by default

## 5. Operators and Expressions

### 5.1 Arithmetic and Comparison Operators

Arithmetic and comparison are infix only:
- `+ - * / %`
- `< <= > >= == !=`

Method-style arithmetic/comparison forms are removed from Rewrite V1 language surface.

### 5.2 Logical Operators

Logical operators are:
- `&&`
- `||`
- `!`

Semantics:
- `&&` and `||` are short-circuit operators with left-to-right evaluation.
- `!` is unary logical negation.
- Operands for logical operators must be `bool`.
- Logical operator results are `bool`.

### 5.3 Assignment Operators

Assignment is infix:
- `=`
- `+= -= *= /= %=`

### 5.4 Precedence

Infix expressions follow TypeScript-like precedence for:
- multiplicative
- additive
- relational
- equality
- logical operators
- assignment

## 6. Declarations and Statements

### 6.1 Variable Declarations

```stasis
let x: i32;
let y: i32 = 10;
let z = 10;
let t = 0.5;
let enemy = state.enemies[0];
```

Rules:
- `let name: Type;` is allowed.
- `let name: Type = expr;` is allowed.
- `let name = expr;` is allowed and uses inference.
- `let name;` is invalid.

### 6.2 Globals

```stasis
global State {
    score: i32;
}
```

All persistent data lives in global memory.

### 6.3 Assignment

```stasis
State.score = 5;
State.score += 1;
let enemy = state.enemies[0];
enemy.hp -= 1;                  // mutates state.enemies[0]
state.enemies[1] = state.enemies[0]; // explicit struct value copy
```

Rules:
- Assignment is explicit and may perform explicit value copies.
- `let enemy = state.enemies[0];` binds an element view/reference alias.
- Assigning directly to a reference/view local or parameter binding (for example `enemy = state.enemies[1];`) is not allowed.
- To change underlying referenced data, mutate fields/elements through the binding (for example `enemy.hp -= 1;`).
- `state.enemies[1] = state.enemies[0];` copies source struct value into destination struct value.
- For SoA-lowered struct arrays, struct copy assignment lowers to per-field writes at source and destination indices.

### 6.4 Control Flow

```stasis
if (condition) {
    // ...
} else {
    // ...
}
```

### 6.5 Looping

Rewrite V1 includes `for` and `foreach`.

#### 6.5.1 `for` loop

Canonical form:

```stasis
for (init; condition; step) {
    // body
}
```

Example:

```stasis
for (let i = 0; i < 10; i += 1) {
    total += i;
}
```

Alternative form with explicit narrow type:

```stasis
for (let i: u8 = 0; i < maxSlots; i += 1) {
    slots[i].active = true;
}
```

Rules:
- `init` runs once before the first iteration.
- `init` may be a declaration (`let i = 0` or `let i: i32 = 0`) or an assignment/expression (`i = 0`).
- `condition` is required, evaluated before each iteration, and must be `bool`.
- `step` is required and runs after each body execution.
- A variable declared in `init` is scoped to the loop (condition, step, and body).
- A variable declared in `init` must not shadow an existing local variable name from an enclosing scope; shadowing is a compile-time error.
- Omitting any of `init`, `condition`, or `step` is a compile-time error.
- `for` lowering is explicit control flow equivalent to:
- run `init`
- branch on `condition`
- execute body
- execute `step`
- repeat

#### 6.5.2 `foreach` loop (value form)

Value-only form:

```stasis
foreach (enemy in enemies) {
    enemy.hp -= 1;
}
```

Primitive example:

```stasis
foreach (value in scores) {
    value += 1;
}
```

Rules:
- Iterates left-to-right from index `0` to `N - 1` for fixed-size arrays `Type[N]`.
- `enemy` is an element view for the current index (not a detached copy).
- Writes through the element view mutate the underlying storage.
- Primitive element variables in `foreach` are also writable views; writes are write-through to backing storage.

#### 6.5.3 `foreach` loop (index + value form)

Indexed form:

```stasis
foreach (i, enemy in enemies) {
    if (i == focusIndex) {
        enemy.hp -= 10;
    }
}
```

Rules:
- `i` is the current element index (type `i32`).
- `enemy` is the element view at `enemies[i]`.
- Iteration order is deterministic: `0 .. N - 1`.

#### 6.5.4 `foreach` lowering model (AoS syntax -> SoA storage)

`foreach` lowers to an index loop over the array extent.

Conceptual desugaring:

```stasis
for (__i = 0; __i < N; __i += 1) {
    // element view bound to array[__i]
    // original foreach body
}
```

For struct arrays lowered to SoA, field access inside `foreach` maps to field arrays at the loop index.

Example source:

```stasis
foreach (i, enemy in enemies) {
    enemy.hp -= 1;
    enemy.transform.position.x += 2.0;
}
```

Conceptual lowered targets:
- `Enemy_hp[i] -= 1`
- `Enemy_transform_position_x[i] += 2.0`

Nested struct paths are flattened deterministically during lowering, and the current iteration index is applied at the array element dimension.

### 6.6 Return

```stasis
return;
return value;
```

## 7. Functions and Calls

### 7.1 Function Declaration

```stasis
function name(param: Type): ReturnType {
    // ...
}
```

For struct/element arguments, Rewrite V1 uses reference/view passing semantics (pointer-like behavior), not implicit by-value copies.

Reference/view bindings for struct/element parameters are not rebindable inside the callee:
- assigning to fields/elements through the parameter is allowed
- assigning a new reference target to the parameter binding is a compile-time error

### 7.2 Receiver-Scoped Resolution

Function identity for receiver-scoped names is:
- function name
- parameter 0 type

Example declarations:

```stasis
function damage(self: Enemy, amount: i32): void {
    self.hp -= amount;
}

function damage(self: Hero, amount: i32): void {
    self.hp -= amount;
}
```

### 7.3 Call Forms

Preferred receiver form:

```stasis
enemy.damage(5);
hero.damage(5);
```

Function form remains supported indefinitely:

```stasis
damage(enemy, 5);
```

### 7.4 Arity Rule

Arity overloading is not supported.
If declarations share a function name, they must use the same parameter count.

### 7.5 Struct and Array Returns

Struct and array returns are allowed.

Rewrite V1 treats these as strongly typed references/views, not implicit by-value copies.
- Struct/array returns must reference global-backed storage (for example a global struct field/element path).
- Struct-typed temporaries are not materialized as standalone local value objects in Rewrite V1.

## 8. Enums

Enums are named types that lower to integer values.

```stasis
enum State {
    Idle,
    Jump,
    Run
}
```

Rules:
- Members default to sequential values from `0`.
- Enum members can be explicitly assigned integer constants.
- Enum comparisons and assignments must be type-correct.
- Enum underlying type is `i32`.
- Explicit enum member values must be within `i32` range; out-of-range values are compile-time errors.
- No implicit enum <-> `i32` conversion is allowed.
- Enum/integer conversion requires explicit conversion helpers.

## 9. Modules and Imports

File is module.

Import syntax:

```stasis
import "relative/path/to/file.stasis";
```

Rules:
- Imports are resolved relative to the importing file.
- Imported files are included once.
- Import graphs are compilation graph edges, not textual expansion.
- Import cycles are hard errors.
- Ambiguous references across modules must produce diagnostics.
- Disambiguation is explicit `module.symbol` only.
- For Rewrite V1, `module` is the imported file basename (without extension).
- If multiple imports map to the same basename `module` name, compilation fails with a hard error.
- When a symbol name collides across imports, unqualified use is invalid and must be rewritten as `module.symbol`.

## 10. Testing Construct

Stasis supports language-level tests:

```stasis
test `enemy takes damage`(): bool {
    return true;
}
```

Rules:
- Tests are discoverable by tooling.
- Tests are excluded from production builds.
- Test execution should be deterministic.
- Runtime test discovery modes:
- entry-file mode: discover tests in the entry file only (no cascading import traversal)
- directory mode: discover tests in all `.stasis` files in the target directory (including root)
- Tests run in deterministic sorted natural path order (numeric path segments compare numerically, not lexicographically).
- Tests may call extern/runtime functions.

## 11. Memory Model

- All persistent data is global.
- No dynamic allocation.
- No hidden copies (no implicit copy temporaries inserted by language semantics).
- Struct arrays lower from AoS syntax to SoA backing storage.
- Layout is deterministic and compile-time known.

Explicit copy operations are still valid.
Example:
- `state.enemies[1] = state.enemies[0];` is an explicit value copy operation.
- Under SoA lowering, this becomes deterministic per-field copy writes.

Illustrative lowering:
- Source: `units[i].hp`
- Lowered target concept: `Unit_hp[i]`

## 12. Runtime Boundary and Extern

### 12.1 Current Rewrite V1 Boundary

Compiled Stasis code calls a stable host API using ABI-stable primitive shapes.

Example boundary areas:
- logging
- input state
- rendering commands
- audio events
- entity/system helpers

Extern declarations:

```stasis
extern function print_i32(value: i32): void;
extern function print_string(value: string): void;
```

### 12.2 Future Direction: Optional Plugin Libraries

Long-term direction is opt-in runtime libraries/plugins rather than one monolithic host surface.

Intent:
- pull only required host libraries into a build/runtime
- keep ABI boundaries explicit
- avoid hard-coupling every project to every host service

Syntax and packaging details for plugin/library declarations are intentionally deferred for a later spec revision.

## 13. Tick Policy

Stasis-level lifecycle counters are tick-based.

Rules:
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
        let ticks_i32: i32 = DebugUI.swapFlashTicks.to_i32();
        let alpha: f32 = ticks_i32.to_f32();
        alpha /= 180.0;
        draw_swap_icon(alpha);
        DebugUI.swapFlashTicks -= 1;
    }
}
```

## 14. Incremental Compilation and Hot Swap

### 14.1 Granularity

- Invalidation unit: file
- Correctness unit: file
- Emission unit: function

### 14.2 Hashes

- `fnSigHash`: signature/ABI relevant shape
- `fnBodyHash`: behavior

Rules:
- Unchanged `fnBodyHash` can reuse generated machine code.
- Layout-affecting changes force conservative rebuild for changed file.

### 14.3 Two-Phase Swap

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

### 14.4 Development File-Change Boundary Contracts

During development, file-change handling uses explicit role ownership and message boundaries.

Role ownership:
- Runtime/main thread owns tick loop, safe-point gating, and final commit.
- Compiler service thread owns lex/parse/index/semantic/hash and patch assembly.
- Codegen service owns backend emission (JIT for dev, AOT for prod artifacts).
- Swap coordinator owns transactional all-or-nothing commit orchestration.

Required high-level message contracts:
- `FileChangeEvent(path, revision, text_source, change_kind)`
- `CompileRequest(request_id, changed_files[], target_mode)`
- `CompileResult(request_id, status, diagnostics[], layout_hash, fn_patch_set)`
- `SwapCommitRequest(request_id, layout_hash, fn_patch_set, hook_symbol)`
- `SwapCommitResult(request_id, status, swapped_fn_ids[], new_generation, error)`

Rules:
- Compiler/codegen services must not mutate runtime game state directly.
- Runtime must not execute parser/semantic/codegen work on tick path.
- Commit may occur only between ticks and must be all-or-nothing.
- Any failure at compile or commit stage preserves old code and old data.

## 15. Swap Hook

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

## 16. Diagnostics

Diagnostics should:
- point to offending source span
- use actionable, concise wording
- fail compilation for invalid/unsupported constructs

Diagnostics should not silently skip invalid semantics.

## 17. Development Target for Rewrite V1

- Development backend: in-process Cranelift JIT.
- Production backend: Cranelift AOT.
- Host runtime: Rust (`winit + glutin + glow`).
- C usage: only where unavoidable for platform bindings.
- Compiler orchestration: implemented in `.stasis` source.

## 18. Status Note

This document defines Rewrite V1 direction.
Legacy bootstrap/tooling details from prior repository generations are intentionally excluded.
