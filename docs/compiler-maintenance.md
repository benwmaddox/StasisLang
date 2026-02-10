# Stasis Compiler Maintenance Guide

This document onboards new contributors to the Stasis compiler (C#) and explains how to maintain and extend it safely.

Scope:
- The C# frontend and IR lowering in `Stasis.Compiler/`.
- How the CLI in `Stasis.Cli/` drives the compiler.
- Where tests live and how to add coverage.

Non-scope (but adjacent): native runtime (`runtime/`), VSCode extension (`vscode-stasis/`), and the LSP server (`Stasis.LanguageServer/`).

## Quick Links

- Language spec (semantics, memory rules): `docs/spec.md`
- Parsing/grammar notes (LL(1) declarations + Pratt expressions): `docs/compilation.md`
- Diagnostic tone and patterns: `docs/error-messages.md`
- Cranelift backend notes/plan: `docs/cranelift-backend-plan.md`
- Callable review checklist + parity matrix: `docs/callable-review-checklist.md`

## Repo Map (Compiler-Relevant)

- `Stasis.Compiler/`
  - `Lexer.cs` -> turns source text into `Token`s
  - `Parser.cs` -> produces syntax tree (`CompilationUnitSyntax`)
  - `Syntax/` -> AST node records (declarations, statements, expressions)
  - `Semantic/` -> symbol table, type checks, diagnostics
  - `Layout/` -> global memory layout plan (AoS syntax lowered to SoA storage)
  - `IR/` -> reachability and backends (LLVM + Cranelift)
  - `SourceImporter.cs` -> `import "..."` expansion, `.md` fenced extraction, platform fallback

- `Stasis.Cli/` (how most devs invoke compilation)
  - `Program.cs` -> `stasis build|run|test|release|format` pipelines, test caching, watch loops

- Tests
  - `Stasis.Compiler.Tests/` -> unit tests for lexer/parser/sema/layout/lowering
  - `Stasis.Cli.Tests/` -> CLI snapshot tests and template output verification

## Build and Test (Fast Path)

From repo root (Windows):

- Build: `build.bat`
- Test: `test.bat`

Dotnet-only (useful when iterating on compiler code):

- Build solution: `dotnet build Stasis.sln -c Release`
- Run compiler unit tests: `dotnet test Stasis.Compiler.Tests/Stasis.Compiler.Tests.csproj -c Release`
- Run CLI unit tests: `dotnet test Stasis.Cli.Tests/Stasis.Cli.Tests.csproj -c Release`

Recommended pre-push gate (callable + backend parity focused):

- `.\scripts\pre-push-gate.ps1`

Notes:
- Some tests require an LLVM toolchain (specifically `clang`) on PATH or in `.tools/llvm-*/bin`.
- LLVMSharp requires a matching native `libLLVM` to be loadable; see "LLVM native loading" below.

## How the CLI Drives Compilation

The CLI (`Stasis.Cli/Program.cs`) contains the highest-level orchestration and is the best place to learn the end-to-end pipeline.

The core "prepare" path is `PrepareForLower(...)`, which does (conceptually):

1. Read the entry file with shared-read semantics (allows watch/edit while compiling).
2. Expand imports (`SourceImporter.ExpandImports(...)`).
3. Lex + parse (`Parser.Parse(...)`).
4. Collect and validate `@link("...");` directives.
5. Run semantic analysis (`SemanticAnalyzer.Analyze(...)`).
6. Compute a layout plan (`LayoutPlanner.Plan()`).
7. Generate IR (LLVM or Cranelift) via `CodeGeneratorFactory.Create(...).Generate(...)`.
8. Run/execute artifacts depending on backend/mode.

The CLI also implements:
- `--emit-ir` to print IR.
- `--backend llvm|cranelift` selection.
- `--watch` and "tick hosted" dev loops.
- A test cache under `.stasis_cache/test` keyed by source + flags to avoid recompiling unchanged tests.

## Source Loading and `import`

The compiler supports Stasis code in two ways:

- Plain `.stasis` files.
- Markdown `.md` files containing fenced code blocks:
  - `SourceImporter` detects `.md` and uses `MarkdownStasisExtractor.Extract(...)` to keep line numbers stable.
  - Only fences marked as ` ```stasis ` are extracted.

Import behavior (`SourceImporter.ExpandImports(...)`):

- Imports are line-based and look like:
  - `import "relative/path/to/file.stasis";`
- The importer expands imports by reading the referenced file and inserting its expanded text.
- Imports are de-duplicated by full path (case-insensitive on Windows via `visited`).
- Platform fallback exists for `.stasis` imports:
  - If `foo.stasis` does not exist, Stasis will try `foo.<platform>.stasis`.
  - Platform is determined by `STASIS_PLATFORM` (override) or OS detection: `windows`, `linux`, `macos`.

Stdlib restriction:
- Any file under `src/stdlib/` is treated as "stdlib" and currently may not declare `global`.
  - This is enforced by lexing the imported text and scanning for `TokenKind.GlobalKeyword`.

Maintenance tip:
- If you change import syntax, update both `SourceImporter.TryParseImportLine` and diagnostics mapping.

## Lexer (Tokenization)

Key file: `Stasis.Compiler/Lexer.cs`

Responsibilities:
- Convert source text to a token stream with `SourceSpan` offsets.
- Recognize keywords, identifiers, numeric literals, string/backtick literals.
- Recognize operators and punctuation.
- Skip whitespace, line comments (`// ...`) and block comments (`/* ... */`).

Diagnostics:
- The lexer stops after `DiagnosticPolicy.MaxErrors` (currently 5) and forces an EOF token.

When adding a token:
1. Add to `TokenKind` (`Stasis.Compiler/TokenKind.cs`).
2. Teach the lexer to emit it.
3. Teach the parser to consume it (or report a clear diagnostic).
4. Add lexer/parser tests.

## Parser (Syntax Tree)

Key file: `Stasis.Compiler/Parser.cs`

High-level structure:
- Declarations are parsed with a recursive-descent approach (LL(1)-friendly at the top level).
- Expressions use a Pratt parser for precedence/associativity.

Top-level items (examples):
- `struct`, `enum`, `global`, `const`, `function`, `test`, `@link("...");`

Expression model:
- Operator-method calls remain supported:
  - `a.+(b)`, `a.<(b)`, `x.==(y)`
- Infix arithmetic and comparisons are supported (TypeScript-like precedence):
  - `a + b * c`, `if (x <= y) { ... }`
- Assignment is infix and right-associative:
  - `x = 5;`
  - `x += 2;` (compound assignments parse as assignment expressions)

Legacy assignment:
- The parser detects `.=` usage and emits a targeted diagnostic telling users to use infix `=`.

Postfix forms:
- Member access: `receiver.member`
- Array access: `receiver[index]`
- Call: `callee(args...)`

When adding a syntactic feature:
- Add a new `...Syntax` record under `Stasis.Compiler/Syntax/`.
- Update parser logic and precedence tables.
- Add parser tests first (happy path + error cases).

## Semantic Analysis (Symbols, Types, Rules)

Key file: `Stasis.Compiler/Semantic/SemanticAnalyzer.cs`

Responsibilities:
- Populate a symbol table (`Dictionary<string, Symbol>`).
- Validate declarations and bodies.
- Enforce Stasis constraints (static memory rules, assignability rules, call correctness, etc.).
- Emit Elm-inspired diagnostics with actionable hints.

Built-ins:
- Built-in types and functions are predeclared before user code.
- Some built-ins are "legacy" and are expected to evolve; keep the compiler and runtime in sync when renaming.

Important semantic model detail:
- User-defined structs are represented as *references/indices* in most places.
  - In LLVM lowering, `NamedTypeSymbol` maps to `i32`.
  - That `i32` typically represents an index into SoA storage for a particular global struct array.

Diagnostic policy:
- The analyzer stops after `DiagnosticPolicy.MaxErrors` errors to avoid floods.
- Warnings (for example, unused struct fields) are emitted after successful analysis.

When adding a semantic rule:
- Prefer adding a precise diagnostic at the point of failure with a hint.
- Add a negative test in `Stasis.Compiler.Tests` that asserts the message and span behavior.

## Layout Planning (AoS Syntax -> SoA Storage)

Key file: `Stasis.Compiler/Layout/LayoutPlanner.cs`

Purpose:
- Compute deterministic offsets for all globals, and transform struct arrays into a SoA storage plan.

What it produces:
- `LayoutPlan` with a list of `GlobalLayout` entries.
- Each `GlobalLayout` contains `FieldLayout` items with:
  - `Name` (flattened/derived)
  - `Offset` (byte offset in global memory)
  - `Size` (byte size)
  - `FieldType` (Bool/U8/U16/U32/I32/F32/F64/String/Unknown)
  - `ArrayCount` (for arrays)

Rules to know:
- `global players: Player[10];` becomes separate SoA fields like:
  - `Player__hp`, `Player__score`, etc.
- `global state: GameState;` flattens nested structs into `state__field__nested` naming.
- Alignment is applied per field element size (or a conservative default).
- Fixed-size string buffers have header sizes:
  - `ascii[N]` -> 4-byte header
  - `utf8[N]` / `string[N]` -> 8-byte header

Common maintenance pitfalls:
- Layout planner sizes are in *bytes*, but string values in lowering are often backed by `i8` arrays with header + payload.
- If you adjust header sizes or string layouts, update BOTH `LayoutPlanner` and lowering code that emits globals.

## Reachability (Dead Function Pruning)

Key file: `Stasis.Compiler/IR/Reachability.cs`

Used by lowering to avoid emitting bodies/signatures for unreachable functions.

Entry points:
- Non-test builds:
  - `main` is an entry point if present.
  - `tick` is also treated as an entry point (tick-hosted workflow).
  - `export function ...` declarations are treated as entry points.
- Test builds:
  - All `test ... { ... }` declarations are entry points.

Fallback:
- If no entrypoints are discovered and `AllowReachabilityFallback` is enabled, the compiler treats all functions as reachable.

Maintenance tip:
- If you add new implicit entrypoints, update reachability and add tests.

## IR Lowering (LLVM)

Key files:
- `Stasis.Compiler/IR/ModuleLowerer.cs` (orchestrates lowering)
- `Stasis.Compiler/IR/Llvm/LlvmModuleBuilder.cs` (module/context helpers)
- `Stasis.Compiler/IR/Llvm/LlvmTypeMapper.cs` (type mapping)

How lowering works today:
- `ModuleLowerer.LowerToIr(...)`:
  - Builds an LLVM module.
  - Computes reachable functions.
  - Emits globals (including SoA expansion for struct arrays).
  - Emits constants (including special handling for string literals in `const`).
  - Emits function signatures.
  - Lowers function bodies via an internal `FunctionLowerer`.
  - Optionally emits a test harness when `IncludeTests` and `EmitTestHarness` are enabled.

Important details:
- The textual IR returned by LLVMSharp may include `getelementptr inbounds nuw`.
  - The lowerer strips `nuw` on GEP because clang's IR parser rejects it in some configurations.
- Host ABI globals are exported (external linkage) via `LlvmModuleBuilder.ShouldExportGlobal(...)`.
  - If you add new host-facing globals, update that list.

Built-ins and external calls:
- `FunctionLowerer` maintains a `_builtIns` allowlist.
- Most built-ins map to runtime/host functions with stable names.
  - If you change a builtin name or signature, treat it as a cross-repo change (compiler + runtime + samples).

## IR Lowering (Cranelift)

Key file: `Stasis.Compiler/IR/Cranelift/CraneliftCodeGenerator.cs`

Current status:
- Generates CLIF text and declares only the external functions it needs.
- Used primarily as a fast debug backend by the CLI when available.

Maintenance note:
- The Cranelift backend intentionally mirrors the frontend contracts (syntax, semantics, layout) even if its runtime execution path differs.
- When you add a language feature, ensure both backends either support it or emit a clear diagnostic stating backend limitations.

## LLVM Native Loading

Key file: `Stasis.Compiler/LlvmNativeLoader.cs`

LLVMSharp uses a native `libLLVM` shared library. The loader:
- Tries `LLVM_NATIVE_PATH` first (and prepends it to the relevant library path env var).
- Tries `AppContext.BaseDirectory`.
- Tries NuGet package native assets (pinned version string in the loader).
- Falls back to the OS loader (`NativeLibrary.TryLoad`).

If you see `DllNotFoundException` during tests:
- Ensure packages restored.
- Or set `LLVM_NATIVE_PATH` to a directory containing `libLLVM`.

## Tests: Where to Add Coverage

`Stasis.Compiler.Tests/` is the primary safety net.

Suggested pattern when changing behavior:
- Add a parser test for the new syntax (or error recovery).
- Add a semantic test for type checking / rule enforcement.
- Add a layout test if the change affects offsets, naming, or sizes.
- Add a lowering test that asserts the produced IR contains expected patterns.
- If execution behavior matters and LLVM toolchain is available, add/update an execution test.

Relevant test files:
- `LexerTests.cs`, `ParserTests.cs`
- `SemanticTests.cs`
- `LayoutTests.cs`
- `LoweringTests.cs`
- `ExecutionTests.cs` (requires `clang`)

## Making a Change: Typical Workflows

### Add or change a language feature

Checklist:
1. Update the spec: `docs/spec.md` (semantics + memory rules).
2. Update grammar notes if needed: `docs/compilation.md`.
3. Implement in compiler passes, usually in this order:
   - Lexer/TokenKind (if new tokens)
   - Parser + Syntax nodes
   - SemanticAnalyzer rules
   - LayoutPlanner (if storage/layout affected)
   - Lowering (LLVM and/or Cranelift)
4. Add tests at each layer.
5. Update samples/examples if user-facing.

### Add or change a builtin

Builtins live in multiple layers:
- Semantic: predeclare the symbol in `SemanticAnalyzer`.
- Lowering: ensure the backend can lower calls to it.
  - LLVM: check `FunctionLowerer` builtin handling.
  - Cranelift: declare externs in `CraneliftCodeGenerator` if needed.
- Runtime/host: implement the function (if it is not purely compiler-internal).

Treat builtin changes as ABI changes:
- Prefer additive changes.
- If you must rename/remove, update samples and write migration diagnostics.

### Change memory layout rules

This is high-impact.

Checklist:
- Update `docs/spec.md` first (layout section).
- Update `LayoutPlanner`.
- Update lowering logic that defines globals and computes indices/offsets.
- Add tests that assert exact offsets/sizes.

## Debugging Tips

- Print IR:
  - `stasis run <file> --emit-ir` (or redirect to `out.ll`)
- Narrow down failures:
  - Start with `dotnet test Stasis.Compiler.Tests -c Release`.
  - Use `--filter` to run a single failing test.
- Backend sanity:
  - If an issue is backend-specific, compare `--backend llvm` vs `--backend cranelift`.
- Import issues:
  - Validate import resolution with `STASIS_PLATFORM` override.
  - Remember `.md` fenced extraction only includes ` ```stasis ` blocks.

## PR Checklist (Compiler Changes)

- [ ] Spec updated if semantics changed (`docs/spec.md`).
- [ ] Parser/grammar notes updated if syntax changed (`docs/compilation.md`).
- [ ] Tests added/updated in `Stasis.Compiler.Tests/`.
- [ ] Diagnostics are actionable and point at the correct span.
- [ ] LLVM backend still compiles and IR tests pass.
- [ ] If touching host ABI globals/builtins, runtime impact reviewed.

---

If you are new: start by reading `docs/spec.md` sections on the memory model and operator-method rules, then follow the CLI prepare path in `Stasis.Cli/Program.cs` to see how the pieces connect.
