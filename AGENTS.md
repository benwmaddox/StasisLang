# Repository Guidelines

## Project Structure & Module Organization
- `docs/spec.md` holds the Stasis language specification (semantics, memory rules, operators, examples).
- `docs/compilation.md` captures the LL(1) grammar used by the compiler/parser design.
- If you add code, prefer `src/` for the compiler/frontend, `tests/` for language fixtures, and `examples/` for minimal Stasis programs kept in sync with the spec.
- Keep supporting assets (diagrams, datasets) in `docs/assets/` or `examples/` and reference them from the specs.

## Build, Test, and Development Commands
- Implementation target: C# with LLVMSharp for lowering to LLVM IR/WASM.
- Prefer `dotnet` tooling (`dotnet build`, `dotnet test`) and keep a solution file at repo root when code lands.
- When you add a toolchain, surface canonical commands in a `README` or `Makefile` (e.g., `make fmt`, `make test`, `make build`). Keep commands fast and deterministic.
- Use `rg` for code/spec searches (faster than `grep`) and favor scriptable tasks over ad-hoc manual steps.

## Coding Style & Naming Conventions
- Preserve operator-method style for arithmetic/comparison (`.+()`, `.==()`, etc.); infix arithmetic/comparison is allowed with TypeScript-like precedence; assignment uses infix `=` (and compound forms).
- Keep files ASCII; avoid introducing non-ASCII unless the surrounding file already uses it.
- Name files and modules with short, lowercase, dash/underscore-separated tokens (`lexing.rs`, `parser.ts`, `memory_layout.md`).
- Document memory layout and lowering decisions alongside code; add brief comments only where behavior is non-obvious.

## Testing Guidelines
- Mirror the language’s `test` construct in fixtures; prefer deterministic, isolated cases. Example naming: ``test `enemy takes damage`()``.
- Place host-side tests under `tests/` with filenames matching the feature under test (`tests/parser_assignment.stasis`, `tests/lowering_offsets.rs`).
- Target high coverage of parsing, lowering, and static memory rules; include negative tests for invalid operator-method usage.
- Keep tests fast; if slow paths are unavoidable, mark them and document expected runtime.
- `stasis test` should discover every `.stasis` file in the working directory, print each file’s “Compiled in …”/`test-time` before test output, and avoid running IO-heavy suites on CI so the runs stay deterministic.

## Commit & Pull Request Guidelines
- Use short, imperative commit subjects; Conventional Commit prefixes (`feat:`, `fix:`, `docs:`) are encouraged for clarity.
- Reference the spec section you touched when relevant (e.g., “align lowering with docs/spec.md §6.3”).
- PRs should summarize intent, list user-visible changes, and call out spec updates or new commands; link issues and include reproduction or screenshots when UI/UX is involved.

## Architecture & Design Notes
- Core principles (per `docs/spec.md`): static global memory only; AoS syntax lowered to SoA storage; deterministic layouts; operator-method arithmetic/comparison (infix allowed) with infix assignment; compilation targets LLVM/WASM.
- Implementation stack: C# front-end with recursive-descent parser (LL(1) per `docs/compilation.md`), lowering through LLVMSharp bindings to produce LLVM IR and WASM.
- Keep the managed/native boundary explicit; encapsulate LLVMSharp interop in a thin layer to keep IR building testable.
- Avoid hidden allocation or implicit copying; make side effects and memory writes explicit in both code and docs.
- Favor Elm-inspired diagnostics that point to the offending span and offer actionable hints so developers fix parser or semantic issues quickly.
- Keep string/seed-heavy samples such as `samples/sudoku.stasis` aligned with the built-in I/O helpers (`print_string`, `read_char`, `read_int`) and the deterministic random seed behavior exposed by the runtime so the CLI stays reproducible.
