param(
  [string]$ExtensionsDir = (Join-Path $env:USERPROFILE ".vscode\\extensions"),
  [switch]$Force
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$source = Join-Path $repoRoot "vscode-stasis-syntax"

if (-not (Test-Path $source)) {
  throw "Missing extension folder: $source"
}

$npx = Get-Command npx -ErrorAction SilentlyContinue
if (-not $npx) {
  throw "npx is required to package the VSIX. Install Node.js and try again."
}

$code = Get-Command code -ErrorAction SilentlyContinue
if (-not $code) {
  throw "VS Code CLI (code) not found in PATH. Enable it from the VS Code command palette."
}

$vsixDir = Join-Path $source ".vsix"
New-Item -ItemType Directory -Force -Path $vsixDir | Out-Null
$vsixPath = Join-Path $vsixDir "stasislang.stasis-syntax.vsix"

Push-Location $source
try {
  & $npx.Path @("@vscode/vsce", "package", "--out", $vsixPath)
  if ($LASTEXITCODE -ne 0) {
    throw "VSIX packaging failed."
  }

  $installArgs = @("--install-extension", $vsixPath)
  if ($Force) {
    $installArgs += "--force"
  }
  & $code.Path @installArgs
  if ($LASTEXITCODE -ne 0) {
    throw "VSIX install failed."
  }
} finally {
  Pop-Location
}

Write-Host "Installed VSIX: $vsixPath"
