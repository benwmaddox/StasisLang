# Remaining Compiler Tasks (C# + LLVMSharp)

## Completed
- Phases 0-6: repo bootstrap, lexing, parsing, AST/symbols, semantics, layout planning, and LLVM builder with native loading + smoke tests.
- Phase 7: lowering & codegen — control-flow lowering (`if`/`for`/`foreach`), operator-method comparisons/unary/boolean coercion, layout-driven SoA globals and field access via `LayoutPlan`, diagnostics for bad operator arity/unsupported targets/invalid field access, with IR coverage tests.

## Phase 8: Testing Harness Integration
- Discover `test` declarations, emit host runner to call compiled test functions, exclude from production roots.
- Verify: `dotnet test` executes compiled Stasis tests; production build omits test roots.

## Phase 9: CLI & UX
- Build `stasisc` CLI to lex/parse/typecheck/lower; flags for output (LLVM IR/WASM), debug dumps, layout inspection; deterministic defaults.
- Verify: snapshot tests for CLI stdout/stderr and exit codes on fixtures.

## Phase 10: CI/CD Hardening
- GitHub Actions: run format/build/test across OSes; cache NuGet/LLVM; publish IR/WASM artifacts for samples.
- Verify: green CI and uploaded artifacts; badges documented in README.

