[CmdletBinding()]
param(
    [string]$Stasis = "target/debug/stasis.exe"
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$stasisPath = [System.IO.Path]::GetFullPath((Join-Path $root $Stasis))
$temporaryManifest = Join-Path $root "stasis.json"
$temporaryAssetDirectory = Join-Path $root "assets"
$temporaryAsset = Join-Path $temporaryAssetDirectory "__typed_drawable_fixture.svg"

if (-not (Test-Path -LiteralPath $stasisPath)) {
    throw "Stasis executable not found: $stasisPath"
}
if (Test-Path -LiteralPath $temporaryManifest) {
    throw "Refusing to replace existing repository-root manifest: $temporaryManifest"
}
if (Test-Path -LiteralPath $temporaryAsset) {
    throw "Refusing to replace existing typed drawable fixture: $temporaryAsset"
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
$assetDirectoryExisted = Test-Path -LiteralPath $temporaryAssetDirectory
$locationPushed = $false

try {
    [System.IO.File]::WriteAllText($temporaryManifest, $manifest, $utf8NoBom)
    [System.IO.Directory]::CreateDirectory($temporaryAssetDirectory) | Out-Null
    [System.IO.File]::WriteAllText(
        $temporaryAsset,
        '<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"><rect width="1" height="1"/></svg>',
        $utf8NoBom
    )
    Push-Location $root
    $locationPushed = $true

    & $stasisPath check --workspace $root
    if ($LASTEXITCODE -ne 0) { throw "Typed drawable check failed with exit code $LASTEXITCODE" }

    & $stasisPath test --workspace $root
    if ($LASTEXITCODE -ne 0) { throw "Typed drawable tests failed with exit code $LASTEXITCODE" }

    & $stasisPath run --workspace $root
    if ($LASTEXITCODE -ne 0) { throw "Typed drawable sample failed with exit code $LASTEXITCODE" }
} finally {
    if ($locationPushed) {
        Pop-Location
    }
    if (Test-Path -LiteralPath $temporaryManifest) {
        Remove-Item -LiteralPath $temporaryManifest -Force
    }
    if (Test-Path -LiteralPath $temporaryAsset) {
        Remove-Item -LiteralPath $temporaryAsset -Force
    }
    if (-not $assetDirectoryExisted -and
        (Test-Path -LiteralPath $temporaryAssetDirectory) -and
        -not (Get-ChildItem -LiteralPath $temporaryAssetDirectory -Force)) {
        Remove-Item -LiteralPath $temporaryAssetDirectory -Force
    }
}

Write-Output "typed drawable contract passed"
