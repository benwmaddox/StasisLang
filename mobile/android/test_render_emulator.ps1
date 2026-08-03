param(
    [string]$AvdName = "Stasis_API_35",
    [int]$Port = 5554,
    [switch]$Headless,
    [switch]$SkipBuild,
    [int]$RenderTimeoutSeconds = 45,
    [int]$TotalTimeoutSeconds = 900,
    [string]$OutputPath = ""
)

$ErrorActionPreference = "Stop"
$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Split-Path -Parent (Split-Path -Parent $scriptRoot)
$startedAt = [System.Diagnostics.Stopwatch]::StartNew()
$serial = "emulator-$Port"
$startedEmulator = $false
$packages = @("com.stasislang.workshop")

$androidHome = if ($env:ANDROID_HOME) {
    $env:ANDROID_HOME
} elseif ($env:ANDROID_SDK_ROOT) {
    $env:ANDROID_SDK_ROOT
} else {
    "C:\Android\Sdk"
}
$adb = Join-Path $androidHome "platform-tools\adb.exe"
if (-not (Test-Path $adb)) { throw "adb.exe was not found: $adb" }

$stamp = (Get-Date).ToUniversalTime().ToString("yyyyMMddTHHmmssZ")
if (-not $OutputPath) {
    $OutputPath = Join-Path $repoRoot "artifacts\android_render_e2e\$stamp"
}
$artifactRoot = [System.IO.Path]::GetFullPath($OutputPath)
New-Item -ItemType Directory -Force -Path $artifactRoot | Out-Null

function Assert-In-Time([string]$Step) {
    if ($startedAt.Elapsed.TotalSeconds -gt $TotalTimeoutSeconds) {
        throw "Android render E2E exceeded ${TotalTimeoutSeconds}s after $Step"
    }
}

function Invoke-BoundedScript([string]$Path, [string[]]$Arguments, [string]$Phase) {
    $remainingSeconds = [math]::Floor($TotalTimeoutSeconds - $startedAt.Elapsed.TotalSeconds)
    $stepSeconds = [math]::Min(300, $remainingSeconds)
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
        & taskkill.exe /PID $process.Id /T /F 2>$null | Out-Null
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

function Resolve-Gradle {
    $wrapper = Join-Path $scriptRoot "gradlew.bat"
    if (Test-Path $wrapper) { return $wrapper }
    if ($env:ChocolateyInstall) {
        $installed = Get-ChildItem (Join-Path $env:ChocolateyInstall "lib\gradle\tools") `
            -Recurse -Filter gradle.bat -ErrorAction SilentlyContinue |
            Sort-Object FullName -Descending | Select-Object -First 1
        if ($installed) { return $installed.FullName }
    }
    $command = Get-Command gradle -ErrorAction SilentlyContinue
    if ($command) { return $command.Source }
    throw "Gradle was not found; install Gradle 8.9 or newer"
}

function Save-Screenshot([string]$Path) {
    $process = Start-Process -FilePath $adb -ArgumentList @(
        "-s", $serial, "exec-out", "screencap", "-p"
    ) -RedirectStandardOutput $Path -RedirectStandardError "$Path.stderr" -NoNewWindow -PassThru -Wait
    if ($process.ExitCode -ne 0 -or -not (Test-Path $Path)) {
        throw "Android screenshot capture failed for $serial"
    }
}

function Read-SurfaceBounds([string]$Description, [string]$XmlPath) {
    $deviceXml = "/data/local/tmp/stasis-render-window.xml"
    $pulled = $false
    for ($attempt = 1; $attempt -le 3 -and -not $pulled; $attempt++) {
        Remove-Item -LiteralPath $XmlPath -Force -ErrorAction SilentlyContinue
        Invoke-AdbQuiet @("shell", "rm", "-f", $deviceXml)
        try {
            Invoke-AdbQuiet @("shell", "uiautomator", "dump", $deviceXml)
            Invoke-AdbQuiet @("pull", $deviceXml, $XmlPath)
            $pulled = (Test-Path -LiteralPath $XmlPath) -and
                    (Get-Item -LiteralPath $XmlPath).Length -gt 0
        } catch {
            if ($attempt -eq 3) { throw }
            Start-Sleep -Seconds 1
        }
    }
    if (-not $pulled) { throw "Android UI hierarchy was not captured" }
    Invoke-Adb @("shell", "rm", "-f", $deviceXml) | Out-Null
    [xml]$tree = Get-Content -Raw -LiteralPath $XmlPath
    $node = @($tree.SelectNodes("//node")) |
        Where-Object { $_.GetAttribute("content-desc") -eq $Description } |
        Select-Object -First 1
    if (-not $node) { throw "render surface '$Description' was absent from the Android UI tree" }
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
    $logFile = Join-Path $artifactRoot "$Name-logcat.txt"
    $log = @()
    try {
        do {
            Start-Sleep -Seconds 2
            $processId = (Invoke-Adb @("shell", "pidof", $Package) | Select-Object -First 1).Trim()
            if (-not $processId) { throw "$Name exited before rendering" }
            try {
                if (-not $viewportArg) {
                    $surface = Read-SurfaceBounds $SurfaceDescription $uiTree
                    $viewport = Fit-LogicalViewport $surface
                    $viewportArg = ($viewport | ForEach-Object { $_.ToString() }) -join ","
                }
                Write-Host "$Name viewport=$viewportArg"
                Save-Screenshot $capture
                & python (Join-Path $repoRoot "tools\ci\verify_render_parity.py") `
                    --capture $capture --capture-only --profile android_emulator `
                    "--viewport=$viewportArg"
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
    $fatal = $log | Select-String -SimpleMatch $fatalPatterns
    if ($fatal) { throw "$Name logged a rendering/runtime failure; see $logFile" }
    $frameCounts = [regex]::Matches(($log -join "`n"), 'RenderAcceptanceFrame: count=(\d+)') |
        ForEach-Object { [int]$_.Groups[1].Value }
    if (-not $frameCounts -or ($frameCounts | Measure-Object -Maximum).Maximum -lt 30) {
        throw "$Name did not prove 30 actively rendered acceptance frames; see $logFile"
    }
    if ($RequireJit -and -not ($log -match 'CompileReady: backend=cranelift-jit reload=InitialCompile status=0 functions=[1-9][0-9]*')) {
        throw "$Name did not log a successful non-empty Workshop JIT compile; see $logFile"
    }
    if (-not $renderPassed) {
        throw "$Name render acceptance timed out: $lastFailure; see $artifactRoot"
    }
    Write-Output "$Name render acceptance passed: $capture"
}

try {
    $runningBefore = @(& $adb devices) | Where-Object {
        $_ -match "^$([regex]::Escape($serial))\s+device(?:\s|$)"
    }
    $startedEmulator = -not [bool]$runningBefore
    $emulatorArguments = @("-AvdName", $AvdName, "-Port", "$Port")
    if ($Headless) { $emulatorArguments += "-Headless" }
    $serial = Invoke-BoundedScript (Join-Path $scriptRoot "start_emulator.ps1") `
        $emulatorArguments "start-emulator"
    $serial = @($serial) | Select-Object -Last 1

    if (-not $SkipBuild) {
        $gradle = Resolve-Gradle
        Invoke-BoundedScript (Join-Path $scriptRoot "build_debug.ps1") @(
            "-RenderAcceptance", "-SkipCodexNative", "-SkipRustBridgeBuild",
            "-NoGradleDaemon", "-GradlePath", $gradle
        ) "build-workshop" | Out-Null
        Assert-In-Time "Workshop build"
    }

    Assert-RenderedVariant "workshop" "com.stasislang.workshop" `
        (Join-Path $scriptRoot "app\build\outputs\apk\workshop\debug\app-workshop-debug.apk") `
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
