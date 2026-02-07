# Incremental Watch Compiler + Cranelift JIT Architecture (Stasis-First)

This document describes a high-level architecture for an incremental Stasis compiler service written in idiomatic Stasis that can keep end-to-end edit -> running code updates close to ~100ms for watched projects.

It is intentionally biased toward:
- Static memory, deterministic behavior, and predictable costs.
- Incremental recomputation (only what changed) with aggressive caching.
- A tight integration path to Cranelift JIT hot-swap.

The goals and constraints below are treated as hard requirements.

## Goals and Constraints

1. Written in idiomatic Stasis
- Stasis owns the long-running incremental pipeline and the invalidation logic.
- Host calls (sys_*) are used for OS integration (filesystem, watching, process I/O), not for the compiler core.

2. Maintain records of files, ASTs, etc. between file changes
- A resident service keeps a project database in global static memory.
- Reuse prior parse trees / function IR when unchanged.

3. Fixed file size and file count
- Max file size: 100 KB per file.
- Max file count: 10,000.
- All in-memory buffers and indices are preallocated.

4. File watching hooks
- Update a single file quickly.
- Diff and impact analysis should allow updating only impacted functions.
- Support Cranelift JIT "pushes" for just the changed functions (plus any invalidated callers).

5. Keep end-to-end updates close to 100ms or lower
- In steady state, common edits should recompile only 1-20 functions and patch the JIT.
- Avoid full rebuilds except when layout/globals change or when the user requests it.

6. Smart invalidation for inlining changes
- Track inline decisions.
- When an inlined callee changes, invalidate the transitive closure of callers that inlined it.
- This likely requires caching backend IR and metadata.

## Non-Goals (for this architecture)

- A complete self-hosting toolchain from day one.
- Perfect incremental parsing at token-level diffs.
  - We aim for function-level stability: re-lex and re-parse the changed file, then diff by declaration identity and hashes.
- Optimizing release builds.
  - This is primarily a dev/watch architecture; release can still use LLVM or a non-incremental pipeline.

## One-Sentence Summary

Run a long-lived Stasis "compiler daemon" that keeps a fixed-capacity project database (files, import graph, per-function AST/HIR, symbol table slices, call graph, inline edges, and per-function backend IR caches), and on each file change recomputes only the minimal affected sets of functions and pushes just those updated functions (and any invalidated inlined callers) to a Cranelift JIT runner.

## System Overview

There are three cooperating layers:

1. Stasis incremental compiler service (written in Stasis)
- Owns: project database, parsing, semantic checks, dependency graphs, invalidation, caching keys, delta planning.
- Produces: per-function IR (CLIF-like) or a compact "lowering IR" that the JIT runner can compile.

2. Host integration (native runtime)
- Owns: OS file watching, timers, and fast interprocess (or in-process) messaging.
- Provides: a fixed-size ring buffer of file change events and a way to deliver file contents quickly.

3. Cranelift JIT runner
- Owns: compilation to machine code, code memory management, patching/indirection, calling convention stability.
- Accepts: a batch of function updates and returns a "publish" result.

The key to the 100ms target is a fast "delta plan": on each change, compute a small set of impacted functions and emit only those.

## Data Model (All Fixed Capacity)

Below are Stasis-style declarations. Note: Stasis structs in globals are lowered to SoA; locals hold indices/references.

### Global constants

```stasis
const MAX_FILES: i32 = 10000;
const MAX_FILE_BYTES: i32 = 100 * 1024;

// Conservative maxima; tune by profiling.
const MAX_PATH_BYTES: i32 = 260;
const MAX_IMPORTS_PER_FILE: i32 = 256;
const MAX_DECLS_PER_FILE: i32 = 4096;
const MAX_FUNCTIONS: i32 = 200000;
const MAX_SYMBOLS: i32 = 400000;

const MAX_STRING_POOL_BYTES: i32 = 64 * 1024 * 1024;
const MAX_STRING_POOL_SLOTS: i32 = 200000;

const MAX_WATCH_EVENTS: i32 = 8192; // ring buffer capacity
const MAX_CHANGED_FILES_PER_TICK: i32 = 256;

const HASH_SEED: u32 = 2166136261u32; // example
```

### Identifiers and strings: interning

Interning is mandatory to keep comparisons and dependency sets cheap.

Design:
- Fixed byte pool for string storage (`string_pool_bytes`).
- Fixed open-addressing hash table (`string_slots`).
- Interned string id (`str_id`) is an `i32` index into the slot table.

```stasis
struct StringSlot {
  hash: u32;
  start: i32;  // byte offset into pool
  len: i32;
  used: bool;
}

global string_pool: u8[MAX_STRING_POOL_BYTES];
global string_pool_len: i32;

global string_slots: StringSlot[MAX_STRING_POOL_SLOTS];
```

Rules:
- Intern all file paths, import paths, symbol names, and qualified names.
- Interned strings are immutable for the lifetime of the process.

### File table

Each file has fixed space for:
- Path
- Current content bytes (100 KB)
- A small summary of its parsed declarations
- Import list
- Versioning and hashing info

```stasis
struct FileRec {
  path: i32;             // interned string id
  exists: bool;

  // Content buffer
  bytes: u8[MAX_FILE_BYTES];
  byte_len: i32;

  // Change tracking
  mtime_ms: i32;
  content_hash: u32;
  parse_version: i32;    // increments when re-parsed
  sema_version: i32;     // increments when sema for this file is updated

  // Imports (interned string ids)
  imports: i32[MAX_IMPORTS_PER_FILE];
  import_count: i32;
  import_hash: u32;

  // Declaration index range into global decl table (or per-file decl list)
  decl_start: i32;
  decl_count: i32;
}

global files: FileRec[MAX_FILES];
global file_count: i32;
```

Important: with 10,000 files at 100 KB each, storing full contents for every file is up to ~1 GB of raw bytes.

This architecture supports two storage modes:

- Mode A (small projects): store all file bytes resident.
- Mode B (large projects): store only changed/recent files resident.
  - Still keep file metadata and hashes for all files.
  - Keep an LRU cache of N file byte buffers (for example 512 or 2048) and reload others on demand.

Because the constraints explicitly say "fixed file size" and "fixed file count", not "must store all contents simultaneously", Mode B is strongly recommended to keep memory reasonable.

Implementation detail for Mode B:
- Replace `bytes` with an index into a fixed pool of `FileBufferRec[BUFFER_SLOTS]`.
- Maintain an LRU list and a pin-count for in-flight compilation.

### Declarations and per-function representation

To achieve fast incremental work, avoid storing a full syntax tree for every token in every file.

Instead:
- Keep a compact per-function AST/HIR that is stable across edits.
- Track declaration identity by (file_id, decl_kind, name_id, signature_hash).

Suggested approach:
- Parse the changed file into a temporary AST arena.
- Convert each function body into a compact HIR (linear nodes) stored in a global HIR arena.
- Free the temporary parse arena after diffing and re-materializing changed HIR.

Key benefit:
- HIR is smaller than a full parse tree and easier to hash and lower incrementally.

Example HIR record layout:

```stasis
enum HirKind { HirInvalid, HirConstI32, HirLoadLocal, HirStoreLocal, HirCall, HirBinOp, HirIf, HirLoop, HirReturn }

struct HirNode {
  kind: i32;       // HirKind
  a: i32;
  b: i32;
  c: i32;
  span_start: i32;
  span_len: i32;
}

global hir_nodes: HirNode[/* big fixed cap */];
global hir_node_len: i32;
```

You still keep enough mapping to report diagnostics using source spans.

### Symbol table and per-function dependency sets

The incremental engine needs to answer:
- If symbol X changes, which functions must be re-analyzed or re-lowered?

Do this with explicit dependency sets:

- Each function maintains:
  - The set of referenced global symbols.
  - The set of referenced types/structs/enums.
  - The set of called functions.

- Maintain reverse indexes:
  - symbol -> functions that depend on it
  - function -> callers (call graph)
  - function -> inline-callers (inline graph)

All sets must be fixed capacity.

Representation options:

1. Sorted small vectors with dedup
- Works well when most functions touch few symbols.
- Great for 100ms target.

2. Bitsets
- Too large at 200k functions and 400k symbols unless heavily compressed.

Recommended:
- Use sorted vectors with a hard cap per function (for example 512 deps) plus spill diagnostics when exceeded.

```stasis
struct DepList {
  start: i32;
  len: i32;
}

global deps_data: i32[/* big cap */];

global fn_symbol_deps: DepList[MAX_FUNCTIONS];
global fn_call_deps: DepList[MAX_FUNCTIONS];

global rev_symbol_deps_head: i32[MAX_SYMBOLS];
// plus a fixed intrusive list node pool: (next, fn_id)
```

In practice, keep both forward and reverse. Forward is used to remove old deps when a function changes; reverse is used to compute impacted sets.

### Function metadata and versioning

```stasis
struct FnRec {
  name: i32;              // interned string id
  file_id: i32;
  decl_span_start: i32;
  decl_span_len: i32;

  sig_hash: u32;          // parameters + return type
  body_hash: u32;         // HIR hash

  // Semantic and layout coupling
  sema_env_hash: u32;     // summary hash of referenced symbols/types
  layout_version: i32;    // version of global layout used

  // Inline and call relationships
  size_estimate: i32;
  inline_hint: bool;

  // Backend caches
  ir_cache_key: u32;
  ir_blob_start: i32;
  ir_blob_len: i32;
  codegen_version: i32;
}

global fns: FnRec[MAX_FUNCTIONS];
global fn_count: i32;
```

Key idea:
- A function can be considered "unchanged" if both `sig_hash` and `body_hash` match.
- A function can reuse previous backend IR if the `ir_cache_key` matches.

The `ir_cache_key` should combine:
- body_hash
- sema_env_hash
- layout_version
- inline configuration version
- backend + optimization mode

## File Watching and Update Ingestion

### Preferred approach: host-driven watcher with a ring buffer

Polling 10,000 files every frame is likely too expensive for a 100ms target.

Instead:
- The host runtime registers OS file watch notifications.
- The host pushes events into a fixed-size ring buffer in exported globals.

Event record:
- file path (interned id or bytes)
- event type (modified, created, deleted)
- monotonic sequence number

Stasis reads events in a tick loop:

```stasis
function watch_tick(): void {
  let n: i32 = sys_pop_watch_events(MAX_CHANGED_FILES_PER_TICK);
  // For each event: map path -> file_id, reload contents, then process incrementally.
}
```

If a pure Stasis + sys_* implementation is required without new host APIs:
- Use `sys_file_mtime_ms` and maintain a directory listing via `sys_list_dir`.
- But still avoid scanning all files every tick: use a coarse timer and batch scanning.

### Loading a single file fast

Constraints:
- Max file size 100 KB.

Use:
- `sys_read_file(path, out_bytes, out_cap)` to load into a preallocated buffer.

Compute:
- `content_hash` using a fast non-crypto hash (FNV-1a, xxhash-like) implemented in Stasis.

Skip work when:
- mtime unchanged and hash unchanged.

## Incremental Pipeline: What Runs on Each Change

At a high level, the pipeline is:

1. Ingest changed file -> update `FileRec` -> compute new `content_hash`.
2. Parse changed file into temp AST arena.
3. Extract the file's declaration summary: imports, globals, types, function signatures, function bodies.
4. Diff old vs new declarations by identity.
5. Update global indices (symbols, import graph, function table).
6. Compute impacted set.
7. Re-analyze impacted functions (semantic).
8. Re-lower impacted functions.
9. Push function deltas to Cranelift JIT.

The key is step 6: compute a minimal impacted set.

### Step 2-4: Parsing and function-level diff

Minimum viable: re-parse the entire changed file.

Then diff by declaration identity:
- Struct/enum/global declarations: identity by name.
- Function declarations: identity by (name, signature hash).
- Tests (if enabled): identity by name.

For each function in the file:
- Compute `body_hash` from HIR.
- If a function with same identity exists and `body_hash` matches, keep previous function record and dependency sets.
- If body changed, mark function as "directly changed".

For deleted functions:
- Mark as removed.
- Invalidate callers (they will get diagnostics or link-time errors depending on policy).

### Step 5: Updating the import graph

Maintain an adjacency list per file:
- file -> imported files

When imports change:
- Any file that (transitively) depends on this file may need re-analysis, but do not eagerly rebuild everything.

Practical rule:
- If imports change, re-run semantic analysis for:
  - all declarations in the changed file
  - any files that import it directly
  - and any functions whose symbol deps include symbols defined in this file

This is conservative but usually small.

## Impact Analysis (Invalidation Engine)

We define several kinds of changes, each with different blast radius.

### Change class A: whitespace / comments

- If content_hash changes but function body hashes do not, do nothing.

### Change class B: function body change (no signature change)

- Re-analyze and re-lower that function.
- If the function is inlined into callers, invalidate those callers (see inline invalidation).
- Otherwise, callers do not need recompilation if calls are indirect through a function pointer table.

### Change class C: function signature change

- Re-analyze and re-lower the function.
- Invalidate all callers.
- Requires updating call sites or dispatch stubs.

### Change class D: global/const change

- If a global declaration changes type/size, layout may change.
- Invalidate any function that reads/writes that global.

### Change class E: struct/enum change

- If used in globals or in layout-relevant contexts, layout may change.
- Invalidate any function that accesses affected fields.

### Change class F: layout version bump

Layout is a cross-cutting concern.

Rule:
- If `layout_version` changes, you must invalidate all functions that embed offsets.

To keep the 100ms target:
- Make layout changes rare in the dev loop.
- Consider a "layout freeze" mode during watch sessions:
  - Disallow (or delay) new globals / layout-affecting edits.
  - Or accept a slower path when layout changes.

## Inlining: Smart Invalidation and IR Caching

Inlining is the hardest part of incremental compilation.

You need to know:
- Which callers inlined which callees.
- Under what inline decisions.
- Whether IR reuse is safe.

### Two-tier call model (recommended)

To achieve low-latency updates while still benefiting from inlining:

Tier 1 (dev/watch mode):
- Default to indirect calls through a per-module function pointer table.
- Inline only very small leaf functions (or disable inlining entirely).
- Benefit: changing a callee usually only requires recompiling the callee and updating the table entry.

Tier 2 (optional "inline hot" mode):
- Allow inlining guided by profile or size threshold.
- Maintain an inline graph and invalidate callers when an inlined callee changes.

This gives you a knob:
- Fewer invalidations (faster updates) vs more inlining (faster runtime).

### Inline edge tracking

When lowering a function, record inline choices:
- For each call site, either:
  - "direct" (not inlined)
  - "inlined" with callee id

Store:
- `inline_edges: callee_fn_id -> list of caller_fn_id`

On callee change:
- Compute transitive closure in the inline graph.
- Invalidate all reachable callers.

### IR cache policy with inlining

If function F inlines G, then F's IR depends on:
- G's body_hash
- G's sema_env_hash (if it affects lowering)
- G's layout_version (if it embeds offsets)

Therefore F's `ir_cache_key` must include the inline dependency summary.

Practical implementation:
- Build an "inline dependency hash" for each function:
  - Start with 0.
  - For each directly inlined callee, mix in (callee_id, callee_body_hash, callee_sig_hash).
  - Optionally include a second-order hash if inlining is transitive.

Then:
- `ir_cache_key = hash(body_hash, sema_env_hash, layout_version, inline_dep_hash, backend_mode)`

### Avoiding transitive explosion

Inlining can create huge invalidation sets.

Mitigations:
- Cap inlining depth in watch mode.
- Prefer non-inlining + indirect dispatch in watch mode.
- Inline only "pure" utility functions with small stable bodies.

## Backend Strategy: Cranelift JIT Pushes

The incremental compiler should produce a batch of updates:

- Added/updated functions:
  - function id
  - signature
  - IR body (or backend-specific IR)

- Removed functions:
  - function id

### Patch strategy options

Option 1: function pointer table (fastest updates)
- All calls go through an indirect dispatch table.
- Changing a function updates its entry.
- Callers do not need recompilation unless signature changes.

Option 2: direct calls + patch call sites
- Requires rewriting call instruction targets.
- More complex and often not worth it for the 100ms target.

Recommended default for watch:
- Option 1.

Recommended for release:
- Option 2 with inlining and full optimization.

### ABI and stable interfaces

To make patching safe:
- Choose a stable calling convention and keep it consistent.
- Enforce that JIT-patched functions have stable stack maps / unwind metadata if required.

In dev/watch mode:
- Prefer simple calling conventions and avoid exotic features that make patching slow.

## Keeping the Pipeline Under ~100ms

Think in budgets (example targets):

- Watch event ingest + file load: 1-5ms
- Re-parse changed file: 2-15ms (depends on file size and parser complexity)
- Diff declarations and update indices: 1-5ms
- Impact analysis: 1-5ms
- Re-sema N impacted functions: 2-20ms
- Lower N impacted functions to IR: 5-40ms
- JIT compile + publish N functions: 5-40ms

Total: 17-130ms typical.

To stay near 100ms:

1. Keep impacted sets small
- Default to indirect calls and minimal inlining.
- Track symbol deps precisely.

2. Cache aggressively
- Reuse sema results when the symbol environment hash is unchanged.
- Reuse backend IR when `ir_cache_key` matches.

3. Batch JIT updates
- Send one update batch per tick, not one per function.

4. Work scheduling
- Use a cooperative scheduler inside Stasis:
  - parse/diff first
  - compute impacted set
  - then compile in priority order (directly changed functions first)

5. Parallelism
- Stasis itself may not be parallel; the host can parallelize JIT compilation.
- If the JIT runner is external, it can compile in parallel and publish at the end.

## Diagnostics and Developer UX

Even in watch mode, diagnostics must be:
- Deterministic.
- Span-precise.
- Actionable.

Key rule:
- Always report diagnostics relative to the current file versions.

Because imports are expanded, keep a source map:
- Map expanded spans back to (file, original span).

This is similar to `SourceImporter.ExpandImportsWithMap(...)` in the current C# toolchain.

## Suggested Implementation Roadmap (Iterate the Plan)

This is a staged plan that can be implemented incrementally. It is designed so each stage delivers value and can be profiled.

### Phase 0: Minimal resident database + full recompile per change
- Keep file table and content hashes.
- On change: re-parse all files, re-sema all, re-lower all.
- JIT push everything.

This will not hit 100ms, but establishes the service shape and the host messaging.

Exit criteria:
- Stable watch loop.
- Deterministic results.

### Phase 1: Per-file parse + function-level diff
- Only re-parse the changed file.
- Extract function HIR and compute body hashes.
- Skip unchanged functions.

Exit criteria:
- Small edits in one file do not recompile unrelated functions.

### Phase 2: Dependency tracking and impacted sets
- Track per-function symbol deps and call deps.
- Maintain reverse dep maps.
- On change: recompute only impacted functions.

Exit criteria:
- Typical edit recompiles 1-20 functions.

### Phase 3: Backend IR caching
- Cache per-function backend IR blobs keyed by `ir_cache_key`.
- Reuse IR and skip lowering when safe.

Exit criteria:
- Many edits only trigger JIT compilation for functions whose IR changed.

### Phase 4: Inline edge tracking + smart invalidation
- Implement inlining (optional) in dev mode.
- Record inline edges.
- Invalidate transitive inline callers.

Exit criteria:
- Changing a small inline callee invalidates only the relevant caller set.

### Phase 5: Performance hardening
- Profile each stage and tune caps.
- Introduce LRU file buffer cache (Mode B) to control memory.
- Add fast paths:
  - early-out when parse hash unchanged
  - early-out when impacted set empty

Exit criteria:
- Consistently near 100ms for typical edits in medium projects.

## Critical Design Choices (Make These Explicit)

To meet the goals, you must decide and document:

1. Watch mode call strategy
- Indirect dispatch table (recommended) vs direct calls.

2. Layout change policy
- Allow slow path vs layout freeze.

3. Inline policy in watch mode
- Disabled, limited, or enabled.

4. Storage mode
- Store all file contents resident vs LRU caching.

These choices dominate the complexity and the 100ms behavior.

## Appendix: Example "Delta Plan" Algorithm

Pseudocode (conceptual):

1. DirectlyChangedFns = functions whose body_hash or sig_hash changed in the edited file.
2. ChangedSymbols = any symbols declared in the file whose declaration hash changed.
3. ImpactedFns = DirectlyChangedFns
4. For each sym in ChangedSymbols: add all rev_symbol_dependents[sym] to ImpactedFns.
5. For each fn in DirectlyChangedFns:
   - add callers if signature changed
   - add inline-callers transitively if fn was inlined
6. If layout_version changed:
   - add all functions that touch affected globals/fields (or all functions if offsets are embedded broadly)
7. Topologically order ImpactedFns by call graph SCCs.
8. Re-sema and lower in that order.
9. Emit a single JIT update batch.

The architecture succeeds if the typical ImpactedFns set stays small.