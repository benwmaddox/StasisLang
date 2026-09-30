# Project and CLI lifecycle

<!-- tags: cli, project, vendor, build, package -->

Run `stasis` from the project containing `stasis.json`. Commands such as
`check` also accept `--workspace PATH`; `play` discovers the entry from its
current directory or an explicit source path.

| Command | Purpose |
| --- | --- |
| `stasis fmt --check` | Check formatting; `fmt` writes canonical source |
| `stasis check` | Compile without executing `main` |
| `stasis test` | Run source tests and supported scenarios |
| `stasis run --headless --ticks 120` | Execute `main` and 120 ticks without rendering |
| `stasis play` | Graphical development play with live code swapping |
| `stasis build --mode release` | Build an AOT executable |
| `stasis package --target desktop` | Assemble a standalone desktop package |
| `stasis package --target web --development-build` | Assemble a development Web bundle |
| `stasis package-mobile --target android-arm64` | Assemble Android AOT output and app shell |
| `stasis inspect` | Report state storage and capacity projections |

`vendor/stasis` is one versioned docs/stdlib snapshot. Use `stasis vendor status`
to inspect it and `stasis vendor update` to adopt the selected toolchain. Review
and commit the vendor tree and manifest pin together. Edit game code in `src/`
and `tests/`; local `check` and `test` preserve the checked-in vendor pin.

See [semantic editing](semantic-edit-and-validation.md), [testing](testing.md),
[captures and replay](record-and-replay.md), and [benchmarking](benchmarking.md)
for the corresponding workflows.
