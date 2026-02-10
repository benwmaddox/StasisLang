# Live Compilation Status (PRD Alignment)

This document tracks implementation progress for the in-process live compilation/hot-swap PRD.

## Implemented in this branch

### File-level semantic fingerprint gate (JIT watch path)

- Added `SemanticFingerprint.ComputeFileFingerprint(...)` in `Stasis.Compiler`.
- Fingerprint combines:
  - deterministic global layout hash
  - normalized token stream hash (whitespace/comments ignored by lexer)
- Integrated into `WatchCraneliftTickJitSwap` (`Stasis.Cli/Program.cs`):
  - full parse + semantic + layout still run per edit
  - if fingerprint is unchanged, no swap is queued (`HOTSWAP(skip)`)
  - existing code remains active; no partial state change

This is an incremental step toward PRD sections:
- file-level correctness/invalidation
- semantic hash gating
- failure-safe swap behavior

### Layout-safe swap rejection (JIT runner)

- Added strict state-layout compatibility check before applying a compiled swap.
- If persisted `state__*` globals differ by name or size, swap is rejected.
- Rejected swaps keep old code and data active and emit an explicit `ERR id=... swap layout changed...`.

This aligns with PRD layout stability and "abort cleanly, never partial" goals.

### Optional `on_code_swap` hook (JIT runner)

- Added optional hook discovery for `{module}__on_code_swap`.
- Supported signatures:
  - `function on_code_swap(): i32`
  - `function on_code_swap()`
- Hook runs on the currently active (old) code before state handoff to new code.
- Non-zero return aborts swap; runner restores pre-hook state snapshot and keeps old code active.

This provides explicit developer-controlled state adjustment while preserving safe failure semantics.

### Experimental in-process tick host (no external JIT runner process)

- Added an experimental watch-mode path enabled by `STASIS_CRANELIFT_INPROC_TICK=1`.
- Path uses the existing Cranelift lowering/AOT pipeline, then runs `main`/`tick` from the compiled module in-process (no `stasis-cranelift-jit-runner` process).
- Supports state-preserving swaps by loading a new module, verifying state-map compatibility, copying persisted state, and atomically switching active tick target under a lock.
- Includes optional `on_code_swap` invocation (`void` or `i32`) before state handoff.
- Current constraints:
  - headless programs only (graphics/runtime host APIs are rejected)
  - no data-binding reload in this path yet

This is the first implementation slice toward the in-process architecture target.

## Already present before this branch

- File watch + debounce + rebuild loop.
- Tick-time swap workflow for Cranelift (runner protocol with `INIT` / `SWAP`).
- Safe-point style behavior (swap between ticks in runner flow).
- State-map and data-binding metadata emission.
- Recovery behavior for failed builds and runner restarts.

## Not implemented yet (planned)

- Per-function semantic hashes (`fnSigHash` / `fnBodyHash`) and function-level codegen gating.
- In-process JIT service replacing external runner process.
- Explicit two-phase commit object model (`pending patch` + atomic commit step).
- Generation-based code memory retirement inside host process.
- VS Code buffer-native compilation input path (LSP push, no disk dependency).

## Next concrete step

Implement a `PendingSwapPlan` model in CLI/runtime boundary:
- build patch in background
- validate layout/signature compatibility
- commit only at safe point
- emit explicit commit telemetry (`compiled`, `queued`, `applied`, `rejected`).
