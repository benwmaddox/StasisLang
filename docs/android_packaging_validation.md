# Android release boundary validation

This document describes the standalone artifact boundary from task #404. The
current slice implements `tools/android_release.py` and its focused unit tests.
The PowerShell builders, generated Gradle template, CI jobs, and device
validator still require the later coordinated port; this document does not
claim that those paths already select release.

## Boundary contract

The boundary accepts three explicit modes:

- `preflight --development-build` describes a debug/development artifact and
  does not read signing credentials.
- `preflight --unsigned-release` describes a release ARM64 handoff that cannot
  be installed or published as signed production output.
- A release preflight without either flag requires a non-debug signing
  certificate and protected inputs. The expected certificate SHA-256 is an
  additional identity check.

`finalize` verifies the generated package manifest, requested variant and ABI,
embedded provenance bytes, package contents through
`tools/ci/check_android_release_package.py`, Android manifest identity/version,
debuggable state, and signer state. Release output is ARM64 (`arm64-v8a`). A
package ID from the generated manifest is compared with the final APK or AAB;
this keeps the signer check scoped to the intended app identity.

The release boundary never accepts the standard `CN=Android Debug` certificate.
Signing passwords are passed to `keytool`, `jarsigner`, and `apksigner` through
environment references. Error output is redacted, and receipts contain hashes,
variant, ABI, version, signer digest or explicit unsigned state, provenance,
package identity, and sidecar paths without secret values.

`--sidecar NAME=PATH` accepts only the supported mapping and native-symbol
sidecars. A supplied sidecar must exist; omitting the option is the explicit
absence case. Artifact, sidecars, and receipt are staged and published with
backup recovery, so a failed publication retains the previous complete set.
The receipt is published with `install_status: pending` before an optional
`adb install -r`. A successful install changes it to `passed`; a failed or
missing ADB tool changes it to `failed` while keeping the verified artifact and
failure detail redacted.

## Supported command shape

The future production builder should invoke the boundary after a fresh
release package has been assembled:

```powershell
python tools/android_release.py preflight --unsigned-release
python tools/android_release.py finalize `
  --source app-release-unsigned.apk `
  --output artifacts/stasis-release.apk `
  --receipt artifacts/stasis-release.receipt.json `
  --package-manifest stasis_mobile_package.json `
  --provenance stasis_provenance.json `
  --format apk --variant release --abi arm64-v8a `
  --unsigned-release
```

Signed release callers provide `STASIS_ANDROID_KEYSTORE`,
`STASIS_ANDROID_KEY_ALIAS`, `STASIS_ANDROID_STORE_PASSWORD`, and
`STASIS_ANDROID_KEY_PASSWORD` through a protected caller environment, with
legacy variable aliases retained for compatibility. The boundary verifies the
keystore certificate and private key before signing. Passwords and private key
material never belong in source, command arguments, receipts, or logs. A
configured keystore path and alias are passed only to the bounded SDK signing
subprocess and are redacted from failure reporting.

## Validation status for this slice

No Android build, signing operation, installation, device interaction, or
remote operation was performed for this helper-only slice. The focused
commands are:

```powershell
python -m py_compile tools/android_release.py tools/ci/test_android_release.py
python -m unittest tools.ci.test_android_release
git diff --check
```

The later integration port must additionally run the package checker, native
library audit, shell structural checks, focused Android policy tests, and fresh
unsigned APK/AAB package/link validation before requesting a signed/device
acceptance. Full task acceptance requires a package-scoped non-debug signer,
final signer/ABI/content/hash receipt, and signed `install -r` evidence. An
unsigned handoff is not that evidence.

## Historical retained evidence

The older retained worktree contains unsigned ARM64 release receipts at
`target/task404-rechecked-apk/stasis-release-unsigned.receipt.json` and
`target/task404-rechecked-aab/stasis-release-unsigned.receipt.json`. They are
historical artifacts from `codex/task-404-stasis-android-unify-debug-rel`
(`c9e25ac3...`), not evidence that the current canonical builders or this
branch have been ported. They show unsigned, non-debuggable package checks
only; they do not prove signing, installation, or device behavior.

Visual evidence: not applicable; this slice changes packaging and verification
contracts rather than rendered behavior.
