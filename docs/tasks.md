# Remaining Compiler Tasks (C# + LLVMSharp)

## Completed
- Phases 0-6: repo bootstrap, lexing, parsing, AST/symbols, semantics, layout planning, and LLVM builder with native loading + smoke tests.
- Phase 7 (partial): control-flow lowering for `if`/`for`/`foreach`; operator-method comparisons/unary/boolean coercion; layout-aware SoA globals and field access driven by `LayoutPlan`; lowering diagnostics for bad operator arity/unsupported targets — all covered by IR tests.

## Phase 7: Lowering & Codegen (in progress)
- Memory-aware lowering: extend use of `LayoutPlan` offsets for any remaining address calculations (monolithic buffer/offset-based addressing if needed).
  - Verify: struct array field load/store points at correct field array; add golden IR assertions if addressing model expands.

## Phase 8: Testing Harness Integration
- Discover `test` declarations, emit host runner to call compiled test functions, exclude from production roots.
- Verify: `dotnet test` executes compiled Stasis tests; production build omits test roots.

## Phase 9: CLI & UX
- Build `stasisc` CLI to lex/parse/typecheck/lower; flags for output (LLVM IR/WASM), debug dumps, layout inspection; deterministic defaults.
- Verify: snapshot tests for CLI stdout/stderr and exit codes on fixtures.

## Phase 10: CI/CD Hardening
- GitHub Actions: run format/build/test across OSes; cache NuGet/LLVM; publish IR/WASM artifacts for samples.
- Verify: green CI and uploaded artifacts; badges documented in README.
