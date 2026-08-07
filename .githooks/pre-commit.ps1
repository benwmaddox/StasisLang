$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot
$cargoPolicy = Join-Path $repoRoot "tools\cargo_cache.py"

Write-Output "Stasis pre-commit: checking staged Stasis source format"
& python $cargoPolicy run -- cargo test --quiet -p stasis_compiler --lib frontend::formatter::tests::staged_repository_stasis_sources_are_formatted -- --exact
if ($LASTEXITCODE -ne 0) {
    $stagedStasis = @(git diff --cached --name-only --diff-filter=ACMR -- ":(glob)**/*.stasis")
    if ($LASTEXITCODE -eq 0 -and $stagedStasis.Count -gt 0) {
        Write-Output "Stasis pre-commit: formatting staged source paths before blocking this commit"
        & python $cargoPolicy run -- cargo run --quiet -p stasis -- format -- $stagedStasis
    }
    Write-Error "Commit blocked: review and stage the formatting changes, then commit again."
    exit 1
}
