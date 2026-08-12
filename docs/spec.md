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
- Reachability roots: lifecycle entries present in the program (`main`, `tick`, `render`,
  `on_code_swap`) and host-required exported entries.
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

Unsigned integers use exact fixed-width storage: `u8` is 1 byte, `u16` is 2 bytes, and
`u32` is 4 bytes. Function parameters and returns use a zero-extended 32-bit ABI lane.
Arithmetic in a declared unsigned target wraps modulo `2^N`; division, remainder, and
ordering comparisons are unsigned. Integer literals assigned to an unsigned target must
fit its range. `i32` remains signed two's-complement with signed division and remainder.

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

### 4.3.1 Deterministic Fixed-Point Intrinsics

Strict cross-target gameplay math uses signed Q16.16 values carried in `i32`. The
following compiler intrinsics emit integer Cranelift operations in both JIT and AOT:

- `fixed32_from_i32(value)` converts an integer by shifting left 16 bits.
- `fixed32_from_ratio(numerator, denominator)` creates a Q16.16 ratio.
- `fixed32_mul(left, right)` multiplies two Q16.16 values.
- `fixed32_div(left, right)` divides two Q16.16 values.
- `fixed32_to_i32(value)` discards the fractional part.

All division and conversion rounding is toward zero. Results wrap to the low 32 bits;
division by zero traps. The representable range is `-32768.0` through
`32767.9999847412109375`, in steps of `1 / 65536`. These rules are independent of host
floating-point modes and are the strict deterministic numeric profile for replayable
simulation.

Ordinary `f32` and `f64`, including `sin_fast` and `cos_fast`, remain the
platform-floating profile. JIT and AOT share lowering and are tested for same-target
parity, but bit-identical results across CPU architectures are not claimed. Code that
requires cross-architecture replay must keep simulation state transitions on integer or
Q16.16 operations and may convert to floating point only at the presentation boundary.

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
- Lowering preserves statically known backing provenance through local aliases. A global or nested
  singleton struct view emits only the AoS field path, while an indexed struct-array element emits
  only the SoA field path. The runtime `index < 0` dispatch remains for parameters or other views
  whose callers can supply either backing kind.

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

The receiver may be any global-backed struct view, including nested fields and
indexed elements:

```stasis
state.ui.aura.draw(24.0, 36.0, 255, 0);
state.enemies[i].damage(5);
```

Entry files should normally group application-owned mutable state beneath one
root global. Fixed host ABI globals are an explicit exception.

Function form remains supported indefinitely:

```stasis
damage(enemy, 5);
```

### 7.4 Arity Rule

Arity overloading is not supported.
If declarations share a function name and parameter 0 type, they must use the same parameter count.

Receiver-scoped declarations with different parameter 0 types may use their natural different arities. Resolution selects compatible candidates using the full argument count and types, including the receiver type.

### 7.5 Struct and Array Returns

Struct and array returns are allowed.

Stasis treats these as strongly typed references/views, not implicit by-value copies.
- Struct/array returns must reference global-backed storage (for example a global struct field/element path).
- Struct-typed temporaries are not materialized as standalone local value objects in Stasis.

### 7.6 Compiled Call Generations

Within one compiled JIT or AOT generation, every resolved Stasis-to-Stasis call is a direct call to
the callee in that generation. This includes recursion and mutually recursive functions. Runtime
lookup by function ID is not part of Stasis call semantics.

Only lifecycle and host-required exports cross the host boundary. Their signatures are stable ABI
contracts. Ordinary internal functions may be added, removed, renamed, or change signature during
a live edit because the compiler rebuilds every reachable caller in the same candidate generation.
The host must not retain any compiled entry address after the execution window that resolved it.

## 8. Enums

Enums are named types that lower to integer values.

```stasis
enum State {
    Idle,
    Jump,
    Run,
}
```

Rules:
- Members default to sequential values from `0`.
- Members are separated by commas. The final comma is optional.
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
import "/project/root/path/to/file.stasis";
```

Rules:
- Imports are resolved relative to the importing file.
- Imports beginning with `/` are resolved from the project root. The leading slash is project-root
  syntax, not an operating-system absolute path.
- Project-root imports may not contain empty, `.` or `..` path components and remain confined to
  the project root.
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

### 10.1 Headless Scenario Tests

Tooling may run schema-versioned scenario files beside language-level tests. A scenario uses the
normal JIT compiler and lifecycle entries; it is not a second language execution path.

Rules:
- `main()` establishes fresh state, then an optional saved-state map is applied.
- A bounded runtime snapshot is restored before each explicit property seed.
- `tick()` runs an exact bounded count without wall-clock pacing and without calling `render()`.
- Typed invariants are checked after every tick.
- Replay hashes cover compiler-owned simulation scalars and collections in deterministic path
  order with exact numeric bits.
- Host input snapshots, host request mailboxes, and graphics/audio command buffers are outside the
  simulation hash and cannot mutate gameplay state through the headless host.
- Failure evidence records the scenario, seed, tick, reason, and observed hashes.
- Cross-architecture hash claims require integer or Q16.16 simulation state; ordinary floating
  point remains the platform-floating profile defined in section 4.3.1.

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

The compiler/runtime ownership, generation state machine, platform matrix, and performance gates are
defined in `docs/jit_generation_contract.md`.

### 14.1 Granularity

- Invalidation unit: file
- Correctness unit: file
- Analysis/cache unit: function
- Publication unit: one validated selective patch through stable host-entry trampolines

### 14.2 Hashes

- `fnSigHash`: signature/ABI relevant shape
- `fnBodyHash`: behavior

Rules:
- Unchanged `fnBodyHash` can reuse target-independent analysis or lowering inputs.
- Unchanged reachable JIT functions may retain their accepted machine code and addresses.
- Layout-affecting changes invalidate functions whose lowered storage facts changed and their
  reverse direct callers.

### 14.3 Two-Phase Swap

1. Background compile:
- Re-lex, parse, index, and semantic-check changed file.
- Compute per-function semantic hashes.
- Resolve the complete lifecycle/host-export root set and canonical call/type graph.
- Finalize the changed function/SCC plus exact reverse direct callers into a `PendingPatch` off the
  runtime thread.
2. Commit between ticks:
- Wait until the current generation's `tick()` and following `render()` have both returned.
- Revalidate that the candidate is the newest requested revision and its host-export ABI is
  compatible.
- Create isolated candidate storage and migrate compatible struct/global fields.
- Run the candidate `on_code_swap()` against candidate storage if present.
- Complete every fallible preflight, then atomically replace one immutable host-entry table.
- Retain superseded JIT code until process restart; automatic retirement is not required.

Swap is rejected if:
- Global layout changes and state-map migration is missing or incompatible.
- A required host-export signature changes.
- `on_code_swap()` fails.
- The candidate is cancelled or superseded.
- The target cannot provide atomic host-entry-table publication.

Current policy (pre-1.0):
- Layout-affecting semantic edits produce a versioned preview and require explicit apply.
- The preview reports the complete candidate patch, state-layout compatibility, struct or
  whole-state scope, migration steps, capacity-shrink warnings, and estimated commit cost.
- Apply regenerates the preview; any preview/commit mismatch rejects the swap.

On rejection, old code and old data remain active.

Current migration policy (pre-1.0):
- JIT and AOT derive layout identity from the same canonical compiler-owned state-layout model; source text and function bodies are not layout identity inputs.
- Development JIT compilation produces a selective staged runtime patch and never activates
  code, literals, collection headers, or state from the compiler thread.
- Every JIT entry point uses the same migration planner and bounded transactional activation at the runtime safe point. There is no scalar-only runner migration path.
- Layout-changing commits without a staged JIT candidate, including current AOT runtime swaps, reject with a restart-required diagnostic.
- Migration compatibility is path-based: overlapping paths must keep compatible scalar or collection-element type shape.
- Compatible scalar and fixed-collection fields are copied; new fields are initialized to their type default; removed fields are discarded with an explicit preview warning.
- Fixed-collection growth is storage-ownership preflighted and bounded before allocation, preserves the old prefix, and initializes the expanded tail.
- Shrink copies the retained prefix, warns about the discarded range, and clamps logical lengths; UTF-8 shrink retains the largest valid code-point prefix and recomputes byte and character counts.
- Incompatible or missing state metadata fails deterministically with an actionable diagnostic.
- Migration or `on_code_swap` failure destroys isolated candidate state; the old active entries
  was never mutated. Partial publication is forbidden.

The migration transaction is a code-swap operation, not a gameplay transaction. Ordinary calls to
`tick()` do not commit pools, normalize gameplay state, or invoke migration lifecycle functions.
When a compiled candidate changes a struct or global layout, the host waits until the current
`tick()` and `render()` have both returned. At that between-ticks safe point it snapshots the active
state into isolated candidate storage, copies compatible fields, initializes new fields, runs
`on_code_swap()` if present, and atomically publishes the one complete candidate generation. The
next `tick()` is the first gameplay call allowed to observe the new generation.

There is one visibility rule: a tick and its following render use one code/layout generation. A
failed migration or swap hook destroys the isolated candidate while the old active generation
remains unchanged; no
candidate field, storage binding, export, or partial value may be visible to the next
tick. The executable fixtures under `samples/between_tick_layout_migration/` cover accepted and
rejected struct growth across this boundary.

### 14.3.1 State memory and development inspection

The canonical compiler state layout also owns development memory reporting and live inspection.
JIT and AOT must describe the same scalar bindings, SoA collection lanes, struct paths, field
types, capacities, and opaque exclusions. `stasis inspect` derives capacity bytes, scalar/lane
alignment, zero intra-allocation padding, struct/field rollups, snapshot size, largest pools,
recognized command buffers, capacity-change projections, and mobile snapshot warnings from that
metadata. A report must label the direct-binding storage model and must not imply an AoS packed
layout when runtime storage is SoA.

The development runtime may read those compiler-indexed bindings at between-tick observation
points. It supports scalar paths, fixed indexes, bounded collection predicates, and scalar
arithmetic/comparisons. Predicate scans and returned matches are bounded and report truncation.
Invalid paths, fields, indexes, types, operators, and expressions fail deterministically. This is
metadata-guided inspection, not general reflection: it exposes no arbitrary memory, lexical stack,
host object, call expression, or release-runtime evaluator. Change-only watches evaluate the same
query contract between ticks.

`samples/state_inspection/` is the representative executable fixture for static reporting, state
tree browsing, indexed/predicate expressions, and change-only watches.

### 14.3.2 Bounded cost, tick budget, and layout reporting

`stasis inspect` exposes schema-versioned structural cost evidence from the same reachable
statement artifacts and state layout used by lowering. Each function reports bounded loops,
zero-based nesting depth, the maximum nested iteration product, conservative field visits and bytes
scanned across enclosing loop boundaries, pools iterated, and reachable host-call names. An unknown loop bound remains explicit and makes the
function's structural bound incomplete; the compiler must not replace it with a guessed value.

`function @tick_budget_us(N) tick(): i32` declares a positive runtime budget in microseconds.
The annotation is valid only on `tick`, and duplicates or malformed values fail deterministically.
The development play loop keeps at most 4096 recent samples, reports whole-run average and overrun
count, and calculates p99 from the bounded recent window. Measured wall-clock time is diagnostic
evidence only and is kept separate from compile-time iteration and byte bounds.

Collection layout reports make the compiler's active `soa` choice explicit. They also calculate an
`aos` choice with stride, per-element padding, total bytes, the active singleton field groups, and
the corresponding whole-record AoS group. The recommendation and reason fields expose the
compiler-visible choice without silently changing lowering. `aos_candidate` means the cost model
found a plausible alternative that still requires an explicit future lowering slice; it never
claims the current runtime is AoS. This avoids treating SoA as universally optimal while preserving
the current truthful runtime storage contract.

Mobile estimates report exact Android-arm64 AOT object bytes, exact literal bytes, projected state
and command-buffer capacity, a peak-state recommendation, and a visibly labeled package estimate.
The package estimate is game payload plus a 512 KiB SDL runtime-shell allowance; it is not a claim
about final signed APK/IPA compression or store metadata.

`samples/bounded_performance/` is the representative executable fixture. Its 32 by 16 nested scan,
mixed-width particle fields, capacity, host call, and tick budget provide deterministic acceptance
evidence for the report.

### 14.3.3 Inline function hint

`function @inline helper(...): T` asks the shared AOT/JIT lowering path to substitute the helper's
body at eligible call sites. The annotation is a performance hint rather than a different function
kind: the compiler still emits the real typed function symbol so ordinary calls, recursive edges,
exports, and future address-taking remain valid. Current eligibility covers a single returned
expression whose arguments can be substituted without duplicating or reordering calls. Other body
shapes retain the ordinary direct call. Calls into a same-name, same-arity overload family also
retain direct calls until typed overload selection, so an inline hint can never preempt the typed
callee chosen by the backend.

Inlining never weakens live-update correctness. The annotated bit participates in the lowering
contract, and an edited inline callee invalidates its reverse caller closure before a JIT patch is
published. Recursive expansion is rejected at the call site and continues through the real
function.

### 14.4 Development File-Change Boundary Contracts

During development, file-change handling uses explicit role ownership and message boundaries.

Role ownership:
- Runtime/main thread owns tick loop, safe-point gating, and final commit.
- Compiler service thread owns an immutable source snapshot, lex/parse/index/semantic/hash,
  reachability, and complete candidate assembly.
- Codegen service owns shared direct-call backend emission and complete module finalization (JIT for
  dev, AOT for prod artifacts).
- Swap coordinator owns request ordering, supersession, and transactional all-or-nothing commit
  orchestration.

Required high-level message contracts:
- `FileChangeEvent(path, revision, text_source, change_kind)`
- `BuildGeneration(request_id, revision, source_snapshot_id, target, host_set, active_contract)`
- `BuildFinished(request_id, revision, status, diagnostics[], pending_generation?)`
- `CommitGeneration(request_id, pending_generation)`
- `CommitFinished(request_id, status, active_generation_number?, diagnostic?)`
- `CancelBuild(request_id, superseded_by_request_id)`

Rules:
- Compiler/codegen services must not mutate runtime game state directly.
- Runtime must not execute parser/semantic/codegen work on tick path.
- Commit may occur only between ticks and must be all-or-nothing.
- Any failure at compile or commit stage preserves old code and old data.
- An execution window owns one immutable generation reference from before `tick()` until after its
  following `render()` returns.
- Candidates superseded before hook entry never run `on_code_swap()`. If supersession arrives while
  a synchronous hook is already running, the hook may finish only to unwind; all isolated effects
  are discarded and that candidate never publishes.
- Guest fibers, suspended frames, threads, or retained host callback/code pointers are unsupported
  and must fail deterministically.

## 15. Swap Hook

Optional hook:

```stasis
function on_code_swap(): void {
    // adjust invariants or transient state
}
```

Rules:
- Runs at most once after a candidate enters hook execution; the attempt may later reject, trap, or
  become superseded.
- Runs exactly once for every successfully published candidate that defines the hook.
- Runs between ticks.
- Runs before new code executes.
- May mutate only isolated candidate global data.
- May call `reject_code_swap()` to abort; the runtime destroys the candidate, and the old active
  code/state remain unchanged.
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
