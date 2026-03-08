[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$VideoPath,

    [Parameter(Mandatory = $false)]
    [string]$NarrationPath,

    [Parameter(Mandatory = $false)]
    [switch]$NoGameAudio,

    [Parameter(Mandatory = $false)]
    [int]$GameAudioStreamIndex = 1,

    [Parameter(Mandatory = $false)]
    [AllowNull()]
    [Nullable[int]]$NarrationStreamIndex = $null,

    [Parameter(Mandatory = $true)]
    [string]$OutputPath,

    [Parameter(Mandatory = $false)]
    [double]$GameAudioVolume = 0.65,

    [Parameter(Mandatory = $false)]
    [double]$NarrationAudioVolume = 1.25,

    [Parameter(Mandatory = $false)]
    [string]$VideoCrf = "18",

    [Parameter(Mandatory = $false)]
    [string]$VideoPreset = "slow",

    [Parameter(Mandatory = $false)]
    [switch]$DryRun
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Resolve-RequiredPath {
    param(
        [Parameter(Mandatory = $true)]
        [string]$PathValue
    )

    $resolved = Resolve-Path -LiteralPath $PathValue -ErrorAction Stop
    return $resolved.Path
}

function Resolve-FfmpegPath {
    $command = Get-Command ffmpeg -ErrorAction SilentlyContinue
    if ($null -ne $command) {
        return $command.Source
    }

    throw "ffmpeg was not found on PATH. Install FFmpeg and try again."
}

function New-ParentDirectory {
    param(
        [Parameter(Mandatory = $true)]
        [string]$TargetPath
    )

    $parent = Split-Path -Parent $TargetPath
    if ([string]::IsNullOrWhiteSpace($parent)) {
        return
    }

    if (-not (Test-Path -LiteralPath $parent)) {
        New-Item -ItemType Directory -Path $parent | Out-Null
    }
}

$resolvedVideoPath = Resolve-RequiredPath -PathValue $VideoPath
$resolvedNarrationPath = $null
if (-not [string]::IsNullOrWhiteSpace($NarrationPath)) {
    $resolvedNarrationPath = Resolve-RequiredPath -PathValue $NarrationPath
}

if ($GameAudioStreamIndex -lt 0) {
    throw "GameAudioStreamIndex must be zero or greater."
}

if (-not $NoGameAudio -and $null -eq $resolvedNarrationPath -and $null -eq $NarrationStreamIndex) {
    throw "Provide either NarrationPath or NarrationStreamIndex."
}

if ($null -ne $NarrationStreamIndex -and $NarrationStreamIndex.Value -lt 0) {
    throw "NarrationStreamIndex must be zero or greater."
}

$resolvedOutputPath = [System.IO.Path]::GetFullPath($OutputPath)
New-ParentDirectory -TargetPath $resolvedOutputPath

$ffmpegPath = $null
if (-not $DryRun) {
    $ffmpegPath = Resolve-FfmpegPath
}

$gameLabel = "game"
$voiceLabel = "voice"
$mixLabel = "mix"

$filterParts = @()
$inputArgs = @("-y", "-i", $resolvedVideoPath)

$ffmpegArgs = @()
$ffmpegArgs += $inputArgs

if ($NoGameAudio) {
    if ($null -eq $resolvedNarrationPath) {
        throw "NoGameAudio requires NarrationPath."
    }

    $inputArgs += @("-i", $resolvedNarrationPath)
    $ffmpegArgs = @()
    $ffmpegArgs += $inputArgs
    $ffmpegArgs += @(
        "-filter_complex", "[1:a:0]loudnorm=I=-16:LRA=11:TP=-1.5,volume=$NarrationAudioVolume[$voiceLabel]",
        "-map", "0:v:0",
        "-map", "[$voiceLabel]",
        "-c:v", "libx264",
        "-preset", $VideoPreset,
        "-crf", $VideoCrf,
        "-pix_fmt", "yuv420p",
        "-c:a", "aac",
        "-b:a", "192k",
        "-movflags", "+faststart",
        "-shortest",
        $resolvedOutputPath
    )
} else {
    if ($null -ne $resolvedNarrationPath) {
        $inputArgs += @("-i", $resolvedNarrationPath)
        $filterParts += "[0:a:$GameAudioStreamIndex]volume=$GameAudioVolume[$gameLabel]"
        $filterParts += "[1:a:0]loudnorm=I=-16:LRA=11:TP=-1.5,volume=$NarrationAudioVolume[$voiceLabel]"
    } else {
        $filterParts += "[0:a:$GameAudioStreamIndex]volume=$GameAudioVolume[$gameLabel]"
        $filterParts += "[0:a:$($NarrationStreamIndex.Value)]loudnorm=I=-16:LRA=11:TP=-1.5,volume=$NarrationAudioVolume[$voiceLabel]"
    }

    $filterParts += "[$gameLabel][$voiceLabel]amix=inputs=2:duration=first:dropout_transition=2[$mixLabel]"
    $filterComplex = $filterParts -join ";"

    $ffmpegArgs = @()
    $ffmpegArgs += $inputArgs
    $ffmpegArgs += @(
        "-filter_complex", $filterComplex,
        "-map", "0:v:0",
        "-map", "[$mixLabel]",
        "-c:v", "libx264",
        "-preset", $VideoPreset,
        "-crf", $VideoCrf,
        "-pix_fmt", "yuv420p",
        "-c:a", "aac",
        "-b:a", "192k",
        "-movflags", "+faststart",
        $resolvedOutputPath
    )
}

Write-Host "Preparing demo video export..."
Write-Host "Video: $resolvedVideoPath"
if ($null -ne $resolvedNarrationPath) {
    Write-Host "Narration: $resolvedNarrationPath"
} elseif (-not $NoGameAudio) {
    Write-Host "Narration stream index: $($NarrationStreamIndex.Value)"
}
Write-Host "Output: $resolvedOutputPath"
if (-not $NoGameAudio) {
    Write-Host "Game audio stream index: $GameAudioStreamIndex"
} else {
    Write-Host "Game audio: disabled"
}

$quotedCommand = @($ffmpegPath) + $ffmpegArgs | ForEach-Object {
    if ($null -eq $_) {
        return ""
    }

    $value = [string]$_
    if ($value.Contains(" ") -or $value.Contains(";") -or $value.Contains("[")) {
        '"' + $value.Replace('"', '\"') + '"'
    } else {
        $value
    }
}

Write-Host "FFmpeg command:"
Write-Host ($quotedCommand -join " ")

if ($DryRun) {
    Write-Host "DryRun enabled; not invoking ffmpeg."
    exit 0
}

& $ffmpegPath @ffmpegArgs

if ($LASTEXITCODE -ne 0) {
    throw "ffmpeg failed with exit code $LASTEXITCODE"
}

Write-Host "Demo MP4 written to $resolvedOutputPath"
