[CmdletBinding()]
param(
    [string]$Stasis = "target/debug/stasis.exe"
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$stasisPath = [System.IO.Path]::GetFullPath((Join-Path $root $Stasis))
$temporaryManifest = Join-Path $root "stasis.json"

if (-not (Test-Path -LiteralPath $stasisPath)) {
    throw "Stasis executable not found: $stasisPath"
}
if (Test-Path -LiteralPath $temporaryManifest) {
    throw "Refusing to replace existing repository-root manifest: $temporaryManifest"
}

$manifest = @"
{
  "manifest_version": 1,
  "name": "stasis_typed_drawable_fixture",
  "entry": "samples/typed_sprite/main.stasis",
  "tests": "samples/typed_sprite/tests",
  "output": "samples/typed_sprite/build"
}
"@
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText($temporaryManifest, $manifest, $utf8NoBom)

Push-Location $root
try {
    & $stasisPath check --workspace $root
    if ($LASTEXITCODE -ne 0) { throw "Typed drawable check failed with exit code $LASTEXITCODE" }

    & $stasisPath test --workspace $root
    if ($LASTEXITCODE -ne 0) { throw "Typed drawable tests failed with exit code $LASTEXITCODE" }

    & $stasisPath run --workspace $root
    if ($LASTEXITCODE -ne 0) { throw "Typed drawable sample failed with exit code $LASTEXITCODE" }
} finally {
    Pop-Location
    if (Test-Path -LiteralPath $temporaryManifest) {
        Remove-Item -LiteralPath $temporaryManifest -Force
    }
}

Write-Output "typed drawable contract passed"
