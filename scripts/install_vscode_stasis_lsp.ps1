param(
  [string]$Runtime = "win-x64",
  [string]$Configuration = "Release",
  [switch]$Force
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$serverProject = Join-Path $repoRoot "Stasis.LanguageServer\\Stasis.LanguageServer.csproj"
$extensionDir = Join-Path $repoRoot "vscode-stasis"
$serverOut = Join-Path $extensionDir "server"

if (-not (Test-Path $serverProject)) {
  throw "Missing language server project: $serverProject"
}

if (-not (Test-Path $extensionDir)) {
  throw "Missing VSCode extension folder: $extensionDir"
}

$dotnet = Get-Command dotnet -ErrorAction SilentlyContinue
if (-not $dotnet) {
  throw "dotnet is required to publish the language server."
}

$npm = Get-Command npm -ErrorAction SilentlyContinue
if (-not $npm) {
  throw "npm is required to build the VSCode extension."
}

$npx = Get-Command npx -ErrorAction SilentlyContinue
if (-not $npx) {
  throw "npx is required to package the VSIX."
}

$code = Get-Command code -ErrorAction SilentlyContinue
if (-not $code) {
  throw "VS Code CLI (code) not found in PATH. Enable it from the VS Code command palette."
}

& $dotnet.Path @("publish", $serverProject, "-c", $Configuration, "-r", $Runtime, "-o", $serverOut)
if ($LASTEXITCODE -ne 0) {
  throw "Language server publish failed."
}

Push-Location $extensionDir
try {
  if (-not (Test-Path (Join-Path $extensionDir "node_modules"))) {
    & $npm.Path @("install")
    if ($LASTEXITCODE -ne 0) {
      throw "npm install failed."
    }
  }

  & $npm.Path @("run", "build")
  if ($LASTEXITCODE -ne 0) {
    throw "Extension build failed."
  }

  $vsixDir = Join-Path $extensionDir ".vsix"
  New-Item -ItemType Directory -Force -Path $vsixDir | Out-Null
  $vsixPath = Join-Path $vsixDir "stasislang.stasis.vsix"

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

  Write-Host "Installed VSIX: $vsixPath"
} finally {
  Pop-Location
}
