# Rust-First JIT/AOT Modularization Plan

## Goal

Move runtime hot-swap execution responsibilities from mixed C#/C code paths to a clear Rust-owned backend, while keeping host libraries (graphics/audio/sys) pluggable through explicit ABI boundaries.

## Current Problems

- Two hot-swap execution approaches existed:
  - Rust JIT runner flow (`stasis-cranelift-jit-runner`)
  - C# in-process tick host flow (`STASIS_CRANELIFT_INPROC_TICK`) using AOT object + clang link + DLL load
- The C# in-process path duplicated swap orchestration logic and created drift risk.
- Host integration is symbol-name heavy; capability negotiation/versioning is not explicit.

## Target Architecture

1. C# frontend/compiler service:
- Parse/sema/layout/lowering orchestration.
- Diagnostics and editor/watch UX.
- Emits CLIF and swap intents.

2. Rust execution core:
- Owns JIT/AOT execution lifecycle and swap commit semantics.
- Owns state migration checks and `on_code_swap` handling.
- Owns generation retirement and telemetry.

3. Host libraries (C or Rust):
- Expose a stable, versioned ABI surface.
- Loaded as capabilities (graphics, audio, input, filesystem/sys).

## Migration Phases

### Phase 1 (this branch)

- Remove deprecated C# in-process tick-host execution path.
- Route tick watch hot-swap to Rust runner path.
- Keep compatibility env var (`STASIS_CRANELIFT_INPROC_TICK`) as deprecated no-op with warning.
- Remove outdated tests that validated removed behavior.
- Add test validating deprecated flag fallback to Rust runner path.

### Phase 2

- Extract Rust swap/state logic into reusable crate (`jit_core` + `swap_core`) shared by runner mode(s).
- Introduce explicit host API capability table (versioned) instead of ad-hoc symbol probes.
- Add structured capability/version mismatch diagnostics.

### Phase 3

- Introduce Rust in-process host API (library mode) for direct embedding without process protocol changes.
- Keep runner process mode as fallback and for isolation.

### Phase 4

- Optional: shrink C runtime boundary further (keep C only for unavoidable platform/media bindings).
- Add compatibility test matrix for host plugin ABI versions.

## Done in Phase 1

- Deprecated C# in-process tick host path removed from CLI.
- Tick watch selection now always uses Rust-runner paths for hot-swap.
- New warning for deprecated env var:
  - `STASIS_CRANELIFT_INPROC_TICK is deprecated and ignored; using Rust JIT runner watch path.`
- Integration coverage updated to assert fallback behavior.

## Cleanup Rules Going Forward

- Do not keep parallel execution engines for the same mode.
- Any new swap semantics must land in Rust execution core first.
- C# should orchestrate compile/watch, not implement competing swap/runtime engines.
- Remove dead code/tests in the same PR that deprecates a path.
