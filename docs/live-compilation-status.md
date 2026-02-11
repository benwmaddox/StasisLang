# Live Compilation Status

This document tracks the current Rust-first hot-swap direction.

## Active Path (Supported)

- Tick watch hot-swap uses Rust runner paths:
  - `WatchCraneliftTickJitSwap` (Rust JIT runner protocol)
  - `WatchCraneliftTickHotSwap` (runner/DLL swap flow)
- The C# CLI remains the frontend/orchestration layer (parse/sema/layout/lowering, diagnostics, watch).
- Swap safety features remain in the Rust runner path:
  - layout compatibility checks
  - `on_code_swap` handling
  - all-or-nothing swap apply/reject behavior

## Removed / Deprecated

- The legacy C# in-process tick host path has been removed.
- `STASIS_CRANELIFT_INPROC_TICK=1` is now a deprecated compatibility flag and is ignored.
- When set, CLI prints:
  - `warning: STASIS_CRANELIFT_INPROC_TICK is deprecated and ignored; using Rust JIT runner watch path.`

## Recent Changes In This Branch

- Removed outdated in-process host implementation from `Stasis.Cli/Program.cs`.
- Removed in-process host integration tests that depended on the removed architecture.
- Added replacement integration test:
  - `WatchTickHotSwap_DeprecatedInProcessFlag_FallsBackToRustRunner`
- Added migration plan doc:
  - `docs/rust-jit-modularization-plan.md`

## Next Steps

1. Extract Rust swap/state logic into reusable crates (`jit_core`, `swap_core`).
2. Introduce a versioned host capability table ABI for graphics/audio/input/system plugins.
3. Add Rust library-mode host embedding for direct in-process usage without reviving C# runtime swap logic.
