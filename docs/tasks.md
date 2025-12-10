# Remaining Compiler Tasks (C# + LLVMSharp)

## Completed
- Phases 0-6: repo bootstrap, lexing, parsing, AST/symbols, semantics, layout planning, and LLVM builder with native loading + smoke tests.
- Phase 7: lowering & codegen — control-flow lowering (`if`/`for`/`foreach`), operator-method comparisons/unary/boolean coercion, layout-driven SoA globals and field access via `LayoutPlan`, diagnostics for bad operator arity/unsupported targets/invalid field access, with IR coverage tests.
- Phase 8: testing harness integration — IR emits `run_tests`; lowering options allow omitting tests/harness for production; `stasisc` CLI defaults to production and `--with-tests` enables harness emission.
- Phase 9: CLI & UX — `stasisc` CLI with `run`/`test` commands, LLVM IR emission, snapshot tests for CLI stdout/stderr/exit codes. SDL2 graphics support with Asteroids demo game.
- Phase 10: CI/CD hardening — GitHub Actions matrix (ubuntu/windows) with NuGet cache, format/build/test gates, sample IR artifacts, and platform-agnostic CLI snapshot tests.

## Phase 10.5: Constants & Structured Globals

### Constant Support
- Add `const` keyword for compile-time constant declarations (numeric, boolean, string literals).
- Lex/parse: `const NAME: type = value;` syntax at module scope; disallow in functions.
- Semantics: validate constants are initialized with literal values or expressions of other constants; disallow mutation attempts.
- Lowering: fold constant expressions at compile time; emit as LLVM `constant` globals or inline directly into IR.
- Error handling: diagnose uninitialized constants, non-literal initializers, and attempts to assign to constants.

### Structured Global State
- Enforce single global state struct pattern: all mutable state must be fields within a single root `global state: GameState;` declaration.
- Allow nested structs and arrays within the state struct (e.g., `state.ship.x`, `state.asteroids[i].active`).
- Update layout planner to handle nested struct access and emit proper GEP chains for deep field paths.
- Diagnostics: warn or error when declaring multiple top-level `global` variables (except for the single state struct and any constants).

### Migration & Samples
- Refactor `samples/asteroids.stasis`:
  - Convert SDL scancodes, screen dimensions, math constants, and limits to `const` declarations.
  - Consolidate ship/asteroid/bullet/game state into a single nested struct:
    ```
    struct Ship { x: f32; y: f32; vx: f32; vy: f32; angle: f32; }
    struct Asteroid { x: f32; y: f32; vx: f32; vy: f32; size: f32; active: bool; }
    struct Bullet { x: f32; y: f32; vx: f32; vy: f32; life: i32; }
    struct GameState {
      ship: Ship;
      asteroids: Asteroid[8];
      bullets: Bullet[5];
      num_asteroids: i32;
      fire_cooldown: i32;
      running: bool;
      last_time: i32;
      rng_state: i32;
    }
    global state: GameState;
    ```
  - Remove `init_constants()` function since constants are now declared inline.
  - Update all function bodies to reference `state.ship.x` instead of `ship_x`, etc.
- Update other samples (`basic.stasis`, `tests.stasis`, etc.) to follow the structured state pattern.
- Add tests: verify constant folding in IR, nested field access lowering, diagnostics for multiple globals/constant mutation.

### Follow-ups
- Document the structured globals pattern in `docs/spec.md` and `AGENTS.md` so contributors understand the design rationale (simpler layout, clearer ownership, easier serialization).
- Consider future extensions: allow multiple named global structs if use cases arise (e.g., `global input: InputState; global physics: PhysicsState;`), but start with single-struct restriction to validate the pattern.

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

