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

On Windows, a source checkout can build and install one verified CLI/runtime pair with
`scripts/install_local_toolchain.ps1`. It derives a shared build fingerprint from the clean source
revision, validates `stasis --json editor-info`, runs a bounded headless record smoke, and promotes
the complete staged bundle to `bin` only after validation.

## Create and use a project

```text
stasis new brick_game
cd brick_game
stasis fmt
stasis check
stasis test
stasis run --headless
stasis run --watch
stasis live --live-stdio
stasis build --mode dev
stasis build --mode release
stasis package --target desktop
stasis package-mobile --target android-arm64
stasis package-mobile --target ios-arm64
stasis prepare
stasis inspect
stasis inspect --capacity state.enemies=512
stasis signing status
stasis signing provision
stasis signing sign path\to\artifact.exe
stasis signing verify path\to\artifact.exe
```

`stasis init --name brick_game .` initializes an existing directory. The built-in template copies
the version-matched Stasis sources into `vendor/stasis/stdlib`, so imports remain
offline and project-local. Generated source uses project-root imports such as
`/vendor/stasis/stdlib/stdlib.stasis`, which resolve consistently from nested source files.
Commands discover
`stasis.json` by walking from the selected path toward the filesystem root, so they work from the
project root and nested directories. `--workspace PATH` selects a project explicitly.

On Windows, opening a visible game window minimizes the attached console once by default.
This applies to development/editor sessions and packaged release games. The game and editor
remain visible; restore the console from the taskbar whenever you need its output. Resizing or
reopening the game window does not minimize a console you restored. Windows Terminal may
share one host window across tabs, so minimizing that host also minimizes its other tabs.
Set `STASIS_CONSOLE_START_MINIMIZED=0` before launching to keep the console visible, including
when using a console-based frontend. Headless recording and commands that do not open a visible
game window leave the console alone. `STASIS_WINDOW_START_MINIMIZED` separately controls the
game window.

## Workspace contract

`stasis.json` is versioned and deterministic:

```json
{
  "manifest_version": 1,
  "name": "brick_game",
  "entry": "src/main.stasis",
  "tests": "tests",
  "output": "build",
  "web": {
    "loading_font": "/assets/fonts/display.ttf",
    "viewport": { "width": 1600, "height": 900 }
  },
  "vendor": {
    "stasis": {
      "release_id": "nightly-20260805-123",
      "sha256": "<lowercase SHA-256>",
      "hash_version": 2
    }
  }
}
```

The optional `web.loading_font` value must identify an existing `.ttf`, `.otf`, `.woff`, or
`.woff2` file under the project `assets/` directory. Both `/assets/...` and `assets/...` forms are
accepted; web packaging normalizes them to a package-relative URL for the static loading shell.

The optional `web.viewport` object sets the authored logical game size used when the browser shell
starts. Both dimensions must be integers from 1 through 8192. A Sheep Herder build authored at 1600 by 900 can use
`{"width":1600,"height":900}` to keep its 1600-by-900 world coordinates stable while the browser
uniformly fits and centers that 16:9 view. Projects without this setting keep the 640-by-360
default.

`vendor.stasis.hash_version` versions the hash contract independently of `manifest_version` and
the selected toolchain release. New projects and `stasis vendor update` write version 2. A version 2
digest includes slash-normalized relative paths in deterministic order. Valid UTF-8 files without
NUL bytes using `.stasis`, `.md`, `.json`, or `.svg` normalize CRLF to LF before hashing, so Git's
LF/CRLF checkout conversion does not create a local vendor edit. Other text changes, path
additions/removals, and every binary byte remain significant. Generated and repository
`.gitattributes` use LF by default, keep `.bat`/`.cmd` files CRLF, and mark common binary assets as
non-text; `.editorconfig` requests LF and a final newline.

For backwards compatibility, an absent `hash_version` means version 1: the original raw-byte
digest. Version 1 pins are first checked against the exact raw tree. If line-ending conversion makes
that differ, status accepts the tree only when it has a trusted canonical baseline: either the
selected release has the same release ID and its bundled raw digest matches the recorded pin, or an
audited release-specific alias matches both the release ID and legacy digest. The initial audited
alias is `nightly-20260919-337`, legacy SHA-256
`08c438cdff0536bf416c0717426dee7a68d986728bb941aee757e01543897af4`, canonical-LF SHA-256
`5c58b2908bcbfe113d942734a5a2a76ffaad98f874f7a5c920ce6fa9a972142d`. It was derived from the
61 files in source tag commit `ad00c329a9d9cd9a3c328c7540034c3e30d5b457`, under `src/stdlib` and
`docs/knowledge`, using the version 2 path framing and CRLF normalization. Other legacy pins whose
raw tree differs and whose baseline is unavailable are reported as `legacy_pin_unverified`, not as
confirmed local edits. Read-only symbol queries also fail closed without modifying files. Mutating
project commands retain automatic vendor synchronization and may replace a stale or unverified
snapshot with the selected release; a clean, verified same-release v1 snapshot remains unchanged.
`stasis vendor update` explicitly migrates even a clean v1 pin to version 2. `vendor status` never
rewrites the manifest or snapshot.

`vendor status` reports the recorded hash version/digest, the authenticated expected canonical digest
when available, the actual canonical digest, and the raw digest for legacy pins. An unverified v1 pin
has no expected canonical digest; status keeps `local_changes` false and sets
`legacy_pin_unverified` instead of comparing unlike hash contracts. These are package-level SHA-256
diagnostics, not a file-by-file diff. Review tracked paths with `git diff -- vendor/stasis stasis.json`
and inspect ignored or untracked vendor files with `git status --ignored --short -- vendor/stasis`
before choosing whether to replace the snapshot.

`manifest_version` versions the JSON schema and is independent of the selected toolchain release.
Mutating project commands verify the on-disk tree against the selected executable. A content mismatch
stages its matching public stdlib and internal host-ABI modules together and publishes the vendor tree
and manifest as one rollback-capable transaction. The recorded release ID changes only with that vendor
content; building or selecting an executable with a different release ID does not rewrite an unchanged
snapshot. The content hash still detects rebuilt development executables whose release ID did not change
and repairs edited or missing vendor files. Stasis owns `vendor/stasis`; Git is the review and rollback
mechanism, so synchronization does not prompt. Review and commit the vendor and manifest changes
together with the compiler upgrade.

The local validation commands `stasis check`, `stasis test`, and `stasis record` deliberately use the
selected executable against the existing checked-in snapshot without reconciling `stasis.json` or
`vendor/stasis`. This lets a local checkout validate a project pinned to an older published nightly
while the local compiler is newer, without changing the project's release or CI pin. Run the explicit
`stasis vendor update` command when the project should adopt the selected executable's vendor snapshot.

The semantic symbol queries `list`, `find`, `read`, and `references` are strictly read-only. They
never reconcile, create, or rewrite the project manifest, vendor tree, or toolchain state. If a
tracked vendor snapshot is missing, locally changed, or stale for the selected executable, the
query reports the condition without changing files. For a vendor-backed project, prepare the
workspace with `stasis vendor status`, then run the explicit `stasis vendor update` before retrying
the query. For a project using `"stdlib": "toolchain"`, run the explicit `stasis prepare`
command instead; queries never materialize or repair that cache.

Projects that should always use the standard library shipped with the selected toolchain can add
`"stdlib": "toolchain"`. Run `stasis prepare` to transactionally materialize that exact stdlib and
its matching runtime modules into `.stasis_cache/toolchain/src/`; source imports it with paths such
as `/.stasis_cache/toolchain/src/stdlib/storage.stasis`. Normal mutating workspace commands may
still synchronize this cache before they run, but semantic symbol queries require the explicit
preparation and never write it. This keeps
CLI, LSP, TUI, and VS Code play on one compiler/stdlib build without checking a dated toolchain
archive into the project.

The project `name` may contain internal ASCII spaces, so display names such as `Chess TD` are
valid; leading or trailing spaces are rejected. Manifest paths must be project-relative and cannot
contain `..`. Generated projects include a
runnable `main()`, a real `.test.stasis` test, an `AGENTS.md` theory-building, semantic-edit, and container-derived UI geometry
guide (sourced from `docs/agent_workflow.md`), a minimal `CLAUDE.md` that points to `AGENTS.md`, and a version-matched
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

Release archives ship the offline knowledge page
[`docs/knowledge/loading-screens.md`](knowledge/loading-screens.md). `stasis new` and
`stasis vendor update` install the matching copy at
`vendor/stasis/docs/loading-screens.md`, so generated projects can discover the asset-IO
loading-state guidance without network access.

### Generated GitHub Actions

`stasis new NAME` adds `.github/workflows/stasis-pr.yml`,
`.github/workflows/stasis-weekly.yml`, `.github/workflows/stasis-quarterly.yml`, and the
PowerShell restore, resolution, pin-update, and pre-PR validation helpers under `tools/`.
`stasis init` does not add them. Conflicting or linked workflow/tool directories fail preflight
without a partial scaffold. Generated `.gitignore` entries exclude restored toolchains, build and
package output, and validation receipts while keeping `vendor/stasis` checked in.

The PR workflow runs for every pull request without path filters and also supports
`workflow_dispatch`. It checks out the exact contributor head, emits a required relevant-change
sentinel in one Ubuntu job, and keeps unrelated changes to that single billed job. Relevant changes validate the checked-in immutable
`stasis.json` release and lowercase vendor SHA-256 pin, restore that exact release, verify the
vendored snapshot, and run only `stasis fmt --check` and `stasis check`. The broad local
`tools/validate-before-pr.ps1` pass accepts an explicit restored Stasis executable, proves its
release/checksum identity matches the checked-in pin, runs vendor status, format, check, test, and
desktop package once, and writes both a machine-readable JSON receipt and a Markdown PR summary.

The scheduled Friday and manually dispatched weekly workflow consumes only the checked-in
immutable Stasis pin. It compares that pin to the last published game release's
`BUILD-MANIFEST.json` and compares game changes since that release, ignoring only the
`vendor.stasis` portion of `stasis.json`; true no-ops skip all matrices. Actual releases run the
full Linux, Windows, and macOS desktop matrix, archive the packages, and publish a prerelease with
an immutable build manifest and checksums. A quarterly pin PR is therefore the only path that
advances the Stasis release used by this matrix.

The quarterly workflow runs on the exact first day of January, April, July, or October. Every
eligible run resolves the newest complete release once, restores it, and mechanically updates the
pin and vendor snapshot; an unchanged pin simply skips the update PR. A stale pin is committed as
`stasis.json` and `vendor/stasis` on a dedicated automation branch, opened as a PR, and merged
without a game compatibility gate. A dispatched pin-only sentinel supplies the stable required
status for repositories with branch protection, but deliberately runs no game commands. The normal
weekly workflow then sees the changed pin and owns
the authoritative all-target release matrix, including surfacing any incompatibility with the new
Stasis release. The quarterly workflow never pushes the default branch directly.

`stasis.json` records the immutable release identity and hash of the checked-in `vendor/stasis`
snapshot; generated automation treats that pair as one release contract.

All templates are embedded in `stasis`, so project creation itself remains offline. Only the
generated Actions jobs access the public `benwmaddox/StasisLang` GitHub releases to resolve and
restore release assets.

## Commands and outputs

- `new` / `init`: create the manifest and built-in starter template without network access. `new`
  also generates GitHub Actions; `init` leaves existing repository automation unchanged.
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
- `replay RECORDING [--entry ENTRY] [--tick-sleep-us N]`: validate the recording identity,
  rebuild each complete HostFrame from sparse exact-bit changes, execute the normal JIT `tick()`
  and `render()` entries, and stop at the first simulation-state hash divergence. `play` accepts
  `--record-replay PATH` and `--replay PATH`; `record` accepts the same session modes so a replay
  can be rendered directly to PNG or MP4. See [Record and replay](record_replay.md).
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
- `live [ENTRY] --live-stdio`: run the graphical hot-swap workflow with versioned JSON-line
  requests on stdin and response envelopes on stdout. `--live-script PATH` instead executes
  a deterministic command script; add `--live-json` for complete response envelopes.
  The manifest supplies the default entry and project window title. See
  [Live workspace protocol](live_cli_workspace.md).
- `build --mode dev`: compile through JIT and write `build/dev-build.json` as a deterministic
  receipt.
- `build --mode release`: use the shared Cranelift AOT pipeline and write the native executable to
  `build/`. On Windows, `--signing required` makes unavailable signer/certificate configuration
  fail before the artifact is accepted; `auto` preserves optional legacy hook behavior.
- `signing status|provision|sign|verify`: inspect Windows signer discovery, explicitly provision a
  CurrentUser-only development certificate, sign explicit executable/toolchain paths, or verify
  Authenticode signatures. These commands do not require `stasis.json`; `provision` is never a
  production credential path. Local provisioning trusts the public certificate only in CurrentUser Root.
  Stasis-controlled signing requests SHA-256 digests with page hashes for EXEs and without them
  for DLLs, then verifies Authenticode and signer identity. See [Windows test signing](windows-app-control.md). The explicit sign/verify operations require a Windows host; `STASIS_AOT_SIGN_TOOL`
  remains supported as a one-argument external hook for existing cross-platform build flows.
- `package --target desktop`: create a standalone directory with the AOT executable, manifest,
  assets, graphics runtime when present, and verified release provenance. Windows packages keep
  the game-named executable as the only root file and place all support files under `app/`.
- `package-mobile --target android-arm64|ios-arm64 [--entry PATH]`: atomically assemble the
  shared AOT output, SDL-only runtime, bundled assets, verified provenance, and thin Gradle or
  Xcode app shell. Network-enabled iOS packages require macOS/Xcode, stage and link the
  `stasis_network` arm64 static library, and include the local-network privacy declaration;
  direct TCP/unicast does not require Bonjour discovery entitlements. Official archives resolve
  prebuilt network libraries from `mobile/network/<target>/` beside the installed executable;
  nightly archives contain all Android arm64/x86_64 and iOS arm64 support libraries, while source
  checkouts may build them from the workspace as a development fallback.
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
- `prepare`: for `"stdlib": "toolchain"` projects, transactionally materialize the selected
  toolchain stdlib into `.stasis_cache/toolchain/src/`; otherwise report that no preparation is
  needed. It never enables vendor mode.

`verify` remains reserved for a future non-presenting batch verifier. `replay` performs verification
while presenting every reconstructed tick.

Validation commands (`check`, `test`, and `record`) and formatting checks and writes leave
`stasis.json` and `vendor/stasis` unchanged, even when the selected toolchain differs from the
project's vendor pin. Generated commit hooks
format explicit `src` and `tests` paths so older formatters also avoid workspace synchronization.
Release changes remain separate from formatting.

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

`stasis --json symbol list` returns compact items in deterministic source order. Default results
cover the manifest entry and its direct imports. The result's `result.imports` map reports dependencies for
each selected file. An explicit `--file` selects that file without expanding its imports; follow the
map with additional file selections when needed.

Use repeated `--file` on `list`, plus `--kind`, `--owner`, and `--query` to scope
results. `--query` matches a substring of name or signature. `--page` is zero-based and defaults
to 0; `--limit` defaults to 32 and is capped at 200. List output omits `imports` and empty
`globals` items. Read those groups directly, for example
`symbol read imports --kind imports --file src/main.stasis`.

`symbol find NAME` matches the exact name across the loaded workspace and returns every match.
`symbol read NAME` requires exactly one match and returns its full source, source spans,
`symbol_id`, and `source_hash`. Both accept one `--file`, with `--kind`, `--owner`,
and `--signature` available for disambiguation. Global declarations are represented by their
editable `globals` group.

`symbol references SYMBOL` accepts one to eight dot-separated Stasis identifiers. It defaults to
128 results and is capped at 256; there is no file filter or paging. Treat a response at the cap as
potentially truncated and supplement it with targeted `rg` searches. Each result has an exact UTF-8
byte span and is classified as `definition`, `read`, `write`, or `call`. Qualified typed
field paths—including indexed receivers such as `state.enemies[0].speed`—return the declaring
struct field and its executable reads and writes; query them without indexes, such as
`state.enemies.speed`.

The VS Code extension projects this command directly: **Go to Definition** uses `definition`
results, while **Find All References** uses the full result set and honors VS Code's
include-declaration request. For edit schema, dry-run, receipt, and recovery behavior, see
[Semantic Edit Protocol](semantic_edit_protocol.md) and
[Semantic edit and validation](knowledge/semantic-edit-and-validation.md).

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
project generated by `stasis new` runs `stasis format` before every commit so canonical formatting
is enforced instead of merely reported. The hook still blocks the commit until any formatter changes
are reviewed and staged.

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
`run --watch`, `live`, generated release executables, and packaged desktop executables with real
PNG, SVG, font, tick, and framebuffer assertions.

## Release callback reachability

Desktop packages (Windows, Linux, macOS), Android arm64/x86_64 and iOS arm64 AOT
bundles, and release Web packages omit the implicit `on_code_swap` root and host
binding. LAN browser guests follow the Web package policy. A function reached by an
ordinary source call is preserved by its resolved function identity, including a
function named `on_code_swap`; its name does not create a release host export.
Shared dependencies and startup/frame/graphics construction roots remain reachable.
Asset manifests and inferred static assets use the same release snapshot.

Live JIT and live AOT swaps retain development reachability. Explicit Web
`--development-build` output also retains the callback and its dependencies.
Desktop/mobile `--development-build` labels provenance, signing and diagnostic
behavior; these standalone AOT shells do not implement code swapping, so their
callback policy remains release. Android x86_64 remains a development-only package
target, using that same standalone AOT policy. Pause/resume and graphics resource
restoration do not invoke the code-swap callback.

The compiler exposes `ReachabilityPolicy` separately from optimization and debug
symbols. Low-level AOT/Wasm processes retain their development-compatible default;
package boundaries select the appropriate policy explicitly. Snapshot revisions
include this policy, and changing it recomputes reachability and active artifacts.
