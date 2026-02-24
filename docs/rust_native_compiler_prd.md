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
- Removed backend trait indirection from the core compiler trial path.
- Wired real Cranelift emission in both mode processes for currently supported function bodies:
- JIT: emits machine code through `cranelift-jit` and records finalized code pointers.
- AOT: emits object bytes through `cranelift-object`.
- Current body support in backend emission is intentionally narrow: `return <i32 literal>;`, `return <i32 literal op literal>;` (`+ - * / %`), and `return;` (for `void`).
- JIT path now supports in-memory execution verification in tests by invoking finalized function pointers directly (`noarg -> i32`) after compile.
- Kept existing compiler host path intact for compatibility while evaluating this approach.

## Trial Test Coverage Added
- `compiler::tests::first_index_marks_all_functions_dirty`
- `compiler::tests::unchanged_source_emits_nothing_after_initial_emit`
- `compiler::tests::body_only_change_marks_only_changed_function_dirty`
- `compiler::tests::signature_change_propagates_dirty_to_dependents`
- `frontend::parser::tests::*`
- `frontend::indexer::tests::*`

## Next Trial Slices
- Expand backend body support beyond literal-return-only while keeping direct one-pass lowering.
- Add deterministic compile-time benchmarks for cold/incremental timings against the PRD targets.
- Add e2e execution validation on emitted JIT/AOT artifacts for a minimal `main` path.
