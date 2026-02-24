# Rust-Native Compiler PRD Trial

This branch tracks an experimental Rust-native compiler direction focused on compile speed and simple architecture.

## Targets
- `<250ms` cold compile at 1k functions.
- `<5ms` typical incremental compile for single-function edits.
- Single frontend and dual backend (`JIT` for dev, `AOT` for release).
- Direct lowering to CLIF without retaining a full AST.
- Minimal unsafe surface (isolated backend/runtime boundaries only).

## Pipeline
1. Index pass:
- Parse function signatures.
- Build symbol table.
- Compute signature/body hashes.
- Mark dirty functions.
- Propagate dirty state across dependents on signature changes.
2. Emit pass:
- Parse/lower dirty function bodies only.
- Emit backend artifacts.
- Finalize backend state.

## Current Trial Slice (This Branch)
- Added Rust-native module layout under `crates/stasis_compiler/src/`:
- `frontend/lexer.rs`
- `frontend/parser.rs`
- `frontend/indexer.rs`
- `frontend/types.rs`
- `ir/hir.rs`
- `backend/mod.rs`
- `backend/jit.rs`
- `backend/aot.rs`
- `compiler.rs`
- Added a minimal two-pass compiler implementation (`index_pass` + `emit_pass`) with:
- function metadata hashing
- symbol table lookup by name hash
- dependency/dependent graph links
- signature-change dirty propagation
- dirty-only emission behavior
- Simplified backend integration to mode-specific full processes:
- `backend::jit::JitProcess` runs end-to-end JIT flow for a compile invocation.
- `backend::aot::AotProcess` runs end-to-end AOT flow for a compile invocation.
- Added explicit engine-facing split:
- JIT emits an in-process engine package (`tick`/`render`/optional `on_code_swap` -> code pointers) for runtime embedding.
- AOT emits an engine bundle (object files + manifest) with required entrypoints validated before output.
- Runtime/graphics ownership target: keep windowing/render-loop integration in Rust host runtime code; dev uses `JitEnginePackage` as active execution handoff, prod uses `AotEngineBundle`.
- `apps/stasis` real backend now uses an engine-mode in-process fast path (`tick` + `render` present) that bypasses legacy external-analysis host compile for that mode and emits compile contracts from rust-native process outputs.
- `apps/stasis` non-engine `JitDev` compile path now enforces rust-native-only JIT compile contracts and returns explicit diagnostics on unsupported cases (no silent legacy fallback for this mode).
- Dev runtime commit path now accepts JIT pointer overrides (`FnId -> code_ptr`) carried through compile results, allowing pointer-table commits to apply real JIT function pointers when present.
- Removed backend trait indirection from the core compiler trial path.
- Wired real Cranelift emission in both mode processes for currently supported function bodies:
- JIT: emits machine code through `cranelift-jit` and records finalized code pointers.
- AOT: emits object bytes through `cranelift-object`.
- Current body support in backend emission is intentionally narrow: `return <i32 literal>;`, `return <i32 expr op expr>;` (`+ - * / %`) where `expr` is a literal or function parameter identifier, and `return;` (for `void`).
- JIT path now supports in-memory execution verification in tests by invoking finalized function pointers directly (`noarg -> i32`) after compile.
- AOT path now supports executable smoke verification in tests (`object -> linked exe -> process exit code`) when a Windows linker is available.
- AOT defaults to an optimization-oriented profile (`speed`) and supports explicit profile selection.
- Kept existing compiler host path intact for compatibility while evaluating this approach.

## Trial Test Coverage Added
- `compiler::tests::first_index_marks_all_functions_dirty`
- `compiler::tests::unchanged_source_emits_nothing_after_initial_emit`
- `compiler::tests::body_only_change_marks_only_changed_function_dirty`
- `compiler::tests::signature_change_propagates_dirty_to_dependents`
- `frontend::parser::tests::*`
- `frontend::indexer::tests::*`

## Benchmark Snapshot (2026-02-24)
- Command:
- `cargo run -p stasis_compiler --example rust_native_jit_bench -- --functions 1000,5000 --cold-samples 3 --incremental-samples 5`
- Results:
- `1,000` functions: cold `p50=349.027ms`, cold `p95=354.803ms`; one-function JIT update `p50=2.982ms`, `p95=3.145ms`.
- `5,000` functions: cold `p50=1738.212ms`, cold `p95=1755.483ms`; one-function JIT update `p50=13.262ms`, `p95=13.672ms`.

## Timing Scope Notes
- Current benchmark numbers measure compiler/JIT path only.
- Not included:
- AOT object writing/linking to a final `.exe`.
- Engine-side packaging/load/swap/render-loop overhead.
- Process startup and `cargo build` overhead.

## Engine Overhead Snapshot (2026-02-24)
- Command:
- `cargo run -p stasis --example engine_overhead_bench -- --mode both --samples 1 --ticks 240`
- Results (single-sample smoke baseline):
- `jit`: total `124.577ms`, compile `2.000ms`, commit `0.000ms`, runtime-overhead `122.577ms`.
- `aot`: total `124.029ms`, compile `2.000ms`, commit `0.000ms`, runtime-overhead `122.029ms`, load-artifact `0.049ms`.

## Next Trial Slices
- Expand backend body support beyond literal-return-only while keeping direct one-pass lowering.
- Improve compile-time performance against target gates (`<250ms @1k cold`, `<5ms typical single-function update`).
- Add a deferred end-to-end engine overhead benchmark/test that includes package/build artifact handoff, load, swap commit, and render-loop progression timing.
- Added engine-overhead benchmark harness command:
- `cargo run -p stasis --example engine_overhead_bench -- --mode both --samples 3 --ticks 240`
