param(
  [switch]$Force,
  [switch]$SkipInstall,
  [switch]$SkipBuild,
  [switch]$RunVsCodeE2E
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot
$releaseRoot = Join-Path $repoRoot "dist/stasis-editor-release-win32-x64"
$builder = Join-Path $PSScriptRoot "build_local_editor_release.ps1"

& $builder -OutputRoot $releaseRoot -SkipBuild:$SkipBuild -RunVsCodeE2E:$RunVsCodeE2E
if ($LASTEXITCODE -ne 0) { throw "Local Stasis editor release failed." }

$vsix = Get-ChildItem $releaseRoot -Filter "*.vsix" | Select-Object -First 1
if (-not $vsix) { throw "Local editor release does not contain a VSIX." }
if ($SkipInstall) {
  Write-Host "Built editor release: $releaseRoot"
  return
}

$code = Get-Command code -ErrorAction SilentlyContinue
if (-not $code) { throw "VS Code CLI (code) is not on PATH. Use -SkipInstall to build only." }
$installArgs = @("--install-extension", $vsix.FullName)
if ($Force) { $installArgs += "--force" }
& $code.Source @installArgs
if ($LASTEXITCODE -ne 0) { throw "VSIX install failed." }
Write-Host "Installed VSIX from atomic editor release: $($vsix.FullName)"
