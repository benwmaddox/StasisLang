# Stasis Compiler Source

This directory is for compiler logic written in `.stasis`.

Planned primary entrypoint:
- `compiler/incremental_compiler.stasis`

Ownership boundary (Rewrite V1):
- `.stasis` owns lexer, parser, semantic checks, diagnostics, and incremental compile policy.
- Rust owns host runtime, file watching, message plumbing, Cranelift integration, and swap commit/runtime ABI.
- Rust `stasis_compiler` crate provides bootstrap/test harness and boundary integration only; compiler language logic stays in `.stasis`.

Smoke compile path:
- Local Windows: `bootstrap\\windows\\stasisc.bat run compiler\\incremental_compiler.stasis --emit-ir`
- Rust smoke test (Windows): `cargo test -p stasis_compiler bootstrap_compiles_incremental_compiler_source -- --nocapture`

Current entrypoint validation API in `compiler/incremental_compiler.stasis`:
- `compiler_validate_entry_main_i32()` returns:
- `0` when exactly one `main(): i32` exists
- `41` when no `main` is present
- `42` when `main` signature is invalid
- `43` when multiple `main` declarations are present
- `run_incremental_compiler_with_main_entry()` runs pipeline + entry validation.
