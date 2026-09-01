param(
    [string]$AvdName = "Stasis_API_35",
    [int]$Port = 5554,
    [switch]$Headless,
    [switch]$SkipBuild,
    [int]$RenderTimeoutSeconds = 45,
    [int]$StepTimeoutSeconds = 300,
    [int]$TotalTimeoutSeconds = 900,
    [double]$MaxRenderP50Millis = 0,
    [double]$MaxRenderP95Millis = 0,
    [string]$OutputPath = ""
)

$ErrorActionPreference = "Stop"
$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Split-Path -Parent (Split-Path -Parent $scriptRoot)
$startedAt = [System.Diagnostics.Stopwatch]::StartNew()
$serial = "emulator-$Port"
$startedEmulator = $false
$packages = @("com.stasislang.workshop")

$runningOnWindows = [System.IO.Path]::DirectorySeparatorChar -eq [char]'\'
$androidHome = if ($env:ANDROID_HOME) {
    $env:ANDROID_HOME
} elseif ($env:ANDROID_SDK_ROOT) {
    $env:ANDROID_SDK_ROOT
} else {
    if ($runningOnWindows) { "C:\Android\Sdk" } else {
        Join-Path ([Environment]::GetFolderPath("UserProfile")) "Android/sdk"
    }
}
$adbExecutableSuffix = if ($runningOnWindows) { ".exe" } else { "" }
$adb = Join-Path (Join-Path $androidHome "platform-tools") "adb$adbExecutableSuffix"
if (-not (Test-Path $adb)) { throw "adb was not found: $adb" }

$stamp = (Get-Date).ToUniversalTime().ToString("yyyyMMddTHHmmssZ")
if (-not $OutputPath) {
    $OutputPath = Join-Path (Join-Path (Join-Path $repoRoot "artifacts") "android_workshop_seam") "e"
}
$artifactRoot = [System.IO.Path]::GetFullPath($OutputPath)
New-Item -ItemType Directory -Force -Path $artifactRoot | Out-Null
$toolsCiRoot = Join-Path (Join-Path $repoRoot "tools") "ci"
$renderParityManifest = Join-Path (Join-Path (Join-Path $repoRoot "samples") "render_parity") "capture_manifest.json"
$workshopApk = Join-Path (Join-Path (Join-Path (Join-Path (Join-Path (Join-Path $scriptRoot "app") "build") "outputs") "apk") "workshop") (Join-Path "debug" "app-workshop-debug.apk")

function Assert-In-Time([string]$Step) {
    if ($startedAt.Elapsed.TotalSeconds -gt $TotalTimeoutSeconds) {
        throw "Android render E2E exceeded ${TotalTimeoutSeconds}s after $Step"
    }
}

function Invoke-BoundedScript([string]$Path, [string[]]$Arguments, [string]$Phase) {
    $remainingSeconds = [math]::Floor($TotalTimeoutSeconds - $startedAt.Elapsed.TotalSeconds)
    $stepSeconds = [math]::Min($StepTimeoutSeconds, $remainingSeconds)
    if ($stepSeconds -le 0) { throw "Android render E2E exceeded ${TotalTimeoutSeconds}s before $Phase" }
    $stdout = Join-Path $artifactRoot "$Phase-stdout.log"
    $stderr = Join-Path $artifactRoot "$Phase-stderr.log"
    $hostExecutable = (Get-Process -Id $PID).Path
    $childArguments = @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", $Path) + $Arguments
    $quotedArguments = $childArguments | ForEach-Object {
        if ($_ -match '[\s"]') { '"' + ($_ -replace '"', '\"') + '"' } else { $_ }
    }
    $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $hostExecutable
    $startInfo.Arguments = $quotedArguments -join ' '
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    $process = [System.Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    if (-not $process.Start()) { throw "$Phase could not start" }
    $outputTask = $process.StandardOutput.ReadToEndAsync()
    $errorTask = $process.StandardError.ReadToEndAsync()
    $timedOut = -not $process.WaitForExit($stepSeconds * 1000)
    if ($timedOut) {
        $killWithTree = $process.GetType().GetMethod("Kill", [Type[]]@([bool]))
        if ($null -ne $killWithTree) {
            $process.Kill($true)
        } elseif ($runningOnWindows) {
            & taskkill.exe /PID $process.Id /T /F 2>$null | Out-Null
        } else {
            $process.Kill()
        }
    }
    $process.WaitForExit()
    $outputTask.Result | Set-Content -LiteralPath $stdout -Encoding UTF8
    $errorTask.Result | Set-Content -LiteralPath $stderr -Encoding UTF8
    $output = @($outputTask.Result -split '\r?\n') | Where-Object { $_ }
    $errors = @($errorTask.Result -split '\r?\n') | Where-Object { $_ }
    if ($output) { $output | Write-Output }
    if ($errors) { $errors | Write-Warning }
    if ($timedOut) { throw "$Phase exceeded its ${stepSeconds}s limit; child processes were stopped" }
    if ($process.ExitCode -ne 0) { throw "$Phase failed with exit code $($process.ExitCode)" }
    return $output
}

function Invoke-Adb([string[]]$Arguments) {
    $result = & $adb -s $serial @Arguments
    if ($LASTEXITCODE -ne 0) { throw "adb command failed: $($Arguments -join ' ')" }
    return $result
}

function Invoke-AdbQuiet([string[]]$Arguments) {
    $previousPreference = $ErrorActionPreference
    try {
        $ErrorActionPreference = "Continue"
        & $adb -s $serial @Arguments 2>$null | Out-Null
        $exitCode = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previousPreference
    }
    if ($exitCode -ne 0) { throw "adb command failed: $($Arguments -join ' ')" }
}

function Find-PackageProcessId([string]$Package) {
    $previousPreference = $ErrorActionPreference
    try {
        $ErrorActionPreference = "Continue"
        $result = @(& $adb -s $serial shell pidof $Package 2>$null)
        $exitCode = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previousPreference
    }
    if ($exitCode -ne 0 -or -not $result) { return "" }
    return $result[0].ToString().Trim()
}

function Resolve-Gradle {
    $wrapperName = if ($runningOnWindows) { "gradlew.bat" } else { "gradlew" }
    $wrapper = Join-Path $scriptRoot $wrapperName
    if (Test-Path $wrapper) { return $wrapper }
    $gradleName = if ($runningOnWindows) { "gradle.bat" } else { "gradle" }
    if ($runningOnWindows -and $env:ChocolateyInstall) {
        $installed = Get-ChildItem (Join-Path (Join-Path (Join-Path $env:ChocolateyInstall "lib") "gradle") "tools") `
            -Recurse -Filter gradle.bat -ErrorAction SilentlyContinue |
            Sort-Object FullName -Descending | Select-Object -First 1
        if ($installed) { return $installed.FullName }
    }
    $command = Get-Command $gradleName -CommandType Application -All -ErrorAction SilentlyContinue |
        Where-Object { [System.IO.Path]::GetFileName($_.Source) -eq $gradleName } |
        Select-Object -First 1
    if ($command) { return $command.Source }
    throw "$gradleName was not found; install Gradle 8.9 or newer"
}

function Save-Screenshot([string]$Path) {
    $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $adb
    $startInfo.Arguments = "-s $serial exec-out screencap -p"
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    $process = [System.Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    if (-not $process.Start()) { throw "Android screenshot capture could not start for $serial" }
    $errorTask = $process.StandardError.ReadToEndAsync()
    $output = [System.IO.File]::Create($Path)
    try {
        $process.StandardOutput.BaseStream.CopyTo($output)
    } finally {
        $output.Dispose()
    }
    $process.WaitForExit()
    $errorTask.Result | Set-Content -LiteralPath "$Path.stderr" -Encoding UTF8
    if ($process.ExitCode -ne 0 -or -not (Test-Path $Path) -or (Get-Item $Path).Length -eq 0) {
        throw "Android screenshot capture failed for $serial"
    }
}

function Dismiss-EmulatorSystemAnr([xml]$Tree) {
    $title = @($Tree.SelectNodes("//node[@resource-id='android:id/alertTitle']")) |
        Where-Object {
            $_.GetAttribute("text") -in @(
                "Pixel Launcher isn't responding",
                "System UI isn't responding"
            )
        } |
        Select-Object -First 1
    if (-not $title) { return $false }

    $close = @($Tree.SelectNodes("//node[@resource-id='android:id/aerr_close']")) |
        Select-Object -First 1
    if (-not $close) { return $false }
    $match = [regex]::Match($close.GetAttribute("bounds"), '^\[(\d+),(\d+)\]\[(\d+),(\d+)\]$')
    if (-not $match.Success) { return $false }

    $x = [int][math]::Floor(([int]$match.Groups[1].Value + [int]$match.Groups[3].Value) / 2)
    $y = [int][math]::Floor(([int]$match.Groups[2].Value + [int]$match.Groups[4].Value) / 2)
    Invoke-AdbQuiet @("shell", "input", "tap", "$x", "$y")
    Write-Host "Dismissed unrelated emulator-system ANR; continuing render acceptance"
    return $true
}

function Read-AppWindowBounds([string]$Package) {
    $dump = @(& $adb -s $serial shell dumpsys window windows 2>$null)
    if ($LASTEXITCODE -ne 0) { throw "window manager dump failed" }
    $source = $dump -join "`n"
    $packagePattern = [regex]::Escape($Package)
    $match = [regex]::Match(
        $source,
        "(?ms)^\s*Window #\d+ Window\{[^\r\n]*$packagePattern/[^\r\n]*MainActivity\}:.*?^\s*Frames:.*?\bframe=\[(\d+),(\d+)\]\[(\d+),(\d+)\]"
    )
    if (-not $match.Success) { throw "Workshop app window bounds were absent from window manager state" }
    $left = [int]$match.Groups[1].Value
    $top = [int]$match.Groups[2].Value
    $right = [int]$match.Groups[3].Value
    $bottom = [int]$match.Groups[4].Value
    if ($right -le $left -or $bottom -le $top) { throw "Workshop app window has invalid bounds" }
    $displayDump = @(& $adb -s $serial shell dumpsys window displays 2>$null)
    if ($LASTEXITCODE -ne 0) { throw "window display dump failed" }
    $barMatches = [regex]::Matches(
        ($displayDump -join "`n"),
        'type=(?:statusBars|navigationBars) frame=\[(\d+),(\d+)\]\[(\d+),(\d+)\] visible=true'
    )
    $windowLeft = $left
    $windowTop = $top
    $windowRight = $right
    $windowBottom = $bottom
    foreach ($bar in $barMatches) {
        $barLeft = [int]$bar.Groups[1].Value
        $barTop = [int]$bar.Groups[2].Value
        $barRight = [int]$bar.Groups[3].Value
        $barBottom = [int]$bar.Groups[4].Value
        if ($barLeft -le $windowLeft -and $barRight -ge $windowRight) {
            if ($barTop -le $windowTop) { $top = [math]::Max($top, $barBottom) }
            if ($barBottom -ge $windowBottom) { $bottom = [math]::Min($bottom, $barTop) }
        }
        if ($barTop -le $windowTop -and $barBottom -ge $windowBottom) {
            if ($barLeft -le $windowLeft) { $left = [math]::Max($left, $barRight) }
            if ($barRight -ge $windowRight) { $right = [math]::Min($right, $barLeft) }
        }
    }
    if ($right -le $left -or $bottom -le $top) { throw "Workshop content window has invalid bounds" }
    Write-Host "Accessibility hierarchy unavailable; using visible Workshop app window bounds"
    return @($left, $top, ($right - $left), ($bottom - $top))
}

function Read-SurfaceBounds([string]$Description, [string]$XmlPath, [string]$Package) {
    if (Test-Path -LiteralPath $XmlPath) {
        [xml]$cachedTree = Get-Content -Raw -LiteralPath $XmlPath
        $cachedNode = @($cachedTree.SelectNodes("//node")) |
            Where-Object { $_.GetAttribute("content-desc") -eq $Description } |
            Select-Object -First 1
        if ($cachedNode) {
            $cachedMatch = [regex]::Match(
                $cachedNode.GetAttribute("bounds"),
                '^\[(\d+),(\d+)\]\[(\d+),(\d+)\]$'
            )
            if ($cachedMatch.Success) {
                $cachedLeft = [int]$cachedMatch.Groups[1].Value
                $cachedTop = [int]$cachedMatch.Groups[2].Value
                $cachedRight = [int]$cachedMatch.Groups[3].Value
                $cachedBottom = [int]$cachedMatch.Groups[4].Value
                return @(
                    $cachedLeft,
                    $cachedTop,
                    ($cachedRight - $cachedLeft),
                    ($cachedBottom - $cachedTop)
                )
            }
        }
    }

    Remove-Item -LiteralPath $XmlPath -Force -ErrorAction SilentlyContinue
    $dump = @(& $adb -s $serial exec-out uiautomator dump --compressed /dev/tty 2>$null)
    $xmlMatch = [regex]::Match(($dump -join "`n"), '(?s)<\?xml.*</hierarchy>')
    if ($LASTEXITCODE -ne 0 -or -not $xmlMatch.Success) {
        return Read-AppWindowBounds $Package
    }
    [System.IO.File]::WriteAllText(
        $XmlPath,
        $xmlMatch.Value,
        [System.Text.UTF8Encoding]::new($false)
    )
    [xml]$tree = Get-Content -Raw -LiteralPath $XmlPath
    $node = @($tree.SelectNodes("//node")) |
        Where-Object { $_.GetAttribute("content-desc") -eq $Description } |
        Select-Object -First 1
    if (-not $node) {
        if (Dismiss-EmulatorSystemAnr $tree) {
            throw "unrelated emulator-system ANR was dismissed; waiting for the render surface"
        }
        throw "render surface '$Description' was absent from the Android UI tree"
    }
    $match = [regex]::Match($node.GetAttribute("bounds"), '^\[(\d+),(\d+)\]\[(\d+),(\d+)\]$')
    if (-not $match.Success) { throw "render surface has invalid bounds: $($node.GetAttribute('bounds'))" }
    $left = [int]$match.Groups[1].Value
    $top = [int]$match.Groups[2].Value
    $right = [int]$match.Groups[3].Value
    $bottom = [int]$match.Groups[4].Value
    return @($left, $top, ($right - $left), ($bottom - $top))
}

function Fit-LogicalViewport([int[]]$Surface) {
    $logicalWidth = 640
    $logicalHeight = 360
    $width = $Surface[2]
    $height = $Surface[3]
    if ($width * $logicalHeight -gt $height * $logicalWidth) {
        $viewportWidth = [int][math]::Floor($height * $logicalWidth / $logicalHeight)
        $viewportLeft = $Surface[0] + [int][math]::Floor(($width - $viewportWidth) / 2)
        return @($viewportLeft, $Surface[1], $viewportWidth, $height)
    }
    $viewportHeight = [int][math]::Floor($width * $logicalHeight / $logicalWidth)
    $viewportTop = $Surface[1] + [int][math]::Floor(($height - $viewportHeight) / 2)
    return @($Surface[0], $viewportTop, $width, $viewportHeight)
}

function Assert-RenderedVariant(
    [string]$Name,
    [string]$Package,
    [string]$Apk,
    [string]$SurfaceDescription,
    [bool]$RequireJit
) {
    if (-not (Test-Path $Apk)) { throw "$Name APK was not found: $Apk" }
    Invoke-Adb @("install", "-r", $Apk) | Out-Null
    Invoke-Adb @("shell", "pm", "clear", $Package) | Out-Null
    Invoke-Adb @("logcat", "-c") | Out-Null
    Invoke-Adb @("shell", "am", "start", "-W", "-n",
        "$Package/com.stasislang.workshop.MainActivity") | Out-Null

    $capture = Join-Path $artifactRoot "$Name.png"
    $uiTree = Join-Path $artifactRoot "$Name-window.xml"
    $deadline = (Get-Date).AddSeconds($RenderTimeoutSeconds)
    $lastFailure = "render did not become ready"
    $renderPassed = $false
    $stableCaptures = 0
    $processId = ""
    $viewportArg = ""
    $viewportResolved = $false
    $logFile = Join-Path $artifactRoot "$Name-logcat.txt"
    $log = @()
    try {
        do {
            Start-Sleep -Seconds 2
            $processId = Find-PackageProcessId $Package
            if (-not $processId) {
                $lastFailure = "$Name process has not started"
                continue
            }
            try {
                if (-not $viewportResolved) {
                    $surface = Read-SurfaceBounds $SurfaceDescription $uiTree $Package
                    $viewport = Fit-LogicalViewport $surface
                    $viewportArg = ($viewport | ForEach-Object { $_.ToString() }) -join ","
                    $viewportResolved = $true
                }
                Write-Host "$Name viewport=$viewportArg"
                Save-Screenshot $capture
                & python (Join-Path $toolsCiRoot "verify_render_parity.py") `
                    --capture $capture --capture-only --profile android_emulator `
                    "--viewport=$viewportArg" --viewport-y-search-radius=32
                if ($LASTEXITCODE -eq 0) {
                    $stableCaptures += 1
                    if ($stableCaptures -ge 3) {
                        $renderPassed = $true
                        break
                    }
                } else {
                    $stableCaptures = 0
                    $lastFailure = "Android render-parity regions did not match"
                }
            } catch {
                $stableCaptures = 0
                $lastFailure = $_.Exception.Message
                if ($lastFailure -eq "unrelated emulator-system ANR was dismissed; waiting for the render surface") {
                    Invoke-Adb @("shell", "am", "start", "-W", "-n",
                        "$Package/com.stasislang.workshop.MainActivity") | Out-Null
                }
            }
        } while ((Get-Date) -lt $deadline)
    } finally {
        if ($processId) { $log = @(& $adb -s $serial logcat "--pid=$processId" -d 2>$null) }
        if (-not $processId -or $LASTEXITCODE -ne 0) {
            $log = @(& $adb -s $serial logcat -d 2>$null)
        }
        $log | Set-Content -LiteralPath $logFile -Encoding UTF8
        & $adb -s $serial shell am force-stop $Package 2>$null | Out-Null
    }
    $fatalPatterns = @(
        "native preview frame failed",
        "Render resource error",
        "resource restore failed",
        "FATAL EXCEPTION"
    )
    # IT-031 intentionally records the real missing-resource diagnostic as a
    # bounded case line. Let the strict seam verifier validate that JSON while
    # keeping malformed/ambient matching text fatal here.
    $fatalScanLog = @($log | ForEach-Object {
        $line = $_
        if ($line -match 'Stasis Workshop IT-031 case:\s+(\{.*\})\s*$') {
            $markerText = $Matches[0]
            try {
                $case = $Matches[1] | ConvertFrom-Json -ErrorAction Stop
                if (($case.test_id -eq "IT-031") -and $case.name -and ($case.equal -eq $true) `
                        -and ($null -ne $case.native) -and ($null -ne $case.ui)) {
                    $markerIndex = $line.IndexOf($markerText)
                    if ($markerIndex -ge 0) {
                        $line = $line.Remove($markerIndex, $markerText.Length)
                    }
                }
            } catch {
                # Leave malformed case lines in the fatal scan.
            }
        }
        $line
    })
    $fatal = $fatalScanLog | Select-String -SimpleMatch $fatalPatterns
    if ($fatal) { throw "$Name logged a rendering/runtime failure; see $logFile" }
    $frameCounts = [regex]::Matches(($log -join "`n"), 'RenderAcceptanceFrame: count=(\d+)') |
        ForEach-Object { [int]$_.Groups[1].Value }
    if (-not $frameCounts -or ($frameCounts | Measure-Object -Maximum).Maximum -lt 30) {
        throw "$Name did not prove 30 actively rendered acceptance frames; see $logFile"
    }
    if ($RequireJit -and -not ($log -match 'CompileReady: backend=cranelift-jit reload=InitialCompile status=0 functions=[1-9][0-9]*')) {
        throw "$Name did not log a successful non-empty Workshop JIT compile; see $logFile"
    }
    $observedAvdLine = Invoke-Adb @("emu", "avd", "name") | Select-Object -First 1
    $observedAvd = if ($null -eq $observedAvdLine) { "" } else { $observedAvdLine.Trim() }
    if (-not $observedAvd) {
        $observedAvdLine = Invoke-Adb @("shell", "getprop", "ro.boot.qemu.avd_name") |
            Select-Object -First 1
        $observedAvd = if ($null -eq $observedAvdLine) { "" } else { $observedAvdLine.Trim() }
    }
    $observedSdk = (Invoke-Adb @("shell", "getprop", "ro.build.version.sdk") | Select-Object -First 1).Trim()
    if ($observedAvd -ne $AvdName -or $observedSdk -ne "35") {
        throw "$Name benchmark identity mismatch: requested=$AvdName/API35 observed=$observedAvd/API$observedSdk"
    }
    $sourceStatus = @(& git -C $repoRoot status --porcelain)
    if ($LASTEXITCODE -ne 0) { throw "Git source status could not be read for benchmark evidence" }
    if ($sourceStatus) {
        throw "$Name benchmark source is dirty; commit or remove source changes before publishing evidence"
    }
    $packageDump = @(Invoke-Adb @("shell", "dumpsys", "package", $Package)) -join "`n"
    $versionName = [regex]::Match($packageDump, '(?m)^\s*versionName=([^\r\n]+)').Groups[1].Value.Trim()
    $versionCode = [regex]::Match($packageDump, '(?m)^\s*versionCode=(\d+)').Groups[1].Value
    $metadataPath = Join-Path $artifactRoot "$Name-performance-metadata.json"
    @{
        scene = "render_parity"
        git_revision = (& git -C $repoRoot rev-parse HEAD).Trim()
        source_dirty = $false
        apk_sha256 = (Get-FileHash -LiteralPath $Apk -Algorithm SHA256).Hash.ToLowerInvariant()
        package_version = "$versionName ($versionCode)"
        device_model = (Invoke-Adb @("shell", "getprop", "ro.product.model") | Select-Object -First 1).Trim()
        device_fingerprint = (Invoke-Adb @("shell", "getprop", "ro.build.fingerprint") | Select-Object -First 1).Trim()
        serial = $serial
        avd = $observedAvd
        android_sdk = [int]$observedSdk
    } | ConvertTo-Json | Set-Content -LiteralPath $metadataPath -Encoding UTF8
    $performanceArguments = @(
        (Join-Path $toolsCiRoot "verify_android_render_performance.py"),
        "--log", $logFile,
        "--metadata", $metadataPath,
        "--evidence", (Join-Path $artifactRoot "$Name-performance.json")
    )
    if ($MaxRenderP50Millis -gt 0) {
        $performanceArguments += @("--max-p50-ms", "$MaxRenderP50Millis")
    }
    if ($MaxRenderP95Millis -gt 0) {
        $performanceArguments += @("--max-p95-ms", "$MaxRenderP95Millis")
    }
    & python @performanceArguments
    if ($LASTEXITCODE -ne 0) {
        throw "$Name render performance evidence failed; see $logFile"
    }
    if (-not $renderPassed) {
        throw "$Name render acceptance timed out: $lastFailure; see $artifactRoot"
    }
    & python (Join-Path $toolsCiRoot "verify_android_workshop_seam.py") `
        --log $logFile --capture $capture --manifest $renderParityManifest `
        --apk $Apk --metadata $metadataPath --evidence (Join-Path $artifactRoot "$Name-workshop-seam.json")
    if ($LASTEXITCODE -ne 0) { throw "$Name IT-025 Workshop seam verification failed; see $logFile" }
    Write-Output "$Name render acceptance passed: $capture"
}

try {
    $runningBefore = @(& $adb devices) | Where-Object {
        $_ -match "^$([regex]::Escape($serial))\s+device(?:\s|$)"
    }
    $startedEmulator = -not [bool]$runningBefore
    if ($startedEmulator) {
        $emulatorArguments = @("-AvdName", $AvdName, "-Port", "$Port")
        if ($Headless) { $emulatorArguments += "-Headless" }
        $serial = Invoke-BoundedScript (Join-Path $scriptRoot "start_emulator.ps1") `
            $emulatorArguments "start-emulator"
        $serial = @($serial) | Select-Object -Last 1
    } else {
        Write-Host "Reusing ready Android emulator $serial"
    }

    if (-not $SkipBuild) {
        $gradle = Resolve-Gradle
        Invoke-BoundedScript (Join-Path $scriptRoot "build_debug.ps1") @(
            "-RenderAcceptance", "-SkipCodexNative",
            "-NoGradleDaemon", "-GradlePath", $gradle
        ) "build-workshop" | Out-Null
        Assert-In-Time "Workshop build"
    }

    Assert-RenderedVariant "workshop" "com.stasislang.workshop" `
        $workshopApk `
        "Interactive Stasis game preview. Touch the game to control it." $true
    Assert-In-Time "render acceptance"

    @{
        status = "passed"
        serial = $serial
        avd = $AvdName
        elapsed_seconds = [math]::Round($startedAt.Elapsed.TotalSeconds, 3)
        workshop = "Workshop JIT"
    } | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $artifactRoot "summary.json") -Encoding UTF8
    Write-Output "Android Workshop rendering passed in $([math]::Round($startedAt.Elapsed.TotalSeconds, 1))s"
} catch {
    @{
        status = "failed"
        serial = $serial
        avd = $AvdName
        elapsed_seconds = [math]::Round($startedAt.Elapsed.TotalSeconds, 3)
        error = $_.Exception.Message
    } | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $artifactRoot "summary.json") -Encoding UTF8
    throw
} finally {
    foreach ($package in $packages) {
        & $adb -s $serial shell am force-stop $package 2>$null | Out-Null
    }
    if ($startedEmulator) {
        & $adb -s $serial emu kill 2>$null | Out-Null
    }
}
