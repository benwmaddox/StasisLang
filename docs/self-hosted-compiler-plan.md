# Self-Hosted Stasis Compiler Plan

This document outlines the architecture, requirements, and implementation strategy for transitioning from the current C#-based Stasis compiler to a self-hosted compiler written in Stasis itself.

## Table of Contents

1. [Current Architecture Overview](#current-architecture-overview)
2. [Self-Hosted Architecture](#self-hosted-architecture)
3. [Language Prerequisites](#language-prerequisites)
4. [Implementation Phases](#implementation-phases)
5. [Component Details](#component-details)
6. [Data Structure Design](#data-structure-design)
7. [Memory Management Strategy](#memory-management-strategy)
8. [External Process Integration](#external-process-integration)
9. [Bootstrap Strategy](#bootstrap-strategy)
10. [Risk Assessment](#risk-assessment)
11. [Milestones and Success Criteria](#milestones-and-success-criteria)

---

## Current Architecture Overview

The existing C# compiler follows this pipeline:

```
Stasis Source (.stasis)
        │
        ▼
┌─────────────────────────────┐
│  C# COMPILER (Stasis.Compiler)  │
├─────────────────────────────┤
│  1. Lexer.cs                │  Tokenization
│  2. Parser.cs               │  AST construction
│  3. SemanticAnalyzer.cs     │  Type checking, symbol resolution
│  4. LayoutPlanner.cs        │  Memory layout, SoA transformation
│  5. CraneliftCodeGenerator  │  CLIF IR generation
│  6. CraneliftModuleBuilder  │  Module structure
│  7. CraneliftFunctionBuilder│  Statement/expression lowering
└─────────────────────────────┘
        │
        ▼ CLIF text output
┌─────────────────────────────┐
│  RUST AOT TOOL              │
│  (tools/cranelift-aot)      │
│  - Parses CLIF text         │
│  - Compiles to COFF object  │
│  - Uses Cranelift backend   │
└─────────────────────────────┘
        │
        ▼ .obj file
┌─────────────────────────────┐
│  LINKER (clang/lld-link)    │
│  - Links with CRT           │
│  - Links with graphics lib  │
│  - Produces executable      │
└─────────────────────────────┘
        │
        ▼
    Executable (.exe)
```

### Current Component Sizes

| Component | C# Lines | Complexity |
|-----------|----------|------------|
| Lexer | ~350 | Low |
| Parser | ~850 | Medium |
| Syntax Nodes | ~400 | Low (data records) |
| SemanticAnalyzer | ~920 | High |
| CraneliftCodeGenerator | ~450 | Medium |
| CraneliftModuleBuilder | ~280 | Low |
| CraneliftFunctionBuilder | ~1800 | High |
| **Total Frontend** | **~5050** | - |

---

## Self-Hosted Architecture

The self-hosted compiler maintains the same pipeline structure but reimplements the frontend in Stasis:

```
Stasis Source (.stasis)
        │
        ▼
┌─────────────────────────────┐
│  STASIS COMPILER (self-hosted)  │
├─────────────────────────────┤
│  1. lexer.stasis            │  Tokenization
│  2. parser.stasis           │  AST construction
│  3. semantic.stasis         │  Type checking, symbols
│  4. layout.stasis           │  Memory layout
│  5. codegen.stasis          │  CLIF IR generation
└─────────────────────────────┘
        │
        ▼ CLIF text (via sys_write_file or stdout)
┌─────────────────────────────┐
│  RUST AOT TOOL              │
│  (unchanged - still external) │
└─────────────────────────────┘
        │
        ▼
    [Linking and Execution unchanged]
```

### Key Design Decision: External Cranelift Retained

The self-hosted compiler does **not** reimplement Cranelift. It:
- Generates CLIF text (same format as today)
- Invokes the existing Rust AOT tool via `sys_exec()` or similar
- Uses the existing linker integration

This keeps the scope manageable while achieving self-hosting.

---

## Language Prerequisites

Before implementing the self-hosted compiler, Stasis requires several language features that may be incomplete:

### Required Features (Critical)

| Feature | Current Status | Notes |
|---------|----------------|-------|
| String manipulation | Partial | Need: concat, substring, comparison |
| Character-by-character iteration | Partial | Need: `str_next_codepoint()` + `str_decode_codepoint()` (byte helpers exist) |
| Dynamic-like arrays | ❌ Missing | Need growable buffers (via fixed upper bound) |
| File I/O | ❌ Missing | Need: `sys_read_file()`, `sys_write_file()` |
| Process execution | ❌ Missing | Need: `sys_exec()` for AOT tool |
| String building | ❌ Missing | Need efficient string concatenation |

### Required Built-in Functions

```stasis
// File I/O
function sys_read_file(path: ascii[256], buffer: ascii[65536]): i32  // returns bytes read
function sys_write_file(path: ascii[256], content: ascii[65536]): bool
function sys_file_exists(path: ascii[256]): bool

// Process execution
function sys_exec(command: ascii[1024], args: ascii[4096]): i32  // returns exit code
function sys_exec_capture(command: ascii[1024], output: ascii[65536]): i32

// String utilities (if not already present)
function ascii_append(dest: ascii[N], src: ascii[M]): i32
function str_from_i32(value: i32, buffer: ascii[16]): i32  // returns length
function str_to_i32(s: ascii[N]): i32
function char_is_alpha(c: u8): bool
function char_is_digit(c: u8): bool
function char_is_whitespace(c: u8): bool

// Memory utilities
function mem_copy(dest: u8[N], src: u8[M], len: i32): void
function mem_zero(dest: u8[N], len: i32): void
```

### Language Extensions Needed

1. **Large Fixed Arrays**: Compiler state requires arrays with ~10,000+ element capacity
2. **Nested Array Access**: `tokens[i].text[j]` must work reliably
3. **String Literals in Arrays**: Initialize keyword lookup tables
4. **Pointer-Like Semantics**: Pass arrays/structs by reference to functions

---

## Implementation Phases

### Phase 1: Language Foundation (Prerequisite)

**Goal:** Ensure Stasis has all language features needed for compiler implementation.

1. **File I/O Built-ins**
   - Implement `sys_read_file()`, `sys_write_file()` in C runtime
   - Add CLIF lowering in `CraneliftFunctionBuilder`
   - Test with simple file operations

2. **Process Execution Built-ins**
   - Implement `sys_exec()` using Windows `CreateProcess` or POSIX `fork/exec`
   - Capture stdout/stderr for error reporting
   - Return exit code

3. **String Building**
   - Efficient append operations
   - Integer-to-string conversion
   - String comparison (lexicographic)

4. **Large Array Support**
   - Verify arrays up to 100,000 elements work
   - Test nested struct arrays

### Phase 2: Lexer Implementation

**Goal:** Tokenize Stasis source code into a token stream.

**Data Structures:**
```stasis
const MAX_TOKENS: i32 = 50000
const MAX_SOURCE: i32 = 500000
const MAX_TOKEN_TEXT: i32 = 256

enum TokenKind {
    // Literals
    IntLiteral,
    FloatLiteral,
    StringLiteral,
    BacktickLiteral,

    // Identifiers and keywords
    Identifier,
    KwStruct,
    KwEnum,
    KwGlobal,
    KwConst,
    KwFunction,
    KwExport,
    KwTest,
    KwReturn,
    KwLet,
    KwIf,
    KwElse,
    KwFor,
    KwForeach,
    KwIn,
    KwTrue,
    KwFalse,

    // Operators
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Eq,
    EqEq,
    BangEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    AmpAmp,
    PipePipe,
    Bang,
    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    PercentEq,

    // Delimiters
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Colon,
    Semicolon,
    Dot,
    Arrow,

    // Special
    Eof,
    Error
}

struct Token {
    kind: TokenKind,
    textStart: i32,      // index into source
    textLen: i32,
    line: i32,
    column: i32
}

struct LexerState {
    source: ascii[500000],
    sourceLen: i32,
    pos: i32,
    line: i32,
    column: i32,
    tokens: Token[50000],
    tokenCount: i32
}
```

**Algorithm:**
1. Character-by-character scan
2. Keyword recognition via string comparison (no hash tables)
3. Numeric literal parsing (integers and floats)
4. String literal parsing with escape sequences
5. Comment skipping (line and block)
6. Track line/column for error reporting

**Estimated Size:** ~400-500 lines of Stasis

### Phase 3: Parser Implementation

**Goal:** Build an Abstract Syntax Tree from tokens.

**AST Node Design:**
```stasis
const MAX_NODES: i32 = 100000
const MAX_CHILDREN: i32 = 32

enum NodeKind {
    // Declarations
    StructDecl,
    EnumDecl,
    GlobalDecl,
    ConstDecl,
    FunctionDecl,
    TestDecl,

    // Statements
    BlockStmt,
    VarDeclStmt,
    IfStmt,
    ForStmt,
    ForeachStmt,
    ReturnStmt,
    ExprStmt,

    // Expressions
    IdentifierExpr,
    IntLiteralExpr,
    FloatLiteralExpr,
    StringLiteralExpr,
    BoolLiteralExpr,
    UnaryExpr,
    BinaryExpr,
    AssignExpr,
    MemberAccessExpr,
    ArrayAccessExpr,
    CallExpr,
    OperatorCallExpr,
    ParenExpr,

    // Types
    NamedType,
    ArrayType,

    // Helpers
    FieldDecl,
    ParamDecl,
    EnumMember
}

struct AstNode {
    kind: NodeKind,
    tokenIndex: i32,         // primary token
    childStart: i32,         // index into children array
    childCount: i32,
    // Type-specific data (union-like via separate arrays)
    dataIndex: i32
}

struct ParserState {
    tokens: Token[50000],
    tokenCount: i32,
    current: i32,

    nodes: AstNode[100000],
    nodeCount: i32,

    children: i32[300000],   // child node indices
    childCount: i32,

    // Error tracking
    hadError: bool,
    errorLine: i32,
    errorMessage: ascii[256]
}
```

**Parsing Strategy:**
1. **Recursive Descent** for declarations and statements
2. **Precedence Climbing** for expressions
3. **Panic Mode Recovery** on errors (skip to next declaration)

**Precedence Table:**
```
Level 1: = += -= *= /= %=  (right assoc)
Level 2: ||
Level 3: &&
Level 4: == !=
Level 5: < <= > >=
Level 6: + -
Level 7: * / %
Level 8: ! - (prefix)
Level 9: . [] () (postfix)
```

**Estimated Size:** ~800-1000 lines of Stasis

### Phase 4: Semantic Analysis

**Goal:** Type checking, symbol resolution, semantic validation.

**Symbol Table Design:**
```stasis
const MAX_SYMBOLS: i32 = 10000
const MAX_SCOPES: i32 = 1000

enum SymbolKind {
    Type,
    Struct,
    Enum,
    EnumMember,
    Global,
    Const,
    Function,
    Parameter,
    Local
}

struct Symbol {
    name: ascii[128],
    kind: SymbolKind,
    typeIndex: i32,       // index into types array
    declNode: i32,        // AST node index
    scopeLevel: i32
}

struct TypeInfo {
    kind: TypeKind,       // Primitive, Struct, Enum, Array, Void
    name: ascii[64],
    size: i32,            // in bytes
    arrayElementType: i32,// for arrays
    arraySize: i32,       // for fixed arrays
    structIndex: i32      // for struct types
}

struct SemanticState {
    symbols: Symbol[10000],
    symbolCount: i32,

    types: TypeInfo[1000],
    typeCount: i32,

    scopeStack: i32[1000], // symbol indices marking scope boundaries
    scopeDepth: i32,

    // Struct field tracking
    structFields: StructField[5000],
    fieldCount: i32,

    // Error collection
    errors: Diagnostic[100],
    errorCount: i32
}
```

**Analysis Passes:**

1. **Pass 1: Declare Built-ins**
   - Register primitive types (i32, f32, bool, etc.)
   - Register built-in functions (~100+)

2. **Pass 2: Declare Types**
   - Collect struct declarations
   - Collect enum declarations
   - Validate field types

3. **Pass 3: Declare Globals/Constants**
   - Register global variables
   - Register constants

4. **Pass 4: Declare Functions**
   - Register function signatures
   - Validate parameter types

5. **Pass 5: Analyze Bodies**
   - Type-check all expressions
   - Resolve identifiers
   - Validate assignments
   - Check function calls

**Estimated Size:** ~1000-1200 lines of Stasis

### Phase 5: Layout Planning

**Goal:** Calculate memory layout with AoS-to-SoA transformation.

```stasis
struct LayoutEntry {
    name: ascii[128],
    typeName: ascii[64],
    offset: i32,
    size: i32,
    isArray: bool,
    arrayLength: i32
}

struct LayoutPlan {
    entries: LayoutEntry[5000],
    entryCount: i32,
    totalSize: i32
}
```

**SoA Transformation Rules:**
- `global state: GameState` with `struct GameState { x: i32, y: i32 }`
- Becomes: `state_x: i32` and `state_y: i32` at consecutive offsets
- Arrays of structs become parallel arrays of fields

**Estimated Size:** ~300-400 lines of Stasis

### Phase 6: CLIF Code Generation

**Goal:** Generate Cranelift IR text output.

**Output Builder:**
```stasis
struct ClifBuilder {
    output: ascii[1000000],  // 1MB output buffer
    outputLen: i32,

    // SSA value tracking
    nextValue: i32,          // v0, v1, v2, ...
    nextBlock: i32,          // block0, block1, ...

    // Stack slot tracking
    slots: StackSlot[1000],
    slotCount: i32,

    // Global references
    globalNames: ascii[10000][64],
    globalCount: i32,

    // External function tracking
    externals: ExternalFunc[200],
    externalCount: i32
}
```

**CLIF Generation Strategy:**

1. **Module Header**
   ```clif
   ; Cranelift module: <name>
   ; Generated by Stasis self-hosted compiler
   ```

2. **Globals Section**
   ```clif
   global gv_varname: i32
   global str_0: i8[N] ; bytes: ...
   ```

3. **External Functions**
   ```clif
   external printf3(i64, i64, i64) -> i32 windows_fastcall
   ```

4. **Function Definitions**
   ```clif
   function %name(i32, i32) -> i32 windows_fastcall {
   block0(v0: i32, v1: i32):
       v2 = iadd v0, v1
       return v2
   }
   ```

**Statement Lowering:**
- `if` → `brif` + blocks
- `for` → `jump` + `brif` loop structure
- `return` → `return`
- Variable declarations → `stack_slot` + `store`

**Expression Lowering:**
- Literals → `iconst`, `f32const`
- Binary ops → `iadd`, `isub`, `imul`, etc.
- Comparisons → `icmp` with condition codes
- Function calls → `call` instruction
- Member access → computed offsets

**Estimated Size:** ~1500-2000 lines of Stasis

### Phase 7: Driver and Integration

**Goal:** Orchestrate compilation and invoke external tools.

```stasis
function main(): i32 {
    // 1. Parse command line arguments
    let inputFile: ascii[256] = ...
    let outputFile: ascii[256] = ...

    // 2. Read source file
    let source: ascii[500000]
    let sourceLen: i32 = sys_read_file(inputFile, source)

    // 3. Lexical analysis
    let lexer: LexerState
    lex(source, sourceLen, lexer)

    // 4. Parsing
    let parser: ParserState
    parse(lexer, parser)

    // 5. Semantic analysis
    let semantic: SemanticState
    analyze(parser, semantic)

    // 6. Layout planning
    let layout: LayoutPlan
    plan_layout(parser, semantic, layout)

    // 7. Code generation
    let codegen: ClifBuilder
    generate(parser, semantic, layout, codegen)

    // 8. Write CLIF output
    let clifPath: ascii[256] = "temp.clif"
    sys_write_file(clifPath, codegen.output)

    // 9. Invoke AOT tool
    let aotCommand: ascii[1024]
    str_format(aotCommand, "stasis-cranelift-aot.exe --input %s --output %s.obj", clifPath, outputFile)
    let aotResult: i32 = sys_exec(aotCommand)

    // 10. Link
    let linkCommand: ascii[1024]
    str_format(linkCommand, "clang %s.obj -o %s.exe", outputFile, outputFile)
    let linkResult: i32 = sys_exec(linkCommand)

    return 0
}
```

**Estimated Size:** ~200-300 lines of Stasis

---

## Data Structure Design

### Memory Budget

With static memory only, we need fixed upper bounds:

| Structure | Element Size (est.) | Count | Total |
|-----------|---------------------|-------|-------|
| Source buffer | 1 byte | 500,000 | 500 KB |
| Tokens | 24 bytes | 50,000 | 1.2 MB |
| AST nodes | 24 bytes | 100,000 | 2.4 MB |
| Children array | 4 bytes | 300,000 | 1.2 MB |
| Symbols | 160 bytes | 10,000 | 1.6 MB |
| Types | 80 bytes | 1,000 | 80 KB |
| CLIF output | 1 byte | 1,000,000 | 1 MB |
| **Total** | - | - | **~8 MB** |

This is well within reasonable limits for a compiler.

### String Handling Challenge

Stasis uses fixed-size strings. For the compiler:

1. **Token text**: Store indices into source (start + length), not copies
2. **Identifiers**: Max 128 characters (reasonable limit)
3. **Error messages**: Max 256 characters per diagnostic
4. **CLIF output**: 1MB buffer with append operations

### No Hash Tables

Without dynamic allocation, we use:
- **Linear search** for small collections (<100 items)
- **Binary search** on sorted arrays for larger collections
- **Direct indexing** where possible (enums as indices)

For keywords (40 words), linear search is acceptable at ~20 comparisons average.

---

## Memory Management Strategy

### Global State Pattern

All compiler state lives in global struct instances:

```stasis
global compiler: CompilerState

struct CompilerState {
    lexer: LexerState,
    parser: ParserState,
    semantic: SemanticState,
    layout: LayoutPlan,
    codegen: ClifBuilder
}
```

### Array Pools

For dynamically-sized collections within fixed bounds:

```stasis
struct TokenPool {
    items: Token[50000],
    count: i32
}

function token_pool_add(pool: TokenPool, token: Token): i32 {
    let index: i32 = pool.count
    pool.items[index] = token
    pool.count = pool.count + 1
    return index
}
```

### String Interning (Optional)

To save memory on repeated identifiers:

```stasis
struct StringPool {
    data: ascii[100000],      // concatenated strings
    dataLen: i32,
    offsets: i32[10000],      // start positions
    lengths: i32[10000],
    count: i32
}
```

---

## External Process Integration

### AOT Tool Invocation

The self-hosted compiler must invoke the Rust AOT tool:

```stasis
function invoke_aot(clifPath: ascii[256], objPath: ascii[256], moduleName: ascii[64]): i32 {
    let command: ascii[1024]

    // Build command line
    str_copy(command, "stasis-cranelift-aot.exe --input ")
    str_append(command, clifPath)
    str_append(command, " --output ")
    str_append(command, objPath)
    str_append(command, " --module-name ")
    str_append(command, moduleName)
    str_append(command, " --target x86_64-pc-windows-msvc")

    return sys_exec(command)
}
```

### Linker Invocation

```stasis
function invoke_linker(objPath: ascii[256], exePath: ascii[256], enableGraphics: bool): i32 {
    let command: ascii[2048]

    str_copy(command, "clang ")
    str_append(command, objPath)
    str_append(command, " -o ")
    str_append(command, exePath)

    if enableGraphics {
        str_append(command, " -L. -lstasis_graphics_static")
        str_append(command, " -lSDL2 -lopengl32 -lgdi32 -luser32")
    }

    return sys_exec(command)
}
```

### Error Handling

When external processes fail:

```stasis
function compile(input: ascii[256], output: ascii[256]): bool {
    // ... generate CLIF ...

    let aotResult: i32 = invoke_aot(clifPath, objPath, moduleName)
    if aotResult != 0 {
        print_error("AOT compilation failed with code: ")
        print_i32(aotResult)
        return false
    }

    let linkResult: i32 = invoke_linker(objPath, exePath, false)
    if linkResult != 0 {
        print_error("Linking failed with code: ")
        print_i32(linkResult)
        return false
    }

    return true
}
```

---

## Bootstrap Strategy

### Stage 0: C# Compiler (Current)

The existing C# compiler serves as the bootstrap compiler.

### Stage 1: Minimal Self-Hosted Compiler

Write a minimal Stasis compiler in Stasis that can compile itself:

1. Implement lexer in Stasis
2. Compile lexer with C# compiler
3. Test lexer thoroughly

4. Implement parser in Stasis
5. Compile lexer + parser with C# compiler
6. Test parser thoroughly

7. Continue for semantic analyzer, codegen
8. Result: Full compiler in Stasis, compiled by C# compiler

### Stage 2: Self-Compilation

Use the Stage 1 compiler (compiled by C#) to compile itself:

```
stasis-stage1.exe compiler.stasis -o stasis-stage2.exe
```

### Stage 3: Verification

Compare Stage 1 and Stage 2 outputs:

1. Compile test programs with both
2. Verify identical behavior
3. Optionally: compare generated CLIF (may differ in cosmetic ways)

### Ongoing: Triple Build

For confidence, maintain triple-stage builds:

```
C# compiler     →  compiles  →  Stage 1 (stasis1.exe)
Stage 1         →  compiles  →  Stage 2 (stasis2.exe)
Stage 2         →  compiles  →  Stage 3 (stasis3.exe)

Stage 2 output == Stage 3 output  ✓ (fixed point)
```

---

## Risk Assessment

### High Risk

| Risk | Impact | Mitigation |
|------|--------|------------|
| Static memory insufficient | Compiler fails on large programs | Generous upper bounds; test with Asteroids sample |
| String operations too slow | Compile times unacceptable | Optimize hot paths; use indices not copies |
| Missing language features | Blocks implementation | Phase 1 fills gaps before starting |
| CLIF generation bugs | Silent miscompilation | Extensive test suite; compare with C# output |

### Medium Risk

| Risk | Impact | Mitigation |
|------|--------|------------|
| Symbol resolution edge cases | Subtle bugs | Port C# test suite to Stasis |
| Precedence climbing errors | Parse errors | Careful implementation; unit tests |
| File I/O failures | Crash | Defensive error handling |
| External tool not found | Runtime failure | Helpful error messages with path hints |

### Low Risk

| Risk | Impact | Mitigation |
|------|--------|------------|
| Lexer performance | Slower tokenization | Acceptable for development |
| CLIF output verbosity | Larger temp files | No significant impact |

---

## Milestones and Success Criteria

### Milestone 1: Language Prerequisites Complete

**Criteria:**
- [ ] `sys_read_file()` reads file into ascii buffer
- [ ] `sys_write_file()` writes ascii buffer to file
- [ ] `sys_exec()` runs external process, returns exit code
- [ ] String append operations work correctly
- [ ] Arrays of 100,000+ elements compile and work

### Milestone 2: Lexer Self-Hosted

**Criteria:**
- [ ] Lexer written entirely in Stasis
- [ ] Tokenizes all valid Stasis programs
- [ ] Produces identical token streams as C# lexer
- [ ] Handles all edge cases (comments, escapes, Unicode)

### Milestone 3: Parser Self-Hosted

**Criteria:**
- [ ] Parser written in Stasis
- [ ] Builds correct AST for all language constructs
- [ ] Error recovery works (doesn't crash on invalid input)
- [ ] Matches C# parser behavior

### Milestone 4: Semantic Analyzer Self-Hosted

**Criteria:**
- [ ] Type checking works for all types
- [ ] Symbol resolution correct
- [ ] All semantic errors detected
- [ ] Built-in functions registered

### Milestone 5: Code Generator Self-Hosted

**Criteria:**
- [ ] Generates valid CLIF for all constructs
- [ ] CLIF compiles with AOT tool
- [ ] Generated executables work correctly
- [ ] Matches C# codegen behavior

### Milestone 6: Self-Compilation Achieved

**Criteria:**
- [ ] Self-hosted compiler compiles itself
- [ ] Resulting compiler produces identical output
- [ ] Triple-build reaches fixed point

### Milestone 7: C# Compiler Deprecated

**Criteria:**
- [ ] All tests pass with self-hosted compiler
- [ ] Development workflow uses self-hosted compiler
- [ ] C# compiler archived (kept for historical reference)

---

## Estimated Effort

| Phase | Lines of Stasis | Relative Effort |
|-------|-----------------|-----------------|
| Phase 1: Prerequisites | N/A (C/Rust) | Medium |
| Phase 2: Lexer | 400-500 | Low |
| Phase 3: Parser | 800-1000 | Medium |
| Phase 4: Semantic | 1000-1200 | High |
| Phase 5: Layout | 300-400 | Low |
| Phase 6: Codegen | 1500-2000 | High |
| Phase 7: Driver | 200-300 | Low |
| **Total** | **4200-5400** | - |

The self-hosted compiler would be roughly the same size as the C# implementation, which validates that Stasis is expressive enough for the task.

---

## Appendix A: CLIF Quick Reference

### Types

```clif
i8, i16, i32, i64   ; signed integers
f32, f64            ; floats
b1                  ; boolean
r64                 ; pointer (reference)
```

### Instructions

```clif
; Constants
v0 = iconst.i32 42
v1 = f32const 3.14

; Arithmetic
v2 = iadd v0, v1
v3 = isub v0, v1
v4 = imul v0, v1
v5 = sdiv v0, v1
v6 = srem v0, v1

; Floating-point
v7 = fadd v0, v1
v8 = fsub v0, v1
v9 = fmul v0, v1
v10 = fdiv v0, v1

; Comparisons
v11 = icmp eq v0, v1
v12 = icmp slt v0, v1
v13 = fcmp gt v0, v1

; Memory
ss0 = stack_slot 4       ; 4-byte slot
v14 = stack_addr.r64 ss0
store.i32 v0, v14
v15 = load.i32 v14

; Globals
v16 = global_value.r64 gv_name
v17 = load.i32 v16

; Control flow
brif v0, block1, block2
jump block3

; Function calls
v18 = call %func(v0, v1)
```

---

## Appendix B: Comparison with Other Self-Hosted Compilers

| Compiler | Bootstrap Language | Target | Complexity |
|----------|-------------------|--------|------------|
| Go | C (original) | Go | High |
| Rust | OCaml (original) | Rust | Very High |
| Zig | C++ (original) | Zig | High |
| Nim | Pascal (original) | Nim | Medium |
| **Stasis** | C# (current) | CLIF+Cranelift | **Medium** |

Stasis has an advantage: by targeting CLIF text and delegating to an external Cranelift AOT tool, the self-hosted compiler avoids implementing a full code generator. This significantly reduces complexity.

---

## Appendix C: Test Strategy

### Unit Tests

Each component has isolated tests:

```stasis
test lexer_integer_literal {
    let source: ascii[32] = "42"
    let lexer: LexerState
    lex(source, 2, lexer)
    assert(lexer.tokens[0].kind == TokenKind.IntLiteral)
}

test parser_binary_expression {
    let source: ascii[32] = "1 + 2"
    let parser: ParserState
    // ... parse and verify AST structure
}
```

### Integration Tests

Compile and run test programs:

```stasis
test compile_hello_world {
    let result: i32 = compile_and_run("samples/hello.stasis")
    assert(result == 0)
}
```

### Comparison Tests

Verify self-hosted matches C# compiler:

```stasis
test codegen_matches_csharp {
    let clifSelfHosted: ascii[100000]
    let clifCSharp: ascii[100000]

    // Generate CLIF with both compilers
    // Compare (ignoring comments and whitespace)
}
```
