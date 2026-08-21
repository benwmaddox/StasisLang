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
the version-matched Stasis sources into `vendor/stasis/stdlib`, so imports remain
offline and project-local. Generated source uses project-root imports such as
`/vendor/stasis/stdlib/stdlib.stasis`, which resolve consistently from nested source files.
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
  "output": "build",
  "vendor": {
    "stasis": {
      "release_id": "nightly-20260805-123",
      "sha256": "<lowercase SHA-256>"
    }
  }
}
```

The vendor release and hash describe the exact checked-in `vendor/stasis` snapshot.
`manifest_version` versions the JSON schema and is independent of the selected toolchain release.
On every normal project command, Stasis verifies the on-disk tree against the selected executable.
A content mismatch stages its matching public stdlib and internal host-ABI modules together and publishes the vendor tree and
manifest as one rollback-capable transaction. The recorded release ID changes only with that vendor content;
building or selecting an executable with a different release ID does not rewrite an unchanged snapshot.
The content hash still detects rebuilt development executables whose release ID did not change and repairs edited or missing vendor files. Stasis owns `vendor/stasis`;
Git is the review and rollback mechanism, so synchronization does not prompt. Review and commit the
vendor and manifest changes together with the compiler upgrade.

Projects that should always use the standard library shipped with the selected toolchain can add
`"stdlib": "toolchain"`. Before a workspace command starts, that exact stdlib and its matching
runtime modules are synchronized transactionally into `.stasis_cache/toolchain/src/`; source imports
it with paths such as `/.stasis_cache/toolchain/src/stdlib/storage.stasis`. This keeps
CLI, LSP, TUI, and VS Code play on one compiler/stdlib build without checking a dated toolchain
archive into the project.

The project `name` may contain internal ASCII spaces, so display names such as `Chess TD` are
valid; leading or trailing spaces are rejected. Manifest paths must be project-relative and cannot
contain `..`. Generated projects include a
runnable `main()`, a real `.test.stasis` test, an `AGENTS.md` theory-building and semantic-edit
guide, a minimal `CLAUDE.md` that points to `AGENTS.md`, and a version-matched
`PROJECT_ARCHITECTURE.md` with practical input, tick, state, and rendering guidance.
Both `new` and `init` also add language-scoped VS Code settings that recommend the Stasis extension
and enable its canonical formatter on save without changing the formatter for other languages.
`stasis new` also initializes a local Git repository, selects the checked-in `.githooks`
directory, and installs a pre-commit hook. The hook checks formatting, formats noncanonical source
when needed, and blocks
that first commit so the developer can review and stage the changes. It also blocks partially
staged Stasis changes. A retry then commits the canonical source. Git must be available when running
`stasis new`; `stasis init` does not alter an existing repository's hook configuration. After
cloning a generated repository, reactivate the checked-in hook with
`git config --local core.hooksPath .githooks`.

## Commands and outputs

- `new` / `init`: create the manifest and built-in starter template without network access.
- `fmt [--check] [PATH ...]` / `format [--check] [PATH ...]`: apply the canonical Stasis source layout described below.
  `format` is an alias for `fmt`; both emit `fmt` as the canonical JSON command name. The operation
  is idempotent and never follows symlinks. With explicit file or directory paths, formatting works
  without a `stasis.json`; this lets mixed-language repositories enforce Stasis formatting too.
  `format --stdin` reads one unsaved source buffer and writes only canonical source to stdout for
  editor integrations; it does not require a manifest and cannot be combined with `--check`,
  explicit paths, `--workspace`, or `--json`.
- `check`: run the shared frontend and Cranelift JIT compilation path without executing `main`.
- `test [PATH]`: discover project `data/` JSON/CSV pairs with matching `.struct-meta.json` metadata and apply them before running Stasis tests, then run schema-v1 `*.scenario.json` simulation cases. Binding is strict and invalid or missing metadata fails the command; a project without data is unchanged. Each scenario
  starts from fresh `main()`, applies its optional saved state, and restores one bounded runtime
  snapshot before every property seed.
- `run [--headless] [--ticks COUNT] [--fast-forward]`: JIT-compile and execute no-argument
  `main(): i32` or `main(): void`; an `i32` result is the process exit code. Headless execution is
  the default. `--ticks` invokes `tick()` exactly `COUNT` times without calling `render()` or
  loading the graphics runtime. `--fast-forward` makes the no-pacing contract explicit and
  requires a positive tick count.
- `record [ENTRY] --output PATH --width PX --height PX --fps FPS (--frames N|--duration S) [--before-tick FUNCTION]`:
  execute the normal desktop JIT/render path on a hidden fixed-size SDL software presentation.
  An extensionless output path publishes an exact, numbered PNG sequence; an `.mp4` path stages
  those PNGs and the existing mixed game audio, then invokes FFmpeg H.264/yuv420p plus AAC at
  the requested rate. An `.mp3` path stages only the existing mixed game audio and invokes
  FFmpeg `libmp3lame` for a 48 kHz stereo audio-only artifact. Audio is rendered offline as deterministic 48 kHz stereo PCM16 using
  cumulative `floor(frame * 48000 / fps)` sample boundaries; no physical device, microphone, or
  system audio is used. Recording starts after `main()`, uses zero tick sleep, applies the existing
  `--input-script` timeline, and preserves logical-canvas fit/letterboxing. With `--before-tick`,
  the required guest function must be `function name(frame: i32): i32`; it receives zero-based
  frames once after input/live overrides and before tick, render, and capture/mix. Hook state changes
  are visible to the normal tick and render. Dimensions, rates,
  counts, output format, staged frame/WAV validation, encoder failures, and partial-output cleanup
  are bounded and diagnosed. See
  [Deterministic headless recording](headless_recording.md).
- `play [ENTRY]`: launch the graphical hot-swap runtime. Without an entry override, discover the
  nearest ancestor `stasis.json` from the current directory and use its project-relative `entry`
  and display `name`. Explicit entries discover their own ancestor manifest, so project-root
  imports and asset preparation remain anchored to the project even when play starts in `src/`.
  The desktop loop uses `--tick-sleep-us` as an absolute tick interval: input, simulation,
  rendering, and a potentially vsynced present consume that interval, and the host sleeps only for
  the remaining budget. An overrun adds no delay, while a pause of at least one whole interval
  resets the deadline instead of producing a catch-up burst. Passing zero disables this pacing.
- `run --watch`: launch the existing graphical runner and hot-swap pipeline for game projects.
  The window title uses the manifest project name. Because it is an unbounded graphical session,
  watch mode rejects `--json` and `--headless`.
- `tui [ENTRY]`: launch the same entry-relative graphical hot-swap workflow as `play`, using the
  manifest `entry` when no override is supplied and the manifest project name as the window title,
  while a desktop terminal uses
  the runner's versioned LiveSession protocol for background-prepared code-aware symbol edits and
  typed between-tick inspection or mutation. The terminal includes history, a Ctrl-P command
  palette with live fuzzy compiler-backed symbol/member completion, keyboard navigation and
  insertion, Tab completion, paging, multiline cancellation, and concise command-specific output. Use
  `--live-json` for complete schema-v1 response envelopes or `--live-script PATH` for a
  deterministic command script. Editor integrations use `--live-stdio`, which accepts versioned
  requests as JSON lines on stdin and emits only response envelopes on stdout while the graphical
  game and hot-swap watcher remain active. See
  [Interactive live workspace](live_cli_workspace.md).
- `build --mode dev`: compile through JIT and write `build/dev-build.json` as a deterministic
  receipt.
- `build --mode release`: use the shared Cranelift AOT pipeline and write the native executable to
  `build/`.
- `package --target desktop`: create a standalone directory with the AOT executable, manifest,
  assets, graphics runtime when present, and verified release provenance. Windows packages keep
  the game-named executable as the only root file and place all support files under `app/`.
- `package-mobile --target android-arm64|ios-arm64 [--entry PATH]`: atomically assemble the
  shared AOT output, SDL-only runtime, bundled assets, verified provenance, and thin Gradle or
  Xcode app shell.
- `package --target android-arm64|ios-arm64`: compatibility spelling that uses the manifest entry.
- Successful human-readable `build`, `package`, and `package-mobile` commands end with a
  `Completed in ...` line. Durations use milliseconds for sub-second work, seconds for work under
  one minute, and minutes plus seconds for longer builds. JSON output remains deterministic and
  does not include wall-clock timing.
- `inspect [--capacity PATH=COUNT] [--mobile-budget-bytes N]`: compile the manifest entry and
  report the canonical direct-storage model: bytes and alignment by state path/field, struct
  rollups, capacity versus active count, snapshot size, the eight largest pools, recognized
  command buffers, projected capacity-change bytes, and mobile-budget warnings. Repeating
  `--capacity` compares several proposed pool sizes without changing source or runtime state.
  JSON output includes the complete deterministic report; human output emphasizes totals,
  largest pools, projections, and warnings.
- `version` and `env`: report installation, cache, and workspace locations.
- `vendor status`: compare the manifest, checked-in vendor tree, and selected executable.
- `vendor update`: transactionally restore `vendor/stasis` from the selected executable and update
  its manifest identity immediately.

`replay` and `verify` intentionally return deterministic unsupported diagnostics until the replay
runtime contract lands; they do not fake successful behavior.

### Headless scenarios

Scenario files live under the manifest's test directory and end in `.scenario.json`. They are
bounded host descriptions that use the normal JIT compiler, not a second Stasis language or
execution path:

```json
{
  "schema_version": 1,
  "name": "seeded headless simulation",
  "ticks": 4,
  "state_file": "baseline.state.json",
  "state": {"optional_inline_scalar": 3},
  "invariants": [
    {"path": "world.score", "op": "gte", "value": 0},
    {"path": "world.enemies[0].hp", "op": "gt", "value": 0}
  ],
  "property": {"seed_path": "world.seed", "seeds": [1, 7, 42]},
  "expected_hashes": []
}
```

`state_file` is relative to the scenario and contains a JSON object from scalar or indexed state
paths to values. Inline `state` entries are applied after the file and duplicate paths are rejected.
The runtime calls `main()`, applies that saved state, captures one bounded full JIT snapshot, and
restores it before each seed. Every case executes at most 1,000,000 ticks, while one invocation is
preflighted before execution and limited to 1,024 cases and 10,000,000 total ticks. Discovery
rejects links/reparse points and bounds directories, total entries, scenarios, seeds, invariants,
state entries, and source bytes.

Invariants run after every tick using the same typed scalar inspection and comparison operators as
live validation. Optional `expected_hashes` contains one SHA-256 hash per tick and is intended for
same-profile replay regressions. Hash input is compiler-owned scalar/collection layout plus exact
value bits. Host input snapshots, host request mailboxes, and graphics/audio command buffers are
excluded, so presentation extraction cannot change the simulation identity. Ordinary floating
point remains same-target deterministic only; use the Q16.16 intrinsics for cross-architecture
hash claims.

On a failed invariant or hash, `stasis test` writes a bounded
`<output>/headless-replays/*.replay.json` receipt with the scenario path, seed, failing tick,
reason, observed hashes, a quoted rerun command, and exact `rerun_argv`. Receipt names include a
scenario-path digest so distinct files cannot overwrite each other. General input-stream recording and the public
`replay`/`verify` commands remain deliberately reserved for the separate replay-runtime slice.
See `samples/headless_scenario` for an executable fixture.

Official packaging fails if the installed compiler or renderer sources differ from the release
manifest. Without an installed release manifest, source-built toolchains generate optimized local
releases with `build_class: "local_release"` and content-addressed provenance. Pass
`--development-build` to explicitly request visibly labeled development output. See
[Release and package provenance](release_provenance.md).

Add `--json` to receive one stable JSON result object. Usage errors exit 2, command/compile/test
failures exit 1, and successful commands exit 0 except `run`, which preserves the guest's `i32`
exit code. Guest program output may precede the final JSON object for `run`.

### Symbol lookup and references

`stasis --json symbol list` returns compact declaration items in deterministic source order. Its
default scope is the manifest entry and that file's direct imports; use repeated `--file`, `--kind`,
`--owner`, `--query`, `--page`, and `--limit` options to narrow or page the catalog.

`symbol find NAME` and `symbol read NAME` select declaration items by exact semantic name, with
optional kind, file, owner, and signature disambiguation. `find` returns metadata for every match;
`read` requires exactly one match and also returns that declaration's source and source spans.
Global declarations are currently represented by their editable `globals` group rather than as
individually readable declaration items.

`symbol references SYMBOL` has a different contract: it accepts one to eight dot-separated Stasis
identifiers and compiler-lexes the editable project files for matching occurrences. Each result has
an exact UTF-8 byte span and is classified as `definition`, `read`, `write`, or `call`, together with
its containing declaration. Function, struct, and test declaration occurrences are classified as
definitions. Qualified typed field paths—including indexed receivers such as
`state.enemies[0].speed` queried as `state.enemies.speed`—return the declaring struct field plus
their executable reads and writes. The VS Code extension projects this command directly:
**Go to Definition** uses `definition` results, while **Find All References**
uses the full result set and honors VS Code's include-declaration request.

## Source formatting

`stasis fmt` is intentionally opinionated about layout while preserving program structure. It
keeps token and comment text, explicit parentheses, declaration order, and import order unchanged.
Before writing, it verifies that both its lossless token stream and the compiler token stream are
unchanged. It plans every source and test-file rewrite first; a formatting or verification error
therefore leaves the workspace untouched. Files whose formatted bytes already match are never
opened for writing. If a later file write fails, it attempts to restore all files already written.

The canonical rules are:

- Indent with four spaces. Tabs are never emitted.
- Put an opening brace on the declaration or control-flow line. Every braced body is multiline,
  including short functions and one-statement branches.
- Put each struct or block-global field, enum member, and semicolon-terminated statement on its own
  line. End every enum member with a comma, including the compiler-optional final comma. Put `else`
  on the same line as the preceding closing brace.
- Use one space around assignment, comparison, arithmetic, and boolean operators. Do not put spaces
  before `:`, `,`, `;`, member access, indexing, or calls; put one space after `:` and `,`.
- Keep adjacent imports together and separate other top-level declarations with one blank line.
  Preserve at most one intentional blank line inside a body, but omit a blank line immediately
  before a closing brace.
- Preserve the file's existing line-ending style, remove trailing whitespace and blank lines at
  end of file, and emit exactly one final newline. LF, CRLF, CR, and mixed line endings are
  accepted.
- Treat 160 columns as a soft limit. When a parenthesized signature, call, or condition would exceed
  it, put its comma-separated items on indented lines without adding trailing commas. Wrapped
  boolean conditions may also break before `&&` and `||`. A comment, string, or other indivisible
  token may exceed 160 columns rather than having its contents changed.

For example:

```stasis
struct Player {
    health: i32;
    active: bool;
}

function update_player(player: Player, damage: i32): void {
    if (damage > 0) {
        player.health -= damage;
    } else {
        player.active = false;
    }
}
```

Use `stasis fmt --check` in CI when formatting differences should fail without modifying files. A
project generated by `stasis new` runs this check before every commit. When it fails, the hook runs
`stasis format` for convenience and still blocks the commit until those changes are reviewed and
staged.

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

Windows graphical launch coverage is defined in
[Windows game launch integration testing](windows_game_launch_testing.md). It exercises `play`,
`run --watch`, `tui`, generated release executables, and packaged desktop executables with real
PNG, SVG, font, tick, and framebuffer assertions.
