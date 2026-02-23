# Stasis Rewrite V1

This branch is a ground-up rewrite focused on a single-process, tick-based runtime with Cranelift JIT for development and Cranelift AOT for production builds.

Kept from previous repository state:
- `docs/spec.md`
- `samples/brickout_revenge/`

Everything else is intentionally rebuilt around a minimal Rust-first architecture.

## Current V1 Layout

- `compiler/`
  Compiler source written in Stasis.
- `compiler/simple_pass_compiler.stasis` (canonical compiler entrypoint)
  Compiler pipeline script. Core pass orchestration lives in `.stasis`.
- `crates/stasis_compiler`
  Rust substrate/binding layer used by the Stasis compiler orchestration.
- `Stasis.Compiler/` and `Stasis.Cli/`
  Bootstrap compiler source imported from `main` for compatibility/testing in this branch.
  These are explicitly bootstrap-only and not the Rewrite V1 self-hosted compiler target.
- `crates/stasis_jit`
  Cranelift code generation management for JIT (dev) and AOT (prod), plus function pointer table support for hot swap flows.
- `crates/stasis_runner`
  Tick-based hot-swap core state and commit sequencing.
- `apps/stasis`
  Single graphical in-process runner (`winit + glutin + glow`) with watch + compile + swap loop.
- `docs/rewrite_v1_checklist.md`
  Build checklist aligned to PRD.
## Build/Test (Current)

```bash
cargo build
cargo test
cargo run -p stasis -- --ticks 300 --watch-dir samples/brickout_revenge
cargo run -p stasis -- --ticks 300 --watch-dir samples/brickout_revenge --events-jsonl
cargo run -p stasis -- --scenario brickout-revenge-v1 --ticks 300
```

Local-only Windows release source ZIP validation (auto-detects Cargo workspace or legacy build/test scripts):

```powershell
powershell -ExecutionPolicy Bypass -File tools/windows/verify-latest-release-source-zip.ps1
```

Local-only Windows release CLI bundle validation (smoke checks `stasis run/build/test` directly from the downloaded release zip):

```powershell
powershell -ExecutionPolicy Bypass -File tools/windows/verify-latest-release-cli.ps1
```

Structured event stream options:
- `--events-jsonl` prints JSONL events to stdout (compile/swap/summary).
- `--events-jsonl-file path\to\events.jsonl` writes JSONL events to a file.

Legacy bootstrap compiler tooling remains available for reference:

```bash
bootstrap\windows\stasisc.bat run path\to\file.stasis --emit-ir
bootstrap\windows\stasisc.bat test --all
```

## Notes

This is V1 foundation work. It is intentionally minimal and deterministic, with strict emphasis on avoiding legacy cruft.

