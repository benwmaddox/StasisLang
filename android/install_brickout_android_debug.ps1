$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Resolve-RepoRoot {
    return (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
}

function Require-Command {
    param([string]$Name)
    if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) {
        throw "$Name not found in PATH"
    }
}

$repoRoot = Resolve-RepoRoot
Require-Command "adb"

$package = "com.stasis.brickoutrevenge"
$activity = "com.stasis.brickoutrevenge.BrickoutRevengeActivity"

$apk = Join-Path $repoRoot "android\\brickout-revenge\\app\\build\\outputs\\apk\\debug\\app-debug.apk"
if (-not (Test-Path $apk)) {
    throw "APK not found. Build first: powershell -ExecutionPolicy Bypass -File android\\build_brickout_android_debug.ps1"
}

Write-Host "Installing APK..."
adb install -r $apk

$deviceRoot = "/sdcard/Android/data/$package/files"

Write-Host ""
Write-Host "Pushing Brickout assets/data to: $deviceRoot"
adb shell "mkdir -p $deviceRoot/samples"
adb push "$repoRoot\\samples\\brickout_revenge" "$deviceRoot/samples/brickout_revenge"

Write-Host ""
Write-Host "Launching..."
adb shell "am start -n $package/$activity"

