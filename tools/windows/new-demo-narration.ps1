[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Text,

    [Parameter(Mandatory = $true)]
    [string]$OutputPath,

    [Parameter(Mandatory = $false)]
    [string]$VoiceName = "Microsoft Zira Desktop",

    [Parameter(Mandatory = $false)]
    [int]$Rate = 0
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

Add-Type -AssemblyName System.Speech

$resolvedOutputPath = [System.IO.Path]::GetFullPath($OutputPath)
$parent = Split-Path -Parent $resolvedOutputPath
if (-not [string]::IsNullOrWhiteSpace($parent) -and -not (Test-Path -LiteralPath $parent)) {
    New-Item -ItemType Directory -Path $parent | Out-Null
}

$synth = New-Object System.Speech.Synthesis.SpeechSynthesizer
try {
    $voices = $synth.GetInstalledVoices() | ForEach-Object { $_.VoiceInfo.Name }
    if ($voices -notcontains $VoiceName) {
        throw "Voice '$VoiceName' is not installed. Installed voices: $($voices -join ', ')"
    }

    $synth.SelectVoice($VoiceName)
    $synth.Rate = $Rate
    $synth.SetOutputToWaveFile($resolvedOutputPath)
    $synth.Speak($Text)
}
finally {
    $synth.Dispose()
}

Write-Host "Narration WAV written to $resolvedOutputPath"
