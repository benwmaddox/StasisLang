param(
    [switch]$Published,
    [switch]$Install,
    [switch]$RequireDevice,
    [string]$OutputPath = ""
)

$ErrorActionPreference = "Stop"
$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Split-Path -Parent (Split-Path -Parent $scriptRoot)
$variant = if ($Published) { "published" } else { "workshop" }
$package = if ($Published) { "com.stasislang.workshop.published" } else { "com.stasislang.workshop" }
$apk = if ($Published) {
    Join-Path $scriptRoot "app\build\outputs\apk\published\debug\app-published-debug.apk"
} else {
    Join-Path $scriptRoot "app\build\outputs\apk\workshop\debug\app-workshop-debug.apk"
}

$adbCandidates = @()
if ($env:ANDROID_HOME) { $adbCandidates += Join-Path $env:ANDROID_HOME "platform-tools\adb.exe" }
if ($env:ANDROID_SDK_ROOT) { $adbCandidates += Join-Path $env:ANDROID_SDK_ROOT "platform-tools\adb.exe" }
$adbCandidates += "C:\Android\Sdk\platform-tools\adb.exe"
if ($env:LOCALAPPDATA) { $adbCandidates += Join-Path $env:LOCALAPPDATA "Android\Sdk\platform-tools\adb.exe" }
$adb = $adbCandidates | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $adb) { throw "adb.exe was not found" }

$stamp = (Get-Date).ToUniversalTime().ToString("yyyyMMddTHHmmssZ")
if (-not $OutputPath) {
    $OutputPath = Join-Path $repoRoot "artifacts\android_device_acceptance\${variant}_${stamp}.json"
}
$outputFile = [System.IO.Path]::GetFullPath($OutputPath)
$outputDirectory = Split-Path -Parent $outputFile
New-Item -ItemType Directory -Force -Path $outputDirectory | Out-Null

function Write-Report([hashtable]$Report) {
    $Report.timestamp_utc = (Get-Date).ToUniversalTime().ToString("o")
    $Report.variant = $variant
    $Report.package = $package
    $Report.apk = $apk
    $Report | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $outputFile -Encoding UTF8
    Write-Output "Android device acceptance: $outputFile"
}

$deviceLines = & $adb devices -l
$deviceLine = $deviceLines | Where-Object { $_ -match '^(\S+)\s+device(?:\s|$)' } | Select-Object -First 1
if (-not $deviceLine) {
    Write-Report @{
        status = "skipped"
        reason = "no authorized Android device or emulator is attached"
        adb = $adb
        devices = @($deviceLines)
    }
    if ($RequireDevice) { exit 3 }
    exit 0
}

$serial = ([regex]::Match($deviceLine, '^(\S+)')).Groups[1].Value
function Invoke-Adb([string[]]$Arguments) {
    $result = & $adb -s $serial @Arguments
    if ($LASTEXITCODE -ne 0) { throw "adb command failed: $($Arguments -join ' ')" }
    return $result
}

try {
    $model = (Invoke-Adb @("shell", "getprop", "ro.product.model") | Select-Object -First 1).Trim()
    $sdk = (Invoke-Adb @("shell", "getprop", "ro.build.version.sdk") | Select-Object -First 1).Trim()
    $abis = (Invoke-Adb @("shell", "getprop", "ro.product.cpu.abilist") | Select-Object -First 1).Trim()
    if ($abis -notmatch 'arm64-v8a') { throw "attached device does not support arm64-v8a: $abis" }

    if ($Install) {
        if (-not (Test-Path $apk)) { throw "APK was not found; build it first: $apk" }
        Invoke-Adb @("install", "-r", $apk) | Out-Null
    }

    Invoke-Adb @("shell", "am", "force-stop", $package) | Out-Null
    $launchOutput = Invoke-Adb @("shell", "am", "start", "-W", "-n", "$package/com.stasislang.workshop.MainActivity")
    Start-Sleep -Seconds 2
    $pid = (Invoke-Adb @("shell", "pidof", $package) | Select-Object -First 1).Trim()
    if (-not $pid) { throw "Android package did not remain running after launch: $package" }
    $packageInfo = Invoke-Adb @("shell", "dumpsys", "package", $package)
    $versionName = ($packageInfo | Select-String -Pattern 'versionName=' | Select-Object -First 1).Line.Trim()

    Write-Report @{
        status = "passed"
        installed = [bool]$Install
        serial = $serial
        model = $model
        sdk = $sdk
        abis = $abis
        pid = $pid
        version = $versionName
        launch = @($launchOutput)
    }
} catch {
    Write-Report @{
        status = "failed"
        serial = $serial
        installed = [bool]$Install
        reason = $_.Exception.Message
    }
    exit 1
}
