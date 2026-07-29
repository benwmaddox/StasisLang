[CmdletBinding()]
param(
    [string]$Stasis = "target/debug/stasis.exe"
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$stasisPath = [System.IO.Path]::GetFullPath((Join-Path $root $Stasis))
$captureRoot = Join-Path $root "target/typed-drawable-visual"
$firstCapture = Join-Path $captureRoot "typed-first.png"
$secondCapture = Join-Path $captureRoot "typed-second.png"

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
    Capture-Drawable -Entry "samples/typed_drawable_visual/typed.stasis" -Output $firstCapture
    Capture-Drawable -Entry "samples/typed_drawable_visual/typed.stasis" -Output $secondCapture
} finally {
    Pop-Location
}

$firstBytes = [System.IO.File]::ReadAllBytes($firstCapture)
$secondBytes = [System.IO.File]::ReadAllBytes($secondCapture)
if (-not [System.Linq.Enumerable]::SequenceEqual($firstBytes, $secondBytes)) {
    $firstHash = (Get-FileHash -LiteralPath $firstCapture -Algorithm SHA256).Hash
    $secondHash = (Get-FileHash -LiteralPath $secondCapture -Algorithm SHA256).Hash
    throw "Typed drawable framebuffer is nondeterministic: first=$firstHash second=$secondHash"
}

$hash = (Get-FileHash -LiteralPath $secondCapture -Algorithm SHA256).Hash
Write-Output "typed drawable deterministic visual passed: sha256=$hash"
