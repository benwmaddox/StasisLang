# Product Requirements Document (PRD)

## Project

Stasis Live Compilation & Hot Swap System (File-Level, In-Process, Tick-Based)

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

### 1.2 Non-Goals

This system does not aim to:

- support arbitrary runtime schema/layout changes (future possibility)
- implement full symbol-level dependency invalidation
- automatically infer state migrations
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
 |  \- Function Pointer Table
 |
 |- Compiler Service
 |  |- In-memory file database
 |  |- File-level incremental pipeline
 |  |- Semantic hashing
 |  \- Swap decision logic
 |
 \- Codegen Service (Cranelift JIT/AOT)
    |- Code generation
    |- Executable memory management
    \- Code versioning
```

Disk I/O is not part of the hot path.

## 4. Compilation Model

### 4.1 Granularity

- Invalidation unit: file
- Correctness unit: file
- Emission unit: function
- Dead-code pruning unit: function + struct metadata (reachability-based)

Semantic analysis always runs for the entire file.
Code generation is gated per function.
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
 -> Per-function codegen (gated)
```

Reachability roots for Rewrite V1:
- `main`
- `tick` (when present)
- `on_code_swap` (when present)
- host-required exported entry symbols

This is intentionally simple: no broad optimizer layer in Stasis and no instruction-level DCE requirement before Cranelift.

### 4.3 Semantic Hashing

Each function produces:

- `fnSigHash` - signature/ABI relevant
- `fnBodyHash` - behavior

Rules:

- If `fnBodyHash` unchanged -> reuse machine code
- If layout-affecting semantic fact changes -> full file re-codegen
- Semantic hash comparison gates backend work only

### 4.4 Layout Stability Rule

Hot swap is permitted only if:

- global struct layouts are unchanged
- function signatures are unchanged (initial implementation constraint)

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

### 5.1 Function Pointer Table ABI

All runtime calls use:

```text
FnId -> code_ptr
```

Rules:

- `FnId` stable across recompiles
- Call sites use indirect calls
- No direct calls to raw compiled addresses

This enables atomic swaps without patching call sites.

### 5.2 Runtime API

Compiled code may only call a small, stable runtime API:

Examples:

- logging
- input state
- entity/system helpers
- rendering commands
- audio events

The runtime API:

- uses ABI-stable primitive types
- does not expose compiler internals
- avoids raw pointer exposure unless guaranteed safe

## 6. Hot Swap Mechanism

### 6.1 Two-Phase Commit

Phase 1 - Background Compilation

- File changes ingested
- Semantics computed
- Changed functions identified
- Cranelift compiles changed functions
- Results stored as a pending patch

Phase 2 - Commit (Main Thread, Between Ticks)

Occurs strictly between ticks.

Order:

1. Run `on_code_swap()`
2. Atomically swap function pointers
3. Retire old code generation
4. Signal runtime (for visual feedback)

If any step fails, swap is aborted.

## 7. Swap Hook (User-Defined)

### 7.1 Purpose

Allow explicit adjustment of runtime data after code changes, assuming layout is unchanged.

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
- Runs once per successful swap attempt
- Runs between ticks
- Executes before new code
- May mutate global data
- Must not invoke gameplay entrypoints

### 7.3 Enforcement Rules

- Hook error -> swap aborted
- Layout change -> swap rejected
- Hook runs using old code pointers

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

- Runtime/Main Thread: owns tick loop, safe-point detection, and final swap commit; never performs parsing/semantic/codegen work inline with tick execution.
- Compiler Service Thread: owns lex/parse/index/semantic/hash analysis for changed files and produces either diagnostics or a swap candidate patch.
- Codegen Service (Cranelift): owns JIT code emission for dev mode and AOT emission for prod mode; never mutates runtime state directly.
- Swap Coordinator: owns two-phase commit transaction boundaries, all-or-nothing swap rules, and generation retirement scheduling after successful commit.

### 12.2 High-Level Interface Contracts (Development Mode)

Interfaces are message-based and versioned. No cross-thread shared mutable compiler/runtime objects.

- `FileChangeEvent`: producer file watcher/input bridge; consumer compiler service; fields `path`, `revision`, `text_source`, `change_kind`.
- `CompileRequest`: producer swap coordinator; consumer compiler service; fields `request_id`, `changed_files[]`, `target_mode=jit-dev`.
- `CompileResult`: producer compiler service; consumer swap coordinator; fields `request_id`, `status`, `diagnostics[]`, `layout_hash`, `fn_patch_set`, optional `hook_symbol`.
- `SwapCommitRequest`: producer swap coordinator; consumer runtime/main thread safe-point gate; fields `request_id`, `layout_hash`, `fn_patch_set`, `hook_symbol`.
- `SwapCommitResult`: producer runtime/main thread; consumer swap coordinator + UI/status bridge; fields `request_id`, `status`, `swapped_fn_ids[]`, `new_generation`, `error`.

### 12.3 Development Change Sequence (Single File Save)

1. Watcher emits `FileChangeEvent`.
2. Swap coordinator coalesces pending events and emits `CompileRequest`.
3. Compiler service runs full-file semantic pass.
4. Compiler/codegen returns `CompileResult` with diagnostics or patch.
5. If diagnostics exist, patch is discarded and old code remains active.
6. If eligible, coordinator waits for between-ticks safe point.
7. Main thread runs `on_code_swap()` (if present) using old pointers.
8. Main thread atomically applies pointer-table update and records new generation.
9. Runtime publishes `SwapCommitResult`; debug UI updates swap indicator only on success.

Failure at any sequence step aborts commit and preserves old code/data.

### 12.4 Language Ownership (Rewrite V1)

- `.stasis` owns compiler language behavior: lexer, parser, semantic diagnostics, and incremental compile policy.
- Rust owns runtime/backends: watcher input bridge, message transport, swap coordinator execution, Cranelift JIT/AOT integration, and runtime ABI boundary.

Constraints:

- Compiler frontend semantics must not diverge between languages; `.stasis` is source of truth.
- Rust wrappers may validate/invoke `.stasis` compiler flows but do not re-implement parser/semantic policy.

## 13. Code Memory Management

- Executable memory allocated per generation
- Each successful swap increments generation
- Old generations retired after safe window
- Memory freed in bulk

## 14. Performance Targets

| Scenario               | Target   |
| ---------------------- | -------- |
| Single function edit   | 10-25 ms |
| Multiple function edit | 15-40 ms |
| Comment-only change    | <15 ms   |
| Swap commit            | <1 tick  |

Disk I/O must not be on hot path.

## 15. Development Phases

Phase 0:
- Pointer table only

Phase 1:
- In-process compiler, file-level semantics

Phase 2:
- Per-function codegen gating

Phase 3:
- Cranelift JIT (dev), swap hook, swap indicator

Phase 4:
- Cranelift AOT (prod) artifact path
Phase 5 (Optional):
- LSP integration, live data inspection

## 16. Key Risks & Mitigations

| Risk                   | Mitigation                                        |
| ---------------------- | ------------------------------------------------- |
| Stale code execution   | Atomic pointer swaps                              |
| Partial state mutation | Swap hook transactional                           |
| Developer confusion    | Swap indicator                                    |
| Complexity creep       | File-level only                                   |
| Inlining side effects  | Inline supported but inline change forces rebuild |

## 17. Success Criteria

System is successful when:

- Save -> new logic runs next tick
- Game state persists
- Swap confirmation is visible
- Errors are safe and clear
- Architecture remains understandable

## 18. Summary

This system is intentionally:

- deterministic
- explicit
- tick-based
- safe
- fast
- developer-trust-focused

It provides a robust, file-level hot reload pipeline with per-function efficiency, an explicit swap hook, and a deterministic tick-based UI confirmation mechanism.
