# Stasis

Stasis is an experimental programming language and toolchain focused on deterministic, game-style programs:

- Static global memory (no hidden allocations)
- Predictable layouts (AoS syntax lowered to SoA storage)
- Fast iteration loops for 2D/game prototypes (tick-hosted workflow)

The implementation is a C# front-end with lowering to either LLVM or Cranelift, plus a small native runtime.

## Status

Early and fast-moving. Expect breaking changes.

## Quickstart (Windows)

From a `cmd.exe` prompt in the repo root:

```bat
build.bat
test.bat
```

Run a sample:

```bat
.\stasis.bat run .\samples\interactive_showcase.stasis --backend llvm --graphics
```

## Quickstart (WSL/Linux)

See `docs/wsl-dev.md` for a WSL-first Brickout Revenge dev loop (recommended if you're comparing hot-swap latency).

## Prereqs (Windows)

`build.bat` and `test.bat` assume:

- .NET 9 SDK
- Visual Studio 2022 Build Tools (MSVC + Windows 10/11 SDK)
- CMake (for `runtime/`)
- Rust (via rustup) for `tools/cranelift-aot`
- vcpkg at `C:\vcpkg` (or `C:\code\vcpkg`) for runtime dependencies

If you have a repo-pinned LLVM toolchain under `.tools/llvm-*/bin`, `env.bat` will prefer it automatically.

## CLI

Use `.\stasis.bat` (Windows) / `./stasis.sh` (Unix) from the repo root.

Common commands:

- `.\stasis.bat run .\samples\asteroids.stasis --backend llvm --graphics`
- `.\stasis.bat test --all --backend cranelift`
- `.\stasis.bat run .\samples\basic.stasis --emit-ir > out.ll`
- `.\stasis.bat run .\samples\basic.stasis --watch` (opt-in dev loop)
- Capture screenshots of windowed demos (writes to `artifacts/screenshots/` and opens the folder): `powershell -ExecutionPolicy Bypass -File .\scripts\capture_sample_screenshots.ps1`

Backends:

- `--backend llvm` runs via `lli` if available, otherwise compiles and links via `clang`.
- `--backend cranelift` defaults to the native runner for fast warm iteration; pass `--no-cranelift-runner` to produce and run an EXE instead.

### Experimental: Cranelift JIT hot-swap (no DLL load)

The `tick` hot-swap watch loop can use an experimental in-process Cranelift JIT runner (avoids writing/loading a hot-swap DLL each change).

- Build: `cd tools/cranelift-jit-runner && cargo build --release`
- Enable: set `STASIS_CRANELIFT_JIT_RUNNER=1`
- Optional: set `STASIS_CRANELIFT_JIT_RUNNER_EXE` to override the runner path.

## Demos and docs

- Overview: `STASIS_OVERVIEW.md`
- Demo day commands: `docs/demo-day.md`
- Game-dev iteration workflow: `docs/game-dev-workflow.md`
- Language spec (semantics, memory rules): `docs/spec.md`
- Parser/grammar notes (LL(1)): `docs/compilation.md`
- Host ABI direction (snapshot + command buffer): `docs/host-snapshot-command-buffer.md`

## Repo layout

- `Stasis.Compiler/` C# parser/sema/lowering
- `Stasis.Cli/` CLI entrypoint (`stasis.bat` / `stasis.sh`)
- `runtime/` native runtime and host integration
- `samples/` runnable programs and interactive demos

## Git hooks (optional)

Shared pre-push hook: `tools/git-hooks/pre-push`
