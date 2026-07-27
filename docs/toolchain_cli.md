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
stasis tui
stasis build --mode dev
stasis build --mode release
stasis package --target desktop
stasis package-mobile --target android-arm64
stasis package-mobile --target ios-arm64
stasis inspect
stasis inspect --capacity state.enemies=512
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

The project `name` may contain internal ASCII spaces, so display names such as `Chess TD` are
valid; leading or trailing spaces are rejected. Manifest paths must be project-relative and cannot
contain `..`. Generated projects include a
runnable `main()`, a real `.test.stasis` test, an `AGENTS.md` theory-building and semantic-edit
guide, and a minimal `CLAUDE.md` that points to `AGENTS.md`.

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
- `tui [ENTRY]`: launch the same entry-relative graphical hot-swap workflow as `play`, using the
  manifest `entry` when no override is supplied, while a desktop terminal uses
  the runner's versioned LiveSession protocol for background-prepared code-aware symbol edits and
  typed between-tick inspection or mutation. The terminal includes history, a Ctrl-P command
  palette with live fuzzy compiler-backed symbol/member completion, keyboard navigation and
  insertion, Tab completion, paging, multiline cancellation, and concise command-specific output. Use
  `--live-json` for complete schema-v1 response envelopes or `--live-script PATH` for a
  deterministic command script. See
  [Interactive live workspace](live_cli_workspace.md).
- `build --mode dev`: compile through JIT and write `build/dev-build.json` as a deterministic
  receipt.
- `build --mode release`: use the shared Cranelift AOT pipeline and write the native executable to
  `build/`.
- `package --target desktop`: create a standalone directory with the AOT executable, manifest,
  assets, graphics runtime when present, and verified release provenance.
- `package-mobile --target android-arm64|ios-arm64 [--entry PATH]`: atomically assemble the
  shared AOT output, SDL-only runtime, bundled assets, verified provenance, and thin Gradle or
  Xcode app shell.
- `package --target android-arm64|ios-arm64`: compatibility spelling that uses the manifest entry.
- `inspect [--capacity PATH=COUNT] [--mobile-budget-bytes N]`: compile the manifest entry and
  report the canonical direct-storage model: bytes and alignment by state path/field, struct
  rollups, capacity versus active count, snapshot size, the eight largest pools, recognized
  command buffers, projected capacity-change bytes, and mobile-budget warnings. Repeating
  `--capacity` compares several proposed pool sizes without changing source or runtime state.
  JSON output includes the complete deterministic report; human output emphasizes totals,
  largest pools, projections, and warnings.
- `version` and `env`: report installation, cache, and workspace locations.

`replay` and `verify` intentionally return deterministic unsupported diagnostics until the replay
runtime contract lands; they do not fake successful behavior.

Official packaging fails if the installed compiler or renderer sources differ from the release
manifest. When working from a source checkout, pass `--development-build` to `package` or
`package-mobile`; the resulting package is permanently labeled non-release. See
[Release and package provenance](release_provenance.md).

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
