# Stasis

Stasis is an experimental programming language and toolchain focused on deterministic, game-style programs:

- Static global memory (no hidden allocations)
- Predictable layouts (stable field offsets and array layouts)
- Tick-based runtime model (host sets a per-frame snapshot; Stasis emits command buffers)
- Fast edit-compile-run loops via in-process hot swap in development

## Status

Fast-moving. Expect breaking changes.

## Architecture (Rewrite V1)

Rewrite V1 is Rust-first:

- `apps/stasis`: main app/CLI. Includes in-process dev runner (`play`) with file watch + hot swap.
- `crates/stasis_compiler`: Rust-native frontend + lowering to Cranelift (JIT/AOT).
- `crates/stasis_jit`: JIT/AOT codegen support + function pointer table commit mechanics.
- `crates/stasis_runner`: swap pipeline contracts + sequencing.
- `runtime/`: native graphics/audio host runtime (currently Windows-focused; used by `play`).
- `src/stdlib/`: Stasis standard library modules.
- `samples/brickout_revenge/`: end-to-end sample game.

Canonical documents:

- `docs/spec.md`: language spec (Rewrite V1)
- `docs/live-compilation-prd.md`: hot swap + product requirements
- `docs/rewrite_v1_checklist.md`: execution plan and slice ordering

## Build/Test (from source)

```bash
cargo build
cargo test
```

## Run (dev in-process play + watch + hot swap)

Build once:

```bash
cargo build -p stasis
```

Run Brickout Revenge v1 (Windows in-process dev runner):

```powershell
.\target\debug\stasis.exe play samples\brickout_revenge\brickout_revenge_v1.stasis --watch-dir samples\brickout_revenge
```

What to expect:

- Save a `.stasis` file under `--watch-dir` and the runner recompiles between ticks.
- On success you should see `[swap] swapped ok ... total=...ms` in stdout.

## Nightly Builds

Nightly prereleases are published from `main` when new commits are detected since the last nightly tag.

- Releases: https://github.com/benwmaddox/StasisLang/releases
- Workflow: `.github/workflows/nightly-release.yml`

Bundles:

- `stasis-nightly-win-x64.zip`
- `stasis-nightly-linux-x64.tar.gz`
- `stasis-nightly-osx-x64.tar.gz`

On Windows, SmartScreen may warn on unsigned nightly binaries.
