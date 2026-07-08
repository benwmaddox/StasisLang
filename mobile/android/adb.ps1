$ErrorActionPreference = "Stop"

$candidates = @()
if ($env:ANDROID_HOME) {
    $candidates += Join-Path $env:ANDROID_HOME "platform-tools\adb.exe"
}
if ($env:ANDROID_SDK_ROOT) {
    $candidates += Join-Path $env:ANDROID_SDK_ROOT "platform-tools\adb.exe"
}
$candidates += "C:\Android\Sdk\platform-tools\adb.exe"
if ($env:LOCALAPPDATA) {
    $candidates += Join-Path $env:LOCALAPPDATA "Android\Sdk\platform-tools\adb.exe"
}

$adb = $candidates | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $adb) {
    throw "adb.exe was not found. Set ANDROID_HOME or ANDROID_SDK_ROOT, or install Android SDK platform-tools."
}

& $adb @args
exit $LASTEXITCODE
