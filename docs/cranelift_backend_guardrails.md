# Cranelift Backend Guardrails

The intended backend split is:

- `crates/stasis_compiler/src/backend/emit.rs` owns compile analysis, shared per-function compilation, runtime import setup, and statement/expression lowering.
- `crates/stasis_compiler/src/backend/jit.rs` is a thin JIT wrapper for host symbol registration, module finalize/commit, and runtime pointer publication.
- `crates/stasis_compiler/src/backend/aot.rs` is a thin AOT wrapper for extern binding policy, object emission, and link/bundle finalization.

Review rule:

- If a normal language feature requires touching both `jit.rs` and `aot.rs`, stop and check whether the work belongs in `emit.rs` instead.
- `jit.rs` and `aot.rs` should not directly own statement parsing or expression/statement lowering.
- Regressions for this contract live in backend tests that fail if wrapper files start calling shared parse/emit hooks directly again.
