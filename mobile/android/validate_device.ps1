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

function Invoke-AdbBestEffort([string[]]$Arguments) {
    $previousPreference = $ErrorActionPreference
    try {
        $ErrorActionPreference = "Continue"
        $result = @(& $adb -s $serial @Arguments 2>$null)
        $exitCode = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previousPreference
    }
    if ($exitCode -ne 0) { return @() }
    return $result
}

function Read-ObservedRotation {
    $sources = @(
        (Invoke-AdbBestEffort @("shell", "dumpsys", "window", "displays")),
        (Invoke-AdbBestEffort @("shell", "dumpsys", "display")),
        (Invoke-AdbBestEffort @("shell", "dumpsys", "input"))
    )
    foreach ($source in $sources) {
        $match = [regex]::Match(($source -join "`n"), '(?m)\bmRotation=(\d+)\b')
        if ($match.Success) { return [int]$match.Groups[1].Value }
        $match = [regex]::Match(($source -join "`n"), '(?m)\bdisplay_rotation=(\d+)\b')
        if ($match.Success) { return [int]$match.Groups[1].Value }
    }
    return -1
}

function Set-ObservedRotation([int]$Rotation) {
    Invoke-AdbBestEffort @("shell", "settings", "put", "system", "accelerometer_rotation", "0") | Out-Null
    Invoke-AdbBestEffort @("shell", "settings", "put", "system", "user_rotation", "$Rotation") | Out-Null
    Invoke-AdbBestEffort @("shell", "cmd", "window", "user-rotation", "lock", "$Rotation") | Out-Null
}

function Wait-ObservedRotation([int]$Expected, [int]$TimeoutSeconds = 20) {
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    do {
        $observed = Read-ObservedRotation
        if ($observed -eq $Expected) { return $observed }
        Start-Sleep -Milliseconds 500
    } while ((Get-Date) -lt $deadline)
    throw "Android rotation did not reach $Expected; observed $observed"
}

function Read-PreviewFrames {
    $lines = Invoke-AdbBestEffort @("logcat", "-d", "-s", "StasisWorkshop:I", "*:S")
    $pattern = 'logical=(\d+)x(\d+)\s+native=(\d+)x(\d+)\s+drawable=(\d+)x(\d+)\s+display_gen=(\d+)\s+density_gen=(\d+)'
    foreach ($line in $lines) {
        $match = [regex]::Match($line.ToString(), $pattern)
        if (-not $match.Success) { continue }
        [pscustomobject]@{
            line = $line.ToString()
            logical_w = [int]$match.Groups[1].Value
            logical_h = [int]$match.Groups[2].Value
            native_w = [int]$match.Groups[3].Value
            native_h = [int]$match.Groups[4].Value
            drawable_w = [int]$match.Groups[5].Value
            drawable_h = [int]$match.Groups[6].Value
            display_gen = [int]$match.Groups[7].Value
            density_gen = [int]$match.Groups[8].Value
        }
    }
}

function Wait-PreviewFrame([int]$PreviousDisplayGeneration, [bool]$Landscape, [int]$LogicalWidth = 0, [int]$LogicalHeight = 0, [int]$TimeoutSeconds = 25) {
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    do {
        $frames = @(Read-PreviewFrames)
        $frame = $frames | Where-Object {
            $matchesLogical = $LogicalWidth -le 0 -or ($_.logical_w -eq $LogicalWidth -and $_.logical_h -eq $LogicalHeight)
            $matchesOrientation = if ($Landscape) { $_.native_w -gt $_.native_h } else { $_.native_h -gt $_.native_w }
            $matchesLogical -and $_.display_gen -gt $PreviousDisplayGeneration -and $matchesOrientation
        } | Sort-Object display_gen | Select-Object -Last 1
        if ($frame) { return $frame }
        Start-Sleep -Milliseconds 500
    } while ((Get-Date) -lt $deadline)
    throw "Workshop preview did not emit a newer $($(if($Landscape){'landscape'}else{'portrait'})) gfx_cmd frame after display_gen=$PreviousDisplayGeneration"
}

function Save-AndroidScreenshot([string]$Path) {
    $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $adb
    $startInfo.Arguments = "-s $serial exec-out screencap -p"
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    $process = [System.Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    if (-not $process.Start()) { throw "Android screenshot capture could not start" }
    $errorTask = $process.StandardError.ReadToEndAsync()
    $stream = [IO.File]::Create($Path)
    try { $process.StandardOutput.BaseStream.CopyTo($stream) } finally { $stream.Dispose() }
    $process.WaitForExit()
    $errorTask.Result | Set-Content -LiteralPath "$Path.stderr" -Encoding UTF8
    if ($process.ExitCode -ne 0 -or -not (Test-Path -LiteralPath $Path) -or (Get-Item -LiteralPath $Path).Length -eq 0) {
        throw "Android screenshot capture failed: $Path"
    }
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
    $previewFrames = @()
    $capturePaths = @()
    $restoreCount = 0
    if ($Lifecycle) {
        $originalAutoRotation = (Invoke-Adb @("shell", "settings", "get", "system", "accelerometer_rotation") | Select-Object -First 1).Trim()
        $originalRotation = (Invoke-Adb @("shell", "settings", "get", "system", "user_rotation") | Select-Object -First 1).Trim()

        # Re-launch into a known portrait frame, then exercise the live Workshop surface.
        Set-ObservedRotation 0
        Invoke-Adb @("shell", "am", "force-stop", $package) | Out-Null
        Invoke-Adb @("shell", "am", "start", "-S", "-W", "-n", "$package/$activity") | Out-Null
        Start-Sleep -Seconds 1
        $appPid = (Invoke-Adb @("shell", "pidof", $package) | Select-Object -First 1).Trim()
        if (-not $appPid) { throw "Android package did not start for Workshop orientation acceptance: $package" }
        Wait-ObservedRotation 0 | Out-Null
        $beforeFrame = Wait-PreviewFrame -PreviousDisplayGeneration -1 -Landscape $false
        $previewFrames += $beforeFrame
        $beforeCapture = Join-Path $outputDirectory "before.png"
        Save-AndroidScreenshot $beforeCapture
        $capturePaths += $beforeCapture

        Set-ObservedRotation 1
        Wait-ObservedRotation 1 | Out-Null
        $landscapeFrame = Wait-PreviewFrame -PreviousDisplayGeneration $beforeFrame.display_gen -Landscape $true -LogicalWidth $beforeFrame.logical_w -LogicalHeight $beforeFrame.logical_h
        $previewFrames += $landscapeFrame
        $landscapeCapture = Join-Path $outputDirectory "landscape.png"
        Save-AndroidScreenshot $landscapeCapture
        $capturePaths += $landscapeCapture
        $tapX = [math]::Floor($landscapeFrame.native_w / 2)
        $tapY = [math]::Floor($landscapeFrame.native_h / 2)
        Invoke-Adb @("shell", "input", "tap", "$tapX", "$tapY") | Out-Null
        Start-Sleep -Milliseconds 500
        $appPid = (Invoke-Adb @("shell", "pidof", $package) | Select-Object -First 1).Trim()
        if (-not $appPid) { throw "Android package died after the landscape Workshop preview tap" }

        Set-ObservedRotation 0
        Wait-ObservedRotation 0 | Out-Null
        $afterFrame = Wait-PreviewFrame -PreviousDisplayGeneration $landscapeFrame.display_gen -Landscape $false -LogicalWidth $beforeFrame.logical_w -LogicalHeight $beforeFrame.logical_h
        $previewFrames += $afterFrame
        $afterCapture = Join-Path $outputDirectory "after.png"
        Save-AndroidScreenshot $afterCapture
        $capturePaths += $afterCapture
        $appPid = (Invoke-Adb @("shell", "pidof", $package) | Select-Object -First 1).Trim()
        if (-not $appPid) { throw "Android package did not survive the restored portrait Workshop preview" }

        # Keep the existing renderer lifecycle proof in the same run.
        Invoke-Adb @("shell", "input", "keyevent", "KEYCODE_HOME") | Out-Null
        Start-Sleep -Seconds 1
        Invoke-Adb @("shell", "am", "start", "-S", "-W", "-n", "$package/$activity") | Out-Null
        Start-Sleep -Seconds 2
        $appPid = (Invoke-Adb @("shell", "pidof", $package) | Select-Object -First 1).Trim()
        if (-not $appPid) { throw "Android package did not survive renderer lifecycle acceptance: $package" }

        $lifecycleLog = @(Invoke-Adb @("logcat", "-d", "-s", "StasisRenderer:I", "StasisWorkshop:I", "*:S"))
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
        lifecycle_log = $lifecycleLog
        restoration_markers = $restoreCount
        preview_frames = $previewFrames
        captures = $capturePaths
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
        & $adb -s $serial shell cmd window user-rotation free 2>$null | Out-Null
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
