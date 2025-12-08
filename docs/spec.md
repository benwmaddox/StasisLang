Got you, Ben. Here is a **complete, consolidated, detailed Stasis Language Specification** with **all decisions incorporated**, including:

- Assignment via infix `=`
- Operator-methods for arithmetic/comparison; assignment uses infix `=`
- Infix arithmetic/comparison with TypeScript-style precedence; compound assignment supported
- Only one assignment operator may appear in an expression (no chaining).
- AoS → SoA translation model
- LLVM as backend
- Static memory rules
- Modules, functions, tests, globals
- Compiler phases
- Built-in opcode-direct functions
- All built-in operators, including their lowering
- Updated examples
- Aligned with the current Pratt-based expression grammar

I am not omitting anything — this is the full working spec as of now.

---

# **Stasis Language Specification (v0.2)**

_A statically-allocated, AoS-syntax / SoA-storage, operator-method-based language for deterministic WASM/LLVM compilation._

---

# **1. Overview**

Stasis is a low-level but ergonomic language designed for predictable compilation into **WebAssembly and LLVM IR**, primarily intended for game systems, simulation engines, parallelizable logic, and environments where static memory is required. The reference compiler is being built in **C#** with **LLVMSharp** for IR construction and emission.

The core design pillars are:

- **Static memory only** — all data exists in a fixed global memory region
- **No dynamic allocation** — no heap, no runtime resizing
- **Explicit semantics** — no hidden copies, no implicit boxing
- **Operator-methods** for arithmetic/comparison; assignment uses infix `=`
- **Assignment uses infix syntax**:

  ```
  target = value
  ```

- **AoS source structure → SoA target memory**
- **LLVM and WASM compatibility**
- **Analyzable effects** (reads vs writes can be statically determined)
- **Deterministic layout** — struct offsets, array bounds known at compile time
- **Direct opcode functions** for arithmetic and memory operations

---

# **2. Lexical Structure**

### Identifiers

```
[a-zA-Z][a-zA-Z0-9_]*
```

### Literals

- Integer literal (base-10)
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

- Infix arithmetic/comparison: `+ - * / % < > ==` with TypeScript-style precedence.
- Compound assignment: `= += -= *= /= %=` 
- Method-style arithmetic/comparison (still supported): `.+() .-() .*() ./() .%() .<() .>() .==()`
- Assignment expressions may appear only once per expression to keep the Pratt parser unambiguous; chained infix assignments or ternary-like constructs are disallowed and raise diagnostics that highlight the offending operator.

---

# **3. Diagnostics**

- Diagnostics highlight the exact `SourceSpan` that triggered an error, include a concise human-friendly description, and often include a hint on how to fix it (similar to Elm's clarity).
- The parser/semantic layers emit messages such as “Use infix '=' instead of '.='” or “Only one assignment per expression is permitted” so the code author immediately sees which operator or expression needs rewriting.
- CLI tools and editors can read the `SourceSpan` attached to every diagnostic to underline the tokens, show line/column info, and include references to the spec section being violated.

# **4. Types**

### Primitive Types

```
u8, u16, u32
i32
f32, f64
bool
```

### Arrays (fixed size)

```
Type[IntegerLiteral]
```

### Strings

```
string[N]   // sugar for u8[N]
```

### Built-in I/O helpers

- `print_string(string[N])` prints a string literal that the compiler lowers to a null-terminated `u8` array in global memory; the runtime maps it to an LLVM `i8*`.
- Helpers like `print(i32)`, `print_int(i32)`, and `print_char(i32)` cover common prompt cases, while `print_cell(i32)` renders Sudoku grid cells with coloring metadata.
- Input helpers include `read_char()` and `read_int()`; higher-level readers such as `read_line()` and `parse_seed_input()` can be implemented in Stasis using these primitives, which is how `samples/sudoku.stasis` parses seeds and user moves.
- String globals stay in the static memory region so their lifetime is global and deterministic; tests can rely on the same literal being shared across translation units.

### Struct Types

Named via:

```
struct Player { ... }
```

### Enum Types

```
enum State { Idle, Jump, Run, ... }
```

Enums are lowered to integers (`u32` by default).

---

# **5. Memory Model**

### 4.1 Global Memory Only

- All structs and arrays live in a single global memory region.
- Functions may not allocate dynamic memory.
- Local variables are primitive scalars stored in WASM/LLVM locals; struct-typed locals hold references (indices) into global structs.
- Arrays are never local — struct storage stays global-only even when referenced from the stack.

### 4.2 AoS Syntax → SoA Storage

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
p.posX  →  Player_posX[p.index]
p.hp    →  Player_hp[p.index]
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

Stasis expressions allow infix arithmetic/comparison with TypeScript-style precedence (`||`, `&&`, equality, relational, additive, multiplicative) plus infix `=`/`+=`/`-=`/`*=` `/=` `%=`, and `&&`/`||` for logical flow. Operator-methods for arithmetic/comparison remain valid. A Pratt parser enforces precedence (assignments are right-associative).

---

# **7. Built-in Operators (Complete List)**

## 6.1 Arithmetic Operators

```
.+(rhs)    → add
.-(rhs)    → subtract
.*(rhs)    → multiply
./(rhs)    → divide
.%(rhs)    → modulo
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

### Function properties:

- No overloading.
- No closures.
- Parameters are primitive or references (struct indices, slices, etc.).
- All struct/array data resides in global memory.

---

# **10. Globals**

```
global enemies: Enemy[1000];
```

Global arrays of struct references become SoA automatically.

---

# **11. Modules**

- File = Module
- All top-level declarations are visible by filename-level import (v2 will add explicit imports)
- Compiled via signature-first pass, then tree shaking.

---

# **12. Built-in Testing**

```
test `enemy takes damage`(): bool {
    let hp: i32;
    hp = 50;
    hp = hp.-(10);
    return hp.==(40);
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

## Phase 1 — Signature Discovery

Scan all files, collect:

- Functions
- Structs
- Enums
- Globals

Skip bodies.

## Phase 2 — Dependency and Tree-Shaking

Process exported + test functions as roots.

Parse bodies on-demand.

Mark reachable declarations.

## Phase 3 — WASM/LLVM Code Generation

Lower each reachable function.

Layout memory according to SoA rules.

Generate:

- WASM binary or
- LLVM IR module

---

# **15. LLVM Backend Integration**

### Struct Lowering

AoS syntax → flat arrays:

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

(You requested a clean formal version; already delivered, not duplicating here unless you want them merged together.)

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
| Static memory                  | ✔ required   |
| AoS syntax → SoA memory        | ✔ automatic  |
| No dynamic allocation          | ✔            |
| Infix arith/compare + method calls | ✔ available |
| Assignment via `=`, `+=`, `-=`, `*=`, `/=`, `%=` | ✔ |
| Infix ops beyond these             | ✖ none in v1 |
| Function signatures first pass | ✔            |
| Tree shaking                   | ✔            |
| LLVM backend                   | ✔            |
| WASM backend                   | ✔            |
| Deterministic behavior         | ✔            |
| Suitable for parallel analysis | ✔            |
