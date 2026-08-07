[CmdletBinding(SupportsShouldProcess = $true)]
param(
    [ValidatePattern("^(?:[01][0-9]|2[0-3]):[0-5][0-9]$")]
    [string] $Time = "09:00",
    [string] $TaskName = "StasisLang - VS Code Nightly Extension Update"
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$updaterPath = Join-Path $PSScriptRoot "update-vscode-stasis-nightly.ps1"
if (-not (Test-Path -LiteralPath $updaterPath -PathType Leaf)) {
    throw "Updater script not found: $updaterPath"
}

$powerShellPath = (Get-Command powershell.exe -CommandType Application).Source
$repoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$quotedUpdaterPath = '"' + $updaterPath + '"'
$actionArguments = "-NoLogo -NoProfile -NonInteractive -WindowStyle Hidden -ExecutionPolicy Bypass -File $quotedUpdaterPath"
$runAt = [DateTime]::ParseExact($Time, "HH:mm", [Globalization.CultureInfo]::InvariantCulture)
$taskIdentity = "$TaskName for $env:USERDOMAIN\$env:USERNAME"
if ($PSCmdlet.ShouldProcess($taskIdentity, "Register or replace daily scheduled task at $Time")) {
    $action = New-ScheduledTaskAction `
        -Execute $powerShellPath `
        -Argument $actionArguments `
        -WorkingDirectory $repoRoot
    $trigger = New-ScheduledTaskTrigger -Daily -At $runAt
    $principal = New-ScheduledTaskPrincipal `
        -UserId "$env:USERDOMAIN\$env:USERNAME" `
        -LogonType Interactive `
        -RunLevel Limited
    $settings = New-ScheduledTaskSettingsSet `
        -StartWhenAvailable `
        -AllowStartIfOnBatteries `
        -DontStopIfGoingOnBatteries `
        -MultipleInstances IgnoreNew `
        -ExecutionTimeLimit (New-TimeSpan -Minutes 15)

    Register-ScheduledTask `
        -TaskName $TaskName `
        -Action $action `
        -Trigger $trigger `
        -Principal $principal `
        -Settings $settings `
        -Description "Checks for a new StasisLang nightly release and installs its Windows VSIX." `
        -Force | Out-Null

    $registered = Get-ScheduledTask -TaskName $TaskName
    Write-Output "Registered task: $($registered.TaskName)"
    Write-Output "User: $env:USERDOMAIN\$env:USERNAME"
    Write-Output "Schedule: Daily at $Time (local time)"
    Write-Output "Action: $powerShellPath $actionArguments"
}
