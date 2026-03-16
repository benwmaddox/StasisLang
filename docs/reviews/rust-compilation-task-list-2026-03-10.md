# Rust Compilation Task List

Date: 2026-03-10

Source review:
- `docs/reviews/rust-compilation-review-2026-03-10.md`

Goal:
- make the Rust compilation process clearer and more efficient
- keep JIT and AOT as close as possible in IR generation, excluding the JIT function-table indirection used for hot swapping

## Task 1: Collapse to One AOT Object-Generation Path

Priority: High

Status:
- Completed
- completed cleanup in this slice:
- removed the legacy C# compiler/CLI/bootstrap trees and stale `.NET` repo wiring so the repo no longer carries a second host implementation language
- removed the old textual-CLIF helper path from `crates/stasis_jit`
- removed `tools/cranelift-aot`
- removed the retained self-host runtime-bridge/test scaffolding from `apps/stasis`
- reduced `apps/stasis` AOT/self-host coverage to tests that exercise the surviving direct AOT path

Task:
- make `crates/stasis_compiler`'s in-process `AotProcess` the primary AOT backend
- remove or strictly demote the textual-CLIF helper flow used through `apps/stasis` and `crates/stasis_jit`
- if the helper must remain temporarily, move it into the workspace and pin it to the same Cranelift version as the main compiler

Why it is needed:
- there are currently two AOT stories in the repo
- the helper path compiles emitted CLIF text through a separate toolchain
- that helper is outside the workspace and uses a different Cranelift version
- this is the largest current source of JIT/AOT drift risk

Risk:
- medium to high
- AOT packaging, self-host flows, and release plumbing may depend on the helper behavior
- some tests or workflows may currently assume CLIF text artifacts exist

Benefit:
- highest impact item on parity and clarity
- one backend path is easier to reason about, test, and maintain
- removes version skew and reduces backend-specific bugs

## Task 2: Replace String-Backed HIR with Structured Frontend Output

Priority: High

Task:
- replace `FunctionHIR { blocks: Vec<Block { source: String }> }` with a typed statement/expression representation
- parse once in the frontend, then lower that structured form in both JIT and AOT
- keep backend code focused on Cranelift lowering rather than reparsing source text

Why it is needed:
- the current "HIR" is only function source slices
- backend emit reparses statements from source text for each emitted function
- this weakens both efficiency and phase clarity

Risk:
- medium
- this changes the compiler seam between frontend and backend
- parser, lowering, diagnostics, and tests will all move together

Benefit:
- improves compile efficiency by removing repeated parsing work
- makes IR generation more deterministic and comparable across backends
- creates a cleaner architecture for future optimizations and diagnostics

## Task 3: Fix `AotProcess` Artifact Retention

Priority: Medium

Status:
- Completed
- `AotProcess` now clears and rebuilds string-literal state each compile
- active object bytes are compacted after reachable-artifact pruning
- artifact object indices are re-based after compaction
- regression tests were added for bounded retained object storage and literal-table rebuild behavior

Task:
- stop `AotProcess` from monotonically growing `object_bytes`
- either:
- store active object bytes directly by `FunctionId`
- or compact stale entries after each compile
- add a regression test that recompiles repeatedly and verifies retained artifact storage stays bounded

Why it is needed:
- replaced AOT artifacts are removed from the active artifact list
- their old object bytes remain in the backing vector
- repeated incremental compiles can therefore grow memory usage unnecessarily

Risk:
- low to medium
- artifact indexing and write-out code will need to change together
- stale index bugs are possible if the storage model changes carelessly

Benefit:
- improves long-running incremental AOT sessions
- reduces memory waste
- simplifies artifact lifecycle ownership

## Task 4: Replace AOT Extern Heuristics with an Explicit Runtime Export Contract

Priority: Medium

Status:
- Completed
- replaced the AOT extern prefix heuristic with an explicit runtime export contract in `crates/stasis_compiler`
- added regression coverage that fake `gfx_*` externs now fail unless the symbol exists in the contract
- kept explicit single-symbol extern annotations working for non-runtime link targets

Task:
- define the runtime symbols AOT is allowed to bind against in one explicit manifest or generated export list
- resolve AOT externs against that contract instead of a hardcoded allowlist heuristic
- fail compile or packaging early when the runtime export set does not satisfy AOT assumptions

Why it is needed:
- AOT extern resolution is shared now, which is good
- but the AOT side still recognizes symbols through a "known runtime symbols" heuristic
- this leaves room for contract drift between compile time and link/runtime time

Risk:
- medium
- build and packaging steps may need to emit or consume an export manifest
- mistakes in the first manifest version can block valid builds until the contract is correct

Benefit:
- stronger correctness guarantees
- clearer ownership of the runtime boundary
- easier to audit and update when host/runtime symbols change

## Task 5: Expand JIT/AOT Parity Coverage

Priority: Medium

Task:
- create a small parity fixture corpus that runs through both JIT and AOT
- cover:
- internal calls
- extern calls
- globals and collection access
- control flow
- struct-view ABI cases
- string literal handling
- compare both behavior and, where practical, emitted Cranelift IR text shape from the shared backend path

Why it is needed:
- current parity tests are real but narrow
- they do not yet cover enough of the shared lowering surface for the stated IR-parity goal

Risk:
- low
- some tests will be environment-gated when linking is unavailable
- IR text comparisons may need normalization if output includes nondeterministic naming

Benefit:
- prevents backend drift from reappearing
- gives confidence when refactoring shared lowering
- turns the JIT/AOT closeness requirement into an enforceable contract

## Suggested Execution Order

1. Collapse to one AOT object-generation path.
2. Fix `AotProcess` artifact retention.
3. Replace AOT extern heuristics with an explicit runtime export contract.
4. Expand JIT/AOT parity coverage around the shared backend.
5. Replace string-backed HIR with structured frontend output.

## Why This Order

Start by removing the biggest architectural split first. That reduces the number of moving parts before deeper compiler refactors.

Artifact retention and extern-contract work are smaller, lower-risk improvements that tighten the system while the backend surface is still fresh.

Parity coverage should be expanded before and during the HIR refactor so the larger structural change has stronger regression protection.

The structured-HIR task is the largest internal compiler change. It is worth doing, but it will be safer after the repo is already using one AOT path and has better parity tests in place.

## Current Progress Notes

- Live AOT execution has been rebased onto the in-process backend first, which removes the main runtime drift point between JIT and AOT.
- Retention cleanup in `AotProcess` is done and covered by focused regression tests.
- Task 1 cleanup is complete: the old helper/test bridge surface and the older C# compiler path have been removed instead of retained as historical scaffolding.
- Task 4 is complete: AOT extern candidate resolution now checks an explicit runtime export list instead of accepting whole `stasis_jit_gfx_*` / `stasis_jit_audio_*` families by prefix.
- Focused validation passed after the cleanup:
- `cargo test -p stasis_jit`
- `cargo test -p stasis_compiler backend:: -- --nocapture`
- `cargo test -p stasis --lib compiler_backend::tests::aot_compile_writes_manifest_with_artifacts_on_success -- --nocapture --test-threads=1`
- `cargo test -p stasis --lib compiler_backend::tests::self_host_aot_cli_writes_default_summary_sidecar -- --nocapture --test-threads=1`
- `cargo test -p stasis --lib compiler_backend::tests::self_host_aot_cli_is_deterministic_across_repeated_runs_with_same_source -- --nocapture --test-threads=1`
- Additional focused validation for Task 4:
- `cargo test -p stasis_compiler aot_process_rejects_fake_runtime_prefix_extern_without_export_contract_entry -- --nocapture`
- `cargo test -p stasis_compiler aot_process_accepts_known_runtime_shim_families -- --nocapture`
- `cargo test -p stasis_compiler aot_process_prefers_known_runtime_extern_symbol_over_source_alias -- --nocapture`
- `cargo test -p stasis_compiler aot_runtime_export_contract_requires_exact_symbol_matches -- --nocapture`
