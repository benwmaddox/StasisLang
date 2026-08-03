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
guide, a minimal `CLAUDE.md` that points to `AGENTS.md`, and a version-matched
`PROJECT_ARCHITECTURE.md` with practical input, tick, state, and rendering guidance.
Both `new` and `init` also add language-scoped VS Code settings that recommend the Stasis extension
and enable its canonical formatter on save without changing the formatter for other languages.
`stasis new` also initializes a local Git repository, writes `.gitattributes` to keep `.stasis`
files on CRLF in every checkout, selects the checked-in `.githooks` directory, and installs a
pre-commit hook. The hook checks formatting, formats noncanonical source when needed, and blocks
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
- `test [PATH]`: run Stasis tests in one isolated JIT session.
- `run [--headless]`: JIT-compile and execute no-argument `main(): i32` or `main(): void`; an
  `i32` result is the process exit code. Headless execution is the default.
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
definitions. Qualified state/field paths return their executable reads and writes; their field
declaration is not currently synthesized as a qualified definition. The VS Code extension projects
this command directly: **Go to Definition** uses `definition` results, while **Find All References**
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
- Use Windows CRLF line endings on every platform, remove trailing whitespace and blank lines at
  end of file, and emit exactly one final newline.
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
