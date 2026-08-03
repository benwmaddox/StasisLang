$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot
$test = Join-Path $repoRoot "mobile\android\test_render_emulator.ps1"

Write-Output "Stasis pre-commit: checking staged Stasis source format"
& cargo test --quiet -p stasis --bin stasis toolchain_formatter::tests::staged_repository_stasis_sources_are_formatted -- --exact
if ($LASTEXITCODE -ne 0) {
    $stagedStasis = @(git diff --cached --name-only --diff-filter=ACMR -- ":(glob)**/*.stasis")
    if ($LASTEXITCODE -eq 0 -and $stagedStasis.Count -gt 0) {
        Write-Output "Stasis pre-commit: formatting staged source paths before blocking this commit"
        & cargo run --quiet -p stasis -- format -- $stagedStasis
    }
    Write-Error "Commit blocked: review and stage the formatting changes, then commit again."
    exit 1
}

Write-Output "Stasis pre-commit: verifying Android Workshop JIT rendering"
try {
    & $test -Headless
} catch {
    Write-Error "Commit blocked: Android render E2E failed. Inspect artifacts/android_render_e2e. $($_.Exception.Message)"
    exit 1
}
