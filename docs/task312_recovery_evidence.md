# Task 312 recovery evidence (2026-09-08)

Implementation revision: `096f67fc891d0bbda88a57ca2ba380497b51927f`. Reconciled file contents with available
`origin/main` revision `cd4d3d4a4604303885e3bc3412566395988d8bc0` using a three-way merge against `01c63d3f`.
Git merge metadata could not be written outside the supplied worktree. The result is
an unstaged file reconciliation; no branch, commit, push, PR, or task mutation was made.

The reconciliation preserves URL activation and network lifecycle hooks together,
both Android manifest capabilities, and both documentation entries. A release
source-closure assertion now checks target source membership rather than adjacency.

## Fresh consumer artifacts

Local development artifacts (not a published release) are available at:

- `target\task312-cargo\debug\stasis.exe`: SHA-256 `16429a415defffeadcb1f0511c3a1ce4a0fa40aa7ee803aa1bc482c6caf41dd6`.
- `target\task312-cargo\debug\stasis_dynload.dll`: SHA-256 `4ec748f597f79e458e492e109ddd4fec4cfe6bde965265089cbed6bf79152694`.
- `target\task312-cargo\debug\stasis_graphics.dll`: SHA-256 `f6ff6d2b49582232831a15d01720d4d06f26ab3148267c2f41305792d856d484`.

CLI/runtime build fingerprint: `1a669b23b8b00ab9acf8fe8711f1720c37f5d7dfcee58ea5914f6e3f20e60ea5`.
This fingerprint identifies the reconciled source snapshot used for the binary build;
the later changes are the source-list test and this evidence documentation.
The refreshed CLI reports source_commit=development; the implementation and reconciliation revisions above identify its source ancestry.
`target/task312-editor-info.json` verifies matching CLI/runtime fingerprints.
`target/task312-consumer-result.json` records a real headless JIT run returning 312:
the typed stdlib API returned ignored for HTTPS and invalid for `file:`.
Use the CLI above with this checkout's `src/stdlib/external_url.stasis` for #313.

When rebuilding these development artifacts in PowerShell, pin the matching
runtime explicitly: the default Cargo staging search can otherwise copy an older
installed release DLL beside the CLI.

```powershell
$env:CARGO_TARGET_DIR = Join-Path $PWD 'target/task312-cargo'
$env:STASIS_BUILD_FINGERPRINT = (Get-Content target/task312-source-fingerprint.txt).Trim()
$env:STASIS_RELEASE_ID = 'task312-recovery-20260908'
$env:STASIS_RUNTIME_DLL_PATH = Join-Path $PWD 'target/task312-runtime-msvc/bin/stasis_graphics.dll'
$env:STASIS_RUNTIME_LIBRARY_PATH = $env:STASIS_RUNTIME_DLL_PATH
python tools/cargo_cache.py run -- cargo build -p stasis --bin stasis -p stasis_dynload
target/task312-cargo/debug/stasis.exe editor-info --json
target/task312-cargo/debug/stasis.exe run --workspace target/task312-consumer --headless --json
```

Reuse this fingerprint only for this recorded source snapshot. Source changes
require rebuilding both the CLI and graphics runtime with a new fingerprint.

## Validation

Final retained-workspace recheck: rebuilt the compiler test target and passed all
four `external_url` tests, including actual AOT executable execution and the JIT
held-pointer fixture. The five web URL tests and 20 release-provenance tests passed
again. All three consumer artifact checksums still match the values above, and the
headless consumer again returned `ok: true` with guest result 312. The optional
repository test-binary signer reported no matching certificate; the configured
runner continued and all four tests actually executed successfully.

Recovery handoff recheck: the three artifact SHA-256 values above were verified
against the supplied files. The consumer command again reported `ok: true`, JIT,
headless, and guest exit code 312 (the CLI process itself returns a nonzero code
for this deliberately nonzero guest result). All five external URL web tests,
all 20 release-provenance tests, and the existing native platform-services/mobile
AOT test executables passed again. `git diff --check` passed.
These are local checks; Git publication and remote CI remain worker-owned.
The four compiler tests and three dynload/Android bridge tests passed after
rebuilding. The AOT executable test was additionally rerun with
`STASIS_AOT_SIGN_TOOL` unset: it selected `lld-link.exe`, linked and ran, and
passed. Optional test-runner signing warnings did not prevent test execution.

- Seven focused compiler/dynload/Android bridge tests passed. The AOT executable
  test was rerun with the unavailable optional signer unset and actually linked/run.
- Web: all 118 runtime tests passed, including five URL action cases.
- CLI binary tests: external URL web metadata and shared Android/iOS package assembly passed.
- Fresh native C platform-services and mobile AOT executables passed.
- Android JNI adapter: arm64 API 26 NDK syntax check passed with `-Wall -Wextra -Werror`.
- Fresh desktop SDL/ThorVG runtime built with MSVC; SDL archives matched both pinned hashes.
- Host runtime audit: 953 comparisons; runtime ABI audit: 797 comparisons.
- Release provenance: all 20 tests passed after fixing the source-list assertion.
- Formatting and `git diff --check` passed. No lingering worktree test executables observed.

The exact `tools/validate_repo.sh` was run with Git Bash utilities and a Python 3
alias. Missing VS Code dependencies were restored from the existing local dependency
tree after npm retrieval failed; all 11 VS Code tests passed. All gates through the
release-provenance check passed after its fix; executing the remaining script commands
reached the pre-existing ignored timing tests in `stasis_dynload/src/lib.rs` and
`stasis_compiler/src/backend/program_snapshot.rs`. Those stop the workspace-wide Cargo
phase. Logs: `target/task312-validation.log`, `target/task312-validation-remaining.log`.

Platform limits: Android/iOS package wiring and Android native compilation were
verified; no device browser dispatch or Xcode build is claimed. Web popup behavior
is covered by the deterministic host harness, not a live browser interaction.

Visual evidence: not applicable to this recovery change; no consumer UI was added.
No browser/device media was captured.

Theory gained: URL input authority and network lifecycle state are independent host
resources. The reconciled lifecycle hooks and passing one-shot tests show that both
must transition on backgrounding; another host capability should share the lifecycle
boundary without replacing an existing capability's cleanup.

Good: fresh executable and package checks caught an actual integration assertion.
Bad: optional signing initially made the AOT execution test return early.
Adjustment: record actual executable execution and use set-based source-closure checks.
