# **Stasis Language Specification (v0.2)**

_A statically-allocated, AoS-syntax / SoA-storage, operator-method-based language for deterministic WASM/LLVM compilation._

---

# **1. Overview**

Stasis is a low-level but ergonomic language designed for predictable compilation into **WebAssembly, LLVM IR, and Cranelift CLIF**, primarily intended for game systems, simulation engines, parallelizable logic, and environments where static memory is required.

Toolchain direction:
- Stage 0 (bootstrap): a C# frontend (LLVMSharp) used for development and bootstrapping.
- Stage 1: improve the C# toolchain UX and stability (fast edit-run loop, clear diagnostics, deterministic outputs).

The core design pillars are:

- **Static memory only** - all data exists in a fixed global memory region
- **No dynamic allocation** - no heap, no runtime resizing
- **Explicit semantics** - no hidden copies, no implicit boxing
- **Operator-methods** for arithmetic/comparison; assignment uses infix `=`
- **Assignment uses infix syntax**:

  ```
  target = value
  ```

- **AoS source structure -> SoA target memory**
- **LLVM, Cranelift, and WASM compatibility**
- **Analyzable effects** (reads vs writes can be statically determined)
- **Deterministic layout** - struct offsets, array bounds known at compile time
- **Direct opcode functions** for arithmetic and memory operations

---

# **2. Lexical Structure**

### Identifiers

```
[_a-zA-Z][_a-zA-Z0-9_]*
```

### Literals

- Integer literal (base-10): `123`
- `u8` integer literal (base-10, 0..255): `123u8`
- Float literal (IEEE-compliant textual form)
- String literal: `" ... "`
- Backtick literal: `` ` ... ` `` (used for test names)
- Boolean literal: `true`, `false`

### Keywords

```
struct enum global function export test return let if else for foreach in
```

(Reserved but potentially unused tokens may be added later.)

### Operators

- Infix arithmetic/comparison: `+ - * / % < <= > >= == !=` with TypeScript-style precedence.
- Compound assignment: `= += -= *= /= %=` 
- Method-style arithmetic/comparison (still supported): `.+() .-() .*() ./() .%() .<() .<=() .>() .>=() .==() .!=()`
- Assignment expressions may appear only once per expression to keep parsing deterministic; chained infix assignments or ternary-like constructs are disallowed and raise diagnostics that highlight the offending operator.

---

# **3. Diagnostics**

- Diagnostics highlight the exact `SourceSpan` that triggered an error, include a concise human-friendly description, and often include a hint on how to fix it (similar to Elm's clarity).
- The parser/semantic layers emit messages such as "Use infix '=' for assignment" or "Only one assignment per expression is permitted" so the code author immediately sees which operator or expression needs rewriting.
- CLI tools and editors can read the `SourceSpan` attached to every diagnostic to underline the tokens, show line/column info, and include references to the spec section being violated.
- Compilation must not silently continue after an error: any invalid program or unsupported construct must produce diagnostics and fail compilation, rather than generating placeholder IR or skipping effects.

# **4. Types**

### Primitive Types

```
u8, u16, u32
i32
f32, f64
bool
```

### Explicit Numeric Casts (No Implicit Widening/Truncation)

Stasis does not perform implicit numeric casts between integer sizes. Use explicit conversion functions:

- `u8_to_i32(u8) -> i32`
- `u16_to_i32(u16) -> i32`
- `i32_to_u8_trunc(i32) -> u8` (low 8 bits)
- `i32_to_u8_checked(i32) -> u8` (aborts if out of range)
- `i32_to_u16_trunc(i32) -> u16` (low 16 bits)
- `i32_to_u16_checked(i32) -> u16` (aborts if out of range)
- `i32_to_f32(i32) -> f32`
- `f32_to_i32(f32) -> i32`

### Arrays (fixed size)

```
Type[IntegerLiteral]
```

### String Types

```
ascii[N]   // fixed-byte ASCII strings (single-byte code units)
utf8[N]    // UTF-8 strings with tracked byte and codepoint lengths
string     // alias for utf8 (default string storage)
string[N]  // alias for utf8[N] (backward compatibility)
ascii[N]   // ASCII-only string buffers with a single length header
```

**ascii[N] layout and invariants**
- Layout: `[len: i32][data: u8[N]]`
- Invariant: all bytes are `< 128`; `len` is the used byte count.
- `data[len]` is set to `0` as a sentinel; sentinel is not counted in `N`.

**utf8[N] layout and invariants**
- Layout: `[byte_length: i32][char_length: i32][data: u8[N]]`
- Invariant: `data[0..byte_length)` is valid UTF-8; `char_length` matches decoded codepoints.
- `data[byte_length]` is set to `0` as a sentinel; sentinel is not counted in `N`.
- C interop: the payload is a null-terminated UTF-8 byte sequence, so host functions can treat `data` as a normal C string and ignore the header unless length metadata is needed.

**string literal typing**
- String literals are context-typed: `""` can target `ascii[N]` or `utf8[N]`/`string` based on the expected type.
- Non-literal values still require explicit conversion between `ascii[N]` and `utf8[N]`.

### Built-in I/O helpers

- `print_string(utf8[N])` prints a UTF-8 buffer; the compiler lowers string literals to static storage with UTF-8 headers and a null sentinel, and the runtime passes the payload pointer to the host I/O layer.
- `string` and `string[N]` are `utf8` aliases, so any string passed to built-ins uses the UTF-8 header layout by default.
- `ascii[N]` and `utf8[N]` are distinct; there is no implicit widening between them. Use explicit conversion helpers (for example, a stdlib `from_ascii` function) when crossing the boundary.
- Helpers like `print(i32)`, `print_int(i32)`, and `print_char(i32)` cover common prompt cases, while `print_cell(i32)` renders Sudoku grid cells with coloring metadata.
- Input helpers include `read_char()` and `read_int()`; higher-level readers such as `read_line()` and `parse_seed_input()` can be implemented in Stasis using these primitives, which is how `samples/sudoku.stasis` parses seeds and user moves.
- `time()` returns the current wall-clock epoch truncated to `i32`, so samples can seed deterministic generators from the clock when the user does not supply a value.
- String globals stay in the static memory region so their lifetime is global and deterministic; tests can rely on the same literal being shared across translation units.

### Host input snapshot

The host runtime owns a canonical per-frame input snapshot and writes it directly into a Stasis global named `input`.

- Stasis projects import `src/host_input_snapshot.stasis`, which declares `global input: InputSnapshot`.
- The host binds the `input` global at startup (and after hot-swap) and fills it once per tick before `tick()` runs.
- No Stasis-side copying is required; games read from `input` directly to keep setup minimal and deterministic.
- Key transition flags (`key_went_down`, `key_went_up`) are computed by the host runtime.
- Pointer coordinates are written in integer pixels (`pointer_x_px`, `pointer_y_px`, `mouse_x_px`, `mouse_y_px`).

### System/host helpers (`sys_*`)

These are host-provided helpers intended for tooling (compilers, asset pipelines, etc.).

- `sys_argc() -> i32`
- `sys_argv(idx: i32, out: utf8[N], out_cap: i32) -> i32` (returns bytes written, `-1` on failure)
- `sys_read_file(path: utf8[N], out: u8[M], out_cap: i32) -> i32` (returns bytes read, `-1` on failure; always writes a `0` sentinel when `out_cap > 0`)
- `sys_write_file(path: utf8[N], data: u8[M], len: i32) -> bool`
- `sys_file_exists(path: utf8[N]) -> bool`
- `sys_file_size(path: utf8[N]) -> i32` (returns bytes, `-1` on failure)
- `sys_file_mtime_ms(path: utf8[N]) -> i32` (returns ms since epoch on supported hosts, `-1` on failure)
- `sys_exec(command: utf8[N]) -> i32` (process exit code)
- `sys_sleep_ms(ms: i32) -> i32` (returns 0; used by polling `watch` loops)
- `sys_memcpy_u8(dst: u8[M], dst_index: i32, src: u8[N], src_index: i32, count: i32) -> void` (copies `count` bytes)
- `sys_memcpy_i32(dst: i32[M], dst_index: i32, src: i32[N], src_index: i32, count: i32) -> void` (copies `count` elements)
- `sys_memcpy_f32(dst: f32[M], dst_index: i32, src: f32[N], src_index: i32, count: i32) -> void` (copies `count` elements)
- `sys_memmove_u8(dst: u8[M], dst_index: i32, src: u8[N], src_index: i32, count: i32) -> void` (copies `count` bytes; overlap-safe)
- `sys_memmove_i32(dst: i32[M], dst_index: i32, src: i32[N], src_index: i32, count: i32) -> void` (copies `count` elements; overlap-safe)
- `sys_memmove_f32(dst: f32[M], dst_index: i32, src: f32[N], src_index: i32, count: i32) -> void` (copies `count` elements; overlap-safe)
- `sys_memset_u8(dst: u8[M], dst_index: i32, value: i32, count: i32) -> void` (runtime internal; compiler/runtime may use for bulk clears; not callable from Stasis source)
- `sys_memset_i32(dst: i32[M], dst_index: i32, value: i32, count: i32) -> void` (runtime internal; compiler/runtime may use for bulk clears; not callable from Stasis source)
- `sys_memset_f32(dst: f32[M], dst_index: i32, value: f32, count: i32) -> void` (runtime internal; compiler/runtime may use for bulk clears; not callable from Stasis source)

### Imports

Stasis supports compilation-unit imports that reference another `.stasis` file as part of the build.

```
import "relative/path/to/file.stasis";
```

- Imports are resolved relative to the importing file.
- Each imported file is a module (file = module); imports introduce modules (see "Modules").
- Imported files are included once (duplicate imports are ignored).
- Imports are graph edges; compilers build a multi-file source graph (no textual import expansion).
- Standard library modules are regular imports; the compiler does not auto-include them.

### Struct Types

Named via:

```
struct Player { ... }
```

### clear()

`clear()` is a convenience operation for bulk-zeroing global state. It is *not* a general-purpose memory primitive and does not expose `memset` to user code.

Rules:

- `clear()` takes no arguments: `some_global.clear()`.
- The receiver must be a global or a global struct field (not a local).
- Supported receivers:
  - Fixed-size arrays of zeroable primitives (`u8/u16/u32/i32/f32/f64/bool`)
  - Struct globals whose fields recursively consist of those fixed-size arrays and zeroable primitives
  - Global arrays of structs where the struct fields are zeroable primitives (clears all backing storage)

AoS vs SoA (important):

- Stasis *syntax* can look AoS (e.g. `global units: Unit[8]; units[i].hp = 1;`) but the compiler lowers global arrays of structs to SoA storage (separate arrays per field).
- `units.clear()` means: clear the entire SoA backing storage for `units` (each field array), not "loop the AoS and assign a default struct".
- Global struct instances (e.g. `global state: GameState;`) are also lowered to flattened globals per field; `state.clear()` clears all backing globals for the instance.

### Enum Types

```
enum State { Idle, Jump, Run, Fall }
```

Enums are **type-safe** named types that lower to integers (`i32`) at runtime. Enum members are automatically assigned sequential integer values starting from 0:

```
State.Idle -> 0
State.Jump -> 1
State.Run -> 2
State.Fall -> 3
```

Enum members may optionally specify an explicit integer value. When a member has an explicit value, subsequent members without an explicit value continue counting upward from that value:

```stasis
enum Scancode { Escape = 41, Space = 44, Left = 80 }
```

**Enum semantics:**
- Members are accessed via dot notation: `EnumName.MemberName`
- Members are implicitly assigned values 0, 1, 2, ... in declaration order unless overridden with `= <int>`
- The first member (value 0) is the default value for uninitialized enum variables
- Each enum member becomes a compile-time constant in the symbol table

**Type safety:**
- Enum variables must be declared with the enum type: `let state: State = State.Idle;`
- Enums are NOT compatible with integer types - you cannot assign an integer to an enum variable
- Enums are NOT compatible with other enum types - you cannot assign `Direction.North` to a `State` variable
- Comparisons between enums and integers are not allowed
- Comparisons between different enum types are not allowed
- Only enum members of the same type can be assigned or compared

Example:
```stasis
enum State { Idle, Jump, Run }

function update(): void {
    let state: State = State.Idle;  // valid
    state = State.Jump;              // valid

    if (state == State.Idle) {       // valid
        state = State.Run;
    }

    // let x: State = 0;              // invalid: cannot assign i32 to State
    // if (state == 0) {              // invalid: cannot compare State with i32
}
```

---

# **5. Memory Model**

### 4.1 Global Memory Only

- All structs and arrays live in a single global memory region.
- Functions may not allocate dynamic memory.
- Local variables are primitive scalars stored in WASM/LLVM locals; struct-typed locals hold references (indices) into global structs.
- Arrays are never local - struct storage stays global-only even when referenced from the stack.

### 4.2 AoS Syntax -> SoA Storage

Example struct:

```stasis
struct Player {
    posX: f32;
    posY: f32;
    hp: u8;
}
```

**Memory layout becomes:**

```
Player_posX[N]
Player_posY[N]
Player_hp[N]
```

Access:

```
p.posX  ->  Player_posX[p.index]
p.hp    ->  Player_hp[p.index]
```

### 4.3 Assignment

Assignment is explicit:

```
p.hp = 5
```

Lowering:

```
store global.Player_hp[p.index], 5
```

### 4.4 Arrays

Fixed size arrays lower to contiguous memory.

Bounds checking rules may be:

- **compile-time optional**
- **lowered to trap**
- **or omitted for speed flags**

---

# **6. Expressions**

Stasis expressions allow infix arithmetic/comparison with TypeScript-style precedence (`||`, `&&`, equality, relational, additive, multiplicative) plus infix `=`/`+=`/`-=`/`*=` `/=` `%=`, and `&&`/`||` for logical flow. Operator-methods for arithmetic/comparison remain valid. Assignment operators are right-associative.

---

# **7. Built-in Operators (Complete List)**

## 6.1 Arithmetic Operators

```
.+(rhs)    -> add
.-(rhs)    -> subtract
.*(rhs)    -> multiply
./(rhs)    -> divide
.%(rhs)    -> modulo
```

### Lowering (WASM example)

| Method | i32         | f32       |
| ------ | ----------- | --------- |
| .+()   | `i32.add`   | `f32.add` |
| .-()   | `i32.sub`   | `f32.sub` |
| .\*()  | `i32.mul`   | `f32.mul` |
| ./()   | `i32.div_s` | `f32.div` |
| .%()   | `i32.rem_s` | n/a       |

`%` for floats is a compile error.

---

## 6.2 Comparison Operators

```
.<(rhs)     // less-than
.>(rhs)     // greater-than
.==(rhs)    // equality
```

### Lowering

| Operator | i32        | f32      |
| -------- | ---------- | -------- |
| .<()     | `i32.lt_s` | `f32.lt` |
| .>()     | `i32.gt_s` | `f32.gt` |
| .==()    | `i32.eq`   | `f32.eq` |

Returns `bool` (`i32`, 0 or 1).

---

## 6.3 Assignment Operator

```
 = rhs
```

### Semantics:

- Writes `rhs` into the l-value receiver.
- Receiver must be a mutable field, array element, or global.
- Returns `void`.

### Lowering:

```
store <resolved address>, rhs
```

---

## 6.4 Unary Operators

```
-(x)    // negation
!(x)    // logical negation
```

Lowering:

- integer negation via `i32.const 0` + `i32.sub`
- boolean negation via `i32.eqz`
- float negation via `f32.neg`

---

# **8. Statements**

### 7.1 Variable Declaration

```
let x: Type;
```

Locals are primitive scalars or struct references (indices into globals). Arrays cannot be local. Initialize them with a subsequent assignment, e.g. `let x: i32; x = 0;`.

### 7.2 Assignment

Just another expression:

```
p.hp = 10;
```

### 7.3 If

```
if (expr) { ... }
else { ... }
```

### 7.4 For

```
for i = 0; i.<(10); i = i.+(1) {
    ...
}
```

### 7.5 Foreach

```
foreach (i in array) {
}
```

Lowers to index iteration.

### 7.6 Return

```
return expr;
return;
```

---

# **9. Functions**

```
function name(param: Type, ...): ReturnType {
    ...
}
```

Attributes may appear between `function` and the name:

```
function @inline name(param: Type): ReturnType { ... }
```

### Function properties:

- No overloading.
- No closures.
- Parameters are primitive or references (struct indices, slices, etc.).
- All struct/array data resides in global memory.

### Extern declarations

Functions declared without a body must be explicitly marked as extern:

```
extern function sleep_ms(ms: i32): void;
```

The attribute form is also supported:

```
function @extern sleep_ms(ms: i32): void;
```

To call a different underlying symbol name, provide a link name:

```
function @extern("stasis_sleep_ms") sleep_ms(ms: i32): void;
```

---

# **10. Globals**

```
global enemies: Enemy[1000];
```

Global arrays of struct references become SoA automatically.

---

# **11. Modules**

- File = Module.
- `import "relative/path/to/file.stasis";` adds that file's module to the build (no textual expansion required).
- Imports introduce modules, and module members are in scope by default after import.
  - No import aliasing.
  - `module_name` defaults to the imported file basename (strip extension, map `-` to `_`, and replace other non-identifier bytes with `_`).
  - Module identity is the canonical (normalized) file path; module names are not required to be unique.
  - If multiple imports introduce the same member name, unqualified references are ambiguous and should produce a diagnostic.
- Imports are transitive for compilation: the build graph includes the imported file and recursively includes its imports.
- Compiled via signature-first pass.
- Platform variants: if an import resolves to `name.stasis` and that file does not exist, the importer will fall back to `name.{platform}.stasis` (e.g. `name.windows.stasis`).

---

# **12. Built-in Testing**

```
test `enemy takes damage`(): bool {
    let hp: i32;
    hp = 50;
    hp = hp - 10;
    return hp == 40;
}
```

Tests are:

- Discovered automatically
- Excluded from production builds through tree-shaking

---

# **13. Compile-Time Memory Offsets**

Stasis provides compile-time offsets:

```
p.posX.memoryOffset()
```

Lowers to a constant `i32`.

Useful for JS interop, debugging, and memory inspection tools.

---

# **14. Compiler Architecture**

## Phase 1 - Signature Discovery

Scan all files, collect:

- Functions
- Structs
- Enums
- Globals

Skip bodies.

## Phase 2 - Dependency and Tree-Shaking

Process exported + test functions as roots.

Parse bodies on-demand.

Mark reachable declarations.

## Phase 3 - WASM/LLVM Code Generation

Lower each reachable function.

Layout memory according to SoA rules.

Generate:

- WASM binary or
- LLVM IR module

---

# **15. LLVM Backend Integration**

### Struct Lowering

AoS syntax -> flat arrays:

Example:

```
struct Player { hp: u8; score: i32; }
```

Generates LLVM globals:

```
@Player_hp = global [N x i8]
@Player_score = global [N x i32]
```

### Operator Lowering

Example:

```
p.hp = p.hp.-(10)
```

Compiles to:

1. Compute index
2. Load hp array element
3. Subtract
4. Store back

---

# **16. LL(1) Grammar (Final)**

See `docs/compilation.md` for the LL(1) grammar used by the compiler.

---

# **17. Examples (Full)**

### Full update step:

```stasis
function updateEnemy(i: u32, dt: f32): void {
    let e: Enemy;
    e = Enemy(i);

    e.posX = e.posX.+( e.vx.*(dt) );
    e.posY = e.posY.+( e.vy.*(dt) );

    if (e.hp.<(1)) {
        e.hp = 0;
    }
}
```

### Array update:

```stasis
scores[i] = scores[i].+(1);
```

### Health handling:

```stasis
function damage(e: Enemy, amt: u8): void {
    e.hp = e.hp.-(amt);
}
```

---

# **18. Summary of Major Language Properties**

| Feature                        | Status       |
| ------------------------------ | ------------ |
| Static memory                      | required |
| AoS syntax -> SoA memory            | automatic |
| No dynamic allocation              | required |
| Infix arith/compare + method calls | available |
| Assignment via `=`, `+=`, `-=`, `*=`, `/=`, `%=` | available |
| Infix ops beyond these             | none in v1 |
| Function signatures first pass     | yes |
| Tree shaking                       | yes |
| LLVM backend                       | yes |
| Cranelift backend (debug)          | experimental |
| WASM backend                       | yes |
| Deterministic behavior             | required |
| Suitable for parallel analysis     | yes |
