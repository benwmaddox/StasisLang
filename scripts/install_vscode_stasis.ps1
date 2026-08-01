param(
  [switch]$Force,
  [switch]$SkipInstall
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$extensionDir = Join-Path $repoRoot "vscode-stasis"
$vsixDir = Join-Path $extensionDir ".vsix"
$vsixPath = Join-Path $vsixDir "stasislang.stasis.vsix"

if (-not (Test-Path (Join-Path $extensionDir "package.json"))) {
  throw "Missing VS Code extension: $extensionDir"
}

$npm = Get-Command npm -ErrorAction SilentlyContinue
if (-not $npm) {
  throw "npm is required to build the Stasis VS Code extension."
}

if (-not $SkipInstall) {
  $code = Get-Command code -ErrorAction SilentlyContinue
  if (-not $code) {
    throw "VS Code CLI (code) is not on PATH. Run with -SkipInstall to build only."
  }
}

Push-Location $extensionDir
try {
  & $npm.Path @("ci")
  if ($LASTEXITCODE -ne 0) {
    throw "npm ci failed."
  }

  & $npm.Path @("test")
  if ($LASTEXITCODE -ne 0) {
    throw "extension tests failed."
  }

  New-Item -ItemType Directory -Force -Path $vsixDir | Out-Null
  & $npm.Path @("run", "package", "--", "--out", $vsixPath)
  if ($LASTEXITCODE -ne 0) {
    throw "VSIX packaging failed."
  }

  if ($SkipInstall) {
    Write-Host "Built VSIX: $vsixPath"
    return
  }

  $installArgs = @("--install-extension", $vsixPath)
  if ($Force) {
    $installArgs += "--force"
  }
  & $code.Path @installArgs
  if ($LASTEXITCODE -ne 0) {
    throw "VSIX install failed."
  }
  Write-Host "Installed VSIX: $vsixPath"
} finally {
  Pop-Location
}
