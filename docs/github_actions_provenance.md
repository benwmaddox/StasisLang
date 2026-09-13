# GitHub Actions runtime audit

Audited 2026-09-11 for task 408 against
[nightly run 33531651996](https://github.com/benwmaddox/StasisLang/actions/runs/33531651996)
(commit `49e6042d189ba0a500694a056182d77bd7ada601`, conclusion: failure).
That run reported Node 20 actions, DEP0040 (punycode), DEP0169 (url.parse),
DEP0005 (Buffer), and two deprecated npm packages. Its failure is not evidence
that the updated workflows pass.

## Pin policy and provenance

All external workflow actions use full commit SHAs with a reviewed version
comment. `tools/ci/action_versions.json` records the GitHub release-tag commit
and `action.yml` runtime inspected through the GitHub API. The Rust action is
the inspected `stable` branch commit (its composite action defaults to the stable
toolchain). Update the manifest and workflow pins together after reviewing release
notes and inputs. The offline policy check runs in PR CI, reused by nightly,
and in `tools/validate_repo.sh`.

| Action | Previous ref | Selected release | Migration review |
| --- | --- | --- | --- |
| actions/checkout | v7 | [v7.0.1](https://github.com/actions/checkout/releases/tag/v7.0.1) | Same major; escaping and branch whitespace fixes. Existing fetch depth, checkout paths, credentials and source refs retained. |
| actions/upload-artifact | v4 | [v7.0.1](https://github.com/actions/upload-artifact/releases/tag/v7.0.1) | [v6](https://github.com/actions/upload-artifact/releases/tag/v6.0.0) switches to Node 24 and fixes punycode; [v7](https://github.com/actions/upload-artifact/releases/tag/v7.0.0) adds direct uploads. Explicit `archive: true` retains artifact names and zip handling. |
| actions/download-artifact | v8 | [v8.0.1](https://github.com/actions/download-artifact/releases/tag/v8.0.1) | Same major, artifact-name/content-type fixes. Existing paths, patterns and merge behavior retained. |
| actions/setup-node | v4 | [v7.0.0](https://github.com/actions/setup-node/releases/tag/v7.0.0) | [v5](https://github.com/actions/setup-node/releases/tag/v5.0.0) changes runtime and automatic caching; v7 changes ESM dependencies and removes dummy auth-token export. No registry authentication inputs are used. Disable automatic caching explicitly; retain both explicit npm caches and their lockfile paths. |
| actions/setup-python | v5 | [v7.0.0](https://github.com/actions/setup-python/releases/tag/v7.0.0) | Node 24 since v6; v7 removes `pip-install`, which we do not use. Python 3.12 remains selected. |
| gradle/actions/setup-gradle | v4 | [v5.0.2](https://github.com/gradle/actions/releases/tag/v5.0.2) | Node 24 since [v5](https://github.com/gradle/actions/releases/tag/v5.0.0); ESM and dependency updates in 5.0.2. Gradle 8.9 and cache defaults retained. See exception below. |
| reactivecircus/android-emulator-runner | v2 | [v2.38.0](https://github.com/ReactiveCircus/android-emulator-runner/releases/tag/v2.38.0) | Inspected action uses Node 24; release updates build tools to 37.0.0. Keep API 35, x86_64, KVM, emulator options and acceptance scripts. |
| android-actions/setup-android | v3 | [v4.0.1](https://github.com/android-actions/setup-android/releases/tag/v4.0.1) | [v4](https://github.com/android-actions/setup-android/releases/tag/v4.0.0) moves to Node 24 and command-line tools 20.0 (14742923). Explicit NDK installation and Rust targets remain unchanged. |
| ilammy/msvc-dev-cmd | v1 | [v1.13.0](https://github.com/ilammy/msvc-dev-cmd/releases/tag/v1.13.0) | Latest release still uses Node 20. Exception below. |
| dtolnay/rust-toolchain | stable | stable commit in manifest | Composite action, no Node runtime; stable toolchain and requested targets preserved. |

Node 24 actions require runner 2.327.1 or newer. These workflows use GitHub-hosted
Ubuntu, Windows and macOS runners. Explicit build/test Node selections move from
22 to 24. VS Code esbuild `--target=node20` is an emitted-JavaScript compatibility
target for the extension host, not a CI runtime; it remains unchanged.

## Bounded upstream exceptions

- **MSVC setup:** latest release v1.13.0 and upstream HEAD still declare
  `runs.using: node20`. Keep the pinned action and its Windows environment setup
  until upstream ships a Node 24 replacement. Do not force insecure Node 20 or
  suppress runner warnings. This exception is limited to this action by tests.
- **Gradle current-major exception:** [v6.0.0](https://github.com/gradle/actions/releases/tag/v6.0.0)
  changes caching ownership/licensing and removes previous configuration-cache
  support. The [v6.3.0 basic provider](https://github.com/gradle/actions/blob/v6.3.0/docs/setup-gradle.md#basic-caching)
  loses restore keys, cleanup and deduplication. Retain v5.0.2's Node 24 runtime
  and existing caching behavior rather than weakening caching or silently
  adopting commercial terms. Revisit v6 as a separate cache migration.
- **npm packages:** the lockfile already selects cheerio 1.2.0, the latest
  [upstream release](https://github.com/cheeriojs/cheerio/releases/tag/v1.2.0).
  `npm view @vscode/vsce version dependencies --json` returned 3.9.2 with the
  same cheerio and keytar dependencies. The chains remain
  `@vscode/vsce -> cheerio -> encoding-sniffer@0.2.1 -> whatwg-encoding@3.1.1`
  and `@vscode/vsce -> keytar@7.9.0 -> prebuild-install@7.1.3`.
  Registry metadata was checked for these exact versions. A fresh Node 24
  `npm ci` reproduces both warnings. There is no compatible upstream replacement
  in these dependency ranges; avoid overrides that change encoding semantics or
  removing optional native credential support. Revisit when vsce/cheerio/keytar
  change their dependency chains.
- **Buffer deprecation:** the referenced run emits DEP0005 during artifact
  downloads. The selected download-artifact 8.0.1 bundle still contains
  `new Buffer(...)` in upstream dependencies. Latest patch is pinned; a new
  hosted run must determine whether that execution path still warns.
  DEP0040/DEP0169 also need fresh-run confirmation after the action dependency
  upgrades. No warning-suppression variables are added.

## Validation and publication handoff

- `actionlint` 1.7.12: all six workflows pass syntax, expressions and action-input
  validation (`-shellcheck= -pyflakes=`; those optional tools are unavailable).
- Action policy plus Android seam placement, nightly network, PR Cargo and release
  provenance tests: 79 passed.
- Fresh `npm ci --prefix vscode-stasis --no-audit --no-fund`, followed by
  `npm test --prefix vscode-stasis` on Node 24.12.0: typecheck and 11 tests pass.
- `node --test runtime/web/tests/*.test.mjs`: 113 pass.
- Parsed YAML comparison with HEAD confirms all six workflow graphs, permissions,
  matrices, source refs, artifact paths/names/retention, explicit caches and release
  gating are identical except the reviewed pins, Node version, explicit defaults
  and new policy commands. `git diff --check` passes.
- The repository-wide Bash entrypoint could not start in the Windows shell's
  default environment (`dirname` and `python3` unavailable). Focused commands
  above ran directly. No compiler/runtime source changed.

The worker must publish the changes before hosted validation can run. Run PR CI
with slow seams and nightly on the published revision, covering Linux x64,
Windows x64, macOS arm64, Android emulators and mobile support packaging. Confirm
artifact downloads, provenance checks and release gates, and compare warning
annotations against the bounded exceptions above. Hosted lane success is **not
claimed** by local source checks. No branch, commit, push or release was created
by this implementation session.

Visual evidence: not applicable (workflow configuration only).

Theory gained: action execution runtimes and installed Node versions are separate
contracts; the run warned about Node 20 actions despite Node 24 runner execution.
Changing only `node-version` would therefore leave action warnings intact.
