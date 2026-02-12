# Windows App Control (Local Dev)

This guide covers common local blocks when running bootstrap-built Stasis binaries.

## Why Blocks Happen

Typical causes:

- Unsigned local `.exe/.dll` outputs.
- Executables launched from `%TEMP%`.
- WDAC/AppLocker policy requiring trusted signer/path/hash rules.

Current bootstrap flow uses a stable repo path for transient artifacts:

- `.stasis_cache/tmp`

## Recommended Local Setup

1. Build bootstrap binaries.

```powershell
powershell -ExecutionPolicy Bypass -File bootstrap/windows/build-bootstrap.ps1
```

2. Sign them locally (self-signed cert for dev).

```powershell
powershell -ExecutionPolicy Bypass -File bootstrap/windows/build-bootstrap.ps1 -Sign -CreateCert -TrustLocalCert
```

3. If your environment allows Defender exclusions, add:

```powershell
Add-MpPreference -ExclusionPath "F:\StasisLang\.stasis_cache\tmp"
Add-MpPreference -ExclusionPath "F:\StasisLang\bootstrap\windows\stasis-cli"
```

Note: this requires elevated/admin PowerShell and does not override WDAC/AppLocker policy.

## WDAC / AppLocker Environments

If Defender exclusions are insufficient, ask IT/security to allow one of:

- Publisher rule for your local signing certificate.
- Hash rule for built binaries.
- Path rule for:
- `F:\StasisLang\.stasis_cache\tmp`
- `F:\StasisLang\bootstrap\windows\stasis-cli`

Publisher rules are preferred to reduce churn when binaries change.

## Troubleshooting

- If a run fails with "Application Control policy has blocked this file":
- confirm the blocked executable path.
- verify it is under `.stasis_cache/tmp` (not `%TEMP%`).
- verify signature exists (`Get-AuthenticodeSignature <file>`).
- check Windows Event Viewer policy logs for WDAC/AppLocker rule details.
