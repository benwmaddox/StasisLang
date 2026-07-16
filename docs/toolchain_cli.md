# Stasis integrated CLI

The `stasis` executable is the supported entry point for ordinary project work. A release
archive contains the compiler, JIT/AOT runtime bridge, standard library, templates, target
metadata, and the runtime bridge needed by the archive's native target. Project commands do not
invoke Cargo and do not download dependencies.

## Install

Extract a release archive and add its executable directory to `PATH`:

- Windows: the archive root contains `stasis.exe`, `stasis_runner.exe`, `stasis_graphics.dll`,
  `lld-link.exe`, `clang-cl.exe`, and the project-built `stasis_dynload.dll` /
  `stasis_dynload.dll.lib` runtime bridge pair. The bundled LLVM tools compile/link generated game
  bridges; no Windows SDK or MSVC import libraries are redistributed.
- Linux/macOS: use `bin/stasis`; the matching static runtime bridge is beside it. Native AOT
  linking currently uses the platform `cc` driver supplied by the supported host image.

Run `stasis version` and `stasis env` to verify the selected installation. Upgrades are explicit:
extract a newer versioned archive and update `PATH`. Keeping two extracted versions is supported;
the first executable on `PATH` wins.

## Create and use a project

```text
stasis new brick_game
cd brick_game
stasis fmt
stasis check
stasis test
stasis run --headless
stasis run --watch
stasis build --mode dev
stasis build --mode release
stasis package --target desktop
```

`stasis init --name brick_game .` initializes an existing directory. The built-in template copies
the version-matched standard library into `stdlib/`, so imports remain offline and project-local.
Commands discover
`stasis.json` by walking from the selected path toward the filesystem root, so they work from the
project root and nested directories. `--workspace PATH` selects a project explicitly.

## Workspace contract

`stasis.json` is versioned and deterministic:

```json
{
  "manifest_version": 1,
  "name": "brick_game",
  "entry": "src/main.stasis",
  "tests": "tests",
  "output": "build"
}
```

Manifest paths must be project-relative and cannot contain `..`. Generated projects include a
runnable `main()` and a real `.test.stasis` test.

## Commands and outputs

- `new` / `init`: create the manifest and built-in starter template without network access.
- `fmt [--check]`: normalize line endings, trailing whitespace, blank EOF lines, and the final
  newline. The operation is idempotent and never follows symlinks.
- `check`: run the shared frontend and Cranelift JIT compilation path without executing `main`.
- `test [PATH]`: run Stasis tests in one isolated JIT session.
- `run [--headless]`: JIT-compile and execute no-argument `main(): i32` or `main(): void`; an
  `i32` result is the process exit code. Headless execution is the default.
- `run --watch`: launch the existing graphical runner and hot-swap pipeline for game projects.
  Because it is an unbounded graphical session, watch mode rejects `--json` and `--headless`.
- `build --mode dev`: compile through JIT and write `build/dev-build.json` as a deterministic
  receipt.
- `build --mode release`: use the shared Cranelift AOT pipeline and write the native executable to
  `build/`.
- `package --target desktop`: create a standalone directory with the AOT executable, manifest,
  assets, and graphics runtime when present.
- `package --target android-arm64|ios-arm64`: delegate to the shared mobile AOT bundle pipeline.
- `inspect`, `version`, and `env`: report workspace, installation, cache, and output locations.

`replay` and `verify` intentionally return deterministic unsupported diagnostics until the replay
runtime contract lands; they do not fake successful behavior.

Add `--json` to receive one stable JSON result object. Usage errors exit 2, command/compile/test
failures exit 1, and successful commands exit 0 except `run`, which preserves the guest's `i32`
exit code. Guest program output may precede the final JSON object for `run`.

## Cache and offline behavior

Compiler artifacts live under `<project>/.stasis_cache`; declared build outputs live under the
manifest's `output` directory, and packages default to `dist/`. These paths can be removed safely
while no Stasis command is running. Core create/format/check/test/run/native-build workflows are
offline after installation. Mobile builds still require the documented platform SDK/NDK and
signing tools for their target.

## CI

A minimal CI job can install one release archive and run:

```text
stasis fmt --check
stasis check --json
stasis test --json
stasis build --mode release --json
```

Release workflows smoke-test a freshly assembled archive rather than borrowing compiler assets
from the repository checkout.
