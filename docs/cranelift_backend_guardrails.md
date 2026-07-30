# Cranelift Backend Guardrails

The intended backend split is:

- `crates/stasis_compiler/src/backend/emit.rs` owns compile analysis, shared per-function lowering,
  direct internal-call declarations, runtime import setup, and
  statement/expression lowering.
- `crates/stasis_compiler/src/backend/jit.rs` owns JIT host-symbol registration, selective
  PatchPlan module finalization, retained-callee address binding, retained code arenas, and staged
  host-entry target extraction. It does not publish runtime pointers.
- `crates/stasis_compiler/src/backend/aot.rs` is a thin AOT wrapper for extern binding policy, object emission, and link/bundle finalization.

JIT and AOT share direct Stasis-to-Stasis call lowering. Internal hash/mutex dispatch is obsolete;
the selective patch ABI, host-entry publication boundary, and retained-code rules are canonical in
`docs/jit_generation_contract.md`.

Review rule:

- If a normal language feature requires touching both `jit.rs` and `aot.rs`, stop and check whether the work belongs in `emit.rs` instead.
- `jit.rs` and `aot.rs` should not directly own statement parsing or expression/statement lowering.
- Neither backend may introduce internal stable trampolines or hash/mutex dispatch. JIT must reuse
  unchanged accepted bodies and may bind them directly from a newer patch; AOT never consumes live
  JIT addresses.
- Regressions for this contract live in backend tests that fail if wrapper files start calling shared parse/emit hooks directly again.
