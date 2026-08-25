$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot
Push-Location $repoRoot
try {
    git config --local core.hooksPath .githooks
    if ($LASTEXITCODE -ne 0) { throw "unable to configure core.hooksPath" }
    $configured = git config --local --get core.hooksPath
    if ($configured -ne ".githooks") {
        throw "core.hooksPath verification failed: $configured"
    }
    Write-Output "Installed Stasis source-format hooks from .githooks"
} finally {
    Pop-Location
}
