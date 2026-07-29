# Cranelift Backend Guardrails

The intended backend split is:

- `crates/stasis_compiler/src/backend/emit.rs` owns compile analysis, shared per-function lowering
  inside a complete reachable module, direct internal-call declarations, runtime import setup, and
  statement/expression lowering.
- `crates/stasis_compiler/src/backend/jit.rs` is a thin JIT wrapper for host symbol registration,
  complete module finalization, generation-owned executable memory, and immutable host-export map
  extraction. It does not publish runtime pointers.
- `crates/stasis_compiler/src/backend/aot.rs` is a thin AOT wrapper for extern binding policy, object emission, and link/bundle finalization.

JIT and AOT share direct Stasis-to-Stasis call lowering. Per-function JIT dispatch is obsolete; the
generation ABI, publication boundary, and lifetime rules are canonical in
`docs/jit_generation_contract.md`.

Review rule:

- If a normal language feature requires touching both `jit.rs` and `aot.rs`, stop and check whether the work belongs in `emit.rs` instead.
- `jit.rs` and `aot.rs` should not directly own statement parsing or expression/statement lowering.
- Neither backend may introduce an internal-call dispatch policy or reuse live machine code from a
  different generation.
- Regressions for this contract live in backend tests that fail if wrapper files start calling shared parse/emit hooks directly again.
