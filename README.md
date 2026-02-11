# Stasis Rewrite V1

This branch is a ground-up rewrite focused on a single-process, tick-based, in-process Cranelift JIT runner.

Kept from previous repository state:
- `docs/spec.md`
- `samples/brickout_revenge/`

Everything else is intentionally rebuilt around a minimal Rust-first architecture.

## Current V1 Layout

- `compiler/`
  Compiler source written in Stasis.
- `compiler/incremental_compiler.stasis` (planned primary entrypoint)
  Compiler pipeline script. Core pass orchestration lives in `.stasis`.
- `crates/stasis_compiler`
  Rust substrate/binding layer used by the Stasis compiler orchestration.
- `crates/stasis_jit`
  Cranelift JIT generation management and function pointer table.
- `crates/stasis_runner`
  Tick-based hot-swap core state and commit sequencing.
- `apps/stasis`
  Single graphical in-process runner (`winit + glutin + glow`) with watch + compile + swap loop.
- `docs/rewrite_v1_checklist.md`
  Build checklist aligned to PRD.
- `docs/rewrite_v1_tdd.md`
  Technical design notes for implementation details.

## Build

```bash
cargo build
cargo test
cargo run -p stasis -- --entry samples/brickout_revenge/brickout_revenge_v1.stasis
```

## Notes

This is V1 foundation work. It is intentionally minimal and deterministic, with strict emphasis on avoiding legacy cruft.
