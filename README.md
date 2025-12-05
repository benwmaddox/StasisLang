# Stasis Language Tooling

This repo contains the Stasis compiler frontend, LLVM IR lowering, and a simple CLI (`stasisc`) for producing LLVM IR or executing Stasis programs/tests through LLVM.

## Quickstart

Prereqs: .NET 9 SDK. For execution, install LLVM tools (`lli` preferred, `clang` fallback). On Windows, the fallback link path expects the Windows 10/11 SDK for C runtime libraries.

Build and self-test:
```sh
dotnet test
```

Run a Stasis program (executes via `lli` if available, else `clang`):
```sh
stasis run samples/basic.stasis
```

Run Stasis tests end-to-end (executes `run_tests` harness; prints pass/fail counts from the harness itself):
```sh
stasis test samples/tests.stasis
```
`run` returns your program's exit code; `test` returns the failure count and prints each test result plus a summary.

Emit LLVM IR without executing:
```sh
stasis test samples/tests.stasis --emit-ir > out.ll
```

Options:
- `run` or `test` subcommands (default is `run` if omitted).
- `--with-tests` include test functions and harness during lowering even for `run`.
- `--emit-ir` write IR to stdout and skip execution.
- `--module <name>` set the LLVM module identifier (default `module`).
- `--help` usage.

## CLI wrapper

`stasis.bat` (Windows) and `stasis.sh` (Unix) are thin shims that just call the CLI project from the repo root. Add the repo root to `PATH` to invoke `stasis` without a path prefix.

## Notes
- Function calls and control flow are lowered; SoA globals follow the layout in `docs/spec.md`.
- Locals and parameters are primitives only; structs and arrays live in static globals per spec.
- When `lli` is unavailable, the CLI compiles the IR with `clang`. On Windows it links against the latest installed Windows SDK (`ucrt`, `kernel32`, `legacy_stdio_definitions`) so the test harness `printf` resolves.

## Samples
- `samples/basic.stasis` basic function plus `main` returning 5.
- `samples/tests.stasis` includes Stasis `test` declarations; the emitted `run_tests` function prints a summary and returns the failure count.

## Git hooks
- Shared pre-push hook lives at `tools/git-hooks/pre-push` and runs `dotnet format --verify-no-changes` then `dotnet test -c Release --no-build`.
- Enable locally:
  ```sh
  git config core.hooksPath tools/git-hooks
  chmod +x tools/git-hooks/pre-push
  ```
