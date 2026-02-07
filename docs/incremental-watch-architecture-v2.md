# Incremental Watch Compiler + Cranelift JIT Architecture (v2: Simpler + Reachable-Resident)

This document is a parallel, simplified variant of `docs/incremental-watch-architecture.md`.

Changes vs v1 (the "why"):
- Relax latency targets to reduce architectural complexity.
- Prefer conservative invalidation (recompile a bit more) over fine-grained bookkeeping.
- Store file contents in memory only for files reachable from the entry point (import graph reachability).

## Goals and Constraints (Updated)

Hard constraints:
- Max file size: 100 KB.
- Max file count: 10,000.
- Static, preallocated storage (idiomatic Stasis).

Latency targets (updated):
- Projects with < 100 files: ~100 ms edit -> published JIT update (typical edits).
- Projects at max file count (10,000): ~200 ms (typical edits), accepting more conservative recompilation.

Incremental goals:
- Persist project state across edits: file metadata, per-file decl summaries, per-function HIR/IR caches.
- File watching: update single file quickly, but also handle batches of edits across multiple files in one tick.
- Diff-driven incremental updates for Cranelift JIT pushes.

Inlining goal (kept):
- Smart invalidation when inlining is enabled.
- But v2 defaults to **no inlining in watch mode** to simplify invalidation and reduce recompiles.

## Design Principle (v2)

If a bookkeeping structure adds complexity, ask:
- Does it materially improve the 100ms (<100 files) or 200ms (10k files) target?
- If not, prefer the simpler, slightly-more-conservative approach.

## Key Simplification: Reachable-Resident Files

Requirement:
- "I want all files in memory if referenced from entry point. but they don't have to be there otherwise."

Definition:
- A file is **reachable** if it is the entry file OR it is imported (directly or transitively) by a reachable file.

Policy:
- Only reachable files must have their full content bytes resident.
- Non-reachable files keep only:
  - path
  - mtime/hash
  - a lightweight declaration summary (optional)

This collapses the worst-case memory problem (10,000 * 100KB) while still keeping fast, single-file edits in active projects.

### Reachability maintenance

Maintain an import graph:
- `file_id -> import_file_ids[]`

When a reachable file's imports change:
- Recompute reachable set from the entry file.
- Load newly-reachable files into memory buffers.
- Optionally evict newly-unreachable files.

Because import changes are relatively rare compared to body edits, a full reachable recompute is acceptable.

## High-Level Architecture

Same three layers as v1:

1) Stasis incremental compiler service
- Owns the project DB, parsing, conservative impact analysis, and delta planning.

2) Host integration
- Owns OS file watching and fast delivery of "file changed" events.

3) Cranelift JIT runner
- Accepts batches of function updates and publishes them.

The main v2 change is: the Stasis service is *less* fine-grained in invalidation and dependency bookkeeping.

## Project Database (Fixed Capacity, v2)

### Files: metadata always, bytes only when reachable

Split file data into:

- `FileMeta[MAX_FILES]`: always resident
- `FileBuf[MAX_FILE_BUFS]`: resident only for reachable files

```stasis
const MAX_FILES: i32 = 10000;
const MAX_FILE_BYTES: i32 = 100 * 1024;

// Tune: for active projects, reachable files are often far less than 10k.
// Set this to an upper bound you can afford (e.g. 2048, 4096).
const MAX_FILE_BUFS: i32 = 4096;

struct FileMeta {
  path: i32;            // interned
  exists: bool;
  file_id: i32;

  mtime_ms: i32;
  content_hash: u32;

  // Import graph (ids), from last successful parse.
  imports: i32[256];
  import_count: i32;
  imports_hash: u32;

  // Declaration summary pointers (per-file decl table indices).
  decl_start: i32;
  decl_count: i32;

  // Residency
  reachable: bool;
  buf_slot: i32;        // -1 if not resident
}

struct FileBuf {
  file_id: i32;
  used: bool;
  bytes: u8[MAX_FILE_BYTES];
  byte_len: i32;
  pin_count: i32;
}

global files: FileMeta[MAX_FILES];
global file_bufs: FileBuf[MAX_FILE_BUFS];
```

Allocation strategy:
- Keep it simple:
  - First-fit scan for an unused buffer slot.
  - Evict only unreachable, unpinned buffers.
- Avoid LRU complexity unless profiling forces it.

Because v2 allows 200ms for worst case, occasional O(MAX_FILE_BUFS) scans are acceptable.

### Strings: intern everything

Same as v1: fixed pool + open-addressed slots.

### Per-file declaration summary (simplified)

The incremental engine mostly needs:
- Which functions exist in the file, their signature hash, and body hash.
- Which symbols the file defines (for dependency mapping at file-granularity).

Store per-file summaries:

```stasis
struct FileDeclSummary {
  file_id: i32;

  // Functions declared in this file
  fn_ids: i32[4096];
  fn_count: i32;

  // Symbols defined in this file (types/globals/consts/functions)
  def_syms: i32[4096];
  def_sym_count: i32;

  summary_hash: u32;
}
```

If the file is huge, cap and emit diagnostics (or refuse the file) rather than dynamically allocating.

### Functions: cache by hash, track deps at file-granularity first

v2 simplification:
- Prefer **file-level dependency tracking** over symbol-level reverse maps.

Store for each function:
- which files it depends on (via referenced symbols/types)
- which functions it calls (still needed for signature change invalidation)

```stasis
struct FnRec {
  name: i32;          // interned
  file_id: i32;

  sig_hash: u32;
  body_hash: u32;

  // Dependency at file granularity
  dep_files_start: i32;
  dep_files_len: i32;

  // Call graph (direct calls by fn_id)
  callees_start: i32;
  callees_len: i32;

  // Backend cache
  ir_key: u32;
  ir_blob_start: i32;
  ir_blob_len: i32;

  codegen_version: i32;
}
```

Why file-level deps?
- Much simpler to maintain on multi-file changes.
- It tends to over-invalidate, but under the 200ms max-file target that is acceptable.

Optional upgrade (only if needed):
- Add symbol-level deps later for hot paths.

## Watch Loop: Multi-File Changes in One Tick

v2 explicitly optimizes for "update multiple places each change" by processing edits in batches.

### Coalesce change events

Maintain a fixed dirty flag array:

```stasis
global file_dirty: bool[MAX_FILES];
```

On ingesting watch events:
- Map event path -> file_id.
- Set `file_dirty[file_id] = true`.
- Keep only the latest event per file per tick.

This avoids complex event queues and makes multi-file edits cheap.

### Compile tick pipeline (batch)

Per tick:

1. Collect dirty file ids into `dirty_list[]`.
2. For each dirty file:
   - If file is reachable (or becomes reachable due to import graph changes), ensure it has a resident buffer.
   - Reload contents into its buffer.
   - Parse the file.
   - Update per-file decl summary (function hashes, import list, defined symbols).
3. If any imports changed in reachable files:
   - Recompute reachable set.
   - Load/unload buffers accordingly.
4. Compute a single impacted set for the whole batch.
5. Re-sema impacted functions.
6. Re-lower impacted functions.
7. Push one JIT update batch.

Because parsing multiple small files is often faster than doing many tiny passes, batching reduces overhead and simplifies logic.

## Conservative Impact Analysis (v2)

v2 impact analysis is designed to be simple and safe.

Inputs:
- `dirty_files[]`
- `changed_defs_files[]`: files where the set or hashes of defined symbols changed
- `changed_sig_fns[]`: functions whose signature changed
- `changed_body_fns[]`: functions whose body changed

Rules:

1) Always recompile functions whose body changed.
- `impacted += changed_body_fns`

2) If a function's signature changed, recompile its callers.
- `impacted += callers_transitive(changed_sig_fns)`

3) If a file changed its defined symbols/types/globals, recompile any function that depends on that file.
- Using file-level deps:
  - For each impacted fn, we maintain `dep_files`.
  - Maintain reverse mapping `file -> dependent_fns`.

This reverse mapping is still simpler than symbol-level and is bounded by `MAX_FILES`.

4) If global layout changes, take the slow path.
- Layout-affecting changes include:
  - global declaration type/size changes
  - struct field layout changes used by globals
- Slow path policy options:
  - Option A (simple): recompile all reachable functions.
  - Option B (still simple): recompile all functions that depend on the file(s) defining the layout-affecting globals/types.

Given the relaxed target, Option A is acceptable for small reachable sets.

## Inlining (v2 default)

### Default: disable inlining in watch mode

This removes the hardest invalidation problem.

- All calls are either:
  - indirect via a dispatch table, or
  - direct calls but without inlining.

Changing a callee:
- only recompiles the callee,
- and only updates the dispatch table entry,
- unless signature changes.

This yields very stable, small impacted sets and makes the 100ms target for <100 files much easier.

### Optional: enable inlining behind a flag

If you enable inlining later:
- Reuse v1's inline edge tracking and inline-dependency hashing.
- Keep it opt-in for watch mode.

## Cranelift JIT Push Model (v2)

Keep the simplest publish protocol:

- Function table based dispatch
  - each function id has an entry pointer
  - callers load the pointer and call indirectly

JIT update batch:
- list of (fn_id, signature, IR)
- list of removed fn_ids

Publish step:
- compile all updated IR
- atomically swap pointers in the dispatch table

This minimizes invalidation and avoids patching call sites.

## IR Caching (Simplified)

Without inlining, IR caching becomes much easier:

- `ir_key = hash(body_hash, sig_hash, layout_version, backend_mode)`

If `ir_key` matches and the backend IR blob exists:
- you can skip lowering and reuse the blob.

If layout_version changes:
- invalidate all IR blobs (or bump a global cache epoch).

## Meeting the 100ms / 200ms Targets

### Why v2 hits targets more easily

- Smaller resident set (reachable files only).
- Batch processing of multi-file edits.
- Conservative invalidation at file-level deps.
- Dispatch-table JIT publish minimizes caller recompiles.
- No inlining by default.

### Expected steady-state behavior

For <100 reachable files:
- Most edits touch 1 file.
- Re-parse 1 file.
- Recompile a handful of functions.
- Publish a small JIT batch.

For large repos (10k files) with a small reachable set:
- still fast, because only reachable files are resident/active.

For worst case (entry imports most of the repo):
- reachable set is large.
- v2 accepts up to ~200ms typical edits.
- layout-affecting edits may still be slower.

## Implementation Plan (v2)

Keep the phases, but simplify what you build first.

Phase 1: Reachable-resident project DB
- File metadata table + buffer pool
- Import graph reachability
- Watch event coalescing (`file_dirty[]`)

Phase 2: Per-file parse + decl summary diff
- Parse only dirty files
- Build/refresh per-file decl summaries
- Compute changed function hashes

Phase 3: Conservative impact analysis
- File-level reverse deps (file -> dependent functions)
- Call graph for signature changes
- Recompile impacted set

Phase 4: Cranelift JIT publish
- Dispatch table
- Batch updates

Phase 5: Optional upgrades
- Symbol-level deps (only if needed)
- Inline tracking (opt-in)
- Smarter eviction policies

## What You Give Up (Explicit Tradeoffs)

- More recompilation than a fine-grained symbol-level approach.
- Less aggressive inlining in watch mode.
- Occasional slow paths for layout-affecting edits.

In return:
- Much simpler implementation.
- Better predictability.
- Easier to keep within 100ms (<100 files) and 200ms (max file count) in practice.