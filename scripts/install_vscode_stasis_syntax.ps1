param(
  [string]$ExtensionsDir = (Join-Path $env:USERPROFILE ".vscode\\extensions"),
  [switch]$Force
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$source = Join-Path $repoRoot "vscode-stasis-syntax"
$target = Join-Path $ExtensionsDir "stasislang.stasis-syntax"

if (-not (Test-Path $source)) {
  throw "Missing extension folder: $source"
}

if (-not (Test-Path $ExtensionsDir)) {
  New-Item -ItemType Directory -Path $ExtensionsDir | Out-Null
}

if (Test-Path $target) {
  if (-not $Force) {
    throw "Target already exists: $target (re-run with -Force to overwrite)"
  }
  Remove-Item -Recurse -Force $target
}

Copy-Item -Recurse -Force $source $target
Write-Host "Installed to: $target"

