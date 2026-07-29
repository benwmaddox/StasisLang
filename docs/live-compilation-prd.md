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
- implement full symbol-level dependency invalidation
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
 |  \- Atomic ActiveGeneration reference
 |
 |- Compiler Service
 |  |- In-memory file database
 |  |- File-level incremental pipeline
 |  |- Semantic/HIR caches
 |  \- Complete-generation build decisions
 |
 \- Codegen Service (Cranelift JIT/AOT)
    |- Shared direct-call JIT/AOT lowering
    |- Complete module finalization
    \- Generation-owned executable memory
```

Disk I/O is not part of the hot path.

## 4. Compilation Model

### 4.1 Granularity

- Invalidation unit: file
- Correctness unit: file
- Lowering/cache unit: function
- Publication unit: complete reachable generation
- Dead-code pruning unit: function + struct metadata (reachability-based)

Semantic analysis always runs for the entire file.
Semantic and target-independent lowering work may be gated per function. Every accepted development
build still finalizes one complete reachable machine-code generation.
Pruning is symbol-level and happens before Cranelift emission.

### 4.2 File-Level Pipeline

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
 -> Complete direct-call module finalization
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
- If layout-affecting semantic fact changes -> full file re-codegen
- Semantic hashes never reuse live machine code, relocations, or pointers from another generation

### 4.4 Generation Compatibility Rule

Hot swap is permitted only if:

- every required host export keeps its declared ABI and phase policy
- the target and host-set versions remain compatible
- global state layout is unchanged or the compiler-owned bounded migration plan is compatible

Ordinary internal functions are not compatibility boundaries. They may be added, removed, renamed,
or change signature because the complete reachable generation and all callers are rebuilt together.

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

### 5.1 Complete Generation ABI

Every reachable Stasis-to-Stasis call uses a direct call inside one finalized JIT/AOT module. Only
explicit lifecycle and host-required exports cross the runtime boundary. The runtime snapshots one
immutable `ActiveGeneration` owning the complete export map, state bindings, and executable memory
for an execution window.

Rules:

- The compiler discovers host exports from lifecycle roots and the selected host-set manifest.
- All host exports are published by one atomic owning-reference exchange; independent pointer swaps
  are forbidden.
- Internal body pointers never leave the generation and cannot be cached by the host.
- `FnId` may identify symbols in diagnostics and caches, but is not a runtime dispatch key.
- A `tick()` and its following `render()` use the same captured generation.
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

Phase 1 - Background Generation Build

- File changes ingested
- Semantics computed
- Reachable functions and host exports resolved
- Cranelift compiles and finalizes the complete direct-call module
- Results transferred as one immutable `PendingGeneration`

Phase 2 - Commit (Main Thread, Between Ticks)

Occurs strictly between ticks.

Order:

1. Finish the active generation's complete `tick()` + `render()` execution window.
2. Revalidate that the candidate is current and compatible.
3. Allocate isolated candidate storage and migrate compatible struct/global fields.
4. Run the candidate `on_code_swap()` against isolated candidate state when present.
5. Preflight all fallible work.
6. Atomically replace the one owning `ActiveGeneration` reference.
7. Release the old generation when its last execution-window reference ends.

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

- `FileChangeEvent`: producer file watcher/input bridge; consumer compiler service; fields `path`, `revision`, `text_source`, `change_kind`.
- `BuildGeneration`: producer swap coordinator; consumer compiler service; fields `request_id`,
  `revision`, immutable `source_snapshot_id`, `target`, `host_set`, and immutable active ABI/layout
  contract.
- `BuildFinished`: producer compiler service; consumer swap coordinator; fields `request_id`,
  `revision`, `status`, `diagnostics[]`, and optional owning `pending_generation`.
- `CommitGeneration`: producer swap coordinator; consumer runtime/main thread safe-point gate; fields
  `request_id` and owning `pending_generation`.
- `CommitFinished`: producer runtime/main thread; consumer swap coordinator + UI/status bridge;
  fields `request_id`, `status`, optional `active_generation_number`, and optional diagnostic.
- `CancelBuild`: producer swap coordinator; consumer compiler service; fields `request_id` and
  `superseded_by_request_id`.

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
- Execution windows hold an owning reference; old code is freed in bulk after the last reference
  ends, not after a fixed tick delay.
- Fibers, suspended Stasis frames, cached code pointers, and callbacks retaining guest pointers are
  unsupported and rejected deterministically.
- Steady operation owns at most active + current pending + one transient retiring generation; a
  superseded pending generation is released immediately.

## 14. Performance Targets

| Scenario | Initial p95 gate |
| --- | ---: |
| Complete 100-function trivial generation | 25 ms background |
| Complete 1,000-function trivial generation | 150 ms background |
| Complete 5,000-function trivial generation | 2,500 ms background |
| Desktop owning-reference publication | 0.25 ms |
| Android arm64 owning-reference publication | 1.0 ms |

Edit-to-visible latency is background build time plus at most two tick intervals. Compilation may
take many ticks while the old generation keeps running; it may not stall the runtime thread. The
full measurement and executable-memory budgets are in `docs/jit_generation_contract.md`. Disk I/O
must not be on the hot path.

## 15. Development Phases

Phase G0:
- Lock the complete-generation ABI, lifecycle, matrix, budgets, and deletion list.

Phase G1:
- Emit one complete direct-call JIT module through shared JIT/AOT lowering.

Phase G2:
- Publish one owning generation reference between windows and retire by ownership.

Phase G3:
- Prove the complete edit/failure transition matrix with deterministic executable fixtures.

Phase G4:
- Enforce the desktop/Android JIT/AOT platform and performance matrix.

The prior pointer-table/patch-set phases describe migration substrate only. They are not retained as
a compatibility path and are deleted by G1-G2.

## 16. Key Risks & Mitigations

| Risk | Mitigation |
| --- | --- |
| Mixed-generation execution | One immutable generation snapshot per execution window |
| Stale build publication | Monotonic request IDs plus current-revision check at commit |
| Partial state mutation | Isolated candidate state and fallible work before publication |
| Use-after-free code | Owning execution-window references and no retained guest pointers |
| Runtime frame stalls | Complete build/finalization restricted to compiler thread |
| Complexity creep | Complete reachable module; no partial-dispatch compatibility path |

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
