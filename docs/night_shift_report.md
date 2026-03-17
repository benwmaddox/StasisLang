# Night Shift Report

## 2026-03-16

- Prepared StasisLang for Night Shift style repo-local runs.
- Added process docs, validation script, and launcher script aligned with the existing Cargo and CI workflow.
- Verification: `tools/validate_repo.sh`
- Needs input from user: decide whether inbox-synced review feedback should map to `docs/bugs.md` only or also back-reference explicit checklist sections when both apply.
- Completed review Task 4 from `docs/reviews/rust-compilation-task-list-2026-03-10.md` by replacing AOT extern prefix heuristics with an explicit runtime export contract in `crates/stasis_compiler/src/backend/runtime_exports.rs`.
- Added regression coverage so fake `gfx_*` externs no longer resolve unless their symbol is explicitly exported, while existing runtime-shim and explicit-symbol extern cases still pass.
- Verification: `cargo test -p stasis_compiler aot_process_rejects_fake_runtime_prefix_extern_without_export_contract_entry -- --nocapture`, `cargo test -p stasis_compiler aot_process_accepts_known_runtime_shim_families -- --nocapture`, `cargo test -p stasis_compiler aot_process_prefers_known_runtime_extern_symbol_over_source_alias -- --nocapture`, `cargo test -p stasis_compiler aot_runtime_export_contract_requires_exact_symbol_matches -- --nocapture`, `tools/validate_repo.sh`
- Good: the failing case was easy to isolate because extern candidate resolution already sat behind one shared helper.
- Bad: the runtime export surface was implicit across `stasis_dynload` and compiler tests, so enumerating the real contract required source spelunking.
- Adjustment: keep runtime-callable export symbols in one compiler-owned table and add focused tests whenever a new export family is introduced.
- Completed review Task 5 from `docs/reviews/rust-compilation-task-list-2026-03-10.md` by adding `parity_corpus_covers_shared_lowering_shapes` in `crates/stasis_compiler/src/backend/aot.rs`.
- The new corpus covers extern calls, globals/collection access, control flow, struct-view field access, and string literal handling; it also captures AOT CLIF text and checks stable shape markers from the shared lowering path.
- Verification: `cargo test -p stasis_compiler parity_corpus_covers_shared_lowering_shapes -- --nocapture`, `cargo test -p stasis_compiler aot_engine_bundle_manifest_includes_string_literals -- --nocapture`, `cargo test -p stasis_compiler aot_process_prefers_known_runtime_extern_symbol_over_source_alias -- --nocapture`, `tools/validate_repo.sh`
- Good: writing the parity cases as one corpus made it easy to tighten IR-shape assertions after the first run exposed where helper calls actually appear.
- Bad: some CLIF expectations that looked obvious at first were wrong because collection and struct-view access lower through shared helper calls rather than inline load/store ops.
- Adjustment: keep future parity CLIF checks at the shared-lowering seam that is actually stable, and use behavior assertions for the rest instead of overfitting to incidental instruction placement.

## 2026-03-17

- Addressed PR #247 review feedback in `tools/nightshift.sh` by rejecting detached-HEAD preserve runs unless `NIGHTSHIFT_EXPECT_BRANCH` names a local branch whose tip matches `HEAD`, in which case the script now reattaches before continuing.
- Added `tools/test_nightshift.sh` and wired it into `tools/validate_repo.sh` so detached preserve-mode rejection and explicit reattach behavior stay covered.
- Verification: `tools/test_nightshift.sh`, `tools/validate_repo.sh`
- Good: the branch-mode logic sits in one small shell block, so the safety fix stayed narrow and easy to regression-test.
- Bad: the first implementation pass landed on the wrong local branch because the synced PR bug and the starting checkout did not match.
- Adjustment: when `docs/bugs.md` points to a specific PR, confirm the local checkout matches that PR head before editing and fast-forward it before the first validation run.
