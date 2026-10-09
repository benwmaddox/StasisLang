$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot
$cargoPolicy = Join-Path $repoRoot "tools\cargo_cache.py"

# Git Bash may put its Unix `link` ahead of MSVC's linker. Prefer the exact
# x64 linker selected by vcvars before Cargo launches the formatter build.
if (-not [string]::IsNullOrWhiteSpace($env:VCToolsInstallDir)) {
    $msvcLinkDirectory = Join-Path $env:VCToolsInstallDir "bin\Hostx64\x64"
    $msvcLinkExe = Join-Path $msvcLinkDirectory "link.exe"
    if (Test-Path -LiteralPath $msvcLinkExe -PathType Leaf) {
        $env:PATH = "$msvcLinkDirectory;$env:PATH"
    }
}

$frozenFixturePolicy = Join-Path $repoRoot "tools\ci\verify_staged_live_ttt_fixtures.py"

# This helper validates the exact staged provenance and blob hashes before it
# returns the four vendored paths that are exempt from canonical formatting.
$frozenStasis = @(& python $frozenFixturePolicy)
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$stagedStasis = @(git diff --cached --name-only --diff-filter=ACMR -- ":(glob)**/*.stasis")
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
$stagedStasis = @($stagedStasis | Where-Object { $_ -notin $frozenStasis })
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
