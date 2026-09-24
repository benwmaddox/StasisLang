[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('RootbeerMaze3', 'SheepHerder', 'HamsterHavenTycoon')]
    [string]$Game,
    [Parameter(Mandatory = $true)]
    [string]$Snapshot,
    [Parameter(Mandatory = $true)]
    [string]$AotManifest,
    [string]$ProjectRoot,
    [string]$Output = ''
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$python = Get-Command python -ErrorAction Stop
$arguments = @(
    (Join-Path $PSScriptRoot 'atlas_affinity_preview.py'),
    '--input', (Resolve-Path -LiteralPath $Snapshot).Path,
    '--game', $Game,
    '--repo-root', $repoRoot,
    '--aot-manifest', (Resolve-Path -LiteralPath $AotManifest).Path
)
if ($ProjectRoot) {
    $arguments += @('--project-root', (Resolve-Path -LiteralPath $ProjectRoot).Path)
}
if ($Output) {
    $arguments += @('--output', $Output)
}
& $python.Source @arguments
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}
