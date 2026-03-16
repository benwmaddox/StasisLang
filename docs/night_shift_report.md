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
