# Repository Guidelines

## Project Structure & Module Organization
- `docs/spec.md` is the canonical language spec for Rewrite V1.
- `docs/live-compilation-prd.md` is the canonical product/architecture requirements document.
- `docs/rewrite_v1_checklist.md` is the execution plan; keep slice ordering and temporary migration details there.
- `compiler/` holds compiler source written in Stasis.
- Planned primary orchestration file is `compiler/incremental_compiler.stasis`.
- `crates/stasis_compiler` hosts Rust compiler substrate/bindings called by Stasis orchestration.
- `Stasis.Compiler/` and `Stasis.Cli/` are bootstrap compiler sources imported from `main` for branch compatibility.
- Treat `Stasis.Compiler/` + `Stasis.Cli/` as bootstrap-only (not the long-term Rewrite V1 self-hosted compiler target).
- `crates/stasis_jit` hosts Cranelift integration for JIT (dev) and AOT (prod), function pointer table integration, and code generation memory management.
- `crates/stasis_runner` hosts tick loop, swap sequencing, and commit orchestration.
- `apps/stasis` is the single in-process graphical runner app.
- `src/stdlib/` contains Stasis standard library modules.
- `samples/brickout_revenge/` is the primary end-to-end sample target.
- `tests/rust/` contains host-side Rust tests. Add deterministic `.stasis` fixtures under `tests/` when needed.

## Build, Test, and Development Commands
- Primary implementation toolchain is Rust/Cargo.
- Use:
- `cargo build`
- `cargo test`
- `cargo run -p stasis -- --ticks 300 --watch-dir samples/brickout_revenge`
- Use `rg` for search (`rg pattern path`, `rg --files`).
- Keep commands deterministic and scriptable.
- Legacy bootstrap tooling remains available for compiler smoke/reference:
- `bootstrap\windows\stasisc.bat run path\to\file.stasis --emit-ir`
- `bootstrap\windows\stasisc.bat test --all`

## Coding Style & Naming Conventions
- Keep files ASCII unless a file already uses non-ASCII and there is a clear reason.
- Prefer short, lowercase, snake_case file/module names.
- Keep comments brief and only for non-obvious behavior.
- Follow spec syntax and semantics:
- Arithmetic/comparison are infix only (`+ - * / %`, `< <= > >= == !=`).
- Assignment is infix (`=`, `+=`, `-=`, `*=`, `/=`, `%=`).
- Method-style arithmetic/comparison forms are removed.
- Receiver-form call style is preferred (`enemy.damage(5)`), function-form remains supported (`damage(enemy, 5)`).
- Conversion helpers:
- `from_*` are mutating target operations (statement-style side effects).
- `to_*` are pure conversions (expression-safe).

## Testing Guidelines
- Ship work in feature slices from `docs/rewrite_v1_checklist.md` and include tests in the same PR.
- Prefer deterministic, isolated tests with explicit expected output/state.
- If test can reasonably be written in stasis for stasis code, do so. It can be in a .test.stasis file next to the .stasis file.
- Cover parser/semantics/lowering/JIT boundaries and hot-swap safety behavior.
- For incremental compilation:
- Validate file-level invalidation correctness.
- Validate per-function gating behavior.
- Validate unchanged-function cache reuse.
- For hot swap:
- Validate all-or-nothing commit.
- Validate rejection paths preserve old code/data.
- Validate `on_code_swap` failure abort behavior.

## Commit & Pull Request Guidelines
- Use short imperative subjects; Conventional Commits are preferred (`feat:`, `fix:`, `docs:`, `test:`).
- Reference affected spec/PRD/checklist sections when relevant.
- Keep PRs scoped to one slice group where possible (exact grouping/sequencing lives in `docs/rewrite_v1_checklist.md`).
- Each PR should include:
- behavioral summary
- tests added/updated
- docs updates
- explicit removal of obsolete paths introduced during the slice

## Architecture & Design Notes (Rewrite V1)
- Single OS process runtime with in-process compiler and Cranelift JIT for development.
- Production build target uses Cranelift AOT output.
- File-level correctness is primary; semantic analysis runs for full changed file.
- Per-function semantic hashes gate backend work only.
- Hot swap is a two-phase model:
- background compilation to produce pending patch
- commit between ticks on main thread
- Dispatch boundary is stable indirection: `FnId -> code_ptr`.
- Swap safety rules:
- reject on layout/signature incompatibility
- reject on `on_code_swap` failure
- no partial commits
- preserve deterministic tick-based semantics; avoid `dt`-driven gameplay progression in Stasis logic.

## Language Ownership Rules
- Rust owns host/runtime boundary, platform integration, Cranelift embedding, pointer-table commit mechanics, and process/watch plumbing.
- `.stasis` owns compiler orchestration policies and language-level compile logic.
- Use C only when unavoidable for platform-level bindings.
