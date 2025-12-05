# Stasis Language Tooling

This repo contains the Stasis compiler frontend, LLVM IR lowering, and a simple CLI (`stasisc`) for producing LLVM IR or running the built-in test harness.

## Quickstart

Prereqs: .NET 9 SDK, LLVM `lli` on PATH for execution.

Build and test:
```sh
dotnet test
```

Emit LLVM IR (production defaults, tests omitted):
```sh
dotnet run -p Stasis.Cli -- path/to/file.stasis > out.ll
```

Emit IR including Stasis tests + harness:
```sh
dotnet run -p Stasis.Cli -- path/to/file.stasis --with-tests > out.ll
```

Execute Stasis tests via LLVM:
```sh
lli -entry-function=run_tests out.ll
echo $?
# exit code 0 means all tests passed; nonzero is the failure count
```

Options:
- `--with-tests` include test functions and `run_tests` harness (default is production: tests omitted, no harness).
- `--module <name>` set the LLVM module identifier (default `module`).
- `--help` prints usage.

Suggested convenience scripts (from repo root):
- `stasis run path/to/file.stasis` -> emits IR (production) and runs `lli` with default entry (main); pass extra CLI args (e.g., `--module foo`) after the file.
- `stasis test path/to/file.stasis` -> emits IR with tests, runs `lli -entry-function=run_tests`, returns failure count as exit code.
Scripts are provided (`stasis.bat` on Windows, `stasis.sh` on Unix). Add the repo root to `PATH` to call `stasis` without `./`.

Notes:
- Function calls and control flow are lowered; SoA globals follow the layout in `docs/spec.md`.
- Locals/params are primitives only; structs/arrays live in static globals per spec.
- If `lli` is not available, you can still produce IR; executing tests requires LLVM tools installed.

Samples:
- `samples/basic.stasis` basic function + main returning 5.
- `samples/tests.stasis` includes Stasis `test` declarations (use `--with-tests` and `lli -entry-function=run_tests`).
