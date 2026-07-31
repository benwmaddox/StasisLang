$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot
$test = Join-Path $repoRoot "mobile\android\test_render_emulator.ps1"

Write-Output "Stasis pre-commit: verifying Android Workshop JIT and Published AOT rendering"
try {
    & $test -Headless
} catch {
    Write-Error "Commit blocked: Android render E2E failed. Inspect artifacts/android_render_e2e. $($_.Exception.Message)"
    exit 1
}
