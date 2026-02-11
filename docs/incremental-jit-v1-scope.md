# Incremental JIT v1 Scope (Locked)

This document locks the backend scope for incremental JIT v1.

## Decision

v1 uses:

- full-module CLIF swap payloads to the JIT runner
- frontend function-level semantic diffing to gate lowering/codegen work
- swap-time state safety checks (layout compatibility + optional `on_code_swap`)

v1 does **not** include changed-function patch protocol messages to the runner.

## Why this scope

- Keeps runtime/runner boundary simple and deterministic (`SWAP` remains one payload, one commit path).
- Preserves current two-phase swap model without introducing per-function patch ordering hazards.
- Captures most iteration wins now via:
  - semantic no-op skip (`HOTSWAP(skip)`)
  - function-body reuse during lowering/codegen
  - deterministic queue/apply/reject telemetry

## v1 Invariants

- File-level semantic correctness remains mandatory on every rebuild.
- Runner swap apply remains atomic (all-or-nothing).
- Layout/signature incompatibilities reject swap and keep old code/data active.

## Deferred to v2

The following are intentionally deferred behind a future patch-protocol design:

- runner commands for changed-function patch apply
- patch-set compatibility checker (signature/layout/callability invariants)
- patch-set telemetry (`patch_fn_count`, `patch_bytes`, apply timing)

## Impact on current task list

- Backend scope is no longer ambiguous for v1.
- Existing work should optimize within full-module swap constraints.
- Patch protocol items remain tracked as v2 tasks.
