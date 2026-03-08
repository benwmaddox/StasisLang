[CmdletBinding()]
param(
    [Parameter(Mandatory = $false)]
    [string]$StasisExePath = "C:\stasis_runtime_reprove_target\release\stasis.exe",

    [Parameter(Mandatory = $false)]
    [string]$RepoRoot = "F:\StasisLang-demo-video",

    [Parameter(Mandatory = $false)]
    [string]$SamplePath = "samples\bucket_catcher.stasis",

    [Parameter(Mandatory = $true)]
    [string]$OutputPath,

    [Parameter(Mandatory = $false)]
    [int]$DurationSeconds = 20,

    [Parameter(Mandatory = $false)]
    [string]$WindowTitle = "bucket_catcher.stasis",

    [Parameter(Mandatory = $false)]
    [string]$VoiceName = "Microsoft Zira Desktop",

    [Parameter(Mandatory = $false)]
    [string]$NarrationText = "This is Bucket Catcher, a simple Stasis game sample. The demo is being driven automatically with scripted keyboard input. The sample shows explicit game state, a tick and render loop, sprite drawing, and a small user interface rendered from Stasis code."
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Resolve-FfmpegPath {
    $command = Get-Command ffmpeg -ErrorAction SilentlyContinue
    if ($null -eq $command) {
        throw "ffmpeg was not found on PATH."
    }

    return $command.Source
}

function Wait-ForWindow {
    param(
        [Parameter(Mandatory = $true)]
        [int]$ProcessId,

        [Parameter(Mandatory = $true)]
        [string]$ExpectedTitle,

        [Parameter(Mandatory = $false)]
        [int]$TimeoutSeconds = 15
    )

    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ((Get-Date) -lt $deadline) {
        $process = Get-Process -Id $ProcessId -ErrorAction SilentlyContinue
        if ($null -ne $process -and $process.MainWindowTitle -eq $ExpectedTitle) {
            return
        }

        Start-Sleep -Milliseconds 200
    }

    throw "Timed out waiting for window '$ExpectedTitle' for process $ProcessId."
}

function Start-BucketDemoDriver {
    param(
        [Parameter(Mandatory = $true)]
        [string]$ExpectedTitle,

        [Parameter(Mandatory = $true)]
        [int]$DurationSeconds
    )

    $driverScript = @'
param(
    [string]$WindowTitle,
    [int]$DurationSeconds
)

$wshell = New-Object -ComObject WScript.Shell
$deadline = (Get-Date).AddSeconds($DurationSeconds)
$pattern = @("{RIGHT}", "{RIGHT}", "{RIGHT}", "{LEFT}", "{LEFT}", "{LEFT}")
$index = 0

while ((Get-Date) -lt $deadline) {
    $null = $wshell.AppActivate($WindowTitle)
    Start-Sleep -Milliseconds 100
    $wshell.SendKeys($pattern[$index % $pattern.Length])
    $index = $index + 1
    Start-Sleep -Milliseconds 250
}
'@

    $tempScriptPath = Join-Path $env:TEMP ("stasis_bucket_driver_" + [guid]::NewGuid().ToString("N") + ".ps1")
    Set-Content -LiteralPath $tempScriptPath -Value $driverScript -Encoding ASCII

    $driver = Start-Process `
        -FilePath "powershell.exe" `
        -ArgumentList @(
            "-NoProfile",
            "-ExecutionPolicy", "Bypass",
            "-File", $tempScriptPath,
            "-WindowTitle", $ExpectedTitle,
            "-DurationSeconds", $DurationSeconds
        ) `
        -PassThru

    return @{
        Process = $driver
        ScriptPath = $tempScriptPath
    }
}

$resolvedStasisExePath = (Resolve-Path -LiteralPath $StasisExePath).Path
$resolvedRepoRoot = (Resolve-Path -LiteralPath $RepoRoot).Path
$resolvedOutputPath = [System.IO.Path]::GetFullPath($OutputPath)
$outputParent = Split-Path -Parent $resolvedOutputPath
if (-not [string]::IsNullOrWhiteSpace($outputParent) -and -not (Test-Path -LiteralPath $outputParent)) {
    New-Item -ItemType Directory -Path $outputParent | Out-Null
}

$ffmpegPath = Resolve-FfmpegPath
$tempRoot = Join-Path $env:TEMP ("stasis_demo_" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $tempRoot | Out-Null
$capturePath = Join-Path $tempRoot "capture.mp4"
$narrationPath = Join-Path $tempRoot "narration.wav"

$gameProcess = $null
$driver = $null
try {
    & (Join-Path $resolvedRepoRoot "tools\windows\new-demo-narration.ps1") `
        -Text $NarrationText `
        -OutputPath $narrationPath `
        -VoiceName $VoiceName | Out-Null

    $gameProcess = Start-Process `
        -FilePath $resolvedStasisExePath `
        -ArgumentList @("play", $SamplePath) `
        -WorkingDirectory $resolvedRepoRoot `
        -PassThru

    Wait-ForWindow -ProcessId $gameProcess.Id -ExpectedTitle $WindowTitle
    Start-Sleep -Milliseconds 500

    $driver = Start-BucketDemoDriver -ExpectedTitle $WindowTitle -DurationSeconds $DurationSeconds

    & $ffmpegPath `
        -y `
        -f gdigrab `
        -framerate 30 `
        -i "title=$WindowTitle" `
        -t $DurationSeconds `
        -c:v libx264 `
        -preset veryfast `
        -crf 18 `
        -pix_fmt yuv420p `
        $capturePath

    if ($LASTEXITCODE -ne 0) {
        throw "ffmpeg window capture failed with exit code $LASTEXITCODE"
    }

    & (Join-Path $resolvedRepoRoot "tools\windows\export-demo-video.ps1") `
        -VideoPath $capturePath `
        -NarrationPath $narrationPath `
        -NoGameAudio `
        -OutputPath $resolvedOutputPath | Out-Null
}
finally {
    if ($null -ne $driver) {
        if ($driver.Process -and -not $driver.Process.HasExited) {
            Stop-Process -Id $driver.Process.Id -Force -ErrorAction SilentlyContinue
        }
        if ($driver.ScriptPath -and (Test-Path -LiteralPath $driver.ScriptPath)) {
            Remove-Item -LiteralPath $driver.ScriptPath -Force -ErrorAction SilentlyContinue
        }
    }

    if ($null -ne $gameProcess -and -not $gameProcess.HasExited) {
        Stop-Process -Id $gameProcess.Id -Force -ErrorAction SilentlyContinue
    }
}

Write-Host "Demo MP4 written to $resolvedOutputPath"
