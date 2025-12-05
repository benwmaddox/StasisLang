Got you, Ben. Here is a **complete, consolidated, detailed Stasis Language Specification** with **all decisions incorporated**, including:

- Assignment via **`.=( )`**
- Operator-methods for arithmetic, comparison, and assignment
- No infix operators in v1
- AoS → SoA translation model
- LLVM as backend
- Static memory rules
- Modules, functions, tests, globals
- Compiler phases
- Built-in opcode-direct functions
- All built-in operators, including their lowering
- Updated examples
- Fully aligned with the LL(1) grammar you approved

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
- **Operator-methods** instead of infix operators
- **Assignment operator is a method call**:

  ```
  target.=(value)
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

### Operators (all method-based)

```
.+() .-() .*() ./() .%()
.<() .>() .==()
.=()
```

---

# **3. Types**

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

# **4. Memory Model**

### 4.1 Global Memory Only

- All structs and arrays live in a single global memory region.
- Functions may not allocate dynamic memory.
- Local variables can only be primitive types (`i32`, `f32`, etc.) stored in WASM/LLVM locals.
- Structs and arrays _cannot_ be local — only **references** to global memory.

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
p.hp.=(5)
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

# **5. Expressions**

Stasis v1 uses **only operator-methods**.
There are **no infix operators** — everything is a postfix chain.

---

# **6. Built-in Operators (Complete List)**

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
.=(rhs)
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

# **7. Statements**

### 7.1 Variable Declaration

```
let x: Type;
```

Locals may only be primitive types. Initialization uses the assignment operator-method on a subsequent line, e.g. `x.=(0);`.

### 7.2 Assignment

Just another expression:

```
p.hp.=(10);
```

### 7.3 If

```
if (expr) { ... }
else { ... }
```

### 7.4 For

```
for i.=(0); i.<(10); i.=( i.+(1) ) {
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

# **8. Functions**

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

# **9. Globals**

```
global enemies: Enemy[1000];
```

Global arrays of struct references become SoA automatically.

---

# **10. Modules**

- File = Module
- All top-level declarations are visible by filename-level import (v2 will add explicit imports)
- Compiled via signature-first pass, then tree shaking.

---

# **11. Built-in Testing**

```
test `enemy takes damage`(): bool {
    let hp: i32;
    hp.=(50);
    hp.=(hp.-(10));
    return hp.==(40);
}
```

Tests are:

- Discovered automatically
- Excluded from production builds through tree-shaking

---

# **12. Compile-Time Memory Offsets**

Stasis provides compile-time offsets:

```
p.posX.memoryOffset()
```

Lowers to a constant `i32`.

Useful for JS interop, debugging, and memory inspection tools.

---

# **13. Compiler Architecture**

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

# **14. LLVM Backend Integration**

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
p.hp.=( p.hp.-(10) )
```

Compiles to:

1. Compute index
2. Load hp array element
3. Subtract
4. Store back

---

# **15. LL(1) Grammar (Final)**

(You requested a clean formal version; already delivered, not duplicating here unless you want them merged together.)

---

# **16. Examples (Full)**

### Full update step:

```stasis
function updateEnemy(i: u32, dt: f32): void {
    let e: Enemy;
    e.=(Enemy(i));

    e.posX.=( e.posX.+( e.vx.*(dt) ) );
    e.posY.=( e.posY.+( e.vy.*(dt) ) );

    if (e.hp.<(1)) {
        e.hp.=(0);
    }
}
```

### Array update:

```stasis
scores[i].=( scores[i].+(1) );
```

### Health handling:

```stasis
function damage(e: Enemy, amt: u8): void {
    e.hp.=( e.hp.-(amt) );
}
```

---

# **17. Summary of Major Language Properties**

| Feature                        | Status       |
| ------------------------------ | ------------ |
| Static memory                  | ✔ required   |
| AoS syntax → SoA memory        | ✔ automatic  |
| No dynamic allocation          | ✔            |
| Operator-methods               | ✔ required   |
| Assignment via `.=( )`         | ✔            |
| Infix ops                      | ✖ none in v1 |
| Function signatures first pass | ✔            |
| Tree shaking                   | ✔            |
| LLVM backend                   | ✔            |
| WASM backend                   | ✔            |
| Deterministic behavior         | ✔            |
| Suitable for parallel analysis | ✔            |
