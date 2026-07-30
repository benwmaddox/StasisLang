param(
    [string]$AvdName = "Stasis_API_35",
    [int]$Port = 5554,
    [int]$BootTimeoutSeconds = 180,
    [switch]$Headless,
    [switch]$WipeData
)

$ErrorActionPreference = "Stop"

$androidHome = if ($env:ANDROID_HOME) {
    $env:ANDROID_HOME
} elseif ($env:ANDROID_SDK_ROOT) {
    $env:ANDROID_SDK_ROOT
} else {
    "C:\Android\Sdk"
}
$emulator = Join-Path $androidHome "emulator\emulator.exe"
$adb = Join-Path $androidHome "platform-tools\adb.exe"
if (-not (Test-Path $emulator)) { throw "Android Emulator was not found: $emulator" }
if (-not (Test-Path $adb)) { throw "adb.exe was not found: $adb" }
if (-not $env:ANDROID_AVD_HOME -and $env:USERPROFILE) {
    $defaultAvdHome = Join-Path $env:USERPROFILE ".android\avd"
    if (Test-Path $defaultAvdHome) { $env:ANDROID_AVD_HOME = $defaultAvdHome }
}

$serial = "emulator-$Port"
$deviceLines = @(& $adb devices)
$isRunning = [bool]($deviceLines | Where-Object { $_ -match "^$([regex]::Escape($serial))\s+device(?:\s|$)" })
if (-not $isRunning) {
    $availableAvds = @(& $emulator -list-avds)
    if ($availableAvds -notcontains $AvdName) {
        throw "Android virtual device '$AvdName' was not found. Create the API 35 x86_64 AVD described in README.md."
    }
    $emulatorArgs = @("-avd", $AvdName, "-port", "$Port", "-no-audio", "-no-boot-anim", "-netdelay", "none", "-netspeed", "full")
    if ($Headless) { $emulatorArgs += "-no-window" }
    if ($WipeData) { $emulatorArgs += "-wipe-data" }
    Start-Process -FilePath $emulator -ArgumentList $emulatorArgs | Out-Null
    Write-Host "Started $AvdName as $serial"
}

$deadline = (Get-Date).AddSeconds($BootTimeoutSeconds)
do {
    $deviceLines = @(& $adb devices)
    $deviceReady = [bool]($deviceLines | Where-Object { $_ -match "^$([regex]::Escape($serial))\s+device(?:\s|$)" })
    $booted = if ($deviceReady) {
        $bootValue = & $adb -s $serial shell getprop sys.boot_completed 2>$null | Select-Object -First 1
        if ($null -ne $bootValue) { $bootValue.ToString().Trim() } else { "" }
    } else {
        ""
    }
    if ($booted -eq "1") { break }
    if ((Get-Date) -ge $deadline) { throw "Android emulator did not finish booting within $BootTimeoutSeconds seconds" }
    Start-Sleep -Seconds 2
} while ($true)

& $adb -s $serial shell input keyevent 82 | Out-Null
Write-Host "$AvdName is ready"
Write-Output $serial
