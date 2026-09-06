$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot
$cargoPolicy = Join-Path $repoRoot "tools\cargo_cache.py"

$stagedStasis = @(git diff --cached --name-only --diff-filter=ACMR -- ":(glob)**/*.stasis")
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
if ($stagedStasis.Count -gt 0) {
    Write-Output "Stasis pre-commit: enforcing canonical format on staged source paths"
    & python $cargoPolicy run -- cargo run --quiet -p stasis -- format -- $stagedStasis
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    $formatterChanges = @(git diff --name-only -- $stagedStasis)
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    if ($formatterChanges.Count -gt 0) {
        Write-Error "Commit blocked: review and stage the enforced formatting changes, then commit again."
        exit 1
    }
}
