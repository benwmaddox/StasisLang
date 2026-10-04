param(
    [string]$AvdName = "Stasis_API_35",
    [int]$Port = 5554,
    [switch]$Headless,
    [switch]$SkipBuild,
    [int]$RenderTimeoutSeconds = 45,
    [int]$StartupReadinessTimeoutSeconds = 90,
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
$script:workshopReadinessElapsedSeconds = 0
$script:workshopCaptureElapsedSeconds = 0
$script:workshopCaptureEffectiveTimeoutSeconds = 0

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

function Get-RemainingPresentationTimeout([int]$RequestedSeconds) {
    $remainingSeconds = [math]::Floor($TotalTimeoutSeconds - $startedAt.Elapsed.TotalSeconds)
    if ($remainingSeconds -le 0) {
        throw ("Android render E2E exceeded " + $TotalTimeoutSeconds + "s during presentation-baseline acceptance")
    }
    return [int][math]::Min($RequestedSeconds, $remainingSeconds)
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
    $phaseTimer = [System.Diagnostics.Stopwatch]::StartNew()
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
    $phaseTimer.Stop()
    $outputTask.Result | Set-Content -LiteralPath $stdout -Encoding UTF8
    $errorTask.Result | Set-Content -LiteralPath $stderr -Encoding UTF8
    $phaseElapsedSeconds = [math]::Round($phaseTimer.Elapsed.TotalSeconds, 3)
    $phaseTimingPath = Join-Path $artifactRoot "${Phase}-timing.json"
    @{
        phase = $Phase
        elapsed_seconds = $phaseElapsedSeconds
        limit_seconds = $stepSeconds
        timed_out = $timedOut
        exit_code = $process.ExitCode
        stdout_log = $stdout
        stderr_log = $stderr
    } | ConvertTo-Json | Set-Content -LiteralPath $phaseTimingPath -Encoding UTF8
    Write-Host "$Phase elapsed ${phaseElapsedSeconds}s (limit ${stepSeconds}s; timed_out=$timedOut)"
    $output = @($outputTask.Result -split '\r?\n') | Where-Object { $_ }
    $errors = @($errorTask.Result -split '\r?\n') | Where-Object { $_ }
    if ($output) { $output | Write-Output }
    if ($errors) { $errors | Write-Warning }
    if ($timedOut) {
        throw "$Phase exceeded its ${stepSeconds}s limit after ${phaseElapsedSeconds}s; child processes were stopped"
    }
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

function Get-WorkshopIT032ReadinessState([string[]]$LogLines) {
    $marker = "Stasis Workshop IT-032: "
    foreach ($line in $LogLines) {
        $markerIndex = $line.IndexOf($marker, [System.StringComparison]::Ordinal)
        if ($markerIndex -lt 0) { continue }
        $jsonText = $line.Substring($markerIndex + $marker.Length).Trim()
        try {
            $receipt = $jsonText | ConvertFrom-Json -ErrorAction Stop
        } catch {
            throw "IT-032 terminal receipt is malformed: $($_.Exception.Message)"
        }
        if ($receipt.schema -ne "stasis.workshop_soak.v1" -or
            $receipt.test_id -ne "IT-032" -or $receipt.event -ne "bounded_soak") {
            throw "IT-032 terminal receipt has an unexpected schema or identity"
        }
        if ($receipt.status -eq "failed") {
            throw "IT-032 reported failure: $($receipt.error)"
        }
        if ($receipt.status -ne "passed") {
            throw "IT-032 terminal receipt did not report passed status"
        }
        if ($receipt.cleanup_receipt.status -ne "Restored") {
            throw "IT-032 terminal receipt did not confirm restored cleanup"
        }
        return "ready"
    }
    return "pending"
}

function Wait-ForWorkshopIT032Readiness(
    [int]$TimeoutMilliseconds,
    [int]$PollIntervalMilliseconds,
    [scriptblock]$ReadProcessId,
    [scriptblock]$ReadProcessLog,
    [scriptblock]$OnPending = {}
) {
    $TimeoutSeconds = Get-RemainingPresentationTimeout $TimeoutSeconds
    $timer = [System.Diagnostics.Stopwatch]::StartNew()
    do {
        $processId = ([string](& $ReadProcessId | Select-Object -First 1)).Trim()
        if ($processId) {
            $logLines = @(& $ReadProcessLog $processId)
            $state = Get-WorkshopIT032ReadinessState $logLines
            if ($state -eq "ready") {
                return [pscustomobject]@{
                    ProcessId = $processId
                    ElapsedSeconds = [math]::Round($timer.Elapsed.TotalSeconds, 3)
                }
            }
            & $OnPending $processId
        }
        $remainingMilliseconds = $TimeoutMilliseconds - [int]$timer.ElapsedMilliseconds
        if ($remainingMilliseconds -gt 0) {
            Start-Sleep -Milliseconds ([math]::Min($PollIntervalMilliseconds, $remainingMilliseconds))
        }
    } while ($timer.ElapsedMilliseconds -lt $TimeoutMilliseconds)
    throw "Workshop IT-032 terminal readiness receipt was not observed within ${TimeoutMilliseconds}ms"
}

function Take-WorkshopSurfaceProbe([hashtable]$State) {
    $now = Get-Date
    if ($now -lt $State.NextProbeAt) { return $false }
    $State.NextProbeAt = $now.AddSeconds(5)
    return $true
}

function Fit-LogicalViewport([int[]]$Surface) {
    $logicalWidth = 640
    $logicalHeight = 360
    $width = $Surface[2]
    $height = $Surface[3]
    if ($width * $logicalHeight -gt $height * $logicalWidth) {
        # Match Java Math.round for the drawable width; GL's x origin is also screenshot-left.
        $viewportWidth = [int][math]::Floor(($height * $logicalWidth / $logicalHeight) + 0.5)
        $viewportLeft = $Surface[0] + [int][math]::Floor(($width - $viewportWidth) / 2)
        return @($viewportLeft, $Surface[1], $viewportWidth, $height)
    }
    # Match renderer Math.round for height and convert its bottom-origin GL y to screenshot top-origin.
    $viewportHeight = [int][math]::Floor(($width * $logicalHeight / $logicalWidth) + 0.5)
    $viewportTop = $Surface[1] + [int][math]::Ceiling(($height - $viewportHeight) / 2.0)
    return @($Surface[0], $viewportTop, $width, $viewportHeight)
}

$script:presentationBaselinePrefixes = @(
    @{ kind = "control"; prefix = "Stasis Workshop presentation-baseline control: " },
    @{ kind = "poison"; prefix = "Stasis Workshop presentation-baseline: " },
    @{ kind = "submission"; prefix = "Stasis Workshop presentation-baseline submission: " },
    @{ kind = "renderer"; prefix = "Stasis Workshop present-only baseline: " }
)

function Read-PresentationBaselineReceipts([string]$Package) {
    $processId = Find-PackageProcessId $Package
    if (-not $processId) { return [pscustomobject]@{ process_id = ""; receipts = @() } }
    $lines = @(& $adb -s $serial logcat "--pid=$processId" -d 2>$null)
    if ($LASTEXITCODE -ne 0) { throw "presentation-baseline logcat read failed" }
    $receipts = [System.Collections.Generic.List[object]]::new()
    foreach ($line in $lines) {
        foreach ($spec in $script:presentationBaselinePrefixes) {
            $markerIndex = $line.IndexOf($spec.prefix, [System.StringComparison]::Ordinal)
            if ($markerIndex -lt 0) { continue }
            $jsonText = $line.Substring($markerIndex + $spec.prefix.Length).Trim()
            try {
                $receipt = $jsonText | ConvertFrom-Json -ErrorAction Stop
            } catch {
                throw "presentation-baseline $($spec.kind) receipt is malformed: $($_.Exception.Message)"
            }
            if ($receipt.schema -ne "stasis.workshop_present_only.v1") {
                throw "presentation-baseline $($spec.kind) receipt has an unexpected schema"
            }
            $receiptKind = if ($spec.kind -eq "poison" -and $receipt.event -eq "accepted_snapshot_replayed") {
                "renderer"
            } else {
                $spec.kind
            }
            $receipt | Add-Member -NotePropertyName receipt_kind -NotePropertyValue $receiptKind -Force
            [void]$receipts.Add($receipt)
            break
        }
    }
    return [pscustomobject]@{ process_id = $processId; receipts = @($receipts.ToArray()) }
}

function Wait-PresentationBaselineReceipt(
    [string]$Package,
    [scriptblock]$Predicate,
    [int]$AfterReceiptCount,
    [int]$TimeoutSeconds,
    [string]$Description,
    [string]$InitialProcessId = "",
    [bool]$AllowProcessRestart = $false
) {
    $TimeoutSeconds = Get-RemainingPresentationTimeout $TimeoutSeconds
    $timer = [System.Diagnostics.Stopwatch]::StartNew()
    do {
        $state = Read-PresentationBaselineReceipts $Package
        if (-not $state.process_id) {
            $remaining = ($TimeoutSeconds * 1000) - [int]$timer.ElapsedMilliseconds
            if ($remaining -le 0) { break }
            Start-Sleep -Milliseconds ([math]::Min(250, $remaining))
            continue
        }
        if ($InitialProcessId -and $state.process_id -ne $InitialProcessId -and -not $AllowProcessRestart) {
            throw "$Description changed Workshop process unexpectedly"
        }
        $startAt = if ($InitialProcessId -and $state.process_id -ne $InitialProcessId) {
            0
        } else {
            [math]::Min($AfterReceiptCount, $state.receipts.Count)
        }
        for ($index = $startAt; $index -lt $state.receipts.Count; $index += 1) {
            $receipt = $state.receipts[$index]
            if (& $Predicate $receipt) {
                return [pscustomobject]@{
                    process_id = $state.process_id
                    receipt = $receipt
                    receipts = $state.receipts
                    marker_count = $state.receipts.Count
                    elapsed_seconds = [math]::Round($timer.Elapsed.TotalSeconds, 3)
                }
            }
        }
        $remaining = ($TimeoutSeconds * 1000) - [int]$timer.ElapsedMilliseconds
        if ($remaining -gt 0) { Start-Sleep -Milliseconds ([math]::Min(250, $remaining)) }
    } while ($timer.Elapsed.TotalSeconds -lt $TimeoutSeconds)
    throw ($Description + " was not observed within " + $TimeoutSeconds + "s")
}

function Assert-PresentationPresentReceipt([object]$Receipt) {
    if ($Receipt.receipt_kind -ne "renderer" -or $Receipt.event -ne "present" -or
        $Receipt.test_id -ne "PRESENTATION-BASELINE" -or [int]$Receipt.flags -ne 2 -or
        $Receipt.physical_target_poisoned -ne $true) {
        throw "presentation-baseline PRESENT receipt is missing identity, flags, or physical poison proof"
    }
    $logicalSize = @($Receipt.logical_size | ForEach-Object { [int]$_ })
    $rect = @($Receipt.rect | ForEach-Object { [int]$_ })
    $rgba = @($Receipt.rgba | ForEach-Object { [double]$_ })
    $surfaceSize = @($Receipt.surface_size | ForEach-Object { [int]$_ })
    if ($logicalSize.Count -ne 2 -or $logicalSize[0] -ne 640 -or $logicalSize[1] -ne 360 -or
        $rect.Count -ne 4 -or $rect[0] -ne 80 -or $rect[1] -ne 45 -or
        $rect[2] -ne 160 -or $rect[3] -ne 90 -or
        $rgba.Count -ne 4 -or [math]::Abs($rgba[0] - 0.9) -gt 0.001 -or
        [math]::Abs($rgba[1] - 0.15) -gt 0.001 -or
        [math]::Abs($rgba[2] - 0.08) -gt 0.001 -or
        [math]::Abs($rgba[3] - 1.0) -gt 0.001 -or
        $surfaceSize.Count -ne 2 -or $surfaceSize[0] -le 0 -or $surfaceSize[1] -le 0 -or
        [int]$Receipt.sequence -notin @(1, 2) -or
        $null -eq $Receipt.surface_generation -or $null -eq $Receipt.renderer_generation -or
        $null -eq $Receipt.display_generation -or $null -eq $Receipt.frame_token -or
        $null -eq $Receipt.presentation_serial) {
        throw "presentation-baseline PRESENT receipt does not match the frozen 640x360 scene contract"
    }
}

function Wait-PresentationBaselinePresentPair(
    [string]$Package,
    [int]$AfterReceiptCount,
    [int]$TimeoutSeconds,
    [string]$Description,
    [string]$InitialProcessId = "",
    [bool]$AllowProcessRestart = $false
) {
    $TimeoutSeconds = Get-RemainingPresentationTimeout $TimeoutSeconds
    $timer = [System.Diagnostics.Stopwatch]::StartNew()
    do {
        $state = Read-PresentationBaselineReceipts $Package
        if (-not $state.process_id) {
            $remaining = ($TimeoutSeconds * 1000) - [int]$timer.ElapsedMilliseconds
            if ($remaining -le 0) { break }
            Start-Sleep -Milliseconds ([math]::Min(250, $remaining))
            continue
        }
        if ($InitialProcessId -and $state.process_id -ne $InitialProcessId -and -not $AllowProcessRestart) {
            throw "$Description changed Workshop process unexpectedly"
        }
        $startAt = if ($InitialProcessId -and $state.process_id -ne $InitialProcessId) {
            0
        } else {
            [math]::Min($AfterReceiptCount, $state.receipts.Count)
        }
        $newPresents = @()
        for ($index = $startAt; $index -lt $state.receipts.Count; $index += 1) {
            $receipt = $state.receipts[$index]
            if ($receipt.receipt_kind -eq "renderer" -and $receipt.event -eq "present") {
                Assert-PresentationPresentReceipt $receipt
                $newPresents += $receipt
            }
        }
        $groups = @($newPresents | Group-Object {
            "{0}/{1}/{2}/{3}x{4}" -f $_.surface_generation, $_.renderer_generation,
                $_.display_generation, $_.surface_size[0], $_.surface_size[1]
        })
        foreach ($group in $groups) {
            $first = @($group.Group | Where-Object { [int]$_.sequence -eq 1 } | Select-Object -First 1)
            $second = @($group.Group | Where-Object { [int]$_.sequence -eq 2 } | Select-Object -Last 1)
            if ($first.Count -gt 0 -and $second.Count -gt 0) {
                if ([string]$first[0].frame_token -eq [string]$second[0].frame_token) {
                    throw "$Description repeated a frame token instead of presenting a fresh second frame"
                }
                if ([long]$second[0].presentation_serial -le [long]$first[0].presentation_serial) {
                    throw "$Description did not advance the accepted presentation serial"
                }
                return [pscustomobject]@{
                    process_id = $state.process_id
                    first = $first[0]
                    second = $second[0]
                    receipts = $state.receipts
                    marker_count = $state.receipts.Count
                    elapsed_seconds = [math]::Round($timer.Elapsed.TotalSeconds, 3)
                }
            }
        }
        $remaining = ($TimeoutSeconds * 1000) - [int]$timer.ElapsedMilliseconds
        if ($remaining -gt 0) { Start-Sleep -Milliseconds ([math]::Min(250, $remaining)) }
    } while ($timer.Elapsed.TotalSeconds -lt $TimeoutSeconds)
    throw ($Description + " did not produce two new poisoned PRESENT receipts within " + $TimeoutSeconds + "s")
}

function Send-PresentationBaselineControl([string]$Package, [string]$Phase) {
    Invoke-Adb @(
        "shell", "am", "broadcast", "-a", "com.stasislang.workshop.PRESENTATION_BASELINE_TEST",
        "-p", $Package, "--es", "phase", $Phase
    ) | Out-Null
}

function Wait-PresentationBaselineControl(
    [string]$Package,
    [string]$Phase,
    [int]$AfterReceiptCount,
    [string]$ProcessId
) {
    $predicate = {
        param($receipt)
        $receipt.receipt_kind -eq "control" -and
            $receipt.phase -eq $Phase -and $receipt.status -eq "ready"
    }.GetNewClosure()
    return Wait-PresentationBaselineReceipt $Package $predicate $AfterReceiptCount $RenderTimeoutSeconds ("presentation-baseline " + $Phase + " control acknowledgement") $ProcessId
}

function Get-AndroidWindowSizeState {
    $output = @(Invoke-Adb @("shell", "wm", "size"))
    $text = $output -join [Environment]::NewLine
    $physicalMatch = [regex]::Match($text, '(?m)^\s*Physical size:\s*(\d+)x(\d+)\s*$')
    $overrideMatch = [regex]::Match($text, '(?m)^\s*Override size:\s*(\d+)x(\d+)\s*$')
    if (-not $physicalMatch.Success) { throw "emulator wm size did not report physical dimensions" }
    $physical = @([int]$physicalMatch.Groups[1].Value, [int]$physicalMatch.Groups[2].Value)
    $override = if ($overrideMatch.Success) {
        @([int]$overrideMatch.Groups[1].Value, [int]$overrideMatch.Groups[2].Value)
    } else {
        @()
    }
    $effective = if ($override.Count -eq 2) { $override } else { $physical }
    return [pscustomobject]@{ physical = $physical; override = $override; effective = $effective }
}

function Restore-AndroidWindowSize([object]$State) {
    if ($State.override.Count -eq 2) {
        Invoke-AdbQuiet @("shell", "wm", "size", "$($State.override[0])x$($State.override[1])")
    } else {
        Invoke-AdbQuiet @("shell", "wm", "size", "reset")
    }
}

function Save-AndVerifyPresentationBaselineCapture(
    [string]$Package,
    [string]$SurfaceDescription,
    [string]$Stage,
    [string]$Mode,
    [object]$Receipt
) {
    $uiTree = Join-Path $artifactRoot "presentation-baseline-$Stage-window.xml"
    Remove-Item -LiteralPath $uiTree -Force -ErrorAction SilentlyContinue
    $surface = @(Read-SurfaceBounds $SurfaceDescription $uiTree $Package)
    $surfaceArg = ($surface | ForEach-Object { $_.ToString() }) -join ","
    if ($Receipt.surface_size) {
        $receiptSize = @($Receipt.surface_size | ForEach-Object { [int]$_ })
        if ($receiptSize.Count -ne 2 -or
            $receiptSize[0] -ne $surface[2] -or $receiptSize[1] -ne $surface[3]) {
            throw "$Stage UI surface and renderer receipt dimensions disagree"
        }
    }
    $capture = Join-Path $artifactRoot "presentation-baseline-$Stage.png"
    $evidence = Join-Path $artifactRoot "presentation-baseline-$Stage.json"
    $verificationLog = Join-Path $artifactRoot "presentation-baseline-$Stage-verify.log"
    $arguments = @(
        (Join-Path $toolsCiRoot "verify_presentation_baseline.py"),
        "--capture", $capture, "--surface", $surfaceArg,
        "--mode", $Mode, "--evidence", $evidence
    )
    $viewport = @()
    if ($Mode -in @("present", "present-alpha-probe")) {
        $viewport = @(Fit-LogicalViewport $surface)
        $viewportArg = ($viewport | ForEach-Object { $_.ToString() }) -join ","
        $arguments += @("--viewport", $viewportArg)
    }
    $captureAttempts = 0
    $verificationPassed = $false
    while ($captureAttempts -lt 5 -and -not $verificationPassed) {
        Assert-In-Time ("during " + $Stage + " presentation-baseline capture")
        $captureAttempts += 1
        Save-Screenshot $capture
        $verificationOutput = @(& python @arguments 2>&1)
        $verificationExitCode = $LASTEXITCODE
        $verificationOutput | Set-Content -LiteralPath $verificationLog -Encoding UTF8
        if ($verificationExitCode -eq 0) {
            $verificationPassed = $true
        } elseif ($captureAttempts -lt 5) {
            Start-Sleep -Milliseconds 200
        }
    }
    if (-not $verificationPassed) {
        throw "$Stage presentation-baseline pixel verification failed after five captures; see $verificationLog"
    }
    $pixelEvidence = Get-Content -Raw -LiteralPath $evidence | ConvertFrom-Json -ErrorAction Stop
    return [pscustomobject]@{
        stage = $Stage
        mode = $Mode
        capture_attempts = $captureAttempts
        capture = $capture
        surface = $surface
        viewport = $viewport
        capture_sha256 = (Get-FileHash -LiteralPath $capture -Algorithm SHA256).Hash.ToLowerInvariant()
        pixel_evidence = $pixelEvidence
        renderer_receipt = $Receipt
    }
}

function Assert-PresentationPoisonReceipt([object]$Receipt, [int[]]$ExpectedSurfaceSize) {
    $size = @($Receipt.surface_size | ForEach-Object { [int]$_ })
    if ($Receipt.receipt_kind -ne "poison" -or $Receipt.event -ne "backbuffer_poison" -or
        $Receipt.physical_target_poisoned -ne $true -or $size.Count -ne 2 -or
        $size[0] -ne $ExpectedSurfaceSize[0] -or $size[1] -ne $ExpectedSurfaceSize[1]) {
        throw "presentation-baseline poison receipt did not cover the current physical surface"
    }
}

function Assert-PresentationFaultReceipt([object]$Receipt, [string]$Phase) {
    $expectedFlags = if ($Phase -eq "no_present") { 0 } else { 2 }
    $expectedFrameValid = $Phase -eq "no_present"
    if ($Receipt.receipt_kind -ne "submission" -or $Receipt.phase -ne $Phase -or
        [int]$Receipt.status -ne 0 -or [int]$Receipt.flags -ne $expectedFlags -or
        $Receipt.frame_valid -ne $expectedFrameValid -or $Receipt.requests_present -ne $false -or
        $Receipt.fault_applied -ne $true -or $Receipt.draw_scheduled -ne $false -or
        [string]::IsNullOrEmpty([string]$Receipt.frame_token) -or
        ($Phase -eq "reject" -and [int]$Receipt.magic -ne 0) -or
        ($Phase -eq "no_present" -and [int]$Receipt.magic -eq 0)) {
        throw ("presentation-baseline " + $Phase + " receipt did not prove a non-publishing native submission")
    }
}

function Assert-PresentationForcedRedrawControl([object]$Receipt, [object]$FaultReceipt, [string]$FaultPhase) {
    $expectedFrameValid = $FaultPhase -eq "no_present"
    if ($Receipt.receipt_kind -ne "control" -or $Receipt.phase -ne "forced_redraw" -or
        $Receipt.status -ne "scheduled" -or $Receipt.frame_valid -ne $expectedFrameValid -or
        $Receipt.requests_present -ne $false -or
        [string]$Receipt.frame_token -ne [string]$FaultReceipt.frame_token -or
        [int]$Receipt.flags -ne [int]$FaultReceipt.flags -or
        [int]$Receipt.magic -ne [int]$FaultReceipt.magic) {
        throw ("presentation-baseline forced redraw after " + $FaultPhase +
            " did not report the expected valid/present state")
    }
}

function Assert-PresentationSnapshotReplay(
    [object]$Receipt,
    [object]$FaultReceipt,
    [object]$LastPresent,
    [string]$FaultPhase
) {
    $expectedFrameValid = $FaultPhase -eq "no_present"
    $expectedFlags = if ($FaultPhase -eq "no_present") { 0 } else { 2 }
    $expectedSurface = @($LastPresent.surface_size | ForEach-Object { [int]$_ })
    $actualSurface = @($Receipt.surface_size | ForEach-Object { [int]$_ })
    if ($Receipt.receipt_kind -ne "renderer" -or $Receipt.event -ne "accepted_snapshot_replayed" -or
        $Receipt.phase -ne $FaultPhase -or $Receipt.frame_valid -ne $expectedFrameValid -or
        $Receipt.requests_present -ne $false -or [int]$Receipt.flags -ne $expectedFlags -or
        [string]$Receipt.frame_token -ne [string]$FaultReceipt.frame_token -or
        [int]$Receipt.magic -ne [int]$FaultReceipt.magic -or
        [string]$Receipt.snapshot_frame_token -ne [string]$LastPresent.frame_token -or
        $null -eq $Receipt.snapshot_presentation_serial -or
        $null -eq $Receipt.presentation_serial -or
        [long]$Receipt.snapshot_presentation_serial -ne [long]$LastPresent.presentation_serial -or
        [long]$Receipt.presentation_serial -ne [long]$LastPresent.presentation_serial -or
        $expectedSurface.Count -ne 2 -or $actualSurface.Count -ne 2 -or
        ($expectedSurface -join ",") -ne ($actualSurface -join ",") -or
        [long]$Receipt.surface_generation -ne [long]$LastPresent.surface_generation -or
        [long]$Receipt.renderer_generation -ne [long]$LastPresent.renderer_generation) {
        throw ("presentation-baseline " + $FaultPhase +
            " forced redraw did not replay the last accepted snapshot with the rejected frame state")
    }
}

function Assert-PresentationAlphaProbeReceipt([object]$Receipt, [object]$LastPresent) {
    $probeLogical = @($Receipt.probe.logical | ForEach-Object { [int]$_ })
    $expectedRgba = @($Receipt.probe.expected_rgba8 | ForEach-Object { [int]$_ })
    $logicalSize = @($Receipt.logical_size | ForEach-Object { [int]$_ })
    $expectedSurface = @($LastPresent.surface_size | ForEach-Object { [int]$_ })
    $actualSurface = @($Receipt.surface_size | ForEach-Object { [int]$_ })
    $rects = @($Receipt.rects)
    $firstRect = if ($rects.Count -gt 0) { @($rects[0] | ForEach-Object { [double]$_ }) } else { @() }
    $secondRect = if ($rects.Count -gt 1) { @($rects[1] | ForEach-Object { [double]$_ }) } else { @() }
    $firstRectExpected = @(80, 45, 160, 90, 0.9, 0.15, 0.08, 1.0)
    $secondRectExpected = @(90, 55, 20, 20, 0.0, 1.0, 0.0, 0.5)
    $rectsMatch = $firstRect.Count -eq 8 -and $secondRect.Count -eq 8
    for ($index = 0; $rectsMatch -and $index -lt 8; $index += 1) {
        $rectsMatch = [math]::Abs($firstRect[$index] - $firstRectExpected[$index]) -le 0.001 -and
            [math]::Abs($secondRect[$index] - $secondRectExpected[$index]) -le 0.001
    }
    if ($Receipt.receipt_kind -ne "renderer" -or $Receipt.event -ne "alpha_probe_present" -or
        $Receipt.test_id -ne "PRESENTATION-BASELINE" -or
        [int]$Receipt.flags -ne 2 -or $Receipt.physical_target_poisoned -ne $true -or
        [string]::IsNullOrEmpty([string]$Receipt.frame_token) -or
        [string]$Receipt.frame_token -eq [string]$LastPresent.frame_token -or
        $null -eq $Receipt.presentation_serial -or
        [long]$Receipt.presentation_serial -ne ([long]$LastPresent.presentation_serial + 1) -or
        $logicalSize.Count -ne 2 -or ($logicalSize -join ",") -ne "640,360" -or
        $probeLogical.Count -ne 2 -or ($probeLogical -join ",") -ne "100,65" -or
        $expectedRgba.Count -ne 4 -or ($expectedRgba -join ",") -ne "115,147,10,255" -or
        -not $rectsMatch -or $expectedSurface.Count -ne 2 -or $actualSurface.Count -ne 2 -or
        ($expectedSurface -join ",") -ne ($actualSurface -join ",") -or
        [long]$Receipt.surface_generation -ne [long]$LastPresent.surface_generation -or
        [long]$Receipt.renderer_generation -ne [long]$LastPresent.renderer_generation) {
        throw "presentation-baseline alpha probe receipt did not describe a fresh blended probe frame"
    }
}

function Assert-PresentationResetReceipt([object]$Receipt) {
    $size = @($Receipt.surface_size | ForEach-Object { [int]$_ })
    $background = @($Receipt.background_rgba8 | ForEach-Object { [int]$_ })
    $label = @($Receipt.restore_label_rgba8 | ForEach-Object { [int]$_ })
    if ($Receipt.receipt_kind -ne "renderer" -or
        $Receipt.event -ne "surface_reset_placeholder" -or
        $Receipt.initialized -ne $true -or $Receipt.restore_label_drawn -ne $true -or
        $size.Count -ne 2 -or $size[0] -le 0 -or $size[1] -le 0 -or
        ($background -join ",") -ne "15,20,28,255" -or
        ($label -join ",") -ne "66,153,225,255" -or
        $null -eq $Receipt.surface_generation -or $null -eq $Receipt.renderer_generation) {
        throw "presentation-baseline resize receipt did not describe the initialized dark-and-blue reset"
    }
}

function Assert-PresentationBaselineMatrix(
    [string]$Package,
    [string]$SurfaceDescription,
    [string]$Apk
) {
    Assert-In-Time "before Android presentation-baseline matrix"
    if ($AvdName -ne "Stasis_API_35") {
        throw "presentation-baseline matrix requires the dedicated Stasis_API_35 emulator"
    }
    $matrixPath = Join-Path $artifactRoot "presentation-baseline-matrix.json"
    $matrix = [ordered]@{
        schema = "stasis.workshop_present_only_android_matrix.v1"
        package = $Package
        apk = [System.IO.Path]::GetFullPath($Apk)
        apk_sha256 = (Get-FileHash -LiteralPath $Apk -Algorithm SHA256).Hash.ToLowerInvariant()
        avd = $AvdName
        serial = $serial
        android_sdk = [int](Invoke-Adb @("shell", "getprop", "ro.build.version.sdk") | Select-Object -First 1)
        device_model = ((Invoke-Adb @("shell", "getprop", "ro.product.model") | Select-Object -First 1).ToString().Trim())
        build_fingerprint = ((Invoke-Adb @("shell", "getprop", "ro.build.fingerprint") | Select-Object -First 1).ToString().Trim())
        captures = [System.Collections.Generic.List[object]]::new()
        stages = [System.Collections.Generic.List[object]]::new()
        raw_logs = [System.Collections.Generic.List[string]]::new()
        restore_errors = [System.Collections.Generic.List[string]]::new()
    }
    $displaySizeState = $null
    $originalRotation = ""
    $originalAutoRotation = ""
    $processId = ""
    $status = "failed"
    try {
        Invoke-Adb @("shell", "am", "force-stop", $Package) | Out-Null
        Invoke-Adb @("logcat", "-c") | Out-Null
        Invoke-Adb @("shell", "am", "start", "-W", "-n", "$Package/com.stasislang.workshop.MainActivity", "--ez", "stasis_presentation_baseline", "true", "--ez", "stasis_presentation_poison", "true") | Out-Null

        $coldPredicate = {
            param($receipt)
            $receipt.receipt_kind -eq "renderer" -and $receipt.event -eq "present"
        }
        $cold = Wait-PresentationBaselineReceipt $Package $coldPredicate 0 $RenderTimeoutSeconds "cold no-CLEAR PRESENT"
        $processId = $cold.process_id
        Assert-PresentationPresentReceipt $cold.receipt
        $matrix.captures.Add((Save-AndVerifyPresentationBaselineCapture $Package $SurfaceDescription "cold-present" "present" $cold.receipt))

        $coldPair = Wait-PresentationBaselinePresentPair $Package 0 $RenderTimeoutSeconds "cold/repeat PRESENT pair" $processId
        $matrix.stages.Add([pscustomobject]@{
            name = "cold-and-repeat-present"
            process_id = $processId
            first = $coldPair.first
            second = $coldPair.second
            elapsed_seconds = $coldPair.elapsed_seconds
        })
        $matrix.captures.Add((Save-AndVerifyPresentationBaselineCapture $Package $SurfaceDescription "repeat-present" "present" $coldPair.second))
        $lastPresent = $coldPair.second

        $surfaceXml = Join-Path $artifactRoot "presentation-baseline-current-window.xml"
        Remove-Item -LiteralPath $surfaceXml -Force -ErrorAction SilentlyContinue
        $currentSurface = @(Read-SurfaceBounds $SurfaceDescription $surfaceXml $Package)
        $beforePause = (Read-PresentationBaselineReceipts $Package).receipts.Count
        Send-PresentationBaselineControl $Package "pause"
        $pause = Wait-PresentationBaselineControl $Package "pause" $beforePause $processId
        $pausedPresents = @($pause.receipts | Where-Object {
            $_.receipt_kind -eq "renderer" -and $_.event -eq "present"
        })
        if ($pausedPresents.Count -eq 0) { throw "pause acknowledgement had no accepted PRESENT baseline" }
        $lastPresent = $pausedPresents[$pausedPresents.Count - 1]
        Assert-PresentationPresentReceipt $lastPresent
        $matrix.stages.Add([pscustomobject]@{
            name = "pause"
            receipt = $pause.receipt
            last_accepted_present = $lastPresent
        })

        foreach ($faultPhase in @("no_present", "reject")) {
            $beforePoison = (Read-PresentationBaselineReceipts $Package).receipts.Count
            Send-PresentationBaselineControl $Package "poison"
            $poisonPredicate = {
                param($receipt)
                $receipt.receipt_kind -eq "poison" -and $receipt.event -eq "backbuffer_poison"
            }
            $poison = Wait-PresentationBaselineReceipt $Package $poisonPredicate $beforePoison $RenderTimeoutSeconds ($faultPhase + " physical-target poison") $processId
            Assert-PresentationPoisonReceipt $poison.receipt @($currentSurface[2], $currentSurface[3])

            $beforeFault = $poison.marker_count
            Send-PresentationBaselineControl $Package $faultPhase
            $phaseToWait = $faultPhase
            $faultPredicate = {
                param($receipt)
                $receipt.receipt_kind -eq "submission" -and $receipt.phase -eq $phaseToWait
            }.GetNewClosure()
            $fault = Wait-PresentationBaselineReceipt $Package $faultPredicate $beforeFault $RenderTimeoutSeconds ($faultPhase + " non-publishing submission") $processId
            Assert-PresentationFaultReceipt $fault.receipt $faultPhase
            $beforeRedraw = $fault.marker_count
            Send-PresentationBaselineControl $Package "redraw"
            $forcedRedrawPredicate = {
                param($receipt)
                $receipt.receipt_kind -eq "control" -and $receipt.phase -eq "forced_redraw"
            }
            $forcedRedraw = Wait-PresentationBaselineReceipt $Package $forcedRedrawPredicate $beforeRedraw $RenderTimeoutSeconds ($faultPhase + " forced redraw acknowledgement") $processId
            Assert-PresentationForcedRedrawControl $forcedRedraw.receipt $fault.receipt $faultPhase
            $replayPredicate = {
                param($receipt)
                $receipt.receipt_kind -eq "renderer" -and $receipt.event -eq "accepted_snapshot_replayed"
            }
            $replay = Wait-PresentationBaselineReceipt $Package $replayPredicate $forcedRedraw.marker_count $RenderTimeoutSeconds ($faultPhase + " accepted snapshot replay") $processId
            Assert-PresentationSnapshotReplay $replay.receipt $fault.receipt $lastPresent $faultPhase
            $unexpectedPresents = @($replay.receipts | Select-Object -Skip $beforeRedraw | Where-Object {
                $_.receipt_kind -eq "renderer" -and $_.event -eq "present"
            })
            if ($unexpectedPresents.Count -gt 0) {
                throw "forced snapshot replay advanced the accepted PRESENT stream after $faultPhase"
            }
            $matrix.stages.Add([pscustomobject]@{
                name = $faultPhase
                poison = $poison.receipt
                submission = $fault.receipt
                forced_redraw = $forcedRedraw.receipt
                snapshot_replay = $replay.receipt
            })
            $matrix.captures.Add((Save-AndVerifyPresentationBaselineCapture $Package $SurfaceDescription "retained-after-$faultPhase" "present" $lastPresent))
            if ($faultPhase -eq "reject") {
                $beforeAlphaProbe = (Read-PresentationBaselineReceipts $Package).receipts.Count
                Send-PresentationBaselineControl $Package "alpha_after_replay"
                $alphaPredicate = {
                    param($receipt)
                    $receipt.receipt_kind -eq "renderer" -and $receipt.event -eq "alpha_probe_present"
                }
                $alphaProbe = Wait-PresentationBaselineReceipt $Package $alphaPredicate $beforeAlphaProbe $RenderTimeoutSeconds "alpha probe after accepted-snapshot replay" $processId
                Assert-PresentationAlphaProbeReceipt $alphaProbe.receipt $lastPresent
                $matrix.stages.Add([pscustomobject]@{
                    name = "alpha-probe-after-replay"
                    source_present = $lastPresent
                    receipt = $alphaProbe.receipt
                    elapsed_seconds = $alphaProbe.elapsed_seconds
                })
                $matrix.captures.Add((Save-AndVerifyPresentationBaselineCapture $Package $SurfaceDescription "alpha-probe-after-replay" "present-alpha-probe" $alphaProbe.receipt))
            }
        }

        $displaySizeState = Get-AndroidWindowSizeState
        if ($displaySizeState.effective[0] -lt 800 -or $displaySizeState.effective[1] -lt 1000) {
            throw "Stasis_API_35 display is too small for a resized presentation-baseline capture"
        }
        $resizedWidth = [int][math]::Floor(($displaySizeState.effective[0] * 0.75) / 2) * 2
        $resizedHeight = [int][math]::Floor(($displaySizeState.effective[1] * 0.75) / 2) * 2
        $beforeReset = (Read-PresentationBaselineReceipts $Package).receipts.Count
        Invoke-Adb @("shell", "wm", "size", "$($resizedWidth)x$($resizedHeight)") | Out-Null
        $resetPredicate = {
            param($receipt)
            $receipt.receipt_kind -eq "renderer" -and $receipt.event -eq "surface_reset_placeholder"
        }
        $reset = Wait-PresentationBaselineReceipt $Package $resetPredicate $beforeReset $RenderTimeoutSeconds "resized surface reset placeholder" $processId
        Assert-PresentationResetReceipt $reset.receipt
        $resetSize = @($reset.receipt.surface_size | ForEach-Object { [int]$_ })
        if ($resetSize[0] -eq $currentSurface[2] -and $resetSize[1] -eq $currentSurface[3]) {
            throw "wm size did not change the Workshop rendering surface dimensions"
        }
        $matrix.stages.Add([pscustomobject]@{
            name = "resize-reset-placeholder"
            requested_size = @($resizedWidth, $resizedHeight)
            receipt = $reset.receipt
        })
        $matrix.captures.Add((Save-AndVerifyPresentationBaselineCapture $Package $SurfaceDescription "resize-reset-placeholder" "reset-placeholder" $reset.receipt))

        $beforeResume = (Read-PresentationBaselineReceipts $Package).receipts.Count
        Send-PresentationBaselineControl $Package "resume"
        $resume = Wait-PresentationBaselineControl $Package "resume" $beforeResume $processId
        $resizedPair = Wait-PresentationBaselinePresentPair $Package $beforeResume $RenderTimeoutSeconds "post-resize PRESENT pair" $processId
        $lastPresent = $resizedPair.second
        $matrix.stages.Add([pscustomobject]@{
            name = "post-resize-present"
            first = $resizedPair.first
            second = $resizedPair.second
            resume_ack = $resume.receipt
            elapsed_seconds = $resizedPair.elapsed_seconds
        })
        $matrix.captures.Add((Save-AndVerifyPresentationBaselineCapture $Package $SurfaceDescription "post-resize-present" "present" $lastPresent))

        $beforeHome = (Read-PresentationBaselineReceipts $Package).receipts.Count
        Invoke-Adb @("shell", "input", "keyevent", "KEYCODE_HOME") | Out-Null
        Start-Sleep -Milliseconds 900
        $background = Read-PresentationBaselineReceipts $Package
        if ($background.process_id -ne $processId) { throw "Workshop process changed while backgrounded" }
        $backgroundPresents = @($background.receipts | Select-Object -Skip $beforeHome | Where-Object { $_.receipt_kind -eq "renderer" -and $_.event -eq "present" })
        if ($backgroundPresents.Count -gt 0) { throw "renderer published a test PRESENT while Workshop was backgrounded" }
        Invoke-Adb @("shell", "am", "start", "-W", "--activity-single-top", "-n", "$Package/com.stasislang.workshop.MainActivity") | Out-Null
        $homePair = Wait-PresentationBaselinePresentPair $Package $background.receipts.Count $RenderTimeoutSeconds "HOME/background-resume PRESENT pair" $processId
        $lastPresent = $homePair.second
        $matrix.stages.Add([pscustomobject]@{
            name = "home-resume"
            process_id = $homePair.process_id
            first = $homePair.first
            second = $homePair.second
            elapsed_seconds = $homePair.elapsed_seconds
        })
        $matrix.captures.Add((Save-AndVerifyPresentationBaselineCapture $Package $SurfaceDescription "home-resume-present" "present" $lastPresent))

        $oldProcessId = $processId
        $oldLogPath = Join-Path $artifactRoot "presentation-baseline-process-$oldProcessId-logcat.txt"
        @(Invoke-Adb @("logcat", "--pid=$oldProcessId", "-d")) | Set-Content -LiteralPath $oldLogPath -Encoding UTF8
        $matrix.raw_logs.Add($oldLogPath)
        Invoke-Adb @("shell", "am", "force-stop", $Package) | Out-Null
        $stopDeadline = (Get-Date).AddSeconds(10)
        do {
            $stoppedProcessId = Find-PackageProcessId $Package
            if (-not $stoppedProcessId) { break }
            Start-Sleep -Milliseconds 250
        } while ((Get-Date) -lt $stopDeadline)
        if ($stoppedProcessId) { throw "Workshop process did not stop for surface-loss relaunch" }
        Invoke-Adb @("logcat", "-c") | Out-Null
        Invoke-Adb @("shell", "am", "start", "-W", "-n", "$Package/com.stasislang.workshop.MainActivity", "--ez", "stasis_presentation_baseline", "true", "--ez", "stasis_presentation_poison", "true") | Out-Null
        $relaunchPair = Wait-PresentationBaselinePresentPair $Package 0 $RenderTimeoutSeconds "surface-loss relaunch PRESENT pair" $oldProcessId $true
        if ($relaunchPair.process_id -eq $oldProcessId) { throw "surface-loss relaunch reused the prior Workshop process" }
        $surfaceCreated = @($relaunchPair.receipts | Where-Object { $_.receipt_kind -eq "renderer" -and $_.event -eq "surface_created_initialized" } | Select-Object -First 1)
        $surfaceCreatedSize = if ($surfaceCreated.Count -gt 0) {
            @($surfaceCreated[0].surface_size | ForEach-Object { [int]$_ })
        } else { @() }
        if ($surfaceCreated.Count -eq 0 -or $surfaceCreated[0].initialized -ne $true -or
            $surfaceCreated[0].restore_label_drawn -ne $false -or
            $surfaceCreatedSize.Count -ne 2 -or $surfaceCreatedSize[0] -le 0 -or $surfaceCreatedSize[1] -le 0 -or
            $null -eq $surfaceCreated[0].surface_generation -or $null -eq $surfaceCreated[0].renderer_generation) {
            throw "fresh process did not report initialization of its recreated GL surface"
        }
        $processId = $relaunchPair.process_id
        $lastPresent = $relaunchPair.second
        $matrix.stages.Add([pscustomobject]@{
            name = "surface-loss-relaunch"
            previous_process_id = $oldProcessId
            process_id = $processId
            surface_created = $surfaceCreated[0]
            first = $relaunchPair.first
            second = $relaunchPair.second
            elapsed_seconds = $relaunchPair.elapsed_seconds
        })
        $matrix.captures.Add((Save-AndVerifyPresentationBaselineCapture $Package $SurfaceDescription "surface-loss-relaunch-present" "present" $lastPresent))

        $originalAutoRotation = (Invoke-Adb @("shell", "settings", "get", "system", "accelerometer_rotation") | Select-Object -First 1).Trim()
        $originalRotation = (Invoke-Adb @("shell", "settings", "get", "system", "user_rotation") | Select-Object -First 1).Trim()
        if ($originalAutoRotation -notmatch '^\d+$' -or $originalRotation -notmatch '^\d+$') {
            throw "emulator rotation settings could not be read for safe restoration"
        }
        Invoke-Adb @("shell", "settings", "put", "system", "accelerometer_rotation", "0") | Out-Null
        foreach ($rotation in @(1, 3)) {
            $beforeRotation = (Read-PresentationBaselineReceipts $Package).receipts.Count
            Invoke-Adb @("shell", "settings", "put", "system", "user_rotation", "$rotation") | Out-Null
            $actualRotation = (Invoke-Adb @("shell", "settings", "get", "system", "user_rotation") | Select-Object -First 1).Trim()
            if ($actualRotation -ne "$rotation") { throw "emulator did not apply requested rotation $rotation" }
            $rotationPair = Wait-PresentationBaselinePresentPair $Package $beforeRotation $RenderTimeoutSeconds ("rotation-" + $rotation + " PRESENT pair") $processId $true
            if ([int]$rotationPair.first.surface_size[0] -le [int]$rotationPair.first.surface_size[1]) {
                throw ("rotation " + $rotation + " did not produce a landscape renderer surface")
            }
            $processId = $rotationPair.process_id
            $lastPresent = $rotationPair.second
            $matrix.stages.Add([pscustomobject]@{
                name = "landscape-rotation-$rotation"
                requested_user_rotation = $rotation
                applied_user_rotation = [int]$actualRotation
                first = $rotationPair.first
                second = $rotationPair.second
                elapsed_seconds = $rotationPair.elapsed_seconds
            })
            $matrix.captures.Add((Save-AndVerifyPresentationBaselineCapture $Package $SurfaceDescription "landscape-rotation-$rotation" "present" $lastPresent))
        }
        $status = "passed"
    } finally {
        if ($processId) {
            $finalLogPath = Join-Path $artifactRoot "presentation-baseline-process-$processId-logcat.txt"
            $finalLog = @(& $adb -s $serial logcat "--pid=$processId" -d 2>$null)
            if ($LASTEXITCODE -eq 0) {
                $finalLog | Set-Content -LiteralPath $finalLogPath -Encoding UTF8
                if (-not $matrix.raw_logs.Contains($finalLogPath)) { $matrix.raw_logs.Add($finalLogPath) }
            }
        }
        if ($originalRotation -match '^\d+$') {
            try {
                Invoke-AdbQuiet @("shell", "settings", "put", "system", "user_rotation", $originalRotation)
                $restoredRotation = (Invoke-Adb @("shell", "settings", "get", "system", "user_rotation") | Select-Object -First 1).Trim()
                if ($restoredRotation -ne $originalRotation) { throw "user rotation restored as $restoredRotation" }
            } catch {
                $matrix.restore_errors.Add("user rotation: $($_.Exception.Message)")
            }
        }
        if ($originalAutoRotation -match '^\d+$') {
            try {
                Invoke-AdbQuiet @("shell", "settings", "put", "system", "accelerometer_rotation", $originalAutoRotation)
                $restoredAutoRotation = (Invoke-Adb @("shell", "settings", "get", "system", "accelerometer_rotation") | Select-Object -First 1).Trim()
                if ($restoredAutoRotation -ne $originalAutoRotation) { throw "accelerometer rotation restored as $restoredAutoRotation" }
            } catch {
                $matrix.restore_errors.Add("accelerometer rotation: $($_.Exception.Message)")
            }
        }
        if ($displaySizeState) {
            try {
                Restore-AndroidWindowSize $displaySizeState
                $restoredDisplaySize = Get-AndroidWindowSizeState
                $sizeRestored = ($restoredDisplaySize.physical -join "x") -eq ($displaySizeState.physical -join "x") -and
                    ($restoredDisplaySize.override -join "x") -eq ($displaySizeState.override -join "x")
                if (-not $sizeRestored) { throw "emulator wm size did not return to its original physical/override state" }
                $matrix.restored_display_size = $restoredDisplaySize
            } catch {
                $matrix.restore_errors.Add("display size: $($_.Exception.Message)")
            }
        }
        if ($matrix.restore_errors.Count -gt 0) { $status = "failed" }
        $matrix.status = $status
        $matrix.elapsed_seconds = [math]::Round($startedAt.Elapsed.TotalSeconds, 3)
        $matrix | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $matrixPath -Encoding UTF8
    }
    if ($status -ne "passed") {
        $restoreDetails = if ($matrix.restore_errors.Count -gt 0) { ": " + ($matrix.restore_errors -join "; ") } else { "" }
        throw "Android presentation-baseline matrix did not complete$restoreDetails"
    }
    Write-Output "Android presentation-baseline matrix passed: $matrixPath"
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
    $readinessTimer = [System.Diagnostics.Stopwatch]::StartNew()
    $captureTimer = $null
    $lastAttemptCapture = ""
    $captureAttempt = 0
    $lastFailure = "render did not become ready"
    $renderPassed = $false
    $stableCaptures = 0
    $processId = ""
    $viewportArg = ""
    $viewportResolved = $false
    $logFile = Join-Path $artifactRoot "$Name-logcat.txt"
    $log = @()
    try {
        $surfaceProbeState = @{ NextProbeAt = [DateTime]::MinValue }
        $onReadinessPending = {
            param($PendingProcessId)
            if (-not (Take-WorkshopSurfaceProbe $surfaceProbeState)) { return }
            Remove-Item -LiteralPath $uiTree -Force -ErrorAction SilentlyContinue
            try {
                # Probe only for bounded ANR recovery; discard these bounds and
                # resolve the viewport again after IT-032 has restored the scene.
                [void](Read-SurfaceBounds $SurfaceDescription $uiTree $Package)
            } catch {
                if ($_.Exception.Message -eq "unrelated emulator-system ANR was dismissed; waiting for the render surface") {
                    Invoke-Adb @("shell", "am", "start", "-W", "-n",
                        "$Package/com.stasislang.workshop.MainActivity") | Out-Null
                }
            }
        }.GetNewClosure()
        $remainingTotalMilliseconds = [int][math]::Floor(
            ($TotalTimeoutSeconds - $startedAt.Elapsed.TotalSeconds) * 1000
        )
        if ($remainingTotalMilliseconds -le 0) {
            throw "Android render E2E exceeded ${TotalTimeoutSeconds}s before IT-032 readiness"
        }
        $readinessTimeoutMilliseconds = [math]::Min(
            ($StartupReadinessTimeoutSeconds * 1000), $remainingTotalMilliseconds
        )
        $readProcessId = { Find-PackageProcessId $Package }.GetNewClosure()
        $readProcessLog = {
            param($ReadyProcessId)
            @(& $adb -s $serial logcat "--pid=$ReadyProcessId" -d 2>$null)
        }.GetNewClosure()
        $readiness = Wait-ForWorkshopIT032Readiness `
            -TimeoutMilliseconds $readinessTimeoutMilliseconds `
            -PollIntervalMilliseconds 1000 `
            -ReadProcessId $readProcessId `
            -ReadProcessLog $readProcessLog `
            -OnPending $onReadinessPending
        $readinessTimer.Stop()
        $script:workshopReadinessElapsedSeconds = [math]::Round($readinessTimer.Elapsed.TotalSeconds, 3)
        Write-Host "$Name IT-032 readiness elapsed $($script:workshopReadinessElapsedSeconds)s (limit ${StartupReadinessTimeoutSeconds}s)"
        $processId = $readiness.ProcessId
        Remove-Item -LiteralPath $uiTree -Force -ErrorAction SilentlyContinue
        $remainingTotalSeconds = [math]::Floor(
            $TotalTimeoutSeconds - $startedAt.Elapsed.TotalSeconds
        )
        if ($remainingTotalSeconds -le 0) {
            throw "Android render E2E exceeded ${TotalTimeoutSeconds}s before pixel capture"
        }
        $captureEffectiveTimeoutSeconds = [math]::Min($RenderTimeoutSeconds, $remainingTotalSeconds)
        $script:workshopCaptureEffectiveTimeoutSeconds = $captureEffectiveTimeoutSeconds
        $captureTimer = [System.Diagnostics.Stopwatch]::StartNew()
        $deadline = (Get-Date).AddSeconds($captureEffectiveTimeoutSeconds)
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
                $captureAttempt += 1
                $lastAttemptCapture = Join-Path $artifactRoot ("{0}-attempt-{1:D3}-{2}.png" -f $Name, $captureAttempt, $stamp)
                Save-Screenshot $lastAttemptCapture
                & python (Join-Path $toolsCiRoot "verify_render_parity.py") `
                    --capture $lastAttemptCapture --capture-only --profile android_emulator `
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
        $captureTimer.Stop()
        $script:workshopCaptureElapsedSeconds = [math]::Round($captureTimer.Elapsed.TotalSeconds, 3)
        Write-Host "$Name capture elapsed $($script:workshopCaptureElapsedSeconds)s (limit ${captureEffectiveTimeoutSeconds}s; completed=$renderPassed)"
        if ($renderPassed) {
            $observedAvdLine = Invoke-Adb @("emu", "avd", "name") | Select-Object -First 1
            $observedAvd = if ($null -eq $observedAvdLine) { "" } else { $observedAvdLine.Trim() }
            if (-not $observedAvd) {
                $observedAvdLine = Invoke-Adb @("shell", "getprop", "ro.boot.qemu.avd_name") |
                    Select-Object -First 1
                $observedAvd = if ($null -eq $observedAvdLine) { "" } else { $observedAvdLine.Trim() }
            }
            $observedSdk = (Invoke-Adb @("shell", "getprop", "ro.build.version.sdk") |
                Select-Object -First 1).Trim()
            if ($observedAvd -ne $AvdName -or $observedSdk -ne "35") {
                throw "$Name benchmark identity mismatch: requested=$AvdName/API35 observed=$observedAvd/API$observedSdk"
            }
            $sourceStatus = @(& git -C $repoRoot status --porcelain)
            if ($LASTEXITCODE -ne 0) { throw "Git source status could not be read for benchmark evidence" }
            if ($sourceStatus) {
                throw "$Name benchmark source is dirty; commit or remove source changes before publishing evidence"
            }
            $packageDump = @(Invoke-Adb @("shell", "dumpsys", "package", $Package)) -join "`n"
            $versionName = [regex]::Match(
                $packageDump, '(?m)^\s*versionName=([^\r\n]+)'
            ).Groups[1].Value.Trim()
            $versionCode = [regex]::Match(
                $packageDump, '(?m)^\s*versionCode=(\d+)'
            ).Groups[1].Value
            $metadataPath = Join-Path $artifactRoot "$Name-performance-metadata.json"
            @{
                scene = "render_parity"
                git_revision = (& git -C $repoRoot rev-parse HEAD).Trim()
                source_dirty = $false
                apk_sha256 = (Get-FileHash -LiteralPath $Apk -Algorithm SHA256).Hash.ToLowerInvariant()
                package_version = "$versionName ($versionCode)"
                device_model = (Invoke-Adb @("shell", "getprop", "ro.product.model") |
                    Select-Object -First 1).Trim()
                device_fingerprint = (Invoke-Adb @("shell", "getprop", "ro.build.fingerprint") |
                    Select-Object -First 1).Trim()
                serial = $serial
                avd = $observedAvd
                android_sdk = [int]$observedSdk
            } | ConvertTo-Json | Set-Content -LiteralPath $metadataPath -Encoding UTF8

            $performancePassed = $false
            for ($attempt = 1; $attempt -le 2; $attempt += 1) {
                $beforeAttemptLog = @(& $adb -s $serial logcat "--pid=$processId" -d 2>$null)
                $reportCountBeforeAttempt = @($beforeAttemptLog |
                    Where-Object { $_ -match 'RenderPerformance:' }).Count
                Invoke-Adb @(
                    "shell", "am", "start", "--activity-single-top", "-n",
                    "$Package/com.stasislang.workshop.MainActivity", "--ez",
                    "stasis_render_performance", "true"
                ) | Out-Null

                $attemptDeadline = (Get-Date).AddSeconds($RenderTimeoutSeconds)
                $attemptReport = $null
                do {
                    Start-Sleep -Milliseconds 500
                    $attemptDeviceLog = @(& $adb -s $serial logcat "--pid=$processId" -d 2>$null)
                    $attemptReports = @($attemptDeviceLog |
                        Where-Object { $_ -match 'RenderPerformance:' })
                    if ($attemptReports.Count -gt $reportCountBeforeAttempt) {
                        $attemptReport = $attemptReports[$reportCountBeforeAttempt]
                        break
                    }
                } while ((Get-Date) -lt $attemptDeadline)

                $attemptLog = Join-Path $artifactRoot "$Name-performance-attempt-$attempt.log"
                if ($null -eq $attemptReport) {
                    Set-Content -LiteralPath $attemptLog -Value "" -Encoding UTF8
                } else {
                    Set-Content -LiteralPath $attemptLog -Value $attemptReport -Encoding UTF8
                }
                $attemptEvidence = Join-Path $artifactRoot "$Name-performance-attempt-$attempt.json"
                $performanceArguments = @(
                    (Join-Path $toolsCiRoot "verify_android_render_performance.py"),
                    "--log", $attemptLog,
                    "--metadata", $metadataPath,
                    "--evidence", $attemptEvidence
                )
                if ($MaxRenderP50Millis -gt 0) {
                    $performanceArguments += @("--max-p50-ms", "$MaxRenderP50Millis")
                }
                if ($MaxRenderP95Millis -gt 0) {
                    $performanceArguments += @("--max-p95-ms", "$MaxRenderP95Millis")
                }
                & python @performanceArguments
                if ($LASTEXITCODE -eq 0) {
                    Copy-Item -LiteralPath $attemptEvidence `
                        -Destination (Join-Path $artifactRoot "$Name-performance.json") -Force
                    Write-Host "$Name render performance attempt $attempt passed"
                    $performancePassed = $true
                    break
                }
                Write-Warning "$Name render performance attempt $attempt failed"
            }
            if (-not $performancePassed) {
                throw "$Name render performance evidence failed after 2 attempts; see $artifactRoot"
            }
        }
    } finally {
        if ($readinessTimer.IsRunning) { $readinessTimer.Stop() }
        if ($captureTimer -and $captureTimer.IsRunning) { $captureTimer.Stop() }
        $script:workshopReadinessElapsedSeconds = [math]::Round($readinessTimer.Elapsed.TotalSeconds, 3)
        if ($captureTimer) {
            $script:workshopCaptureElapsedSeconds = [math]::Round($captureTimer.Elapsed.TotalSeconds, 3)
        }
        if ($lastAttemptCapture -and (Test-Path -LiteralPath $lastAttemptCapture) -and
            (Get-Item -LiteralPath $lastAttemptCapture).Length -gt 0) {
            Copy-Item -LiteralPath $lastAttemptCapture -Destination $capture -Force
        }
        if ($processId) { $log = @(& $adb -s $serial logcat "--pid=$processId" -d 2>$null) }
        if (-not $processId -or $LASTEXITCODE -ne 0) {
            $log = @(& $adb -s $serial logcat -d 2>$null)
        }
        $log | Set-Content -LiteralPath $logFile -Encoding UTF8
        & $adb -s $serial shell am force-stop $Package 2>$null | Out-Null
    }
    if (-not $renderPassed) {
        throw "$Name render acceptance timed out: $lastFailure; see $artifactRoot"
    }
    $it029CaptureNames = @(
        "project_a_first",
        "project_b_before_recreation",
        "project_b_after_recreation",
        "project_a_return"
    )
    $it029Captures = @()
    foreach ($phase in $it029CaptureNames) {
        $localCapture = Join-Path $artifactRoot "workshop-it029-$phase.png"
        $remoteCapture = "/sdcard/Android/data/$Package/files/it029/$phase.png"
        Invoke-Adb @("pull", $remoteCapture, $localCapture) | Out-Null
        if (-not (Test-Path $localCapture)) {
            throw "$Name IT-029 capture was not collected: $phase"
        }
        $it029Captures += $localCapture
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
    $workshopVerifyArguments = @(
        (Join-Path $toolsCiRoot "verify_android_workshop_seam.py"),
        "--log", $logFile, "--capture", $capture, "--manifest", $renderParityManifest,
        "--apk", $Apk, "--metadata", $metadataPath,
        "--evidence", (Join-Path $artifactRoot "$Name-workshop-seam.json")
    )
    foreach ($it029Capture in $it029Captures) {
        $workshopVerifyArguments += @("--it029-capture", $it029Capture)
    }
    & python @workshopVerifyArguments
    if ($LASTEXITCODE -ne 0) { throw "$Name IT-025 Workshop seam verification failed; see $logFile" }
    Write-Output "$Name render acceptance passed: $capture"
}

try {
    $runningBefore = @(& $adb devices) | Where-Object {
        $_ -match "^$([regex]::Escape($serial))\s+device(?:\s|$)"
    }
    $startedEmulator = -not [bool]$runningBefore
    if ($runningBefore) {
        $observedAvdLine = @(& $adb -s $serial emu avd name 2>$null | Select-Object -First 1)
        $observedAvd = if ($LASTEXITCODE -eq 0 -and $observedAvdLine) {
            $observedAvdLine[0].ToString().Trim()
        } else {
            ""
        }
        if (-not $observedAvd) {
            $observedAvdLine = @(& $adb -s $serial shell getprop ro.boot.qemu.avd_name 2>$null |
                Select-Object -First 1)
            if ($LASTEXITCODE -eq 0 -and $observedAvdLine) {
                $observedAvd = $observedAvdLine[0].ToString().Trim()
            }
        }
        if ($observedAvd -ne $AvdName) {
            $identity = if ($observedAvd) { "'$observedAvd'" } else { "unknown" }
            throw "Android emulator serial $serial runs AVD $identity; requested '$AvdName'."
        }
        Write-Host "Reusing ready Android emulator $serial for AVD $AvdName"
    } elseif ($runningOnWindows) {
        $emulatorArguments = @("-AvdName", $AvdName, "-Port", "$Port")
        if ($Headless) { $emulatorArguments += "-Headless" }
        $serial = Invoke-BoundedScript (Join-Path $scriptRoot "start_emulator.ps1") `
            $emulatorArguments "start-emulator"
        $serial = @($serial) | Select-Object -Last 1
    } else {
        throw "Workshop seam expects the platform runner to provide a ready Android emulator on non-Windows hosts."
    }

    if (-not $SkipBuild) {
        $gradle = Resolve-Gradle
        Invoke-BoundedScript (Join-Path $scriptRoot "build_debug.ps1") @(
            "-RenderAcceptance",
            "-NoGradleDaemon", "-GradlePath", $gradle
        ) "build-workshop" | Out-Null
        Assert-In-Time "Workshop build"
    }

    $renderAcceptanceTimer = [System.Diagnostics.Stopwatch]::StartNew()
    $renderAcceptanceSucceeded = $false
    try {
        Assert-RenderedVariant "workshop" "com.stasislang.workshop" `
            $workshopApk `
            "Interactive Stasis game preview. Touch the game to control it." $true
        Assert-In-Time "render acceptance"
        Assert-PresentationBaselineMatrix "com.stasislang.workshop" "Interactive Stasis game preview. Touch the game to control it." $workshopApk
        Assert-In-Time "presentation-baseline matrix"
        $renderAcceptanceSucceeded = $true
    } finally {
        $renderAcceptanceTimer.Stop()
        $renderAcceptanceElapsed = [math]::Round($renderAcceptanceTimer.Elapsed.TotalSeconds, 3)
        $renderAcceptanceTimingPath = Join-Path $artifactRoot "render-acceptance-timing.json"
        @{
            phase = "render-acceptance"
            elapsed_seconds = $renderAcceptanceElapsed
            startup_readiness_timeout_seconds = $StartupReadinessTimeoutSeconds
            readiness_elapsed_seconds = $script:workshopReadinessElapsedSeconds
            capture_timeout_seconds = $RenderTimeoutSeconds
            capture_effective_timeout_seconds = $script:workshopCaptureEffectiveTimeoutSeconds
            capture_elapsed_seconds = $script:workshopCaptureElapsedSeconds
            render_timeout_seconds = $RenderTimeoutSeconds
            total_elapsed_seconds = [math]::Round($startedAt.Elapsed.TotalSeconds, 3)
            total_timeout_seconds = $TotalTimeoutSeconds
            completed = $renderAcceptanceSucceeded
        } | ConvertTo-Json | Set-Content -LiteralPath $renderAcceptanceTimingPath -Encoding UTF8
        Write-Host "render-acceptance elapsed ${renderAcceptanceElapsed}s (render limit ${RenderTimeoutSeconds}s; total $([math]::Round($startedAt.Elapsed.TotalSeconds, 3))/${TotalTimeoutSeconds}s; completed=$renderAcceptanceSucceeded)"
    }

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
