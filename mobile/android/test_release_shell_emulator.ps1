param(
    [string]$Serial = $env:ANDROID_SERIAL,
    [string]$ArtifactRoot = "artifacts",
    [string]$TestId = "",
    [int]$PerSeamTimeoutSeconds = 660
)

$ErrorActionPreference = "Stop"
$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Split-Path -Parent (Split-Path -Parent $scriptRoot)
$runningOnWindows = [System.IO.Path]::DirectorySeparatorChar -eq [char]'\'
$executableSuffix = if ($runningOnWindows) { ".exe" } else { "" }
$androidHome = if ($env:ANDROID_HOME) {
    $env:ANDROID_HOME
} elseif ($env:ANDROID_SDK_ROOT) {
    $env:ANDROID_SDK_ROOT
} elseif (-not $runningOnWindows) {
    Join-Path ([Environment]::GetFolderPath("UserProfile")) "Library/Android/sdk"
} else {
    "C:\Android\Sdk"
}
$adb = Join-Path (Join-Path $androidHome "platform-tools") "adb$executableSuffix"
if (-not (Test-Path $adb)) { throw "adb was not found: $adb" }

if (-not $Serial) {
    $emulators = @(& $adb devices) | ForEach-Object {
        if ($_ -match '^(emulator-\d+)\s+device(?:\s|$)') { $Matches[1] }
    }
    if ($emulators.Count -ne 1) {
        throw "Expected exactly one ready Android emulator, found $($emulators.Count)"
    }
    $Serial = $emulators[0]
}
if ($Serial -notmatch '^emulator-\d+$') {
    throw "Android CI seams require an emulator serial, got '$Serial'"
}

$abiList = (& $adb -s $Serial shell getprop ro.product.cpu.abilist).Trim() -split ','
if ($LASTEXITCODE -ne 0 -or "x86_64" -notin $abiList) {
    throw "Android CI seams require an x86_64 emulator; $Serial reports '$($abiList -join ',')'"
}

$artifactRootPath = if ([System.IO.Path]::IsPathRooted($ArtifactRoot)) {
    [System.IO.Path]::GetFullPath($ArtifactRoot)
} else {
    [System.IO.Path]::GetFullPath((Join-Path $repoRoot $ArtifactRoot))
}
$seams = @(
    @{
        TestId = "IT-020"
        Project = "samples/android_resource_restore_seam"
        Output = "android_resource_restore"
    },
    @{
        TestId = "IT-017"
        Project = "samples/android_aot_seam"
        Output = "android_release_shell"
    },
    @{
        TestId = "IT-018"
        Project = "samples/android_touch_seam"
        Output = "android_touch_roundtrip"
    },
    @{
        TestId = "IT-019"
        Project = "samples/android_orientation_seam"
        Output = "android_orientation_metrics"
    },
    @{
        TestId = "IT-021"
        Project = "samples/android_packaged_assets_seam"
        Output = "android_packaged_assets"
    },
    @{
        TestId = "IT-022"
        Project = "samples/android_packaged_assets_seam"
        Expectations = "samples/android_asset_rejection_seam/android_seam_expectations.json"
        Output = "android_asset_rejection"
    },
    @{
        TestId = "IT-023"
        Project = "samples/android_storage_seam"
        Output = "android_storage_persistence"
    },
    @{
        TestId = "IT-024"
        Project = "samples/android_lifecycle_failure_seam/main"
        Output = "android_entry_failures/main"
    },
    @{
        TestId = "IT-024"
        Project = "samples/android_lifecycle_failure_seam/tick"
        Output = "android_entry_failures/tick"
    },
    @{
        TestId = "IT-024"
        Project = "samples/android_lifecycle_failure_seam/render"
        Output = "android_entry_failures/render"
    }
)

$validTestIds = @($seams | ForEach-Object { $_.TestId })
if ($TestId -and $TestId -notin $validTestIds) {
    throw "Unknown Android release-shell seam test ID '$TestId'; expected one of $($validTestIds -join ', ')"
}
$selectedSeams = if ($TestId) {
    @($seams | Where-Object { $_.TestId -eq $TestId })
} else {
    $seams
}

foreach ($seam in $selectedSeams) {
    $seamTimeout = if ($seam.TestId -eq "IT-022") {
        900
    } elseif ($seam.TestId -eq "IT-024") {
        360
    } else {
        $PerSeamTimeoutSeconds
    }
    & (Join-Path $scriptRoot "test_release_shell.ps1") `
        -Serial $Serial `
        -ProjectPath $seam.Project `
        -Target android-x86_64 `
        -OutputPath (Join-Path $artifactRootPath $seam.Output) `
        -ExpectationsPath $(if ($seam.Expectations) { $seam.Expectations } else { "" }) `
        -TotalTimeoutSeconds $seamTimeout
    if ($LASTEXITCODE -ne 0) {
        throw "Android emulator seam $($seam.Project) failed with exit code $LASTEXITCODE"
    }
}

Write-Output "Android emulator release-shell seams passed on $Serial"
