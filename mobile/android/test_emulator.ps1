param(
    [string]$AvdName = "Stasis_API_35",
    [int]$Port = 5554,
    [switch]$Headless,
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"
$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$serial = & (Join-Path $scriptRoot "start_emulator.ps1") -AvdName $AvdName -Port $Port -Headless:$Headless
$serial = @($serial) | Select-Object -Last 1

$previousSerial = $env:ANDROID_SERIAL
try {
    $env:ANDROID_SERIAL = $serial
    if (-not $SkipBuild) {
        & (Join-Path $scriptRoot "build_debug.ps1") -Install
        if ($LASTEXITCODE -ne 0) { throw "Workshop emulator build/install failed with exit code $LASTEXITCODE" }
    }
    & (Join-Path $scriptRoot "validate_device.ps1") -RequireDevice -Serial $serial
    if ($LASTEXITCODE -ne 0) { throw "Workshop emulator acceptance failed with exit code $LASTEXITCODE" }
} finally {
    $env:ANDROID_SERIAL = $previousSerial
}
