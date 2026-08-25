param(
    [Parameter(Mandatory)]
    [string] $Toolchain,

    [string] $ArtifactRoot = (Join-Path $PSScriptRoot 'artifacts'),

    [switch] $DevelopmentBuild
)

$ErrorActionPreference = 'Stop'
$Toolchain = (Resolve-Path -LiteralPath $Toolchain).Path
$ArtifactRoot = [IO.Path]::GetFullPath($ArtifactRoot)
New-Item -ItemType Directory -Force -Path $ArtifactRoot | Out-Null
$projectOutputName = ".windows-launch-build-$PID"
$projectOutputRoot = Join-Path $PSScriptRoot $projectOutputName

function Invoke-Bounded {
    param(
        [Parameter(Mandatory)] [string] $Description,
        [Parameter(Mandatory)] [string] $FilePath,
        [string[]] $Arguments = @(),
        [string] $WorkingDirectory = $PSScriptRoot
    )
    $logName = $Description -replace '[^A-Za-z0-9_.-]', '_'
    $stdoutPath = Join-Path $ArtifactRoot "$logName.stdout.log"
    $stderrPath = Join-Path $ArtifactRoot "$logName.stderr.log"
    $quotedArguments = $Arguments | ForEach-Object {
        if ($_ -match '\s') { '"' + ($_ -replace '"', '\"') + '"' } else { $_ }
    }
    $startInfo = [Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $FilePath
    $startInfo.Arguments = $quotedArguments -join ' '
    $startInfo.WorkingDirectory = $WorkingDirectory
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    if (-not $process.Start()) { throw "failed to start $Description" }
    $stdoutTask = $process.StandardOutput.ReadToEndAsync()
    $stderrTask = $process.StandardError.ReadToEndAsync()
    if (-not $process.WaitForExit(60000)) {
        Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
        throw "$Description exceeded 60 seconds"
    }
    $process.WaitForExit()
    $stdout = $stdoutTask.Result
    $stderr = $stderrTask.Result
    [IO.File]::WriteAllText($stdoutPath, $stdout)
    [IO.File]::WriteAllText($stderrPath, $stderr)
    if ($stdout) { Write-Host $stdout }
    if ($stderr) { Write-Host $stderr }
    if ($process.ExitCode -ne 0) {
        throw "$Description failed with exit code $($process.ExitCode)"
    }
}

function Invoke-Capture {
    param(
        [Parameter(Mandatory)] [string] $Description,
        [Parameter(Mandatory)] [string] $Screenshot,
        [Parameter(Mandatory)] [string] $FilePath,
        [string[]] $Arguments = @(),
        [string] $WorkingDirectory = $PSScriptRoot,
        [switch] $ExitAfterScreenshot
    )
    $env:STASIS_SCREENSHOT_ONCE = $Screenshot
    $env:STASIS_SCREENSHOT_FRAME = '2'
    if ($ExitAfterScreenshot) {
        $env:STASIS_EXIT_AFTER_SCREENSHOT = '1'
    }
    else {
        Remove-Item Env:STASIS_EXIT_AFTER_SCREENSHOT -ErrorAction SilentlyContinue
    }
    try {
        Invoke-Bounded -Description $Description -FilePath $FilePath `
            -Arguments $Arguments -WorkingDirectory $WorkingDirectory
    }
    finally {
        Remove-Item Env:STASIS_SCREENSHOT_ONCE -ErrorAction SilentlyContinue
        Remove-Item Env:STASIS_SCREENSHOT_FRAME -ErrorAction SilentlyContinue
        Remove-Item Env:STASIS_EXIT_AFTER_SCREENSHOT -ErrorAction SilentlyContinue
    }
    if (-not (Test-Path -LiteralPath $Screenshot)) {
        throw "$Description did not create $Screenshot"
    }
}

function Assert-SmokeFrame {
    param(
        [Parameter(Mandatory)] [string] $Description,
        [Parameter(Mandatory)] [string] $Screenshot
    )
    Add-Type -AssemblyName System.Drawing
    $bitmap = [Drawing.Bitmap]::new($Screenshot)
    try {
        if ($bitmap.Width -ne 320 -or $bitmap.Height -ne 180) {
            throw "$Description captured $($bitmap.Width)x$($bitmap.Height), expected 320x180"
        }
        $png = $bitmap.GetPixel(80, 62)
        $svg = $bitmap.GetPixel(220, 50)
        if ($png.R -le 180 -or $png.B -le 180) {
            throw "$Description PNG probe was $png"
        }
        if ($svg.G -le 150) {
            throw "$Description SVG probe was $svg"
        }
    }
    finally {
        $bitmap.Dispose()
    }
}

$captures = @{}
$captures.play = Join-Path $ArtifactRoot 'play.png'
Invoke-Bounded -Description 'play' -FilePath $Toolchain -Arguments @(
    'play', (Join-Path $PSScriptRoot 'main.stasis'), '--watch-dir', $PSScriptRoot,
    '--ticks', '2', '--screenshot', $captures.play, '--screenshot-frame', '2',
    '--exit-after-screenshot'
)

$captures.run_watch = Join-Path $ArtifactRoot 'run-watch.png'
Invoke-Capture -Description 'run --watch' -Screenshot $captures.run_watch -ExitAfterScreenshot `
    -FilePath $Toolchain -Arguments @('--workspace', $PSScriptRoot, 'run', '--watch')

$captures.tui = Join-Path $ArtifactRoot 'tui.png'
Invoke-Capture -Description 'tui' -Screenshot $captures.tui -FilePath $Toolchain `
    -Arguments @('--workspace', $PSScriptRoot, 'tui', '--live-script', 'live.commands', '--live-json')

$releaseRoot = Join-Path $projectOutputRoot 'release'
New-Item -ItemType Directory -Force -Path $releaseRoot | Out-Null
$releaseExe = Join-Path $releaseRoot 'windows_launch_smoke.exe'
Invoke-Bounded -Description 'release build' -FilePath $Toolchain -Arguments @(
    '--workspace', $PSScriptRoot, 'build', '--mode', 'release', '--out',
    "$projectOutputName/release/windows_launch_smoke.exe"
)
$captures.release = Join-Path $ArtifactRoot 'release.png'
Invoke-Capture -Description 'release executable' -Screenshot $captures.release `
    -ExitAfterScreenshot -FilePath $releaseExe

$packageRoot = Join-Path $projectOutputRoot 'package'
$packageArguments = @('--workspace', $PSScriptRoot, 'package', '--target', 'desktop', '--out', "$projectOutputName/package")
if ($DevelopmentBuild) { $packageArguments += '--development-build' }
Invoke-Bounded -Description 'desktop package' -FilePath $Toolchain -Arguments $packageArguments
$packagePayload = Join-Path $packageRoot 'app'
$requiredPayload = @(
    'assets/manifest.json',
    'stasis.json',
    'stasis_provenance.json'
)
foreach ($relative in $requiredPayload) {
    if (-not (Test-Path -LiteralPath (Join-Path $packagePayload $relative))) {
        throw "desktop package payload is missing $relative"
    }
}
$obsoletePayload = @(
    'stasis_dynload.dll',
    'stasis_graphics.dll',
    'windows_launch_smoke.dll',
    'windows_launch_smoke.exe.launch'
)
foreach ($relative in $obsoletePayload) {
    if (Test-Path -LiteralPath (Join-Path $packagePayload $relative)) {
        throw "desktop production package retained obsolete modular payload $relative"
    }
}
$unexpectedRootEntries = @(Get-ChildItem -LiteralPath $packageRoot | Where-Object {
    $_.Name -ne 'app' -and $_.Name -ne 'windows_launch_smoke.exe'
})
if ($unexpectedRootEntries.Count -ne 0) {
    throw "desktop package root contains unexpected entries: $($unexpectedRootEntries.Name -join ', ')"
}
$captures.package = Join-Path $ArtifactRoot 'package.png'
Invoke-Capture -Description 'packaged executable' -Screenshot $captures.package `
    -ExitAfterScreenshot -FilePath (Join-Path $packageRoot 'windows_launch_smoke.exe') `
    -WorkingDirectory $packageRoot

foreach ($case in $captures.Keys) {
    Assert-SmokeFrame -Description $case -Screenshot $captures[$case]
}
Remove-Item -LiteralPath $projectOutputRoot -Recurse -Force
Write-Host "Windows launch smoke passed: $($captures.Keys.Count) paths"
