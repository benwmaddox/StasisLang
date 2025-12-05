# Compiler Implementation Plan (C# + LLVMSharp)

## Phase 0: Repository Bootstrap
- Tasks: create `Stasis.sln` and `Stasis.Compiler` project; add `LLVMSharp` and testing packages; configure `dotnet format` and EditorConfig; add GitHub Actions for `dotnet format`, `dotnet build`, `dotnet test`.
- Verification: `dotnet build`; `dotnet format --verify-no-changes`; `dotnet test` (empty scaffold initially).

## Phase 1: Lexing (per docs/spec.md §2 and docs/compilation.md §9)
- Tasks: implement tokenizer with exact keywords, identifiers, numeric/string/backtick literals, operator-method tokens (`.+`, `.-`, `.*`, `./`, `.%`, `.<`, `.>`, `.==`, `.=`), parentheses/brackets/braces, and comments if allowed; produce source spans for diagnostics.
- Verification: unit tests covering all token categories, backtick test names, and error cases (unterminated string/backtick).

## Phase 2: Parsing (LL(1) per docs/compilation.md)
- Tasks: build recursive-descent parser for the grammar; produce AST nodes for structs/enums/globals/functions/tests; parse postfix operator-method chains and assignment `.=( )`; ensure left-factored rules to keep LL(1); emit friendly diagnostics with span info.
- Verification: golden parse trees for valid samples; negative tests for ambiguity (e.g., missing `)`/`;`, invalid operator token).

## Phase 3: AST + Symbol Tables
- Tasks: define AST types for modules, declarations, statements, expressions; implement symbol table with scopes for types, globals, and functions; enforce uniqueness; collect signatures in a first pass (per spec §13 Phase 1).
- Verification: tests for duplicate symbol detection, unknown references, and multi-file module indexing (if added).

## Phase 4: Semantic Analysis & Type Checking
- Tasks: enforce static memory rules (structs/arrays only in global memory, locals restricted to primitives/references); type-check operator-methods by receiver type; validate assignment receivers; check return types; flag disallowed dynamic allocation.
- Verification: unit tests for valid/invalid programs, especially operator-method typing and assignment l-values; ensure diagnostics cite spans.

## Phase 5: Memory Layout (AoS syntax → SoA storage)
- Tasks: compute deterministic SoA layouts for structs and globals; generate per-field arrays with offsets; expose `memoryOffset()` constants; decide alignment/padding rules and document in `docs/spec.md` if refined.
- Verification: golden layout tests (input struct → emitted field arrays and offsets); compare offsets to spec expectations.

## Phase 6: IR Construction Layer (LLVMSharp)
- Tasks: wrap LLVMSharp in a thin builder to isolate interop; map Stasis types to LLVM types; implement helpers for globals, functions, blocks, and operator-method intrinsics; support both LLVM IR and WASM-compatible targets.
- Verification: unit tests on the builder (creates expected IR snippets); round-trip `llc`/`lli` on tiny programs (e.g., arithmetic, assignment) to confirm correctness.

## Phase 7: Lowering & Codegen
- Tasks: lower typed AST to LLVM IR using the builder; implement operator-method lowering tables (spec §6); generate control flow for if/for/foreach; emit explicit stores/loads for SoA fields and arrays; ensure no hidden allocations.
- Verification: compile sample programs and run with `lli` or wasm runtime; assertions on emitted IR text for key constructs; negative tests for disallowed patterns.

## Phase 8: Testing Harness Integration
- Tasks: implement discovery of `test` declarations; generate host-side runners (C#) that invoke compiled test functions; mark tests as non-rooted for production builds (tree shaking).
- Verification: `dotnet test` running compiled Stasis tests; ensure production build excludes test roots.

## Phase 9: CLI & UX
- Tasks: build a `stasisc` CLI (`dotnet`) to lex/parse/typecheck/emit IR/WASM; add flags for output type, debug dumps, and layout inspection; default to deterministic builds.
- Verification: manual and automated CLI invocations on fixtures; snapshot tests for CLI stdout/stderr and exit codes.

## Phase 10: CI/CD Hardening
- Tasks: ensure GitHub Actions run format/build/test across supported OSes; add caching for NuGet and LLVM toolchain; publish IR/WASM artifacts for sample programs as checks.
- Verification: green CI with artifact uploads; documented badges in README.
