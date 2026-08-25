# Product Requirements Document (PRD)

## Project

Stasis Live Compilation & Atomic Generation Swap System (File-Level, In-Process, Tick-Based)

## 1. Purpose & Goals

### 1.1 Purpose

Enable fast, reliable, low-friction iteration on a running Stasis-based game by embedding:

- the Stasis compiler
- Cranelift backends (JIT for development, AOT for production)
- a hot code swap mechanism

directly inside the running game engine, avoiding disk I/O and process restarts.

The cross-cutting deterministic live simulation promise, capability order, and evidence gates are
defined in [`docs/deterministic_live_simulation_roadmap.md`](deterministic_live_simulation_roadmap.md).
This PRD owns the hot-swap architecture and requirements; it does not duplicate that roadmap.

The system must:

- preserve game state across code changes
- guarantee correctness at file-level
- provide explicit developer control over state adjustment
- integrate naturally with VS Code workflows
- provide clear visual confirmation of successful swaps
- operate deterministically using tick-based semantics

Android workshop requirements are tracked in `docs/android_workshop_prd.md`. That document locks the sideload-first Android app direction, symbol-first editor model, Stasis-syntax AI patch contracts, GitHub API v1 Git workflow, and preview-renderer selection criteria.

### 1.2 Non-Goals

This system does not aim to:

- support unbounded or semantically ambiguous runtime schema/layout migration
- implement instruction-level dependency invalidation or automatic JIT-code garbage collection
- infer semantic transformations beyond the compiler's deterministic path/type/capacity migration
  rules
- replace a full debugger
- optimize production builds beyond baseline Cranelift AOT

## 2. Core Design Principles

1. File-level correctness over symbol-level cleverness
2. Explicitness over magic
3. Compilation is a service, not a command
4. Runtime owns data; compiler owns code
5. Hot swap occurs only between ticks
6. Failure aborts cleanly, never partially
7. All Stasis-level lifecycle counters are tick-based
8. Developer trust > micro-optimizations

## 3. High-Level Architecture

### 3.1 Process Model

Single OS Process (Game Engine)

```text
Game Process
 |- Runtime
 |  |- Game loop (tick-based)
 |  |- Global Data
 |  |- Debug UI
 |  \- Atomic host-entry table reference
 |
 |- Compiler Service
 |  |- In-memory file database
 |  |- File-level incremental pipeline
 |  |- Semantic/HIR caches
 |  \- Exact changed/SCC/reverse-caller patch planning
 |
 \- Codegen Service (Cranelift JIT/AOT)
    |- Shared direct-call JIT/AOT lowering
    |- Selective JIT patch finalization / complete AOT finalization
    \- Retained JIT code arenas
```

Disk I/O is not part of the hot path.

## 4. Compilation Model

### 4.1 Granularity

- Invalidation unit: file
- Correctness unit: file
- Lowering/cache unit: function
- Publication unit: one validated selective patch through the host-entry table
- Dead-code pruning unit: function + struct metadata (reachability-based)

Semantic analysis always runs for the entire file.
Semantic, lowering, and JIT emission work is gated per function. A warm development build emits the
changed function/SCC plus the minimum reverse direct-call closure required to reach stable
host-entry trampolines. Unaffected machine-code bodies keep their accepted addresses.
Pruning is symbol-level and happens before Cranelift emission.

### 4.2 File-Level Pipeline

The implementation map for this pipeline is [compiler_architecture.md](compiler_architecture.md).

```text
Raw Text
 -> Lex
 -> Parse
 -> Index (imports / declarations)
 -> Semantic Analysis (whole file)
 -> Reachability Mark (functions + structs)
 -> Prune Unreachable Symbols
 -> Per-function semantic hashing
 -> Per-function lowering/cache lookup
 -> Exact reverse-caller PatchPlan
 -> Selective direct-call patch finalization
```

Reachability roots:
- `main`
- `tick` (when present)
- `render` (when present)
- `on_code_swap` (when present)
- host-required exported entry symbols

This is intentionally simple: no broad optimizer layer in Stasis and no instruction-level DCE requirement before Cranelift.

### 4.3 Semantic Hashing

Each function produces:

- `fnSigHash` - signature/ABI relevant
- `fnBodyHash` - behavior

Rules:

- If `fnBodyHash` is unchanged -> reuse target-independent analysis/lowering inputs when safe
- If any compiler-visible layout fact changes -> recompile every reachable function into one
  coherent generation
- Unchanged reachable functions may reuse their accepted live machine code and addresses in JIT dev
  only while the compiler-layout digest is unchanged. This keeps ordinary body and constant edits
  selective without requiring per-function proofs for embedded struct offsets or collection shapes.

### 4.4 Generation Compatibility Rule

Hot swap is permitted only if:

- every required host export keeps its declared ABI and phase policy
- the target and host-set versions remain compatible
- global state layout is unchanged or the compiler-owned bounded migration plan is compatible

Ordinary internal functions are not host compatibility boundaries. They may be added, removed,
renamed, or change signature when every affected direct caller type-checks and is included in the
selective reverse-caller patch.

If violated:

- swap rejected
- diagnostics reported
- old code continues running

### 4.5 Language Conversion Semantics

Numeric conversions use receiver-form helpers in two categories:

`from_*` conversions (mutating target):
- Assignment-like operations that write into the receiver target.
- Statement-style side-effect operations.
- Example: `f32Value.from_i32(i32Value);`

`to_*` conversions (pure value):
- Pure operations on basic numeric types.
- Expression-safe and may be used in declarations/initializers.
- Example: `let alpha: f32 = ticks_i32.to_f32();`

Example:

```stasis
let ticks_i32: i32;
let alpha: f32;

ticks_i32.from_u32(DebugUI.swapFlashTicks);
alpha.from_i32(ticks_i32);
alpha /= 180.0;
```

Equivalent declaration+initializer style with pure conversions:

```stasis
let ticks_i32: i32 = DebugUI.swapFlashTicks.to_i32();
let alpha: f32 = ticks_i32.to_f32();
alpha /= 180.0;
```

## 5. Codegen & Runtime Boundary

Dev/runtime hot-swap mode uses Cranelift JIT.
Production build mode uses Cranelift AOT outputs.

### 5.1 Selective JIT Patch ABI

Every Stasis-to-Stasis call uses a direct native call. Warm JIT patches may call unchanged retained
bodies from earlier code arenas directly. Only lifecycle and host-required entries use stable
trampolines. The runtime snapshots one immutable host-entry table for an execution window.

Rules:

- The compiler discovers host exports from lifecycle roots and the selected host-set manifest.
- All affected host entries publish by one atomic entry-table exchange; independent root swaps are
  forbidden.
- Internal body pointers never leave compiler-owned patch metadata or escape to the host.
- `FnId` may identify symbols in diagnostics and caches, but is not a runtime dispatch key.
- A `tick()` and its following `render()` use the same captured entry table.
- JIT and AOT share one direct-call lowering contract.

The detailed ABI and lifetime contract is `docs/jit_generation_contract.md`.

### 5.2 Runtime API

Compiled code may only call a small, stable runtime API:

Examples:

- logging
- input state
- entity/system helpers
- rendering commands
- audio events

### First-Class Project Data

- Editable runtime data lives under the project-level `data/` directory.
- Every JSON or CSV file has a same-name `.struct-meta.json` mapping and is discovered automatically; ordinary development does not require binding flags or project-specific loader code.
- JSON may map nested properties. CSV headers are flat and bind either scalar/primitive-array data or variable rows into a fixed-capacity struct array. Table mappings automatically expose `row_count`, clear unused slots, and validate non-blank unique key columns (including composite keys).
- Binding rejects extra source properties/columns, missing metadata paths, duplicate mappings, and paths absent from compiled globals before mutating runtime data.
- Development watches both files and applies a validated set between ticks. Rejected edits preserve the last accepted runtime data.
- Production AOT packages stage the same data and compile its accepted values into the runtime bridge, so startup never depends on loose development files.
- The JIT and AOT paths use the same global names, field types, array bounds, and JSON-path mapping.
- Explicit binding paths are compatibility overrides, not the standard project workflow.

### First-Class Sprite and Audio Assets

Sprite and audio support is a cross-platform runtime contract, not an editor-only feature.

- Project-relative asset paths resolve through a versioned asset manifest with deterministic IDs/hashes.
- JIT, AOT, desktop, Android Workshop, and published Android builds use the same asset identity and packaging rules.
- Sprite loading supports bounded decode, texture lifetime management, batching, hot reload, missing-asset diagnostics, and safe fallback rendering.
- Audio supports decoded sound/music assets, bounded voices/streams, volume/pan/loop controls, deterministic event submission, device lifecycle handling, underrun diagnostics, and graceful unavailable-device behavior.
- Asset hot reload swaps only complete decoded resources and preserves the previous accepted resource on failure.
- Headless tests validate asset manifests/events without requiring graphics or audio hardware; representative device tests validate actual pixels and sound.
- Host-set permissions remain deny-by-default for file/device access; Stasis code addresses packaged project assets rather than arbitrary host paths.

The runtime API:

- uses ABI-stable primitive types
- does not expose compiler internals
- avoids raw pointer exposure unless guaranteed safe

## 6. Hot Swap Mechanism

### 6.1 Two-Phase Commit

Phase 1 - Background Selective Patch Build

- File changes ingested
- Semantics computed
- Changed functions/SCCs and exact reverse callers resolved
- Cranelift compiles only the affected direct-call closure
- Results transferred as one immutable `PendingPatch`

Phase 2 - Commit (Main Thread, Between Ticks)

Occurs strictly between ticks.

Order:

1. Finish the active entry table's complete `tick()` + `render()` execution window.
2. Revalidate that the candidate is current and compatible.
3. Allocate isolated candidate storage and migrate compatible struct/global fields.
4. Run the candidate `on_code_swap()` against isolated candidate state when present.
5. Preflight all fallible work.
6. Atomically replace the immutable host-entry table.
7. Retain old JIT arenas until a development process restart.

If any step fails, swap is aborted.

This commit exists only for a pending code generation. It is not part of ordinary gameplay tick
semantics: the host does not infer gameplay changes, and Stasis programs expose no
`commit_tick`, `normalize_tick`, or `validate_tick` lifecycle functions. The first candidate
`tick()` runs only after migration and publication succeed; rejection destroys isolated candidate
code/state while the old active generation remains unchanged.

## 7. Swap Hook (User-Defined)

### 7.1 Purpose

Allow explicit adjustment of migrated runtime data after code changes.

Solves:

- invariant updates
- transient state reset
- logic reinterpretation

### 7.2 Definition

```stasis
function on_code_swap(): void {
    // Optional
}
```

Properties:

- Optional
- Runs at most once after a candidate enters hook execution; an attempt may later reject, trap, or
  become superseded
- A successfully published candidate with a hook runs it exactly once
- Runs between ticks
- Executes as candidate code before publication and before the next `tick()`
- May mutate only isolated candidate global data
- Must not invoke gameplay entrypoints

### 7.3 Enforcement Rules

- Hook error -> swap aborted
- Incompatible layout change -> swap rejected
- Hook runs against candidate code and migrated candidate storage

## 8. Failure Handling

### 8.1 Compile Errors

- Swap aborted
- Old code and data preserved
- Diagnostics surfaced

### 8.2 Swap Hook Errors

- Swap aborted
- Old state preserved
- Error clearly reported

No partial state mutation allowed.

## 9. Visual Swap Indicator (Tick-Based)

### 9.1 Purpose

Provide immediate confirmation of successful swap.

### 9.2 Runtime State

```stasis
global DebugUI {
    swapFlashTicks: u32;
}
```

### 9.3 Behavior

On successful swap commit:

```stasis
DebugUI.swapFlashTicks = 180; // Example: 3 seconds @ 60 ticks/sec
```

During draw:

```stasis
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

Properties:

- Purely tick-based
- Deterministic
- Non-modal
- No sound
- Dev-only

No indicator appears on failed swap.

## 10. Tick-Based Policy

All Stasis-level counters must use ticks.

Rules:

- No `dt`-driven logic inside Stasis gameplay semantics
- Rendering may interpolate visually
- Simulation remains deterministic

The engine defines `TICKS_PER_SECOND`.

## 11. VS Code Workflow

Phase 1 - File Watcher

- VS Code edits files
- Game watches directory
- Compiler ingests file changes
- Diagnostics shown in console

Phase 2 - LSP Integration (Future)

- Text buffers sent directly
- Disk removed from hot path
- Diagnostics + status surfaced in VS Code

## 12. Threading Model

- Main thread: game loop + swap commit
- Compiler thread: lex/parse/semantics/codegen
- Communication via queue

Rules:

- Compiler never mutates runtime state
- Runtime never blocks frame loop
- Swap commit bounded and deterministic

### 12.1 Dev File-Change Ownership

When a source file changes during development, ownership is:

- Runtime/Main Thread: owns the tick loop, execution-window generation snapshot, safe-point detection,
  isolated state migration/hook transaction, and one-reference publication. It never parses, checks,
  lowers, generates, links, or finalizes code.
- Compiler Service Thread: owns an immutable source snapshot, lex/parse/index/semantic/hash analysis,
  reachability, and complete `PendingGeneration` construction. It never reads or mutates runtime
  values.
- Codegen Service (Cranelift): owns shared direct-call JIT/AOT emission and complete module
  finalization. It transfers an owning artifact rather than per-function pointers.
- Swap Coordinator: owns request ordering, cancellation/supersession, message transport, and
  all-or-nothing commit orchestration.

### 12.2 High-Level Interface Contracts (Development Mode)

Interfaces are message-based and versioned. No cross-thread shared mutable compiler/runtime objects.

- `FileChangeEvent(path, revision, text_source, change_kind)`: producer file watcher/input bridge;
  consumer swap coordinator.
- `BuildGeneration(request_id, revision, source_snapshot_id, target, host_set, active_contract)`:
  producer swap coordinator; consumer compiler service; the snapshot and active contract are
  immutable.
- `BuildFinished(request_id, revision, status, diagnostics[], pending_generation?)`: producer
  compiler service; consumer swap coordinator; the optional generation transfers ownership.
- `CommitGeneration(request_id, pending_generation)`: producer swap coordinator; consumer
  runtime/main thread safe-point gate; the generation transfers ownership.
- `CommitFinished(request_id, status, active_generation_number?, diagnostic?)`: producer
  runtime/main thread; consumer swap coordinator + UI/status bridge.
- `CancelBuild(request_id, superseded_by_request_id)`: producer swap coordinator; consumer compiler
  service.

### 12.3 Development Change Sequence (Single File Save)

1. Watcher emits `FileChangeEvent`.
2. Swap coordinator coalesces pending events, supersedes older requests, and emits
   `BuildGeneration`.
3. Compiler service runs full-file semantic pass.
4. Compiler/codegen returns `BuildFinished` with diagnostics or one finalized
   `PendingGeneration`.
5. If diagnostics exist or the result is stale, the candidate is discarded and old code remains
   active.
6. If eligible, coordinator waits for between-ticks safe point.
7. Main thread creates isolated candidate state, runs bounded migration and candidate
   `on_code_swap()` (if present), and preflights publication.
8. Main thread atomically replaces the complete `ActiveGeneration` reference and assigns the next
   successful generation number.
9. Runtime publishes `CommitFinished`; debug UI updates swap indicator only on success.

Failure at any sequence step aborts commit and preserves old code/data.

### 12.4 Language and Implementation Ownership

- `.stasis` owns user program source, language feature usage, and gameplay/runtime logic.
- Rust owns compiler implementation end-to-end: lexer/tokenization, parser, semantic diagnostics, incremental compile policy, lowering, and Cranelift JIT/AOT backend integration.
- Rust owns runtime/backends: watcher input bridge, message transport, swap coordinator execution, executable memory management, and runtime ABI boundary.

Constraints:

- Compiler semantics are spec-driven by `docs/spec.md`; Rust compiler behavior must conform to the spec.
- New compiler frontend/backend behavior is implemented in Rust.

## 13. Code Memory Management

- Executable memory, exports, metadata, and state bindings share one generation owner.
- Each successful one-reference publication increments the generation number.
- Execution windows hold one host-entry-table snapshot; old JIT code may remain allocated until a
  process restart.
- Fibers, suspended Stasis frames, cached code pointers, and callbacks retaining guest pointers are
  unsupported and rejected deterministically.
- Superseded pending patches never publish. Automatic executable-code retirement is deferred.

## 14. Performance Targets

| Scenario | Initial p95 gate |
| --- | ---: |
| 100/1,000-function narrow selective patch | 25 ms compile-ready |
| 5,000-function narrow selective patch | 75 ms compile-ready |
| Chess TD narrow body edit | 50 ms compile-ready, commonly fewer than ten functions |
| Desktop entry-table publication | 0.25 ms |
| Android arm64 entry-table publication | 1.0 ms |

Edit-to-visible latency is background build time plus at most two tick intervals. Compilation may
take many ticks while the old generation keeps running; it may not stall the runtime thread. The
full measurement and executable-memory budgets are in `docs/jit_generation_contract.md`. Disk I/O
must not be on the hot path.

## 15. Development Phases

These local development phases own the hot-compilation implementation sequence and status. The
cross-cutting deterministic simulation outcome and capability dependencies remain in the
[`deterministic_live_simulation_roadmap.md`](deterministic_live_simulation_roadmap.md).

Phase P0 (#184):
- Lock selective reverse-caller invalidation and the host-entry-only trampoline ABI.

Phase P1 (#185):
- Plan exact changed/SCC/reverse-caller closures with reason chains.

Phase P2 (#186):
- Emit selective direct-call JIT patch modules and bind unchanged retained callees.

Phase P3 (#187):
- Publish affected host entries atomically between windows; retain old code until restart.

Phase P4 (#188):
- Enforce exact edit-shape sets and the desktop/Android performance matrix.

The superseded complete-generation #173-#178 track is not a compatibility path.

## 16. Key Risks & Mitigations

| Risk | Mitigation |
| --- | --- |
| Partial patch visibility | One immutable host-entry table per execution window |
| Stale build publication | Monotonic request IDs plus current-revision check at commit |
| Partial state mutation | Isolated candidate state and fallible work before publication |
| Use-after-free code | Retain JIT arenas for the process lifetime; restart reclaims them |
| Runtime frame stalls | Planning/build/finalization restricted to compiler thread |
| Complexity creep | Exact PatchPlan; no internal trampoline/dispatch compatibility path |

## 17. Success Criteria

System is successful when:

- Save -> the latest complete successful generation runs after a safe-point publication
- Game state persists
- Swap confirmation is visible
- Errors are safe and clear
- A tick/render window never mixes generations
- Internal calls contain no runtime dispatch lookup
- Architecture remains understandable

## 18. Summary

This system is intentionally:

- deterministic
- explicit
- tick-based
- safe
- fast
- developer-trust-focused

It provides a robust, file-correct hot reload pipeline with reusable semantic work, complete
direct-call generations, an explicit transactional swap hook, and deterministic tick-based UI
confirmation.

## Program Snapshot Ownership

Each accepted compiler candidate publishes one immutable `ProgramSnapshot`. It is the canonical
owner of source/function metadata, reachability, typed state and collection metadata, literals,
data-flow summaries, and the canonical `StateLayout` digest. Candidate diagnostics remain separate
from an accepted snapshot so a failed compile cannot overwrite accepted program metadata. JIT and AOT attach only
target artifact mappings (object paths or code pointers), which must not change semantic or layout
identity. A failed parse, semantic check, lowering, finalize, or activation leaves the previously
accepted snapshot and its artifacts active; the candidate diagnostic is reported separately.
