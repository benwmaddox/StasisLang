param(
  [string]$Runtime = "",
  [string]$Configuration = "Release",
  [switch]$Force,
  [switch]$SkipInstall
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

New-Item -ItemType Directory -Force -Path $serverOut | Out-Null
New-Item -ItemType File -Force -Path (Join-Path $serverOut ".gitkeep") | Out-Null

$publishArgs = @("publish", $serverProject, "-c", $Configuration, "-o", $serverOut,
  "-p:StasisIncludeLibLLVM=false",
  "-p:SelfContained=false",
  "-p:PublishSingleFile=false",
  "-p:PublishReadyToRun=false",
  "-p:UseAppHost=false")

if (-not [string]::IsNullOrWhiteSpace($Runtime)) {
  $publishArgs += @("-r", $Runtime)
}

& $dotnet.Path $publishArgs
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
