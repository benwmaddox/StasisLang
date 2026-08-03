param(
    [switch]$Release,
    [switch]$Install,
    [switch]$Lifecycle,
    [switch]$RequireDevice,
    [string]$Serial = "",
    [string]$OutputPath = ""
)

$ErrorActionPreference = "Stop"
$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Split-Path -Parent (Split-Path -Parent $scriptRoot)
$variant = if ($Release) { "release" } else { "workshop" }
$package = if ($Release) { "com.stasislang.pong" } else { "com.stasislang.workshop" }
$activity = if ($Release) { "com.stasislang.game.MainActivity" } else { "com.stasislang.workshop.MainActivity" }
$apk = if ($Release) {
    Join-Path $scriptRoot "app\src\main\assets\workshop_sample\build\android-release\android\app\build\outputs\apk\debug\app-debug.apk"
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
$connectedDevices = @($deviceLines | Where-Object { $_ -match '^(\S+)\s+device(?:\s|$)' })
$deviceLine = if ($Serial) {
    $connectedDevices | Where-Object { $_ -match "^$([regex]::Escape($Serial))\s" } | Select-Object -First 1
} elseif (-not $Release) {
    $connectedDevices | Sort-Object { if ($_ -match '^emulator-') { 0 } else { 1 } } | Select-Object -First 1
} else {
    $connectedDevices | Select-Object -First 1
}
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

$originalAutoRotation = $null
$originalRotation = $null
try {
    $model = (Invoke-Adb @("shell", "getprop", "ro.product.model") | Select-Object -First 1).Trim()
    $sdk = (Invoke-Adb @("shell", "getprop", "ro.build.version.sdk") | Select-Object -First 1).Trim()
    $abis = (Invoke-Adb @("shell", "getprop", "ro.product.cpu.abilist") | Select-Object -First 1).Trim()
    $requiredAbiPattern = if ($Release) { 'arm64-v8a' } else { 'arm64-v8a|x86_64' }
    if ($abis -notmatch $requiredAbiPattern) { throw "attached device does not support a packaged ABI: $abis" }

    if ($Install) {
        if (-not (Test-Path $apk)) { throw "APK was not found; build it first: $apk" }
        Invoke-Adb @("install", "-r", $apk) | Out-Null
    }

    Invoke-Adb @("logcat", "-c") | Out-Null
    Invoke-Adb @("shell", "am", "force-stop", $package) | Out-Null
    $launchOutput = Invoke-Adb @("shell", "am", "start", "-W", "-n", "$package/$activity")
    Start-Sleep -Seconds 2
    $appPid = (Invoke-Adb @("shell", "pidof", $package) | Select-Object -First 1).Trim()
    if (-not $appPid) { throw "Android package did not remain running after launch: $package" }
    $packageInfo = Invoke-Adb @("shell", "dumpsys", "package", $package)
    $versionName = ($packageInfo | Select-String -Pattern 'versionName=' | Select-Object -First 1).Line.Trim()

    $lifecycleLog = @()
    $restoreCount = 0
    if ($Lifecycle) {
        $originalAutoRotation = (Invoke-Adb @("shell", "settings", "get", "system", "accelerometer_rotation") | Select-Object -First 1).Trim()
        $originalRotation = (Invoke-Adb @("shell", "settings", "get", "system", "user_rotation") | Select-Object -First 1).Trim()
        Invoke-Adb @("shell", "settings", "put", "system", "accelerometer_rotation", "0") | Out-Null
        Invoke-Adb @("shell", "settings", "put", "system", "user_rotation", "1") | Out-Null
        Start-Sleep -Seconds 2
        Invoke-Adb @("shell", "input", "keyevent", "KEYCODE_HOME") | Out-Null
        Start-Sleep -Seconds 1
        Invoke-Adb @("shell", "am", "start", "-W", "-n", "$package/$activity") | Out-Null
        Start-Sleep -Seconds 2
        Invoke-Adb @("shell", "settings", "put", "system", "user_rotation", "0") | Out-Null
        Start-Sleep -Seconds 2
        Invoke-Adb @("shell", "am", "start", "-S", "-W", "-n", "$package/$activity") | Out-Null
        Start-Sleep -Seconds 2
        $appPid = (Invoke-Adb @("shell", "pidof", $package) | Select-Object -First 1).Trim()
        if (-not $appPid) { throw "Android package did not survive lifecycle acceptance: $package" }
        $lifecycleLog = @(Invoke-Adb @("logcat", "-d", "-s", "StasisRenderer:I", "*:S"))
        $restoreCount = @($lifecycleLog | Select-String -SimpleMatch "resources restored").Count
        if ($restoreCount -lt 2) {
            throw "renderer lifecycle emitted only $restoreCount successful restoration markers"
        }
        if ($lifecycleLog | Select-String -SimpleMatch "resource restore failed") {
            throw "renderer lifecycle reported a restoration failure"
        }
    }

    Write-Report @{
        status = "passed"
        installed = [bool]$Install
        serial = $serial
        model = $model
        sdk = $sdk
        abis = $abis
        pid = $appPid
        version = $versionName
        launch = @($launchOutput)
        lifecycle = [bool]$Lifecycle
        restoration_markers = $restoreCount
        lifecycle_log = $lifecycleLog
    }
} catch {
    Write-Report @{
        status = "failed"
        serial = $serial
        installed = [bool]$Install
        reason = $_.Exception.Message
    }
    exit 1
} finally {
    if ($serial) {
        if ($null -ne $originalAutoRotation) {
            & $adb -s $serial shell settings put system accelerometer_rotation $originalAutoRotation | Out-Null
        }
        if ($null -ne $originalRotation) {
            & $adb -s $serial shell settings put system user_rotation $originalRotation | Out-Null
        }
        & $adb -s $serial shell am force-stop $package | Out-Null
    }
}
