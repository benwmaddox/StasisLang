# Self-Hosted Stasis Compiler (Branch: `self-host`)

Goal: replace the current C# frontend (`Stasis.Compiler`) with a Stasis-written compiler that can run as a standalone CLI.

This is a living design + progress document for the `self-host` branch.

## Non-Negotiables

- Self-hosted compiler is written in Stasis and is directly runnable as a CLI (no C# wrapper required at runtime).
- Supports up to 300 source files per build (imports), up to 50 KiB per file.
- Provides CLI modes: `watch`, `run`, `release` (and `build`/`test` as needed for parity).
- Emits either:
  - Cranelift CLIF text (compiled via existing `tools/cranelift-aot` + `clang` link), or
  - LLVM IR text (executed via `lli` when possible, else `clang` link).
- Remains deterministic: static memory only; explicit I/O; no hidden allocation.

## CLI Contract (Target)

Initial deliverables should keep the CLI small but stable. The intent is to match `stasis` behavior over time.

Commands (final shape; implemented incrementally):

- `stasisc-self expand <entry.stasis> <out.stasis>`
  - Expands imports into a single file (duplicate imports removed).
  - Enforces file/count limits early, before parsing.
- `stasisc-self check <entry.stasis> [--backend cranelift|llvm]`
  - Parses + semantics only (fast feedback, no codegen), later used by `watch`.
- `stasisc-self build <entry.stasis> -o <out.exe|out.wasm> [--backend ...] [--release]`
- `stasisc-self run <entry.stasis> [--backend ...] [--] <args...>`
  - Build to a temp output then exec it; preserves exit code.
- `stasisc-self test [path-or-glob] [--backend ...]`
  - Discovers `.stasis` files; runs in-file `test` blocks; prints per-file timings.
- `stasisc-self watch (check|build|run|test) ...`
  - Polling-based watch loop first (mtime); event-based later.

Notes:
- Stasis has no dynamic allocation; CLI parsing uses fixed buffers and simple tokenization.
- We keep path handling ASCII-first (Windows-friendly), but treat data as bytes on disk.

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

## Fixed Memory Budgets (Static)

Hard limits (per compilation):

- Max files: 300
- Max bytes per file: 51200 (50 KiB)
- Max total source bytes: 300 * 51200 = 15360000

Planned static allocations in `stasisc-self` (tunable as we learn real-world pressure):

- Source pool: ~16 MiB (all expanded file contents, null-sentinels between files)
- Expanded output buffer (for `expand` and for "single-file parse" bootstrap): ~16 MiB
- Token buffer: fixed array sized for worst-case token density
  - Conservative estimate: 1 token per 2 bytes -> ~8 million tokens is too large; use a smaller cap and fail with a diagnostic if exceeded.
  - First implementation will target common code sizes and adjust once we have lexer metrics.
- AST storage: arena-like arrays (nodes + edges) with fixed caps; fail with a diagnostic when exceeded.
- Symbol/type tables: fixed arrays + simple string interner into a byte pool.

All "out of memory" failures must name the pool/cap and the source span or file that caused it.

## Bootstrap Strategy

Stage 0 (today): C# toolchain compiles `stasisc_self` (the Stasis compiler) so we can iterate.

Stage 1: `stasisc_self` compiles itself.

Stage 2: triple-build fixed point:

- C# -> stasisc_self_v1
- stasisc_self_v1 -> stasisc_self_v2
- stasisc_self_v2 -> stasisc_self_v3

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

### M1: `stasisc_self` skeleton CLI

- Parse argv, print usage
- Read a single `.stasis` file into a fixed buffer
- Implement import expansion (up to 300 files, 50 KiB each)
- Emit "phase timing" logs similar to the C# CLI (optional but useful for watch)

Implementation notes:
- Import expansion should preserve ordering (inline at first import site) and ignore duplicates (per `docs/spec.md` "Imports").
- Import expansion should be implemented without recursion-dependent buffers (store file contents in a stable global pool).
- First CLI command should be `expand` so later stages can reuse the "expanded single-file view" for lexer/parser bring-up.

### M2: Lexer (Stasis)

- Zero-copy tokens (spans into the source buffer)
- Tests in Stasis (`test` blocks)

### M3: Parser (Stasis)

- Decls via recursive descent, expressions via Pratt parser (per `docs/compilation.md`)
- Diagnostic spans with actionable hints

### M4: Semantic + Layout (Stasis)

- Symbols, types, const-eval needed for layout
- AoS -> SoA lowering aligned to `docs/spec.md`

### M5: Codegen (Stasis)

- Cranelift CLIF emitter first (matches current backend semantics)
- LLVM IR emitter second

### M6: CLI parity

- `run`: compile + execute (Cranelift runner DLL mode optional later)
- `release`: optimized build (LTO hooks preserved)
- `watch`: polling-based loop first; event-based later

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
- 2026-01-05: added `src/stasisc_self/main.stasis` minimal standalone CLI (argv + read_file smoke).
- 2026-01-05: fixed Cranelift backend to accept string literals for `sys_*` string args; updated syscalls smoke test to use `argv0` paths (backend-independent).
- 2026-01-05: added `sys_file_size` to support enforcing per-file byte limits (50 KiB) without ambiguous truncation.
