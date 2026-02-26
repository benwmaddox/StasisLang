# Stasis Compiler PRD
## Version 2 - Compile Speed Optimized

Primary intent:
- Cold start < 250ms @ 1k funcs
- Incremental edits < 5ms typical
- Minimal architectural overhead
- No unnecessary abstraction
- Fast dependency invalidation
- Direct-to-CLIF emission
- Compile-speed prioritized over memory footprint

Scope note:
- This document is the active scope anchor for the compiler speed lock-in slices in `docs/rewrite_v1_checklist.md` (`CS0` - `CS8`).

## 1. Goals

Primary:
- Sub-5ms incremental compile for single-function edits
- <1s cold start for 5k functions
- Deterministic rebuilds
- No multi-phase IR pipeline
- O(1) symbol lookup
- Minimal heap churn

Non-goals:
- Minimal memory usage
- Generic system
- Macro system
- Cross-module linking

## 2. Compile Pipeline Overview

Two-phase model:
1. Index pass
   - Parse signatures only
   - Build global symbol table
   - Detect signature changes
   - Mark dirty functions
2. Emit pass
   - Parse body of dirty functions
   - Type resolve
   - Emit CLIF
   - Finalize via Cranelift
   - Patch call table

No AST retained after emission.

## 3. Global Compiler Structure

```stasis
struct Compiler {
    files: File[]                    // all loaded source files
    functions: FunctionMeta[]        // flat array of all functions
    symbol_table: SymbolTable
    dep_graph: DependencyGraph
    jit: JitBackend
    scratch: CompileScratch
}
```

Flat arrays only. No trees. No pointers to small allocations.

## 4. Source File Model

```stasis
struct File {
    path: ascii[256]
    content: utf8[?]                 // full file buffer
    hash: u64                        // fast rolling hash
    functions: u32[]                 // indices into Compiler.functions
}
```

Files are immutable between compiles. Entire file hash decides whether re-index needed.

## 5. Function Metadata (Hot Path Struct)

```stasis
struct FunctionMeta {
    name: ascii[64]
    name_hash: u64

    file_id: u32
    source_start: u32
    source_end: u32

    signature_hash: u64
    body_hash: u64

    param_types: TypeId[8]
    param_count: u8
    return_type: TypeId

    dependency_start: u32
    dependency_count: u32

    dependent_start: u32
    dependent_count: u32

    code_ptr: u64
    dirty: bool
}
```

Design notes:
- No dynamic allocations per function
- Dependencies stored in global flat arrays
- Max 8 params (expandable)
- Fixed-size param array avoids heap churn
- Hashes allow fast change detection

## 6. Type System Representation

Types are interned.

```stasis
type TypeId = u16

struct TypeInfo {
    name: ascii[32]
    size: u16
    flags: u16
}
```

```stasis
struct TypeTable {
    types: TypeInfo[256]
    count: u16
}
```

Type lookup is O(1) via interned ID.
No string comparisons during body parse.

## 7. Symbol Table

Pure hash table.

```stasis
struct SymbolEntry {
    name_hash: u64
    function_id: u32
}

struct SymbolTable {
    entries: SymbolEntry[8192]
    count: u32
}
```

Open addressing. Linear probe. No chaining.

## 8. Dependency Graph

Flat adjacency lists.

```stasis
struct DependencyGraph {
    edges: u32[?]            // function indices
}
```

Each function stores:
- dependency_start
- dependency_count
- dependent_start
- dependent_count

Enables O(N) dirty propagation with zero allocations during propagation.

## 9. JIT Backend

Thin wrapper around Cranelift.

```stasis
struct JitBackend {
    module_ptr: u64
    builder_ptr: u64
}
```

Compiler does not store CLIF.
CLIF emitted directly during parse.
No intermediate IR layer.

## 10. Compile Scratch (No Heap Thrash)

```stasis
struct CompileScratch {
    token_buffer: Token[2048]
    value_stack: ValueId[256]
    type_stack: TypeId[256]
}
```

Reused per function compile.
Zero per-function heap allocs.

## 11. Index Pass Algorithm

For each file:
1. If `file.hash` unchanged -> skip
2. Scan tokens
3. For each `fn`:
   - Parse name
   - Parse param types
   - Parse return type
   - Compute `signature_hash`
   - Compare with previous
   - If changed -> mark dirty

## 12. Emit Pass Algorithm

For each dirty function:
1. Reset scratch
2. Parse body directly from source buffer
3. During parse:
   - Resolve identifiers via `symbol_table`
   - Record dependencies
   - Emit CLIF inline
4. Finalize via Cranelift
5. Store `code_ptr`
6. Mark clean

Dependency ripple:

```stasis
propagate_dirty(function_id)
```

DFS over dependents; marks only affected functions.

## 13. Compile Time Model (Target)

Assume 1,000 functions.

Cold start target model:
- Load: 10 ms
- Index: 15 ms
- Parse+Resolve: 50 ms
- CLIF Emit: 35 ms
- Cranelift: 90 ms
- Total: ~200 ms

Incremental single-function target model:
- Hash check: 0.01 ms
- Reparse: 0.07 ms
- CLIF: 0.05 ms
- Cranelift: 0.1 ms
- Dependents (3 avg): 0.5 ms
- Total: ~1-2 ms

Medium change (25 functions): ~8-12 ms

Pathological ripple (400 functions): ~70-90 ms

## 14. Why This Design Is Fast

- No AST retained
- No IR transform passes
- No heap allocation per node
- Flat arrays only
- Signature hashing avoids deep comparisons
- O(1) symbol lookup
- Direct CLIF emission
- Minimal abstraction layers

## 15. Potential Future Optimizations

- Parallel emission of independent dirty functions
- Batched Cranelift finalize
- Function-level code caching across sessions
- Precomputed lexical token maps

## 16. Tradeoffs

Accepted:
- Slightly higher memory usage
- No macro system
- No heavy type inference
- Hard limit on param count unless expanded

Rejected:
- Multi-phase IR
- Visitor patterns
- Tree-based AST storage
- String-based type resolution
- Linked-list dependency storage

## 17. Final Architectural Character

The Stasis compiler is:
- Single-pass where possible
- Two-phase where required
- Flat-memory oriented
- Hash-driven
- Deterministic
- JIT-focused
- Optimized for compile latency over memory
