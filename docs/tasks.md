# Remaining Compiler Tasks (C# + LLVMSharp)

## Completed
- Phases 0-6: repo bootstrap, lexing, parsing, AST/symbols, semantics, layout planning, and LLVM builder with native loading + smoke tests.
- Phase 7: lowering & codegen — control-flow lowering (`if`/`for`/`foreach`), operator-method comparisons/unary/boolean coercion, layout-driven SoA globals and field access via `LayoutPlan`, diagnostics for bad operator arity/unsupported targets/invalid field access, with IR coverage tests.
- Phase 8: testing harness integration — IR emits `run_tests`; lowering options allow omitting tests/harness for production; `stasisc` CLI defaults to production and `--with-tests` enables harness emission.
- Phase 10: CI/CD hardening — GitHub Actions matrix (ubuntu/windows) with NuGet cache, format/build/test gates, and sample IR artifacts for `samples/basic.stasis` and `samples/tests.stasis`.

## Phase 8: Testing Harness Integration
- Verify: `dotnet test` executes compiled Stasis tests; production build omits test roots.

## Phase 9: CLI & UX
- Build `stasisc` CLI to lex/parse/typecheck/lower; flags for output (LLVM IR/WASM), debug dumps, layout inspection; deterministic defaults.
- Verify: snapshot tests for CLI stdout/stderr and exit codes on fixtures (basic/test samples covered).
- CLI runs `run`/`test` end-to-end via `lli` (preferred) or `clang` fallback; `stasis.{bat,sh}` are thin shims that delegate to the CLI.

## Phase 11: LLVM Execution Path
- Implement end-to-end IR emission runnable via `lli` for sample programs (function calls, arithmetic, control flow, globals).
- Add minimal runtime stubs if needed; ensure production defaults link/execute without tests.
- Verify: sample Stasis programs run under `lli` with expected stdout/return codes; golden IR snapshots for samples.
- Add integration test that compiles Stasis `test` declarations, emits `run_tests`, and executes via `lli` to verify harness exit code.

## Phase 12: Expression & Locals Refresh
- Switch assignment syntax to infix `=` and move expressions to a Pratt parser (assignment right-associative; logical ops stay infix).
- Keep arithmetic/comparison as operator-method calls; emit diagnostics for legacy `.=` usage. Allow infix arithmetic/comparison with TypeScript precedence and compound assignment, but only one assignment per expression.
- Allow stack locals for primitive scalars and struct references (indices) while keeping struct storage global-only.
- Refresh docs/samples/tests to the new syntax; ensure lowering/semantics match updated rules.

### Follow-ups
- Add clarity on the new assignment rules (single assignment per expression, compound variants) to `docs/spec.md` and `AGENTS.md` so contributors understand the guarded Pratt parser expectations.
- Ensure `samples/sudoku.stasis` and other fixtures follow the new syntax (no `>=`/ternary tokens) and include explicit tests that exercise the Pratt parser changes.

## CLI Quality-of-Life
- `stasis test` with no path (or `--all`) should discover and run all `.stasis` files under the working directory.
- `stasis release` builds optimized binaries via clang (defaults `-O3` + LTO); `build` remains unoptimized unless `--opt-level`/`--lto` are provided.

### Follow-ups
- Document the discovery behavior of `stasis test` and the per-file compile/test timing (`test-time` vs `total-time`) in the README or a CLI reference so users understand what’s running.

## String literals and printing
- Add first-class string literal support to the frontend (lex/parse), carry type info through symbols/sema, and lower string constants to immutable global buffers (null-terminated `i8` arrays).
- Expose a `print(string)`/`puts`-style intrinsic in lowering: map a Stasis `string` to `i8*` in LLVM, emit globals for literals, and generate `printf("%s", ptr)`/`puts(ptr)` calls.
- Update samples and tests to use string printing instead of manual `print_char` sequences; add negative tests for unterminated/invalid escapes.
- Follow-ups: convert `samples/sudoku.stasis` prompts/labels/messages to string literals once lowering + `print(string)`/`puts` land; add Stasis-side tests that string-based prompts render and CLI tests that they appear.
- Ensure spec/AGENTS mention the new built-ins (`print_string`, `print`, `read_line`, etc.) and capture the expectation that string input/output is now first-class (including a plan for Elm-style diagnostics when strings fail to parse).

## Phase 13: Diagnostics & Samples
- Improve diagnostic clarity (Elm-style) by describing expected message structure, pointer to source spans, and user-friendly hints in `docs/spec.md`; link this guidance from AGENTS so formatter/resilience work can reference it.
- Update `samples/sudoku.stasis` to:
  - Use string-based prompts instead of numeric char sequences.
  - Support a random seed (prompt input) to generate reproducible puzzles.
  - Include Stasis-level tests for the seed parser and random puzzle generator to prevent regressions.
- Record that `stasis test` must skip blocking IO-heavy tests on CI while still allowing the CLI to run interactive suites locally; add this to the tasks so future CI work can ramp in the right direction.

## Sudoku CLI Mini-Game (Stasis)
- Design: CLI-driven Sudoku (fixed 9x9 puzzle) fully authored in Stasis; interactive loop via CLI `run` command.
- Language/runtime gaps to close:
  - Add string literal lowering to immutable `i8*` buffers and expose `puts/printf` for user-facing text.
  - Add basic stdin support (e.g., host shim for numeric input or readline) and a minimal formatting helper for board rendering.
  - CLI flag `stasis play-sudoku` that wires console I/O to the Stasis program (bridge host I/O to LLVM intrinsics).
- Game logic:
  - Stasis sample defines board storage (global arrays), helpers (indexing, validity checks), backtracking solver, and commands to place numbers.
  - Main loop: render board, prompt for row/col/value, validate move, allow quit/reset; exit code 0 on solved, nonzero on abort/error.
  - Tests: host-side CLI test drives scripted input; Stasis-side tests validate solver correctness on the seed puzzle.
- Deliverables:
  - `samples/sudoku.stasis` playable program + solver.
  - CLI documentation in README for Sudoku mode and controls.
  - Integration test ensuring Sudoku sample builds/solves via CLI.
  - Follow-ups: replace char-by-char prints with strings in `samples/sudoku.stasis` and add regression tests once string support is implemented.

