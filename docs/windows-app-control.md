# Windows App Control (Local Dev)

This guide covers common local blocks when running locally built Stasis binaries.

## Why Blocks Happen

Typical causes:

- Unsigned local `.exe/.dll` outputs.
- Executables launched from `%TEMP%`.
- WDAC/AppLocker policy requiring trusted signer/path/hash rules.

Current local execution flow uses stable repo paths for transient artifacts:

- `.stasis_cache/tmp`

## Recommended Local Setup

1. Build the Rust CLI/runtime outputs.

```powershell
cargo build -p stasis --release
runtime\build.bat
```

2. Sign them locally (self-signed cert for dev) if your environment requires signed execution.

```powershell
$env:STASIS_AOT_SIGN_TOOL = "C:\tools\sign-stasis.cmd"
```

3. If your environment allows Defender exclusions, add:

```powershell
$repoRoot = (Resolve-Path .).Path
Add-MpPreference -ExclusionPath (Join-Path $repoRoot ".stasis_cache\tmp")
Add-MpPreference -ExclusionPath (Join-Path $repoRoot "target\release")
```

Note: this requires elevated/admin PowerShell and does not override WDAC/AppLocker policy.

4. Configure signing hooks for generated analysis/runtime artifacts.

- `STASIS_AOT_SIGN_TOOL` signs AOT-produced executables.
- `STASIS_COMPILER_ANALYSIS_SIGN_TOOL` signs blocked compiler-analysis artifacts (for example `.stasis_cache\run\*.dll`) and retries once automatically.
- Cargo execution in this repo now runs through `.cargo\stasis-sign-and-run.cmd` on Windows and will sign each launched executable first when `STASIS_AOT_SIGN_TOOL` is set.
- An unavailable optional signer is reported and local execution continues unsigned. Set `STASIS_REQUIRE_SIGNED_EXECUTION=1` to fail fast when signing is required or the configured tool is unavailable.

Example (PowerShell):

```powershell
$env:STASIS_AOT_SIGN_TOOL = "C:\tools\sign-stasis.cmd"
$env:STASIS_COMPILER_ANALYSIS_SIGN_TOOL = "C:\tools\sign-stasis.cmd"
$env:STASIS_REQUIRE_SIGNED_EXECUTION = "1"
```

Signer contract: tool is invoked as `<tool> <artifact_path>` and must return exit code `0` on success.

## WDAC / AppLocker Environments

If Defender exclusions are insufficient, ask IT/security to allow one of:

- Publisher rule for your local signing certificate.
- Hash rule for built binaries.
- Path rule for:
- `<repo>\.stasis_cache\tmp`
- `<repo>\target\release`

Publisher rules are preferred to reduce churn when binaries change.

## Troubleshooting

- If a run fails with "Application Control policy has blocked this file":
- confirm the blocked executable path.
- verify it is under `.stasis_cache/tmp` or `.stasis_cache/run` (not `%TEMP%`).
- verify signature exists (`Get-AuthenticodeSignature <file>`).
- check Windows Event Viewer policy logs for WDAC/AppLocker rule details.
