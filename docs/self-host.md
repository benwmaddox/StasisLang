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

