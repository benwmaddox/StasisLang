# Stasis Compiler Source

This directory is for compiler logic written in `.stasis`.

Planned primary entrypoint:
- `compiler/simple_pass_compiler.stasis`
- Shared compiler state/types:
- `compiler/compiler_state.stasis`
- `compiler/incremental_compiler.stasis` is retired; all compiler work lands in `compiler/simple_pass_compiler.stasis`.

Ownership boundary (Rewrite V1):
- `.stasis` owns lexer, parser, semantic checks, diagnostics, and incremental compile policy.
- Rust owns host runtime, file watching, message plumbing, Cranelift integration, and swap commit/runtime ABI.
- Rust `stasis_compiler` crate provides bootstrap/test harness and boundary integration only; compiler language logic stays in `.stasis`.

Smoke compile path:
- Local Windows: `bootstrap\\windows\\stasisc.bat run compiler\\simple_pass_compiler.stasis --emit-ir`
- Rust smoke test (Windows): `cargo test -p stasis_compiler bootstrap_compiles_incremental_compiler_source -- --nocapture`

Current entrypoint validation API in `compiler/simple_pass_compiler.stasis`:
- `compiler_validate_entry_main_i32()` returns:
- `0` when exactly one `main(): i32` exists
- `41` when no `main` is present
- `42` when `main` signature is invalid
- `43` when multiple `main` declarations are present
- `run_incremental_compiler_with_main_entry()` runs simple-pass pipeline + entry validation.

Active reachability-DCE roots (for lowering/pruning policy):
- `main`
- `tick` (when present)
- `on_code_swap` (when present)
- host-required exported entry symbols

Current single-source execution mode:
- `compiler_set_source(...)` updates the in-memory source buffer directly.
- `run_incremental_compiler()` and `run_incremental_compiler_with_main_entry()` are compatibility names over the simple-pass compiler and parse/validate single source when no file database entries are present.
- Fixtures can read compiler metrics directly from `Compiler.*` fields (no trivial getter wrappers).
- Layout metadata baseline is exposed via `Compiler.layout_hash`.
- Flattened global field layout metadata is exposed via `Compiler.layout_flat_field_*` + `Compiler.layout_memory_size_bytes` and is used for direct-offset CLIF global field load/store lowering against deterministic shared arena symbols (`sp_global_mem_layout_<layout_hash>`), with one owning `global` definition and `global_import` in other lowered functions.
- Function reachability flags are exposed via `Compiler.function_reachable_flags`; current simple-pass reachability roots are `main`, `tick`, and `on_code_swap`, and host analysis emits reachable functions only.
- Additional host-required reachability roots are injected via `compiler_add_required_reachability_root_hash(...)` into `Compiler.required_reachability_root_hashes` before compile.
- Struct/global layout reachability flags are exposed via `Compiler.layout_struct_reachable_flags` and `Compiler.layout_global_reachable_flags`; semantic validation prunes unreachable struct/global layout metadata before flattened-offset lowering.
- File-db incremental layout updates are exposed via `Compiler.incremental_layout_changed_files`.

Console extern naming (Rewrite V1):
- Source-level preferred name: `print_i32`.
- Bootstrap runtime currently exposes `print_int`; stdlib provides `print_i32(value: i32)` wrapper over `print_int(value)`.
- `print_string` is available directly in bootstrap runtime.

