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

### Per-function semantic hashes + gated function-body codegen (#156)

- Added `FunctionSemanticFingerprint` in `Stasis.Compiler`:
  - computes per-function `fnSigHash` and `fnBodyHash` for reachable functions
  - tracks layout hash as part of diff classification
  - classifies edits as no-op, partial body-only change, or conservative rebuild
- Integrated diffing into both watch flows:
  - `WatchCraneliftTickJitSwap`
  - `WatchCraneliftTickInProcessSwap`
- Watch behavior now:
  - skips swap when function hashes are unchanged (`HOTSWAP(skip)`)
  - uses partial rebuild set for body-only changes
  - forces conservative rebuild on layout/signature/function-set changes
- Added incremental Cranelift function-body reuse:
  - unchanged functions reuse cached lowered bodies
  - changed functions are rebuilt
  - telemetry now includes `fnBuilt` / `fnReused` counts
- Added test coverage:
  - `FunctionSemanticFingerprintTests` for hash/diff behavior
  - `CraneliftIncrementalCodegenTests` for body reuse parity and counters

### Explicit two-phase swap commit model (#157)

- Added explicit swap transition states in watch output:
  - `HOTSWAP(state): compiled ...`
  - `HOTSWAP(state): queued ...`
  - `HOTSWAP(state): applied ...`
  - `HOTSWAP(state): rejected ...`
- In-process tick host now uses an explicit pending-swap artifact:
  - compile/link runs before queueing swap work
  - swaps are queued via `QueueSwap(...)`
  - commit is executed by the tick thread at safe points between ticks
  - hook order is deterministic: old-code `on_code_swap` executes before state transfer to the new module
- Swap commit remains all-or-nothing:
  - layout mismatch, hook failure, load failure, or restore failure rejects the full swap
  - old code/data remain active on rejection
- Runner watch path now tracks pending patches with build IDs and emits transition telemetry for queued/applied/rejected outcomes.

### Generation-based code memory retirement (#158)

- In-process tick host now assigns a monotonic generation ID to each loaded module.
- On successful swap:
  - active generation increments
  - previous module is moved to a retired-generation queue (not freed immediately)
- Retired generations are disposed in bulk after a bounded safe window (`STASIS_INPROC_RETIRE_WINDOW_FRAMES`, default `2`).
- Added generation telemetry to in-process swap state output:
  - `gen=...`
  - `retire_pending=... retire_pending_bytes=...`
  - `retired=... retired_bytes=...`
  - `retire_window=... tick=...`
- Added integration coverage (`WatchTickInProcessSwap_ReportsGenerationRetirementTelemetry`) to validate:
  - generation increases monotonically across swaps
  - pending retired generations remain bounded
  - retired-generation counters advance over repeated swaps

### Swap-time buffer overlay input for watch mode (#159, incremental)

- Added a built-in source overlay bridge in `Stasis.Cli` watch mode (`BufferOverlayBridge`):
  - enabled explicitly with `STASIS_BUFFER_OVERLAY_STDIN=1`
  - reads JSON line commands from stdin (`set` / `clear` / `clear_all`)
  - overlays source text by absolute path (supports `file://` URIs)
- Wired overlay source loading through all watch swap paths:
  - `WatchCraneliftTickHotSwap`
  - `WatchCraneliftTickJitSwap`
  - `WatchCraneliftTickInProcessSwap`
- Import expansion and runtime-import detection now honor overlay sources for imported files.
- Diagnostic printing in watch mode now resolves line/column from the overlay text for imported files when available.
- Added coverage:
  - `SourceImporterTests.ExpandImports_UsesSourceLoaderForImportedFiles`
  - `SourceImporterTests.ExpandImports_UsesOverlayPlatformSpecificFile`
  - `HotSwapIntegrationTests.WatchTickInProcessSwap_AcceptsOverlaySourceOnSwap`

## Already present before this branch

- File watch + debounce + rebuild loop.
- Tick-time swap workflow for Cranelift (runner protocol with `INIT` / `SWAP`).
- Safe-point style behavior (swap between ticks in runner flow).
- State-map and data-binding metadata emission.
- Recovery behavior for failed builds and runner restarts.

## Not implemented yet (planned)

- In-process JIT service replacing external runner process.
- Direct VS Code LSP push transport for swap status/diagnostics (currently stdin overlay protocol is available as the ingestion path).

## Next concrete step

Complete issue #159 end-to-end editor integration.
- Wire the VS Code extension/LSP host to emit stdin overlay commands automatically for unsaved buffers.
- Add structured swap outcome events back to editor surfaces (status + diagnostics mapping).
