# Incremental JIT Task List

This tracks remaining work after the merged incremental hot-swap foundations.

## 0) Housekeeping

- [x] Close issue `#157` (two-phase commit model) as implemented.
- [x] Close issue `#158` (generation retirement) as implemented.
- [ ] Keep `docs/live-compilation-status.md` synchronized with every task completion.

## 1) Issue #159 Completion (Editor Buffer Path)

- [x] Add swap-time buffer ingestion path in CLI watch mode (stdin overlay protocol).
- [x] Wire VS Code/LSP path to automatically publish unsaved buffer content to watch mode.
- [x] Add structured swap status + diagnostics event channel suitable for editor consumption.
- [x] Add end-to-end doc for editor workflow (watch process + buffer overlay protocol).

## 2) Incremental JIT Backend Scope

- [ ] Decide and document v1 scope:
  - full-module CLIF swaps with frontend function gating, or
  - true changed-function patch protocol to runner.
- [ ] If true patch protocol is chosen, add runner command(s) for changed-function patch apply.
- [ ] Add compatibility checks for patch-set apply (signature/layout invariants).
- [ ] Add telemetry for patch-set size and apply timing.

## 3) In-Process JIT Path Parity

- [ ] Replace in-process AOT+link loading with direct in-process JIT code generation (target architecture from PRD).
- [ ] Keep generation retirement semantics under in-process JIT path.
- [ ] Add data-binding support for in-process watch path.
- [ ] Lift headless-only restriction when host/runtime APIs are ready.

## 4) Testing Matrix

- [x] Unit tests for overlay import expansion (`SourceImporterTests`).
- [x] Deterministic tests for overlay command handling (`set`, `clear`, `clear_all`) in watch loop.
- [x] JIT-runner watch integration test for overlay-triggered swap without disk edits.
- [ ] Long-run swap soak test (100+ swaps) for stability and bounded retirement/memory metrics.
  - Note: a short multi-swap stability test is now in place (`WatchTickJitSwap_PipeOverlay_MultiSwapStability`); full 100+ soak remains.
- [ ] Performance regression harness for edit->applied latency (single-function and multi-function edits).

## 5) Reliability and Ops

- [ ] Standardize process timeout + bounded output drain behavior in all runner interactions.
- [ ] Add explicit stale-process cleanup guidance to docs/test scripts.
- [ ] Ensure CI excludes IO-heavy/graphics-heavy suites by default while preserving deterministic coverage.

## Immediate Execution Order

1. Add structured watch event channel (machine-readable) for swap status + diagnostics. [DONE]
2. Hook VS Code/LSP workflow to emit overlay updates into watch process. [DONE]
3. Add deterministic integration tests for overlay-triggered swap in JIT runner path. [DONE]
