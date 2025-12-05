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

