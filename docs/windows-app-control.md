# Windows local test signing

Local test signing is separate from production signing. The repository policy is
`tools/windows/stasis-signing.ps1`; no personal-tools script is required.
Prerequisites are Windows PowerShell, Python, Rust/MSVC, and Windows SDK `signtool.exe`.
A Windows Application Control policy must permit the selected certificate. A valid
Authenticode signature alone does not grant permission under every WDAC policy.

## Configure explicitly before building

Inspect the signer and certificate selection from the checkout, without building Stasis:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File tools/windows/stasis-signing.ps1 status
```

Remove a legacy hook from the current shell when migrating to repository signing, then
explicitly provision local test signing:

```powershell
Remove-Item Env:STASIS_AOT_SIGN_TOOL -ErrorAction SilentlyContinue
powershell -NoProfile -ExecutionPolicy Bypass -File tools/windows/stasis-signing.ps1 provision
$env:STASIS_SIGNING_MODE = 'required'
python tools/windows/test_signing_acceptance.py --target-dir target/signing-acceptance
python tools/cargo_cache.py run -- cargo test -p stasis_compiler --lib
```

The acceptance command verifies the native launcher with a 9,000-character argument,
then builds a fresh dependency-free Cargo fixture in a path containing spaces. Its
build script, proc macro, and executable must all sign and verify before use; the
executable asserts that the build script and macro ran. It also verifies all three
emitted signatures explicitly. Run provisioning and validation as the same current
user; a sandbox account does not inherit another user's certificate stores.

Provisioning reuses a suitable development certificate or creates a non-exportable
code-signing key in `Cert:\CurrentUser\My`. It trusts only the public certificate in
`Cert:\CurrentUser\Root`; it does not modify LocalMachine stores or WDAC policy.
The command reports the exact changes. Do not provision under a different execution
account from the account running builds. The thumbprint record defaults to
`%LOCALAPPDATA%\Stasis\signing\development-thumbprint.txt`;
`STASIS_SIGNING_LOCAL_RECORD` can select another record. No private key is exported.
Retain the provisioning report to identify the exact certificate/store additions if
removing this local setup later; do not remove a reused certificate or unrelated trust.

`STASIS_SIGNING_CERT_THUMBPRINT` selects an existing CurrentUser code-signing certificate.
Missing, expired, unsuitable, or inaccessible certificates fail with a diagnostic.
Trust is changed only by the explicit provision command, never by status, sign, or builds.

To compare an already-provisioned signer without changing persistent configuration:

```powershell
Remove-Item Env:STASIS_AOT_SIGN_TOOL -ErrorAction SilentlyContinue
Remove-Item Env:STASIS_SIGNING_CERTIFICATE -ErrorAction SilentlyContinue
$env:STASIS_SIGNING_CERT_THUMBPRINT = '<existing current-user certificate thumbprint>'
$env:STASIS_SIGNING_MODE = 'required'
powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File tools/windows/stasis-signing.ps1 status
python tools/windows/test_signing_acceptance.py --target-dir target/signing-acceptance-selected
```

These settings affect only the current shell and its children; close the shell to
restore the prior selection. Verify actual execution as well as signature validity.
For this workstation's recovery, any separately supplied certificate material must
be stored in and retrieved from the user-directed OneDrive location, never the
checkout or build output. Do not export the provisioner's non-exportable key, copy
certificate contents into diagnostics, or log passwords. An existing usable store
certificate needs no material retrieval or export. OneDrive storage does not itself
install a certificate or authorize it under Application Control.

## Signing policy

Stasis-controlled Authenticode signing always requests a SHA-256 file digest and page hashes.
Production credentials are supplied externally with `STASIS_SIGNING_CERT_THUMBPRINT` or
`STASIS_SIGNING_CERTIFICATE`; Stasis never generates, exports, prints, or logs private keys.
The nightly currently uses a pinned, self-signed Maddox Labs release identity. Its Windows job
does not install that certificate into the runner's trust stores. It verifies the pinned signer
identity before packaging and performs strict Authenticode verification afterward, with only the
narrow expected self-signed root-trust failure bridged in production verification. This proves
artifact integrity and stable private publisher identity in CI, but it does not confer public CA
trust or SmartScreen reputation on downloaded binaries.

## Canonical validation and executable boundaries

Run `bash -l tools/validate_repo.sh` from Git Bash on Windows. The canonical validator
uses the configured signer through the repository Cargo wrapper and test runner.
If overriding `CARGO_TARGET_DIR` for Windows CMD execution, use native backslash
separators (for example, `D:\checkout\target\validation`). A forward-slash override
can make CMD interpret Cargo's relative executable path as a command switch.
Run native C integration checks with the MSVC developer environment loaded. Ensure
MSVC's bin directory precedes Git's `usr/bin` after Bash startup: Git supplies an
unrelated `link.exe` that cannot link Rust/MSVC outputs. Automatic MSVC environment
selection is tracked separately from signer configuration.
Keep `STASIS_SIGNING_MODE=required` (or `STASIS_REQUIRE_SIGNED_EXECUTION=1`) set on a
machine that requires signatures. Required signing failures stop execution. An
unconfigured environment that does not require signing retains unsigned operation.
Once Cargo signing is configured, a signing failure stops the build even for an
optional legacy hook; unset stale signing configuration to use an unsigned environment.
The standalone Cargo test runner retains optional-hook compatibility, but both required
settings always prevent launch after a signing failure.

The sequence is emit, sign, verify, then execute/load. EXEs use `/fd SHA256 /ph`;
DLLs use `/fd SHA256 /nph`. Verification requires Authenticode `Status=Valid` and the
selected certificate; the pinned self-signed production bridge accepts only the expected
untrusted-root diagnostic. Timestamp authority retries are bounded and never execute an
unsigned fixture.

| Boundary | Integration |
| --- | --- |
| Cargo build scripts and proc-macro/helper DLLs | `tools/cargo_cache.py run -- cargo ...` installs the repository rustc wrapper; its native launcher is signed before Cargo starts, and emitted Windows binaries are signed before rustc returns to Cargo. |
| Compiler/app test executables | `.cargo/stasis-sign-and-run.cmd` invokes the repository policy before launch. |
| Staged graphics runtime DLL | The app build script signs and verifies its staged copy before the app can load it. |
| Compiler AOT executable/DLL fixtures and app AOT outputs | Rust invokes the same PowerShell policy before execution/load. |
| Canonical C audio test | Validator signs the emitted Windows executable before launch. |
| Native HostFrame compiler fixture | Uses the same repository signing adapter after C compilation and before execution. |
| Workshop Rust bridge and phone-native Codex host dependencies | Both build scripts route Cargo through the repository wrapper. Android ELF libraries are not Authenticode inputs. |
| Restored Stasis-owned Windows tools | Explicitly sign and verify the restored file before its first launch using the commands below. The Workshop debug build does not launch a restored `stasis.exe`. |
| SDK/JDK/NDK/Gradle tools | Externally installed prerequisites retain vendor signatures and policy; local signing does not rewrite them. |
| Workshop APK | Gradle debug APK signing is Android signing, verified separately with `apksigner`. |

Do not reuse unsigned cached host dependencies for a signed acceptance run. Select a
fresh target directory inside the checkout before the first build. Custom rustc wrappers
must not bypass the repository signing wrapper.

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File tools/windows/stasis-signing.ps1 sign -Artifact '.\path with spaces\stasis.exe'
powershell -NoProfile -ExecutionPolicy Bypass -File tools/windows/stasis-signing.ps1 verify -Artifact '.\path with spaces\stasis.exe'
$env:CARGO_TARGET_DIR = Join-Path $PWD 'target\signed-workshop'
powershell -NoProfile -ExecutionPolicy Bypass -File mobile/android/build_debug.ps1 -NoGradleDaemon
# Use the installed Android build-tools version:
& "$env:ANDROID_HOME\build-tools\<version>\apksigner.bat" verify --verbose mobile/android/app/build/outputs/apk/workshop/debug/app-workshop-debug.apk
```

A clean supported WDAC acceptance run must record the execution account, provision
report, fresh target path, successful host build-script execution after verified
signing, compiler/app/AOT test results, and APK verification. If WDAC still denies a
verified artifact, retain the failing path and CodeIntegrity event; do not disable
policy, add exclusions, or silently skip the executable test.

## CI and production

CI supplies its own certificate configuration. Production profiles
(`STASIS_SIGNING_PROFILE=production` or `STASIS_SIGNING_MODE=production`) ignore the
local development record and refuse local provisioning. Production release credentials
and policy are managed separately; this workflow does not create or rotate them.
Do not commit PFX files, private keys, or certificate passwords. Local test signing uses a store thumbprint and requires no password. Do not put
passwords in shell history, logs, test evidence, or artifacts. Existing PFX-based
release compatibility remains separate: signtool receives its password as a process
argument, so prefer an externally provisioned store certificate for CI.

`STASIS_AOT_SIGN_TOOL` remains a compatibility hook accepting exactly one artifact
path for direct signing callers. Windows automation through `tools/cargo_cache.py`
(including pre-commit) and `tools/validate_repo.sh` removes this inherited hook in
local profiles and requires repository signing instead. Configure the build
account's certificate with `STASIS_SIGNING_CERT_THUMBPRINT` or the provision command;
an obsolete hook alone now produces an actionable missing-certificate error.
Production mode/profile preserves its explicit signer configuration.

The hook must return success only after signing; the repository policy additionally
requires a valid Authenticode chain and checks the selected identity when certificate
configuration is supplied. Otherwise the hook owns certificate selection. The wrapper supplies
`STASIS_SIGN_PAGE_HASHES=1` for EXEs and clears it for DLLs, restoring the previous
value afterward; legacy hooks must honor this page-hash contract. Prefer SDK discovery or an explicit `signtool.exe` path instead
of a personal hook. No canonical workflow requires `D:\code\Tools\Stasis`.

## Recovery validation evidence (2026-09-07)

The supported Windows run used account `mad-bee\ben`, cleared the legacy hook in
the validation process, and discovered Windows SDK 10.0.26100.0 `signtool.exe`.
Explicit provisioning reused development certificate
`C6FA078B7BEC95E40338784709E49A55AAD00335`; CurrentUser\Root trust was already
present and the selection record was unchanged. No certificate/store changes or
key exports occurred. The restricted worker account could not provision a key
(CertEnroll 0x80070002); validation ran under the provisioned user instead.

- `python tools/windows/test_signing_acceptance.py --target-dir target/signing-acceptance`
  passed with a freshly compiled launcher, build script, proc macro, and executable.
  Authenticode identity/trust verification passed before execution.
- Windows Python checks: 42 passed. Cargo cache/policy checks: 16 passed.
- `python tools/cargo_cache.py run -- cargo test -p stasis --lib windows_signing::tests -- --test-threads=1`:
  9 passed.
- `python tools/cargo_cache.py run -- cargo test -p stasis_compiler --lib backend::aot::tests -- --test-threads=1`:
  68 passed, including linked executable behavior. These Rust checks used
  `target/signed-validation-recovery` and required signing.
- `mobile/android/build_debug.ps1 -NoGradleDaemon` passed with required signing
  and fresh bridge target `target/signed-workshop-recovery`. Both bridge ABIs and
  both native Codex libraries were packaged. Native Codex and Gradle retained
  existing dependency caches; this was not a newly imaged machine.
- Android build-tools 36.0.0 `apksigner verify --verbose` verified the resulting
  `mobile/android/app/build/outputs/apk/workshop/debug/app-workshop-debug.apk`
  with one signer and APK signature scheme v2. SHA256:
  `9EC28E30EF85DD758EE4F2327631DCCC9D1A5817BE238C0611096F3DE9C6BCBC`.

### Final recovery recheck

The resumed run again used `mad-bee\ben`, cleared `STASIS_AOT_SIGN_TOOL`,
and selected the repository SDK signer and existing development certificate.
It made no certificate-store, credential, or Application Control policy changes.

- All 56 focused Windows signing/Cargo Python tests passed.
- Fresh `target/signing-acceptance-final` acceptance passed for the native
  launcher, 9,000-character argument, Cargo build script, proc macro, and EXE.
- All 52 app compiler-backend tests and all 68 compiler AOT tests passed with
  required signing, including linked executable assertions.
- The Workshop build command above passed again. `apksigner verify --verbose`
  verified v2 signing with one signer; its APK SHA256 remained the value above.
- Rust formatting and diff checks passed. No lingering task test processes
  were found after validation.

Canonical validation now runs both formerly ignored timing checks by default.
The checks completed within two seconds of test execution. Fake-linker tests
isolate ambient signing configuration, and a marker-only hook must fail Windows
Authenticode verification. Windows source assertions normalize CRLF and no longer
require a particular checkout name. The asynchronous live-request test helper
uses a bounded 30-second wall-clock deadline after a full-suite polling timeout;
the rollback test passed in isolation and in the subsequent full app suite.

`target/task278-canonical-complete.log` records passing architecture/policy gates,
294 app library tests, and 363 CLI tests. The full validator remains incomplete:
the desktop hot-swap seam first required a matching CLI/runtime fingerprint.
The documented CMake/vcpkg fallback built a fresh runtime and runner under
`target/task278-runtime`, using `tools/compute_toolchain_fingerprint.py` and
`local-task278` as the local release ID. Both artifacts were signed and verified
before the seam retry, and the CLI was rebuilt with that same fingerprint.

The retry command was:

```powershell
python tools/cargo_cache.py run -- cargo test -p stasis --test desktop_hot_swap_generation_seam -- --test-threads=1
```

It passed three tests but failed the live runtime case: Application Control denied
loading `target/signed-validation-recovery/debug/stasis_graphics.dll` with OS error
4551. `Get-AuthenticodeSignature` reported `Valid` and the expected development
thumbprint `C6FA078B7BEC95E40338784709E49A55AAD00335` for that exact staged DLL.
CodeIntegrity event 3077 at `2026-09-07T23:30:03.0308184Z` identifies policy
`{0283ac0f-fff1-49ae-ada1-8a933130cad6}` and unmet Enterprise signing-level
requirements. Event 3118 also records Smart App Control blocking. Non-secret event
data is retained in `target/task278-runtime-policy.json`.

Current-user root trust is therefore sufficient for the successful signing
fixtures on this account, but does not authorize every native runtime artifact
under this machine's policy. Completing canonical desktop validation requires a
policy-authorized signing environment; do not disable policy or skip this test.
No physical-device or visual acceptance was added to the signing task.

Visual evidence: not applicable; this change concerns build and signing behavior.
Theory gained: valid Authenticode trust and Application Control authorization are
separate checks. Fresh Cargo/AOT fixtures executed, while the valid runtime DLL
was denied by a named policy; another native DLL can encounter the same gate.
Good: fresh acceptance proved emit/sign/verify/execute ordering with no legacy hook.
Bad: canonical validation exposed stale test assumptions and an additional native
runtime policy gate after focused signing checks passed.
Adjustment: retain both executable fixture evidence and exact CodeIntegrity events
when validating local signing; do not equate a valid signature with policy approval.

### Recovery recheck (2026-09-08)

The retained source changes were preserved. Prior generated runtime and event files
were absent, so the September 7 results above are historical evidence. This run
again used the already-provisioned `mad-bee\ben` account with the legacy hook
cleared. SDK discovery and certificate selection succeeded without provisioning,
store changes, key exports, or policy changes.

- All 56 focused signing/Cargo Python tests passed (49 signing/cache checks and
  seven Cargo CI policy checks). Rust formatting and diff checks passed.
- Fresh `target/signing-acceptance-resume` acceptance passed for the signed native
  launcher, 9,000-character argument, Cargo build script, proc macro, and EXE.
- All nine app signing tests passed with required signing.
- All 68 compiler AOT tests passed with required signing, including linked
  executable behavior (84.19 seconds of test execution).
- `mobile/android/build_debug.ps1 -NoGradleDaemon` passed within the 900-second
  command limit with required signing and the legacy hook cleared. Both Rust
  bridge and native Codex ABIs were built and packaged; Gradle executed 40 tasks.
  Android build-tools 36.0.0 `apksigner verify --verbose` verified the fresh
  Workshop debug APK with one signer and APK Signature Scheme v2. SHA256:
  `9A0F1DE03E06269846AFFAED2966EDDE92EFB41CD7B0ADFB717EFD495D287BA6`.
  Build output is retained in `target/task278-workshop-resume.log`. No task-owned
  test/compiler processes remained after verification.
- `bash -l tools/validate_repo.sh` failed at the shared runner characterization
  build. The exact retry through `tools/cargo_cache.py` failed identically:
  `target/debug/build/proc-macro2-ec19e076f019f180/build-script-build.exe` was
  never executed because Application Control returned OS error 4551.
- The repository `verify` command succeeded for that exact EXE. Its Authenticode
  status was `Valid` with the development thumbprint recorded above.
  CodeIntegrity events 3077 and 3033 identify unmet Enterprise signing-level
  requirements and policy `{0283ac0f-fff1-49ae-ada1-8a933130cad6}`. Current evidence
  is in `target/task278-canonical-resume.log`, `target/task278-policy-resume.json`,
  and `target/task278-signature-resume.json`.
- The fresh CMake/vcpkg runtime fallback configured, but linking failed because
  the installed SDL library lacks `SDL_SetDefaultTextureScaleMode`. This run
  therefore did not reproduce the historical runtime DLL load itself.

The current canonical blocker is a verified policy denial of a generated build
script, not missing certificate enrollment. A valid local chain does not grant
authorization under that policy; validation must not bypass it.

### Explicit signer comparison (2026-09-09 UTC)

The resumed run used `mad-bee\ben`, cleared the legacy hook and certificate-file
selection, and explicitly selected existing certificate
`F177D9D4A26CB965240947FDF14620C8929BEC88` with required signing. No certificate
stores, selection records, keys, or Application Control policies were changed.
No certificate material needed retrieval from OneDrive.

- All 56 focused signing/cache/policy Python tests passed, as did Rust formatting.
- Fresh `target/signing-acceptance-new-signer` acceptance passed for the native
  launcher, long argument, Cargo build script, proc macro, and EXE.
- The formerly denied `proc-macro2-ec19e076f019f180/build-script-build.exe`
  verified with the selected identity and executed during canonical validation.
- A fresh CMake runtime built with pinned bundled SDL 3.4.10 and SDL_image 3.4.4.
  Its signature was `Valid` with the selected identity. With matching CLI/runtime
  fingerprints, all four desktop hot-swap tests and the desktop input test passed.
- Canonical validation exposed an outdated integer sprite handle and relative
  screenshot path in the manifest-assets fixture. The fixture now uses `SpriteRef`
  and resolves its evidence path before changing directories. Its exact test passed.
- With the MSVC developer environment loaded, the generated mobile AOT executable
  and packaged-assets tests passed after adding their required
  `stasis_platform_services.c` link input.
- The native HostFrame fixture now supplies MSVC source/output filenames relative
  to its working directory and signs/verifies its generated executable before
  launch. Its exact test passed. The full workspace run also identified two stale
  app fixtures: a missing toolchain-stdlib manifest setting and a missing explicit
  network-send length; both fixtures were aligned with the current public API.
- `mobile/android/build_debug.ps1 -NoGradleDaemon` built both bridge and native
  Codex ABIs and executed all 40 Gradle tasks. SDK 36.0.0 `apksigner verify --verbose`
  verified v2 signing with one signer. APK SHA256:
  `E2697385F7AD7DAE8CA961EFA9406ADD8319E1D6A711B68462512C51D9FC75DD`.
  Log: `target/task278-workshop-new-signer.log`. This used an existing provisioned
  workstation and dependency caches, not a newly imaged machine.

Final validation used bounded commands rather than repeating the whole slow launch
matrix after each fixture edit. Canonical architecture and policy gates passed in
`target/task278-canonical-msvc-final.log`. The full
`cargo test --workspace --all-targets --no-fail-fast -- --test-threads=1` run through
`tools/cargo_cache.py` completed within 900 seconds; all targets passed except the
three fixture failures described above (`target/task278-workspace-final.log`).
After repairs, the complete CLI integration target passed 34 tests, the complete
Windows launch target passed seven tests (361.97 seconds), and the native HostFrame
target passed its test. Logs: `target/task278-app-integration-final.log` and
`target/task278-host-frame-final.log`. Thus all observed failing targets were
retested successfully; the original full-run log intentionally retains its failures.
Rust formatting and `git diff --check` passed. No tests were disabled to obtain
these results.

Visual evidence: inspected `target/task278-assets-preview.png`, a reduced preview
of the manifest-assets test capture, showing the magenta sprite and yellow/cyan
text regions. No product UI change or physical-device acceptance is part of this work.
Theory gained: signer identity matters independently of root trust; the newer
certificate allowed both the formerly denied Cargo EXE and a fresh runtime DLL to
execute. Another machine's policy still requires its own execution verification.
Good: the explicit signer comparison recovered real native execution without policy changes.
Bad: canonical validation revealed stale fixture API and working-directory assumptions.
Adjustment: retain fresh executable and DLL execution evidence alongside signer identity.

### Retained-workspace handoff recheck

The provisioned build account again passed fresh signing acceptance in
`target/signing-acceptance-final-recheck` with the legacy hook and certificate-file
override cleared and the newer thumbprint selected in required mode. All 58 focused
Windows signing, Cargo cache, and Cargo policy tests passed. Cargo formatting and
diff checks passed; no task-owned compiler/test processes remained. No trust stores,
keys, persistent signer selection, or Application Control policies were changed.
The broader workspace/runtime/Workshop results above are retained validation notes;
their generated logs were absent in this handoff and were not regenerated.

Visual evidence: not applicable to this signing recheck; the prior fixture preview
inspection is recorded above and was not repeated.

The subsequent handoff independently repeated acceptance in
`target/signing-acceptance-current-handoff`: the native launcher, 9,000-character
argument, fresh Cargo build script, proc macro DLL, and executable all passed with
the explicitly selected newer signer and required signing. All 58 focused Python
tests passed. Formatting passed under the provisioned build account; the restricted
account correctly failed before Cargo launch because its certificate store lacks
the signer. No task-owned test/compiler processes remained. Broader validation
above remains retained evidence, not a new full-suite run.

### Canonical signer selection recovery (2026-09-09)

Windows automation now removes an inherited local compatibility hook while keeping
signing required. Production profiles retain their configured signer. All 60
focused signing/cache/policy Python tests passed, and validator shell syntax and
diff checks passed.

Under `mad-bee\ben`, with `STASIS_SIGNING_CERT_THUMBPRINT` explicitly set to
`F177D9D4A26CB965240947FDF14620C8929BEC88`, required mode enabled, and the
certificate-file override cleared, the exact `.githooks/pre-commit.ps1` command
passed. The parent environment still contained the legacy hook, proving Cargo
replaced it before signing. Publication must use this provisioned build account
and explicit certificate selection; the restricted account lacks the certificate.

Fresh `target/signing-acceptance-repository-selection` acceptance passed for the
native launcher, 9,000-character argument, Cargo build script, proc macro DLL,
and executable. No persistent selection, certificate stores, keys, or policy were
changed; process-local environment settings were discarded after validation.
No task-owned compiler/test processes remained. Broader compiler/runtime/Workshop
results above are retained evidence and were not repeated for this selection fix.

Visual evidence: not applicable; this recovery changes build signer selection.

The September 9 handoff independently repeated all 60 focused Python checks and
fresh acceptance in `target/signing-acceptance-sept9-recheck`. The exact
`powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File .githooks/pre-commit.ps1`
command passed under the provisioned build account with the same explicit signer,
required mode, and the inherited legacy hook still present. Its worktree-local
`target/signed-validation-recovery` rebuild signed generated build scripts,
proc-macro DLLs, and `stasis.exe` before execution; all staged Stasis files were
formatted. Validator shell syntax and staged/unstaged diff checks passed. No
certificate material was retrieved or persisted, and no trust or policy changed.
The broader validation above was not repeated in this handoff.

### Final worker verification (2026-09-09)

All 62 focused Windows signing, Cargo cache, and Cargo policy Python tests passed.
Under the existing provisioned build account and explicitly selected
`F177D9D4A26CB965240947FDF14620C8929BEC88` certificate, the exact pre-commit
PowerShell command passed with the legacy hook inherited: Cargo replaced that
hook, signed generated build scripts and DLLs, and ran the Stasis formatter.
Fresh acceptance in `target/signing-acceptance-repo-final` passed with the legacy
hook cleared as documented, including the 9,000-character argument, paths with
spaces, build script, proc macro, and executable. A direct acceptance invocation
with the legacy hook retained rejected its mismatched signer as intended.
No certificate stores, selection records, keys, or policies were changed.
Broader compiler/runtime/Workshop validation remains the retained evidence above;
it was not repeated for this final signer-selection recheck.

Visual evidence: not applicable; this recheck concerns signing and build execution.

### Retained-workspace verification (2026-09-10)

All 62 currently collected signing/cache/policy Python tests passed. Fresh
`target/signing-acceptance-sept10` acceptance passed under the provisioned
`mad-bee\ben` account with the explicitly selected F177D9D4A26CB965240947FDF14620C8929BEC88
certificate, required signing, and the legacy hook and certificate-file override
cleared. The launcher preserved 9,000 characters; fresh Cargo build-script EXE,
proc-macro DLL, and executable signing, verification, and execution passed from
paths containing spaces. The exact PowerShell pre-commit command also passed,
rebuilding and signing the formatter through the repository Cargo wrapper.
Validator shell syntax and staged/unstaged diff checks passed. Git used explicit
worktree paths to avoid the shared configuration issue; no Git config changed.
No certificate material, trust stores, persistent selection, or policy changed.
Broader compiler/runtime/Workshop results above are retained evidence, not reruns.

Visual evidence: not applicable; this recheck concerns build execution.

The subsequent recovery repeated all 62 focused Python checks and fresh signing
acceptance in `target/signing-acceptance-sept10-recovery` under the same provisioned
account and explicit signer. The exact pre-commit command passed with a native
Windows `CARGO_TARGET_DIR`; an initial forward-slash override signed the formatter
successfully but failed at CMD launch. No certificate stores or persistent settings
changed. Broader validation remains the retained evidence above.

### Retained-workspace verification (2026-09-12)

The existing `mad-bee\ben` build account passed fresh acceptance in
`target/signing-acceptance-sept12` with the explicit
`F177D9D4A26CB965240947FDF14620C8929BEC88` signer, required mode, and the legacy
hook and certificate-file override cleared. The launcher preserved 9,000
characters; fresh build-script EXE, proc-macro DLL, and executable verification
and execution passed from paths containing spaces. The exact noninteractive
PowerShell pre-commit command passed after rebuilding and signing the formatter
in `target/signed-validation-recovery`.

All 62 currently collected signing/cache/policy Python tests passed. Validator
shell syntax, the ignored-test audit, and staged/unstaged diff checks passed.
No certificate material was retrieved, exported, or persisted; no trust stores,
persistent configuration, or policy changed. The broader compiler/runtime/AOT
and Workshop/APK results above are retained evidence, not September 12 reruns.

Visual evidence: not applicable; this recheck concerns build execution.

The handoff recheck passed fresh acceptance in
`target/signing-acceptance-sept12-handoff` and the exact noninteractive pre-commit
command under the same provisioned account and explicit signer. All 62 focused
Python checks, validator shell syntax, the ignored-test audit, and diff checks
passed; no task-owned target processes remained. No certificate stores or
persistent configuration changed. This was an existing-workstation recheck,
not a newly imaged machine run.

The final worker recheck passed fresh acceptance in
`target/signing-acceptance-sept12-final-worker` and the exact noninteractive
pre-commit command using the same account and explicit signer, with the legacy
hook cleared. The six focused signing/cache/policy unittest modules collected
60 tests in this invocation, all passing. Validator shell syntax and the
ignored-test audit passed. Broader validation remains the retained evidence
above; no trust stores, certificate material, or persistent settings changed.

Visual evidence: not applicable; this recheck concerns build execution.
