# Self-Hosted Stasis Compiler (Branch: `self-host`)

Goal: replace the current C# frontend (`Stasis.Compiler`) with a Stasis-written compiler that can run as a standalone CLI.

This is a living design + progress document for the `self-host` branch.

## Non-Negotiables

- Self-hosted compiler is written in Stasis and is directly runnable as a CLI (no C# wrapper required at runtime).
- Supports up to 300 source files per build (imports), up to 50 KiB per file.
- Provides CLI modes: `watch`, `run`, `release` (and `build`/`test` as needed for parity).
- No import expansion/flattening: compilation operates on a multi-file source graph.
- Emits either:
  - Cranelift CLIF text (compiled via existing `tools/cranelift-aot` + `clang` link), or
  - LLVM IR text (executed via `lli` when possible, else `clang` link).
- Remains deterministic: static memory only; explicit I/O; no hidden allocation.
- Uses a single global state struct (`sh`) to own all compiler/static allocations.
- Prefer iteration over recursion where practical (explicit stacks for parse/sema/codegen).

## Names (Stage0 vs Stage1)

- Stage0 (today): existing C# toolchain invoked via `stasis.bat` / `stasis.sh` (requires `dotnet`).
- Stage1 (this branch): `stasis`, built from `src/stasis/main.stasis` as a native executable (no `dotnet` at runtime).
- Stage0 remains bootstrap-only until Stage1 can build itself end-to-end.

## CLI Contract (Target)

Initial deliverables should keep the CLI small but stable. The intent is to match `stasis` behavior over time.

Commands (final shape; implemented incrementally):

- `stasis check <entry.stasis> [--backend cranelift|llvm]`
  - Parses + semantics only (fast feedback, no codegen), later used by `watch`.
- `stasis build <entry.stasis> -o <out.exe|out.wasm> [--backend ...] [--release]`
- `stasis run <entry.stasis> [--backend ...] [--] <args...>`
  - Build to a temp output then exec it; preserves exit code.
- `stasis test [path-or-glob] [--backend ...]`
  - Discovers `.stasis` files; runs in-file `test` blocks; prints per-file timings.
- `stasis watch (check|build|run|test) ...`
  - Polling-based watch loop first (mtime); event-based later.

Notes:
- Stasis has no dynamic allocation; CLI parsing uses fixed buffers and simple tokenization.
- We keep path handling ASCII-first (Windows-friendly), but treat data as bytes on disk.

## Current Status (Today)

Implemented:
- `stasis check <entry.stasis>`: loads the import dependency graph then runs lexer + minimal parsing (paren/brace/bracket balance) + top-level decl scan; also does a first-pass function-body statement parse; prints `files/lex_errors/parse_errors/sig_errors/stmt_errors/import_collisions`.
- `stasis build ...`:
  - Cranelift: still minimal compiler path for `function main(): i32 { return <const-expr>; }` (const-folded i32/bool expression subset).
  - LLVM: can now emit IR for multi-function programs (across all loaded modules) for the current bring-up subset: i32/bool locals, assignments, expression statements, structured control flow (`if`/`else`, `for`), return-slot (so `return` can appear anywhere), function calls with scalar args (i32ish/u8) and ptr args (u8[]/i32ish[]), struct field access (`a.b`), and indexing (`a[i]`) for fixed arrays/byte buffers. Module-qualified calls like `math.add(1, 2)` are supported; functions are mangled as `module__name` (entry `main` remains `@main`). `u8_to_i32()` and `i32_to_u8_trunc()` are recognized as bring-up intrinsics. String literals lower to static utf8 buffers (header + null sentinel); `print_*` and `sys_*` are lowered via `printf` and `stasis_sys_*`. `&&`/`||` are still eager (no short-circuit) in IR.
  - `--emit-ir` writes CLIF/LLVM IR to `--out`
  - `--backend llvm` without `--emit-ir` builds a native EXE via `clang`
- `stasis run ...`: LLVM+`lli` runner for the current LLVM subset (executes the produced IR and returns the program exit code).
- `stasis release ...`: LLVM+`clang -O3` for the current LLVM subset.
- `stasis watch (check|build|run) ...`: polling watch loop based on `sys_file_mtime_ms` + `sys_sleep_ms`; reruns the selected subcommand on changes.

Not yet implemented (but planned in the contract above):
- `test`.

## Backend Strategy (No Reimplementation of LLVM/Cranelift)

The self-hosted compiler does not embed LLVM/Cranelift libraries.

- Cranelift:
  - Emit CLIF text compatible with the existing Rust tool `tools/cranelift-aot`.
  - Invoke `stasis-cranelift-aot` via `sys_exec()` to produce a `.obj`, then link with `clang`.
- LLVM:
  - Emit LLVM IR text.
  - If IR contains only libc intrinsics, `lli` can execute directly.
  - If IR requires external runtime libs (for example `stasis_sys_*`), skip `lli` and go straight to `clang` link + execute.

This keeps the scope to "frontend + textual IR emit" while retaining current codegen toolchains.

## Source Graph Model (No Flattening)

The compiler operates on a "source graph":

- Each file is loaded once into a fixed-capacity byte pool.
- A file table records `(module, path, offset, len, mtime_ms)` for all files in the build.
- Imports are scanned from each loaded file; newly discovered files are appended to the table and scanned in turn.
- Import edges are recorded per file (fixed-capacity table) for later module resolution and diagnostics.
- Later passes (lexer/parser/sema/codegen) iterate the file table; no pass requires a single concatenated source blob.

This matches the user-facing intent: "just reference other files" rather than expanding imports into one file.

## Modules (Imports Introduce Modules)

Each imported file is treated as a module.

- Module name is derived from the imported file basename (strip extension, map `-` and other non-identifier bytes to `_`).
- Duplicate module names are rejected (fail-fast with a clear diagnostic). Import aliasing is intentionally not supported for now.
- Imported module members are in scope by default; if multiple imports define the same name, unqualified use is an error and the frontend will require `ModuleName.symbol`.
- Imports are file-scoped (not transitive): a module must explicitly import what it uses.

## Iteration First (Avoid Recursion Where Possible)

We prefer iterative algorithms in the self-hosted compiler:

- Import graph loading is iterative (no recursive import traversal).
- Expression parsing will use a Pratt parser (iterative loop with operator precedence).
- Other recursion is allowed only when it is shallow and bounded (for example, structured blocks), and should be replaced with explicit stacks where it becomes a risk.

## Bootstrap / How To Run (Today)

`stasis` is a Stasis program that is currently bootstrapped by the existing C# toolchain (stage0).

Build prerequisites:
- `dotnet` (for `Stasis.Cli`)
- `clang` (for link steps)

1) Build the sys runtime library (Windows):
- `cd runtime`
- `cmake -S . -B build`
- `cmake --build build --config Release --target stasis_sys_static`

2) Build `stasis` as a standalone EXE:
- Cranelift: `.\stasis.bat build src\stasis\main.stasis --backend cranelift --out build\stasis.exe`
- LLVM: `.\stasis.bat build src\stasis\main.stasis --backend llvm --out build\stasis-llvm.exe`

3) Run the currently-implemented command:
- `.\build\stasis.exe check <entry.stasis>`
- `.\build\stasis.exe watch check <entry.stasis>` (polling watch; rebuilds on mtime changes)

Notes:
- Stage0 `stasisc run` does not currently forward argv to programs; build the EXE and run it directly.
- The produced `build/stasis*.exe` runs without a `dotnet` runtime (native executable).
- `sys_*` is linked automatically by the stage0 CLI; set `STASIS_SYS_LIB` to override discovery if needed.

## Fixed Memory Budgets (Static)

Hard limits (per compilation):

- Max files: 300
- Max bytes per file: 51200 (50 KiB)
- Max total source bytes: 300 * 51200 = 15360000

Planned static allocations in `stasis` (tunable as we learn real-world pressure):

- Source pool: ~16 MiB (all source file contents, null-sentinels between files)
- Text output buffer (IR/debug): ~32 MiB
- Token buffer: fixed array sized for worst-case token density
  - Conservative estimate: 1 token per 2 bytes -> ~8 million tokens is too large; use a smaller cap and fail with a diagnostic if exceeded.
  - First implementation will target common code sizes and adjust once we have lexer metrics.
- AST storage: arena-like arrays (nodes + edges) with fixed caps; fail with a diagnostic when exceeded.
- Symbol/type tables: fixed arrays + simple string interner into a byte pool.

All "out of memory" failures must name the pool/cap and the source span or file that caused it.

## Bootstrap Strategy

Stage 0 (today): C# toolchain compiles `src/stasis` (the Stasis compiler) so we can iterate.

Stage 1: `stasis` compiles itself.

Stage 2: triple-build fixed point:

- C# -> stasis_v1
- stasis_v1 -> stasis_v2
- stasis_v2 -> stasis_v3

Success is when v2 and v3 outputs match (or match modulo cosmetic IR formatting if we compare IR).

## Work Breakdown (Milestones)

### M0: Runtime syscalls (unblocks standalone CLI)

We need a minimal host API accessible from Stasis code:

- argv:
  - `sys_argc() -> i32`
  - `sys_argv(i: i32, out: utf8[], out_cap: i32) -> i32`
- file I/O:
  - `sys_read_file(path: utf8[], out: u8[], out_cap: i32) -> i32`
  - `sys_write_file(path: utf8[], data: u8[], len: i32) -> bool`
  - `sys_file_exists(path: utf8[]) -> bool`
  - `sys_file_size(path: utf8[]) -> i32`
  - `sys_file_mtime_ms(path: utf8[]) -> i32` (for watch polling)
- process (for driving AOT/link from Stasis):
  - `sys_exec(command: utf8[]) -> i32`

This milestone includes:
- runtime implementation (C)
- LLVM lowering
- Cranelift lowering + external declarations
- CLI link integration so produced EXEs can resolve these symbols

### M1: `stasis` skeleton CLI

- Parse argv, print usage
- Read a single `.stasis` file into a fixed buffer
- Load the import graph (up to 300 files, 50 KiB each) without flattening
- Emit "phase timing" logs similar to the C# CLI (optional but useful for watch)

Implementation notes:
- Imports should ignore duplicates (per `docs/spec.md` "Imports") and preserve per-file spans for diagnostics.
- The import graph loader should avoid recursion-dependent buffers (store file contents in a stable global pool).
- First CLI command should be `check` so later stages can reuse the file table for lexer/parser bring-up.

### M2: Lexer (Stasis)

- Zero-copy tokens (spans into the source buffer)
- Tests in Stasis (`test` blocks)

### M3: Parser (Stasis)

- Decls via iterative scan first (already in place), then a real parser for all constructs in `docs/compilation.md`.
- Expressions:
  - Prefer iterative parsing (shunting-yard / explicit stacks) to avoid deep recursion.
  - Full operator precedence (TypeScript-like) + operator-method calls (`x.+(y)` etc).
  - Postfix forms: call `f(x)`, member `a.b`, index `a[i]`.
  - Assignments: `=`, `+=`, `-=`, etc.
- Statements:
  - Block `{ ... }`, `let` (with/without initializer), `return`, `if`/`else`, loops (`for`, `foreach`), expression statements.
  - Keep parsing iterative where reasonable (for example, explicit stacks for nested blocks).
- Diagnostics:
  - Always emit `path:line:col` with a short "what" + "hint" where practical.

### M4: Semantic + Layout (Stasis)

- Name resolution:
  - Per-module symbol tables (decls) + import/module scope rules.
  - Imported module members visible by default; collisions require `Module.symbol`.
  - No import aliasing (for now).
- Types:
  - Primitive types + arrays + structs/enums.
  - Operator-method validation (reject invalid operator-method usage early).
  - Const-eval support as needed for layout (array sizes, global initializers).
- Memory/layout (match `docs/spec.md` and Stage0):
  - Static globals only; deterministic layout.
  - AoS syntax lowered to SoA storage (flattening struct globals/fields into separate globals).
  - String buffer headers + negative indexing rules (keep consistent with Stage0 lowering).

### M5: Codegen (Stasis)

- General goal: emit textual IR and reuse existing native toolchains (`tools/cranelift-aot`, `lli`, `clang`) rather than embedding backends.
- Function codegen:
  - SSA-ish temps + stack locals (as needed) for LLVM IR.
  - Control flow blocks for `if`/loops; structured lowering (no recursion required).
  - Calls, returns, arithmetic/comparison, and eventually memory ops (globals/struct fields/arrays).
- Global lowering:
  - Single global struct holding program state (the Stasis model), with deterministic field offsets.
  - Emit the backing storage and accessor ops in IR.
- Backend parity:
  - Keep CLIF and LLVM semantics aligned (tests should run on both).

### M6: CLI parity

- `run`: compile + execute (Cranelift runner DLL mode optional later)
- `release`: optimized build (LTO hooks preserved)
- `watch`: polling-based loop first; event-based later

## Definition Of Done (Self-Hosted)

Stage1 is considered "self-hosted" when it can:

- Compile any valid Stasis program (per `docs/spec.md`) from a source graph (no import expansion), producing either LLVM IR or CLIF, and then an executable via the existing toolchain.
- Run without `dotnet` at runtime (native executable; `.NET` is bootstrap-only).
- Compile itself end-to-end (`src/stasis/main.stasis` entry), producing a `stasis` executable that can rebuild the same sources.
- Provide CLI parity for `check`, `build`, `run`, `release`, `watch`, and `test` (test discovery + timings per repo guidelines).

We will reach this in stages: first "compile more programs", then "compile most programs", then "compile everything", then "compile itself".

## Import + Source Limits (Enforced)

- Max files: 300 (including stdlib imports).
- Max bytes per file: 51200.
- Max total source bytes: 300 * 51200 = 15360000 (plus small headers).

If a limit is exceeded, compilation fails with a precise diagnostic:
- which import exceeded the limit
- which file caused the total to overflow

## Worklog

- 2026-01-05: created branch `self-host` from updated `main`; added this document.
- 2026-01-05: added `runtime/stasis_sys.c` + `stasis_sys_static` build target (argv/file/exec), and wired `sys_*` builtins through LLVM + Cranelift + CLI linking.
- 2026-01-05: added `tests/syscalls_basic.stasis` smoke tests for `sys_*`.
- 2026-01-05: added `src/stasis/main.stasis` minimal standalone CLI (argv + read_file smoke).
- 2026-01-05: fixed Cranelift backend to accept string literals for `sys_*` string args; updated syscalls smoke test to use `argv0` paths (backend-independent).
- 2026-01-05: added `sys_file_size` to support enforcing per-file byte limits (50 KiB) without ambiguous truncation.
- 2026-01-05: implemented import graph loading (300 files / 50 KiB limits) and fixed Cranelift `print_string` to accept array/string args.
- 2026-01-05: added `tests/stasis_imports.stasis` coverage for import graph loading + limits; taught LLVM lowering to accept string literals as array arguments (needed for stasis under LLVM).
- 2026-01-05: added `sys_sleep_ms` (polling watch support) and implemented `stasis watch check` based on `sys_file_mtime_ms`.
- 2026-01-05: added `src/stasis/lexing.stasis` streaming lexer + `tests/stasis_lexing.stasis` coverage (comments, numbers, keywords, punctuation).
- 2026-01-05: fixed LLVM lowering for nested short-circuit `&&`/`||` so verifier passes when RHS emits control flow.
- 2026-01-05: added `src/stasis/parsing.stasis` minimal parse pass (balance check) + `tests/stasis_parsing.stasis`; wired `stasis check` to run lexer+parse across the loaded source graph.
- 2026-01-05: imports now assign a deterministic module name per file (derived from basename) and reject duplicate module names.
- 2026-01-05: import scanning now records per-file import edges (fixed table) for future semantic module resolution.
- 2026-01-05: refactored self-host compiler to a single global `sh: ShState` in `src/stasis/state.stasis` (no other globals under `src/stasis/`).
- 2026-01-05: fixed LLVM + Cranelift lowering for string-buffer headers on flattened struct fields (needed for `sh.scratch_*` and other `ascii[N]` fields inside `sh`).
- 2026-01-05: fixed `tools/cranelift-aot` to accept `load.r64` (pointer) instructions.
- 2026-01-05: fixed Stage0 Cranelift artifact cache invalidation under `dotnet run` by salting with loaded assembly stamps (CLI + compiler).
- 2026-01-05: tokenizer now uses `enum ShTok { ... }` (explicit numeric values) instead of `const SH_TOK_*`.
- 2026-01-05: clarified module import semantics: imported module members are in scope by default; use `ModuleName.symbol` only to disambiguate.
- 2026-01-05: `stasis check` now prints lexer error diagnostics (file + line/col) for unknown bytes and unterminated string literals.
- 2026-01-05: added `--quiet` to `stasis check` and `stasis watch check` to suppress diagnostics (useful for scripting/watch output).
- 2026-01-05: made `src/stasis/*.stasis` imports explicit (avoid relying on transitive imports under the module model).
- 2026-01-05: added a top-level signature scan pass (`src/stasis/signatures.stasis`) + `tests/stasis_signatures.stasis`; `stasis check` now reports `sig_errors`.
- 2026-01-05: added import-member collision detection (`src/stasis/modules.stasis`) + `tests/stasis_modules.stasis`; `stasis check` now reports `import_collisions` (non-fatal note).
- 2026-01-05: standardized diagnostics formatting to `path:line:col` via `src/stasis/diagnostics.stasis` (lexer/parser/signatures).
- 2026-01-06: grew the signature scan into an iterative top-level decl parser that records `struct`/`enum` declarations + fields/members + basic type shapes into fixed-capacity tables under `sh`; added `tests/stasis_decls.stasis`.
- 2026-01-06: signature scan now also records `function`/`test` decls (params, return type, body start) and `global`/`const` decls (type + init start) into fixed-capacity tables.
- 2026-01-06: added minimal `stasis build --emit-ir` + IR emit helpers (`src/stasis/emit_ir.stasis`, `src/stasis/build_ir.stasis`) and coverage in `tests/stasis_build_ir.stasis`.
- 2026-01-06: added minimal `stasis run` for LLVM via `lli` (no linking yet; used for smoke-testing the IR path).
- 2026-01-06: `stasis build` can now produce a native EXE for LLVM by invoking `clang` on emitted IR (still minimal-subset only).
- 2026-01-06: minimal build now supports constant `return` expressions (literals + `+ - * /` + parentheses) via an iterative shunting-yard evaluator.
- 2026-01-06: added minimal `stasis release` (LLVM+clang `-O3`).
- 2026-01-06: extended `stasis watch` to support `build` and `run` (polling).
- 2026-01-06: expanded roadmap details for parser/sema/codegen and added a "definition of done" checklist for self-hosting.
- 2026-01-06: added iterative expression parser + const evaluator (`src/stasis/expr_parse.stasis`) and reused it for minimal `build` return expressions (more operators, better errors).
- 2026-01-06: added a first-pass function-body statement parser (`src/stasis/stmt_parse.stasis`) and wired it into `stasis check` (syntax-only; expressions are skipped structurally for now).
- 2026-01-06: LLVM IR emit now supports a larger `main` subset (locals + assignments + expression statements + non-constant returns) and `stasis run/build/release` can execute/build that subset.
- 2026-01-06: LLVM IR emit now supports structured control flow (`if`/`else`, `for`) and a return-slot + `exit:` block so `return` no longer has to be the final statement; added IR-level tests for `if` and `for`.
- 2026-01-06: expression parser now supports empty-arg calls like `foo()`; LLVM IR emitter can now include additional entry-file no-arg `i32` helper functions (minimal return-only body) so `main` can call them; added `foo()` IR test coverage.
- 2026-01-06: expression parser now supports slice types in decls and statements, and indexing expressions (`a[i]`).
- 2026-01-06: LLVM IR emitter can now emit struct type definitions and global variable definitions (zeroinitializer + align).
- 2026-01-06: LLVM expression emission now uses a typed iterative stack; added lowering for struct members + indexing (including global `u8` assignment via trunc/zext); added IR tests.
- 2026-01-06: fixed expression parsing for nested calls by staging per-call arg lists in a stack buffer before copying into the call-node arg arena; added LLVM IR tests for nested calls.
- 2026-01-06: LLVM bring-up: accept `u8[]`/`i32[]` as `ptr` params (length passed separately) and recognize `u8_to_i32` / `i32_to_u8_trunc` as intrinsics; added IR tests.
- 2026-01-06: grew `sh.ir_out` to 32MiB; added LLVM module-level string literal emission (utf8 header + null sentinel) and added IR tests.
- 2026-01-06: LLVM bring-up: lowered `print_string`/`print_int`/`print_char` and `sys_*` builtins via `printf` and `stasis_sys_*`; added IR tests.
- 2026-01-06: fixed expression parsing of `)` so calls can close inside grouping parentheses (needed for self-host compiler code).
- 2026-01-06: refactored LLVM IR emitter files to keep each `.stasis` source <50KiB; added void returns, char_* builtins, const resolution, and enum member lowering.
- 2026-01-06: stage1 self-build now works: `stasis build --emit-ir src/stasis/main.stasis` emits `build/stasis_self.ll` with no stage0 help beyond bootstrap.
- 2026-01-06: `stasis build`/`run`/`release` now link the sys runtime (`runtime/stasis_sys.c`) when invoking `clang`; `run` uses a temp exe instead of `lli`.
- 2026-01-06: LLVM bring-up: added `f32` support end-to-end (float literals, `f32` types, locals/params/returns, `+ - * /`, comparisons, consts); added `i32_to_f32` / `f32_to_i32` intrinsics; split call lowering to keep files <50KiB; added `examples/f32_smoke.stasis`.
