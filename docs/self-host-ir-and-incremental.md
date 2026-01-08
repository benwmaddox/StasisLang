# Self-host compiler: IR layer + incremental compilation plan

This document is a concrete plan to introduce a backend-agnostic IR step to the self-host Stasis compiler and then use it to make `watch` and everyday builds fast and predictable.

Goals:
- Add a structured mid-level IR (MIR) between frontend and backends.
- Make LLVM and CLIF emitters mostly "printers" over MIR (shared semantics).
- Enable fast incremental compilation for `watch` and `test` (reuse tokens/decls/type info, avoid full rebuilds).
- Keep everything deterministic and bounded (static memory, explicit caps, explicit clears).

Non-goals (initially):
- A full optimizer pipeline (beyond "cheap, local" canonicalizations).
- Import expansion / macro-style source rewriting (imports remain modules; no aliasing; no flattening).
- A JIT or embedded LLVM/Cranelift (still textual IR + external tools).

Constraints (must hold):
- Single global state struct `stasis` owns all compiler allocations.
- Up to 300 files, up to 1 MiB per file.
- Prefer iteration over recursion (explicit stacks/queues).
- Keep files ASCII (docs and source).

## 1) Current architecture (summary)

Today the self-host pipeline is roughly:
1. Load source graph (import scan + file table).
2. Lex + decl scan + stmt parsing (bring-up subset).
3. Emit backend text directly (LLVM IR for most of the working subset; CLIF is still minimal).

This is great for bootstrap speed, but it makes:
- LLVM and CLIF parity expensive (two independent implementations of semantics),
- Incremental compilation harder (because a lot of semantic work is intertwined with emitting backend text),
- Optimization and diagnostics harder to make consistent.

The IR step fixes those issues by making "frontend semantics" the single source of truth.

## 2) Proposed pipeline with MIR

New pipeline (steady state):
1. Source graph: load modules, record imports, mtimes, hashes.
2. Lex: produce tokens (spans into source pool).
3. Parse: produce a syntactic AST for decls + bodies (or a compact HIR).
4. Resolve + typecheck: names, imports/modules, types, const-eval for sizes/layout.
5. Lower to MIR (typed, explicit control-flow, explicit memory ops).
6. (Optional) MIR passes: cheap canonicalization, local simplification.
7. Backend emit:
   - MIR -> LLVM IR text
   - MIR -> CLIF text
8. Tool invocation: `lli` fast path / `clang` link, and `tools/cranelift-aot` for CLIF.

Key property: steps 5-7 are the only backend-specific parts, and steps 1-4 are shared.

## 3) MIR scope and design

The MIR should model exactly what Stasis can do, with as little "backend flavor" as possible.

### 3.1 IR units

- Module unit: one file = one module (as today).
- Function unit: function body lowered to CFG blocks.
- Global unit:
  - A single program-global struct (per Stasis model) is still the conceptual model.
  - Lowering may split storage into SoA behind the scenes, but MIR should represent this explicitly.

### 3.2 Types in MIR

MIR uses resolved type IDs (indices into `stasis.types[]`) and a small set of primitive MIR value kinds:
- `I32`, `F32`, `Ptr`, `Void`
- `I8` only when needed for byte-addressing and mem ops (otherwise keep `u8` as `I32` value semantics and lower stores/loads explicitly).

The MIR should not carry "named" types as strings; it references the compiler's type table by index.

### 3.3 Values and locations (critical for Stasis)

Stasis is "explicit memory writes" and static storage; MIR must separate:
- Values (SSA-ish, pure): temps produced by ops.
- Places (addresses): where stores happen.

Represent:
- `MirValueId` for SSA-like temporaries.
- `MirPlaceId` for addressable locations:
  - local slot
  - global slot
  - struct field address
  - array element address

This also makes it straightforward to support:
- `_ = expr;` discard (evaluate for effects only),
- compound assignment `x += y`,
- efficient `mem_clear` / bulk zero.

### 3.4 Instructions (initial set)

Minimal set for parity with current LLVM subset:
- Consts: `const_i32`, `const_f32`, `const_ptr` (for string/global addresses).
- Arithmetic/compare: `add/sub/mul/div/rem`, `icmp`, `fcmp`.
- Casts: `i32_to_f32`, `f32_to_i32`, `zext_i8_to_i32`, `trunc_i32_to_i8`, plus checked casts as explicit control-flow helpers.
- Control flow:
  - `br`, `br_if`, `switch` (optional later)
  - blocks with phi arguments OR explicit `phi` nodes (choose one style; block args are simpler to emit consistently).
- Memory:
  - `addr_of_local`, `addr_of_global`
  - `load`, `store`
  - `gep_struct_field`, `gep_array_index`
  - `memset_zero`, `memcpy` (both length as i32)
- Calls:
  - direct call to resolved function ID
  - extern call to symbol (sys/graphics/libc)

Builtin lowering should happen before MIR emission finalizes:
- Convert surface-level builtins into explicit MIR calls or MIR intrinsics with explicit semantics.

### 3.5 CFG representation (iteration-friendly)

Use explicit arrays for:
- `mir_blocks[]`: start offsets into `mir_insts[]`, successor lists, optional block args.
- `mir_insts[]`: fixed-size instruction structs with tagged unions.
- `mir_operands[]`: packed operand stream referenced by offsets for variable-arity ops (calls, phi/block args).

No recursion is required:
- Expression lowering uses an explicit value stack (already in spirit with current code).
- Statement lowering uses an explicit block stack (already used in LLVM emitter).

### 3.6 Diagnostics mapping

Every MIR instruction and place should carry a source span `at` (token offset) so:
- backend errors can point at source,
- MIR verification failures are actionable.

## 4) MIR verification (must-have)

Before emitting backends, validate MIR cheaply:
- Every value is defined before use.
- Block arguments match predecessor edges.
- Types of operands match instruction expectations.
- Places are valid (no field access on non-struct, no index on non-array).
- All blocks are terminated (br/ret).

This is key to catching bugs early and keeping LLVM/CLIF emitters simpler.

## 5) Backend emission strategy

### 5.1 LLVM emitter becomes a printer

LLVM emitter reads MIR and prints:
- function signatures
- allocas for locals (still LLVM-friendly)
- instructions mapped 1:1 where possible
- control-flow labels and phis (or block arg translation)

Avoid IR "feature detection by text search" long-term:
- Track link requirements while lowering to MIR (sys/graphics/libc usage flags).
- Emit those requirements directly in compiler state (not by scanning output).

### 5.2 CLIF emitter parity path

CLIF emitter reads the same MIR and prints:
- function bodies and blocks
- calls to the same extern symbols

Because MIR is shared, CLIF parity becomes "finish emitter coverage" rather than "re-implement semantics".

## 6) Incremental compilation plan (watch-focused)

The IR step enables incremental compilation because it creates stable phase boundaries with cacheable artifacts.

### 6.1 What to cache (in memory for watch)

Per file (module):
- File hash (content hash; use a fast deterministic 64-bit hash).
- Token stream (spans into source pool).
- Decl index ranges:
  - types/structs/enums/globals/consts/functions declared in that module
- Optional: parsed body AST/HIR for each function in the module.

Global caches (cross-module):
- Resolved import graph and module name table.
- Symbol table per module (exported members).
- Type table + layout results (struct sizes/align/field offsets).

Per function:
- Typed HIR (or directly MIR) for the function body.
- Backend text can be rebuilt from MIR cheaply.

### 6.2 Invalidation rules

On a file change:
1. Re-read file, recompute hash.
2. If hash unchanged: skip.
3. If changed:
   - Re-lex that file.
   - Re-parse decls (and bodies if needed).
   - Update module symbol table for that file.
4. Compute impacted set:
   - Any module that imports the changed module may need name resolution updates.
   - Any function that depends on changed symbols/types must be re-typechecked and re-lowered.

Impact tracking approach (bounded and explicit):
- Maintain reverse import edges: `imported_by[module] -> list of modules`.
- Maintain "use edges" for symbols/types:
  - During resolve/typecheck, record "this function depends on these type IDs and function IDs".
  - Store edges in fixed-capacity adjacency lists.

Initial version (simpler, still fast enough):
- On file change: re-typecheck and re-lower all functions in the transitive import closure.
- Optimize later by using dependency edges once correctness is proven.

### 6.3 Output artifacts and reuse

For watch:
- Keep MIR for unchanged functions and reuse it.
- Only re-emit backend text for changed functions (or changed globals/layout).
- If backend tool requires whole-module text (LLVM IR file): re-print the module file, but reuse cached MIR to keep this CPU-cheap.

### 6.4 On-disk cache (optional, phase 2)

Goal: speed up cold starts (not just watch).

Cache key:
- Compiler version hash (or git commit).
- Backend + target triple + toolchain mode (llvm/clang flags).
- File graph hash (hash of per-file hashes + import edges).

Cache contents:
- Per module: tokens + decl summaries.
- Per function: MIR blob (serialized).

Constraints:
- Deterministic serialization.
- Versioned format with a clear invalidation story.
- Hard size limits (avoid unbounded disk growth).

## 7) Iteration speed: process + tooling improvements

Incremental compilation only pays off if tool invocation overhead is also controlled.

High-impact follow-ups that complement MIR:
- Add `sys_spawn` argv-based process execution to avoid shell quoting and reduce overhead vs `system()`.
- Add "tool discovery" cached per process (find `lli`, `clang`, cranelift tools once).
- Add a "fast check" mode:
  - graph + lex + parse + resolve/typecheck only
  - no MIR emission unless requested

## 8) Milestones (actionable sequence)

### M1: Introduce MIR data structures (no behavior change yet)
- Add MIR tables to `src/stasis/state.stasis` with conservative caps.
- Add `mir_reset()` with explicit clear ranges (prefer bulk zero where safe).
- Add minimal MIR verifier scaffolding (no-op initially).

### M2: Lower a small subset to MIR and emit LLVM from MIR
- Start with `main` only, i32 locals/consts, return, simple arithmetic.
- Emit LLVM IR from MIR for that subset.
- Keep old direct LLVM path behind a flag until parity is proven.

### M3: Expand MIR lowering to current LLVM bring-up subset
- `if`/`else`, `for`, function calls, struct fields, indexing, string literals, sys/graphics builtins.
- Add MIR verifier rules as features land.
- Delete duplicated direct-to-LLVM logic as coverage moves to MIR.

### M4: Add CLIF emitter from MIR (subset first, then parity)
- Emit CLIF for the same MIR subset accepted by LLVM path.
- Wire into `stasis build --backend cranelift --emit-ir`.

### M5: In-memory incremental compilation for `watch`
- Keep token+decl caches across ticks.
- Implement transitive import-closure invalidation first (correctness).
- Add timings per phase (lex/parse/resolve/typecheck/mir/emit/link).

### M6: Dependency-edge refinement (speed)
- Record per-function dependencies (types/functions/globals).
- Recompute minimal impacted function set on changes.
- Add stable "what changed" diagnostics in watch output.

### M7: Cold-start improvements
- Optional on-disk cache for tokens/decls/MIR.
- Persisted watch daemon mode (optional) so editors can reuse one process.

## 9) Acceptance criteria

Correctness:
- LLVM and CLIF backends run the same test corpus with matching behavior for the supported subset.
- MIR verifier catches internal inconsistencies before backend emission.

Iteration speed:
- `stasis watch check` responds to a small edit in a leaf module without reprocessing unrelated modules (visible in phase timings).
- `stasis test tests --quiet` p50 and p95 improve vs today, with reduced variance (especially from tool invocation).

Maintainability:
- There is exactly one place to define semantics: MIR lowering rules.
- Backend emitters are thin, deterministic printers over MIR.

