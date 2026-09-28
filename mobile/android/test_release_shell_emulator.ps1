param(
    [string]$Serial = $env:ANDROID_SERIAL,
    [string]$ArtifactRoot = "artifacts",
    [string]$TestId = "",
    [int]$PerSeamTimeoutSeconds = 660,
    [int]$HostRuntimeBuildTimeoutSeconds = 900
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
    },
    @{
        TestId = "IT-032"
        Project = "samples/android_nested_self_mutation_seam"
        Output = "android_nested_self_mutation"
        RequiredScalarBindingSymbol = "stasis_state_scalar__state__zzz_commands__count"
        MinimumScalarBindingOrdinal = 2049
    },
    @{
        TestId = "ANDROID-GENERICS"
        Project = "samples/generics_collections"
        Output = "android_generics_collections"
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

$sourceCommit = $env:STASIS_SOURCE_COMMIT
if (-not $sourceCommit) {
    $sourceCommit = (& git -C $repoRoot rev-parse HEAD).Trim()
    if ($LASTEXITCODE -ne 0 -or -not $sourceCommit) {
        throw "Unable to resolve the source commit for the Android emulator run"
    }
}
$releaseId = $env:STASIS_RELEASE_ID
if (-not $releaseId) { $releaseId = "android-emulator-$sourceCommit" }
$buildFingerprint = (& python (Join-Path $repoRoot "tools/compute_toolchain_fingerprint.py") `
    --source-commit $sourceCommit --release-id $releaseId).Trim()
if ($LASTEXITCODE -ne 0 -or $buildFingerprint -notmatch '^[0-9a-f]{64}$') {
    throw "Unable to compute the verified toolchain fingerprint for the Android emulator run"
}
if ($env:STASIS_BUILD_FINGERPRINT -and $env:STASIS_BUILD_FINGERPRINT -ne $buildFingerprint) {
    throw "STASIS_BUILD_FINGERPRINT does not match STASIS_SOURCE_COMMIT and STASIS_RELEASE_ID"
}
$env:STASIS_SOURCE_COMMIT = $sourceCommit
$env:STASIS_RELEASE_ID = $releaseId
$env:STASIS_BUILD_FINGERPRINT = $buildFingerprint

function Invoke-BoundedCMake([string[]]$Arguments, [string]$Phase) {
    $remainingSeconds = [math]::Floor(
        $script:hostRuntimeBuildTimeoutSeconds - $script:hostRuntimeBuildTimer.Elapsed.TotalSeconds
    )
    if ($remainingSeconds -le 0) {
        throw "Android host runtime build exceeded its $($script:hostRuntimeBuildTimeoutSeconds)s budget before $Phase"
    }
    $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $script:cmakeExecutable
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    foreach ($argument in $Arguments) {
        $startInfo.ArgumentList.Add($argument)
    }
    $process = [System.Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    if (-not $process.Start()) { throw "$Phase could not start" }
    $stdoutTask = $process.StandardOutput.ReadToEndAsync()
    $stderrTask = $process.StandardError.ReadToEndAsync()
    $timedOut = -not $process.WaitForExit([int]($remainingSeconds * 1000))
    if ($timedOut) {
        $process.Kill($true)
        $process.WaitForExit(5000) | Out-Null
    }
    $stdout = $stdoutTask.GetAwaiter().GetResult()
    $stderr = $stderrTask.GetAwaiter().GetResult()
    $exitCode = $process.ExitCode
    $process.Dispose()
    if ($stdout) { Write-Host $stdout }
    if ($stderr) { Write-Host $stderr }
    if ($timedOut) {
        throw "Android host runtime $Phase exceeded its bounded build budget"
    }
    if ($exitCode -ne 0) {
        throw "Android host runtime $Phase failed with exit code ${exitCode}"
    }
}

if ($env:STASIS_RUNTIME_LIBRARY_PATH) {
    if (-not [System.IO.Path]::IsPathRooted($env:STASIS_RUNTIME_LIBRARY_PATH)) {
        $env:STASIS_RUNTIME_LIBRARY_PATH = [System.IO.Path]::GetFullPath(
            (Join-Path $repoRoot $env:STASIS_RUNTIME_LIBRARY_PATH)
        )
    }
    if (-not (Test-Path -LiteralPath $env:STASIS_RUNTIME_LIBRARY_PATH -PathType Leaf)) {
        throw "Configured STASIS_RUNTIME_LIBRARY_PATH does not exist: $env:STASIS_RUNTIME_LIBRARY_PATH"
    }
} else {
    if (-not $env:STASIS_SDL3_SOURCE -or -not $env:STASIS_SDL3_IMAGE_SOURCE) {
        throw "Set STASIS_SDL3_SOURCE and STASIS_SDL3_IMAGE_SOURCE to the pinned source trees"
    }
    if (-not (Test-Path -LiteralPath $env:STASIS_SDL3_SOURCE -PathType Container) -or
            -not (Test-Path -LiteralPath $env:STASIS_SDL3_IMAGE_SOURCE -PathType Container)) {
        throw "The configured SDL3/SDL3_image source paths must both exist"
    }
    $cmakeCommand = Get-Command cmake -CommandType Application -ErrorAction SilentlyContinue |
        Select-Object -First 1
    if (-not $cmakeCommand) { throw "cmake is required to build the matching Android emulator host runtime" }
    $script:cmakeExecutable = $cmakeCommand.Source
    $script:hostRuntimeBuildTimeoutSeconds = $HostRuntimeBuildTimeoutSeconds
    if ($script:hostRuntimeBuildTimeoutSeconds -le 0) {
        throw "HostRuntimeBuildTimeoutSeconds must be positive"
    }
    $script:hostRuntimeBuildTimer = [System.Diagnostics.Stopwatch]::StartNew()
    $runtimeBuildDirectory = Join-Path $repoRoot "target/android-emulator-host-runtime"
    $configureArguments = @(
        "-S", (Join-Path $repoRoot "runtime"),
        "-B", $runtimeBuildDirectory,
        "-DCMAKE_BUILD_TYPE=Release",
        "-DSTASIS_GRAPHICS_BUILD_SHARED=ON",
        "-DSTASIS_GRAPHICS_BUILD_STATIC=OFF",
        "-DSTASIS_GRAPHICS_BUNDLE_SDL=ON",
        "-DSTASIS_BUILD_RUNNER=OFF",
        "-DSTASIS_BUILD_SYS=OFF",
        "-DSTASIS_RELEASE_ID=$releaseId",
        "-DSTASIS_BUILD_FINGERPRINT=$buildFingerprint"
    )
    Invoke-BoundedCMake $configureArguments "configure"
    Invoke-BoundedCMake @(
        "--build", $runtimeBuildDirectory, "--config", "Release",
        "--target", "stasis_graphics", "--parallel", "2"
    ) "compile"
    $runtimeOutputDirectory = Join-Path $runtimeBuildDirectory "bin"
    if ($runningOnWindows) {
        $runtimeOutputDirectory = Join-Path $runtimeOutputDirectory "Release"
        $runtimeFileName = "stasis_graphics.dll"
    } elseif ($IsMacOS) {
        $runtimeFileName = "libstasis_graphics.dylib"
    } else {
        $runtimeFileName = "libstasis_graphics.so"
    }
    $env:STASIS_RUNTIME_LIBRARY_PATH = [System.IO.Path]::GetFullPath(
        (Join-Path $runtimeOutputDirectory $runtimeFileName)
    )
    if (-not (Test-Path -LiteralPath $env:STASIS_RUNTIME_LIBRARY_PATH -PathType Leaf)) {
        throw "Matching Android emulator host runtime was not produced: $env:STASIS_RUNTIME_LIBRARY_PATH"
    }
}

foreach ($seam in $selectedSeams) {
    $seamTimeout = if ($seam.TestId -eq "IT-022") {
        900
    } elseif ($seam.TestId -eq "ANDROID-GENERICS") {
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
        -RequiredScalarBindingSymbol $(if ($seam.RequiredScalarBindingSymbol) {
            $seam.RequiredScalarBindingSymbol
        } else { "" }) `
        -MinimumScalarBindingOrdinal $(if ($seam.MinimumScalarBindingOrdinal) {
            $seam.MinimumScalarBindingOrdinal
        } else { 0 }) `
        -TotalTimeoutSeconds $seamTimeout
    if ($LASTEXITCODE -ne 0) {
        throw "Android emulator seam $($seam.Project) failed with exit code $LASTEXITCODE"
    }
}

Write-Output "Android emulator release-shell seams passed on $Serial"
