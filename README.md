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

Build an optimized executable (defaults to `-O3` and LTO unless overridden):
```sh
stasis release samples/basic.stasis --out dist/basic.exe --module basic --opt-level 3 --lto
```

Options:
- `run` or `test` subcommands (default is `run` if omitted).
- `build` produces a binary via `clang` (no optimizations by default); `release` produces an optimized binary (`-O3` + `-flto` by default).
- `test` with no path (or `--all`) runs every `.stasis` file under the working directory.
- `--with-tests` include test functions and harness during lowering even for `run`.
- `--emit-ir` write IR to stdout and skip execution.
- `--opt-level <0|1|2|3|s|z>` and `--lto|--no-lto` control clang optimization flags for `build`/`release`.
- `--module <name>` set the LLVM module identifier (default `module`).
- `--out <path>` write the built binary to a specific path (default is alongside the source).
- `--help` usage.

## CLI wrapper

`stasis.bat` (Windows) and `stasis.sh` (Unix) are thin shims that just call the CLI project from the repo root. Add the repo root to `PATH` to invoke `stasis` without a path prefix.

## Notes
- Function calls and control flow are lowered; SoA globals follow the layout in `docs/spec.md`.
- Infix arithmetic/comparison (`+ - * / % < > ==`) with TypeScript-like precedence; compound assignment `= += -= *= /= %=`; operator-method forms remain supported.
- Locals and parameters are primitive scalars or struct references; arrays and struct storage live in static globals per spec.
- When `lli` is unavailable, the CLI compiles the IR with `clang`. On Windows it links against the latest installed Windows SDK (`ucrt`, `kernel32`, `legacy_stdio_definitions`) so the test harness `printf` resolves.

## Samples
- `samples/basic.stasis` basic function plus `main` returning 5.
- `samples/tests.stasis` includes Stasis `test` declarations; the emitted `run_tests` function prints a summary and returns the failure count.
- `samples/sudoku.stasis` playable Sudoku: `stasis run samples/sudoku.stasis` then enter `row col value` (1-9, 0 clears) or `q` to quit. Clue cells are colored differently; invalid moves are rejected.
- `samples/guess.stasis` number guessing game: `stasis run samples/guess.stasis` then enter guesses (1-99, 0 to quit); it tells you higher/lower or win.

## Git hooks
- Shared pre-push hook lives at `tools/git-hooks/pre-push` and runs `dotnet format --verify-no-changes` then `dotnet test -c Release --no-build`.
- Enable locally:
  ```sh
  git config core.hooksPath tools/git-hooks
  chmod +x tools/git-hooks/pre-push
  ```
