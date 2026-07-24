# Stasis Language Specification

This document is the language-level specification for Stasis.
It is aligned with:
- `docs/live-compilation-prd.md`
- `docs/build_checklist.md`
- `docs/spec_implementation_status.md` (spec section -> Rust implementation status table)
- `docs/android_workshop_prd.md` for Android workshop product/editor requirements

The focus is deterministic simulation/game logic with static memory, in-process incremental compilation, and safe hot swap.

## 1. Overview

Stasis is a statically allocated language with explicit behavior.

Core direction:
- Single process runtime.
- In-process Cranelift JIT for development.
- Cranelift AOT for production builds.
- File-level incremental compilation.
- Symbol-level reachability pruning before lowering (functions + struct metadata).
- Reachability roots: `main`, `tick`, `on_code_swap` (when present), and host-required exported entries.
- Hot swap only between ticks.
- Rust host/runtime with a Rust-implemented compiler pipeline.

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
- `string` (alias for UTF-8 string type in current runtime conventions)
- `string[N]` (alias form for compatibility)

### 4.2.1 String Layout and Invariants

String-like storage is fixed-layout and deterministic.

`Type[N]` layout:
- header `max_length: i32`
- payload `elements[N]`

Header access:
- Header fields are accessed via built-in properties (e.g. `.max_length`, `.length`, `.char_length`), not by indexing into the header.
- Negative indices are not allowed in source-level collection indexing.

`Type[]` call-site compatibility:
- A `Type[]` parameter is a view/reference type and may accept storage values with different fixed capacities (`Type[N]`, `Type[M]`, ...).
- The storage header still carries `max_length` so bounds metadata remains available at runtime.

`ascii[N]` layout:
- header `byte_length: i32`
- header `max_length: i32`
- payload `bytes[N]`

`utf8[N]` layout:
- header `byte_length: i32`
- header `max_length: i32`
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
- Local variable bindings must not shadow any already-visible local binding name (including parameters, `for`-init `let` bindings, and `foreach` item/index bindings); shadowing is a compile-time error.
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

Method-style arithmetic/comparison forms are not part of the language surface.

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
} else if (otherCondition) {
    // ...
} else {
    // ...
}
```

Rules:
- `else if` is supported as a direct language form.
- `else if` chains are evaluated top-to-bottom; the first `true` branch executes.
- `else` is optional.
- If no branch condition is `true` and no `else` is present, control falls through.

### 6.5 Looping

Stasis includes `for` and `foreach`.

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
- `init` declarations follow the general no-shadowing local binding rule.
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
foreach (let enemy in enemies) {
    enemy.hp -= 1;
}
```

Primitive example:

```stasis
foreach (let value in scores) {
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
foreach (let enemy, i in enemies) {
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
foreach (let enemy, i in enemies) {
    enemy.hp -= 1;
    enemy.transform.position.x += 2.0;
}
```

Conceptual lowered targets:
- `Enemy_hp[i] -= 1`
- `Enemy_transform_position_x[i] += 2.0`

Nested struct paths are flattened deterministically during lowering, and the current iteration index is applied at the array element dimension.

#### 6.5.5 `continue`

`continue` is supported.

Rules:
- Valid only inside `for` and `foreach` loops.
- Skips the remainder of the current iteration body.
- In `for`, control jumps to the loop `step` segment, then re-evaluates `condition`.
- In `foreach`, control jumps to the loop's index increment, then re-evaluates iteration bounds.

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

For struct/element arguments, Stasis uses reference/view passing semantics (pointer-like behavior), not implicit by-value copies.

Reference/view bindings for struct/element parameters are not rebindable inside the callee:
- assigning to fields/elements through the parameter is allowed
- assigning a new reference target to the parameter binding is a compile-time error

#### 7.1.1 Struct View ABI (Lowering Model)

Struct and struct-element parameters lower to an explicit "struct view" ABI:
- `base: i32`
- `index: i32`
- `len: i32`

Interpretation:
- AoS-backed single struct view (for example a global `pipe: Pipe` passed as `read_active(pipe)`):
- `base = hash_global_path("pipe")`
- `index = -1`
- `len = 0`
- SoA-backed array element view (for example `enemies[i]` passed as `damage(enemies[i], 5)`):
- `base = hash_global_path("enemies")` (the collection hash)
- `index = i`
- `len = enemies.length` (the array extent)

Field access on a struct view:
- If `index < 0` (AoS): compute `field_path_hash = hash_combine(base, "." + field_suffix)` and load/store the scalar field at that global path.
- Otherwise (SoA): compute `field_hash = hash(field_suffix)` and load/store at `(base, field_hash, index)` in the SoA field arrays.

Rationale:
- This allows the same function signature to accept either a single global struct or an element view from a struct array.
- The `len` field supports bounds checks and enables caching/pointer-fast-path optimizations in hot loops.

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

Stasis treats these as strongly typed references/views, not implicit by-value copies.
- Struct/array returns must reference global-backed storage (for example a global struct field/element path).
- Struct-typed temporaries are not materialized as standalone local value objects in Stasis.

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
- Enum typed locals are valid and preferred for enum state:
```stasis
let phase: Phase = Phase.Play;
```
- Enum/integer conversion uses explicit conversion calls.
- Current conversion surface: `enum_to_i32(value: EnumType): i32`.
- `enum_to_i32` is a compiler intrinsic with a stable call shape.

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
- `module` is the imported file basename (without extension).
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

### 12.1 Current Runtime Boundary

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

Console output contract:
- `print_i32(i32)` prints integer text in deterministic decimal form.
- `print_string(string)` prints string data without implicit formatting.
- `print_string` accepts `string`, `ascii[]`, and `utf8[]` call sites in current runtime conventions.
- For `ascii[]`/`utf8[]` call sites, argument passing is by string-view/reference semantics (no implicit full-buffer copy).

### 12.2 Future Direction: Optional Plugin Libraries

This section is intentionally **out of scope** for the current release approach.

Current release approach:
- Development: in-process Cranelift JIT
- Production/release: Cranelift AOT

Optional plugin libraries may be revisited later, but they must not be a dependency for shipping AOT builds or for running the sample games.

Historical note:
Long-term direction could be opt-in runtime libraries/plugins rather than one monolithic host surface.

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
- Snapshot active bounded state when migration or `on_code_swap()` may mutate it.
- Activate candidate storage and migrate compatible struct/global fields.
- Run the candidate `on_code_swap()` if present.
- Atomically publish candidate storage bindings and function pointers.
- Retire the previous code generation.

Swap is rejected if:
- Global layout changes and state-map migration is missing or incompatible.
- Signature compatibility changes.
- `on_code_swap()` fails.

Current policy (pre-1.0):
- Layout-affecting semantic edits produce a versioned preview and require explicit apply.
- The preview reports candidate dispatch-patch functions, state-layout compatibility, struct or whole-state scope, migration steps, capacity-shrink warnings, and estimated commit cost.
- Apply regenerates the preview; any preview/commit mismatch rejects the swap.

On rejection, old code and old data remain active.

Current migration policy (pre-1.0):
- JIT and AOT derive layout identity from the same canonical compiler-owned state-layout model; source text and function bodies are not layout identity inputs.
- Development JIT compilation produces a staged runtime candidate and never activates dispatch, literals, collection headers, or state from the compiler thread.
- Every JIT entry point uses the same migration planner and bounded transactional activation at the runtime safe point. There is no scalar-only runner migration path.
- Layout-changing commits without a staged JIT candidate, including current AOT runtime swaps, reject with a restart-required diagnostic.
- Migration compatibility is path-based: overlapping paths must keep compatible scalar or collection-element type shape.
- Compatible scalar and fixed-collection fields are copied; new fields are initialized to their type default; removed fields are discarded with an explicit preview warning.
- Fixed-collection growth is storage-ownership preflighted and bounded before allocation, preserves the old prefix, and initializes the expanded tail.
- Shrink copies the retained prefix, warns about the discarded range, and clamps logical lengths; UTF-8 shrink retains the largest valid code-point prefix and recomputes byte and character counts.
- Incompatible or missing state metadata fails deterministically with an actionable diagnostic.
- Migration, `on_code_swap`, or pointer commit failure restores the old code and complete bounded runtime snapshot; partial migration is forbidden.

The migration transaction is a code-swap operation, not a gameplay transaction. Ordinary calls to
`tick()` do not commit pools, normalize gameplay state, or invoke migration lifecycle functions.
When a compiled candidate changes a struct or global layout, the host waits until the current
`tick()` and `render()` have both returned. At that between-ticks safe point it snapshots the active
state, activates candidate storage, copies compatible fields, initializes new fields, runs
`on_code_swap()` if present, and atomically publishes the candidate code and migrated state. The
next `tick()` is the first gameplay call allowed to observe the new generation.

There is one visibility rule: a tick and its following render use one code/layout generation. A
failed migration or swap hook restores the complete old generation before gameplay resumes; no
candidate field, storage binding, function pointer, or partial value may be visible to the next
tick. The executable fixtures under `samples/between_tick_layout_migration/` cover accepted and
rejected struct growth across this boundary.

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
- `CompileResult(request_id, status, diagnostics[], layout_hash, fn_patch_set, hook_symbol?, staged_candidate?)`
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
- May call `reject_code_swap()` to abort; the runtime restores the old code and complete bounded state snapshot.
- Must not invoke gameplay entrypoints.

## 16. Diagnostics

Diagnostics should:
- point to offending source span
- use actionable, concise wording
- fail compilation for invalid/unsupported constructs

Diagnostics should not silently skip invalid semantics.

## 17. Development Target

- Development backend: in-process Cranelift JIT.
- Production backend: Cranelift AOT.
- Host runtime: Rust (`winit + glutin + glow`).
- C usage: only where unavoidable for platform bindings.
- Compiler implementation: fully Rust (`stasis_compiler` + `stasis_jit` + runtime integration).

### 17.1 Language and Implementation Ownership Boundary

- `.stasis` owns user program source, language surface usage, and gameplay/runtime logic.
- Rust owns compiler implementation end-to-end: lexing/tokenization, parsing (including incremental parse behavior), semantic rules/diagnostics, compile policy (file invalidation and hash-gating decisions), lowering, and Cranelift backend integration.
- Rust also owns host/runtime integration: file watcher/input bridge, cross-thread message transport and swap coordinator, executable memory management, and runtime ABI/extern bridge.

Rules:
- New compiler frontend/backend behavior is implemented in Rust.
- Language semantics remain spec-driven from this document; compiler behavior must conform to it.
- Tick-path runtime must remain free of parser/semantic/codegen work.

## 18. Status Note

This document defines the current direction.
Legacy bootstrap/tooling details from prior repository generations are intentionally excluded.
