# Project and CLI lifecycle

<!-- tags: cli, project, vendor, build, package -->

Run the installed `stasis` executable from the project root. A generated
project has `stasis.json`, `src/`, `tests/`, `assets/`, and the version-matched
`vendor/stasis` snapshot. `--workspace PATH` selects a different project;
normal commands can discover the nearest ancestor manifest.

## Edit and verify

```text
stasis vendor status
stasis fmt --check
stasis check
stasis test
stasis run --headless --ticks 120
stasis play
```

`fmt --check` reads without formatting; `fmt` writes canonical source.
`check` compiles without calling `main`. `test` discovers `.test.stasis`
declarations and supported scenario files. `run --headless --ticks N` calls
`main` and then `tick` exactly N times without rendering. `play` opens the
graphical development runner with live code swapping. `run --watch` is the
graphical watch spelling. A successful `check` does not prove runtime or
rendered behavior; use [focused tests](testing.md) and captures as needed.

## Build and package

```text
stasis build --mode dev
stasis build --mode release
stasis package --target desktop
stasis package --target web --development-build
stasis package-mobile --target android-arm64
stasis inspect
```

The dev build writes a receipt; the release build uses AOT and writes an
executable. Desktop packaging collects a standalone AOT package, assets, and
runtime files. Web packaging produces a browser bundle; mobile packaging
assembles an AOT build and platform app shell. These targets have their own
host requirements and manifest fields. `inspect` reports direct state storage,
largest pools, and capacity projections without changing game state.

## Vendor and manifest changes

`vendor/stasis` belongs to Stasis. Do not edit its docs or stdlib in a game
project. `stasis vendor status` is read-only. `stasis vendor update` installs
the selected toolchain's snapshot and updates the `stasis.json` pin together.
Review and commit those two changes together. Source edits belong in project
`src/`, `tests/`, or `assets/`. Local `check` and `test` can validate an older
checked-in snapshot without advancing its pin; update explicitly when adopting
a new toolchain.

Use [semantic edit and validation](semantic-edit-and-validation.md) for source
item changes. Use [record and replay](record-and-replay.md) for reproducible
input evidence and [benchmarking](benchmarking.md) for cost comparisons.
