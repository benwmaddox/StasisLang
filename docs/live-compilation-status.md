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
- Supports data-binding reload from JSON in this path:
  - auto-discovers `data/*.json` near the source like the runner path
  - emits `DATABIND: registered ...` on startup
  - emits `DATABIND: reloaded ...` when file content changes and bindings are applied
- Added integration coverage:
  - `HotSwapIntegrationTests.WatchTickInProcessSwap_AppliesAndReloadsDataBinding`
- Current constraints:
  - headless programs only (graphics/runtime host APIs are rejected)

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
  - enabled by `STASIS_BUFFER_OVERLAY_STDIN=1` and/or `STASIS_BUFFER_OVERLAY_PIPE=<name>`
  - reads JSON line commands (`set` / `clear` / `clear_all`)
  - supports stdin and named-pipe ingestion for editor-driven updates
  - overlays source text by absolute path (supports `file://` URIs)
- Wired overlay source loading through JIT and in-process watch swap paths:
  - `WatchCraneliftTickJitSwap`
  - `WatchCraneliftTickInProcessSwap`
- AOT runner watch mode (`WatchCraneliftTickHotSwap`) intentionally ignores overlay stdin to avoid consuming guest program stdin.
- Import expansion and runtime-import detection now honor overlay sources for imported files.
- Diagnostic printing in watch mode now resolves line/column from the overlay text for imported files when available.
- Added coverage:
  - `SourceImporterTests.ExpandImports_UsesSourceLoaderForImportedFiles`
  - `SourceImporterTests.ExpandImports_UsesOverlayPlatformSpecificFile`
  - `HotSwapIntegrationTests.WatchTickJitSwap_SwapsFromPipeOverlayWithoutDiskEdit`
  - `HotSwapIntegrationTests.WatchTickJitSwap_PipeOverlay_SetClearAndClearAll`
  - `HotSwapIntegrationTests.WatchTickJitSwap_PipeOverlay_MultiSwapStability`
- VS Code extension now supports pushing unsaved buffers to watch mode via configured pipe:
  - `stasis.watchOverlayPipe` setting writes `set`/`clear` overlay commands on open/change/close.
- Added workflow guide: `docs/editor-buffer-overlay-workflow.md`.

### Structured watch event channel (incremental #159 support)

- Added optional machine-readable watch events in `Stasis.Cli`:
  - enable with `STASIS_WATCH_EVENT_JSON=1`
  - emitted as `WATCH_EVENT {json}` lines on stdout
- Event types currently emitted:
  - `swap_state` (in-process and JIT runner watch paths)
  - `diagnostic` (includes severity/message/file/span metadata)
- This provides a stable bridge for editor tooling to consume swap outcomes without parsing human-readable log lines.
- Added coverage:
  - `CliSnapshotTests.Error_ParseError_EmitsStructuredDiagnosticEvent_WhenEnabled`
  - `HotSwapIntegrationTests.WatchTickInProcessSwap_SwapsOnEdit_WithoutJitRunnerProcess` (asserts `swap_state` events when enabled)

### Brickout portrait-window verification on JIT watch path

- Added optional JIT runner window-size telemetry:
  - set `STASIS_JIT_LOG_WINDOW_SIZE=1`
  - runner emits `WINDOW init size=<w>x<h> orientation=<...>` (and after swap apply)
- Added integration coverage:
  - `HotSwapIntegrationTests.WatchTickJitSwap_BrickoutV1_StartsPortraitWindow`
  - verifies Brickout v1 starts with a portrait window (`height > width`) while running through watch + Cranelift JIT runner.

### Incremental JIT backend scope locked (v1)

- Locked v1 backend scope to full-module CLIF swap payloads with frontend function-level gating.
- Documented in `docs/incremental-jit-v1-scope.md`.
- Deferred function-patch runner protocol to v2 (explicitly tracked in task list).

### Long-run soak + latency harness coverage

- Added env-gated long soak coverage:
  - `HotSwapIntegrationTests.WatchTickJitSwap_PipeOverlay_LongSoak_100PlusSwaps`
  - enabled with `STASIS_RUN_LONG_HOTSWAP=1`
  - default cycles `120` (override `STASIS_LONG_HOTSWAP_CYCLES`)
- Added env-gated latency harness coverage:
  - `HotSwapIntegrationTests.WatchTickJitSwap_PipeOverlay_LatencyHarness_SingleVsMultiFunction`
  - enabled with `STASIS_RUN_HOTSWAP_PERF=1`
  - default iterations per phase `8` (override `STASIS_HOTSWAP_PERF_ITERATIONS`)
  - optional budgets:
    - `STASIS_PERF_MAX_SINGLE_LATENCY_MS`
    - `STASIS_PERF_MAX_MULTI_LATENCY_MS`

### Hotstate metadata write hardening

- Hardened `WriteAllTextAtomic` in `Stasis.Cli/Program.cs`:
  - unique per-attempt temporary files
  - retry-safe cleanup on sharing/permission errors
- Prevents watch-loop crashes from locked stale `.tmp` files during hotstate metadata writes.

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

Execute remaining in-process architecture steps.
- Replace in-process AOT+link load path with direct in-process JIT codegen.
- Keep generation-retirement semantics as direct-JIT plumbing lands.
- Keep graphics/headful support scoped separately after parity is stable.
