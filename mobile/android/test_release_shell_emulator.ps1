param(
    [string]$Serial = $env:ANDROID_SERIAL,
    [string]$ArtifactRoot = "artifacts",
    [int]$PerSeamTimeoutSeconds = 840
)

$ErrorActionPreference = "Stop"
$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Split-Path -Parent (Split-Path -Parent $scriptRoot)
$isWindows = [System.IO.Path]::DirectorySeparatorChar -eq [char]'\'
$executableSuffix = if ($isWindows) { ".exe" } else { "" }
$androidHome = if ($env:ANDROID_HOME) {
    $env:ANDROID_HOME
} elseif ($env:ANDROID_SDK_ROOT) {
    $env:ANDROID_SDK_ROOT
} elseif (-not $isWindows) {
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
        Project = "samples/android_aot_seam"
        Output = "android_release_shell"
    },
    @{
        Project = "samples/android_touch_seam"
        Output = "android_touch_roundtrip"
    },
    @{
        Project = "samples/android_orientation_seam"
        Output = "android_orientation_metrics"
    }
)

foreach ($seam in $seams) {
    & (Join-Path $scriptRoot "test_release_shell.ps1") `
        -Serial $Serial `
        -ProjectPath $seam.Project `
        -Target android-x86_64 `
        -OutputPath (Join-Path $artifactRootPath $seam.Output) `
        -TotalTimeoutSeconds $PerSeamTimeoutSeconds
    if ($LASTEXITCODE -ne 0) {
        throw "Android emulator seam $($seam.Project) failed with exit code $LASTEXITCODE"
    }
}

Write-Output "Android emulator release-shell seams passed on $Serial"
