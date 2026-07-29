[CmdletBinding()]
param(
    [string]$Stasis = "target/debug/stasis.exe"
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$stasisPath = [System.IO.Path]::GetFullPath((Join-Path $root $Stasis))
$captureRoot = Join-Path $root "target/typed-drawable-visual"
$rawCapture = Join-Path $captureRoot "raw.png"
$typedCapture = Join-Path $captureRoot "typed.png"

if (-not (Test-Path -LiteralPath $stasisPath)) {
    throw "Stasis executable not found: $stasisPath"
}

New-Item -ItemType Directory -Force -Path $captureRoot | Out-Null

function Capture-Drawable {
    param([string]$Entry, [string]$Output)

    if (Test-Path -LiteralPath $Output) {
        Remove-Item -LiteralPath $Output -Force
    }
    & $stasisPath play $Entry `
        --watch-dir "samples/typed_drawable_visual" `
        --ticks 3 `
        --screenshot $Output `
        --screenshot-frame 3 `
        --exit-after-screenshot
    if ($LASTEXITCODE -ne 0) {
        throw "Typed drawable capture failed for $Entry with exit code $LASTEXITCODE"
    }
    if (-not (Test-Path -LiteralPath $Output)) {
        throw "Typed drawable capture was not created: $Output"
    }
}

Push-Location $root
try {
    Capture-Drawable -Entry "samples/typed_drawable_visual/raw.stasis" -Output $rawCapture
    Capture-Drawable -Entry "samples/typed_drawable_visual/typed.stasis" -Output $typedCapture
} finally {
    Pop-Location
}

$rawBytes = [System.IO.File]::ReadAllBytes($rawCapture)
$typedBytes = [System.IO.File]::ReadAllBytes($typedCapture)
if (-not [System.Linq.Enumerable]::SequenceEqual($rawBytes, $typedBytes)) {
    $rawHash = (Get-FileHash -LiteralPath $rawCapture -Algorithm SHA256).Hash
    $typedHash = (Get-FileHash -LiteralPath $typedCapture -Algorithm SHA256).Hash
    throw "Typed drawable framebuffer differs from raw baseline: raw=$rawHash typed=$typedHash"
}

$hash = (Get-FileHash -LiteralPath $typedCapture -Algorithm SHA256).Hash
Write-Output "typed drawable visual parity passed: sha256=$hash"
