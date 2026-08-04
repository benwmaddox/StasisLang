param(
  [switch]$Force,
  [switch]$SkipInstall,
  [string]$ExecutablePath = "",
  [switch]$SkipToolchainBuild,
  [switch]$SkipGraphicsRuntimeBuild,
  [switch]$KeepLegacySyntax
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$extensionDir = Join-Path $repoRoot "vscode-stasis"
$vsixDir = Join-Path $extensionDir ".vsix"
$vsixPath = Join-Path $vsixDir "stasislang.stasis.vsix"
$toolchainPath = $ExecutablePath

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

if (-not $toolchainPath -and -not $SkipToolchainBuild) {
  $cargo = Get-Command cargo -ErrorAction SilentlyContinue
  if (-not $cargo) {
    throw "cargo is required to build the matching Stasis toolchain. Pass -ExecutablePath or -SkipToolchainBuild to use an existing toolchain."
  }
  Push-Location $repoRoot
  try {
    & $cargo.Path @("build", "-p", "stasis", "--release")
    if ($LASTEXITCODE -ne 0) {
      throw "Stasis release build failed."
    }
  } finally {
    Pop-Location
  }
  $toolchainPath = Join-Path $repoRoot "target\release\stasis.exe"
}

if (-not $toolchainPath) {
  $existingToolchain = Get-Command stasis -ErrorAction SilentlyContinue
  if (-not $existingToolchain) {
    throw "No Stasis toolchain was selected or found on PATH."
  }
  $toolchainPath = $existingToolchain.Source
}
$toolchainPath = (Resolve-Path -LiteralPath $toolchainPath -ErrorAction Stop).Path
$toolchainHelp = & $toolchainPath --help | Out-String
if ($LASTEXITCODE -ne 0 -or $toolchainHelp -notmatch '(?m)^\s+lsp\s+' -or $toolchainHelp -notmatch '(?m)^\s+dap\s+') {
  throw "The selected Stasis toolchain does not provide both lsp and dap: $toolchainPath"
}

$repoReleaseToolchain = (Join-Path $repoRoot "target\release\stasis.exe")
if ($toolchainPath -eq $repoReleaseToolchain -and -not $SkipGraphicsRuntimeBuild) {
  $runtimeDll = Join-Path $repoRoot "target\release\stasis_graphics.dll"
  $runtimeInputs = Get-ChildItem -LiteralPath (Join-Path $repoRoot "runtime") -File | Where-Object {
    $_.Extension -in @(".c", ".h", ".def", ".txt") -or $_.Name -eq "CMakeLists.txt"
  }
  $runtimeNeedsBuild = -not (Test-Path -LiteralPath $runtimeDll)
  if (-not $runtimeNeedsBuild) {
    $runtimeTimestamp = (Get-Item -LiteralPath $runtimeDll).LastWriteTimeUtc
    $runtimeNeedsBuild = $null -ne ($runtimeInputs | Where-Object { $_.LastWriteTimeUtc -gt $runtimeTimestamp } | Select-Object -First 1)
  }
  if ($runtimeNeedsBuild) {
    & cmd.exe /d /c (Join-Path $repoRoot "runtime\build.bat")
    if ($LASTEXITCODE -ne 0) {
      throw "Stasis graphics runtime build failed. Pass -SkipGraphicsRuntimeBuild only for an LSP/DAP-only installation."
    }
    $runtimeOutput = Join-Path $repoRoot "runtime\build\bin\Release"
    Get-ChildItem -LiteralPath $runtimeOutput -Filter "*.dll" | Copy-Item -Destination (Join-Path $repoRoot "target\release") -Force
    $runner = Join-Path $runtimeOutput "stasis_runner.exe"
    if (Test-Path -LiteralPath $runner) {
      Copy-Item -LiteralPath $runner -Destination (Join-Path $repoRoot "target\release\stasis_runner.exe") -Force
    }
  }
}

if (-not $SkipGraphicsRuntimeBuild) {
  $runtimeProbe = & $toolchainPath probe-graphics-runtime 2>&1 | Out-String
  if ($LASTEXITCODE -ne 0 -or $runtimeProbe -notmatch '(?m)^graphics_runtime_loaded=1\s*$') {
    throw "The selected Stasis toolchain cannot load its graphics runtime. $runtimeProbe"
  }
}

Push-Location $extensionDir
$previousLocalToolchain = [Environment]::GetEnvironmentVariable("STASIS_LOCAL_TOOLCHAIN", "Process")
try {
  $env:STASIS_LOCAL_TOOLCHAIN = $toolchainPath
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
  if (-not $KeepLegacySyntax) {
    $installedExtensions = & $code.Path @("--list-extensions")
    if ($installedExtensions -contains "stasislang.stasis-syntax") {
      & $code.Path @("--uninstall-extension", "stasislang.stasis-syntax")
      if ($LASTEXITCODE -ne 0) {
        throw "Legacy Stasis syntax extension uninstall failed."
      }
    }
  }
  Write-Host "Installed VSIX: $vsixPath"
  Write-Host "Pinned toolchain: $toolchainPath"
} finally {
  if ($null -eq $previousLocalToolchain) {
    Remove-Item Env:STASIS_LOCAL_TOOLCHAIN -ErrorAction SilentlyContinue
  } else {
    $env:STASIS_LOCAL_TOOLCHAIN = $previousLocalToolchain
  }
  Pop-Location
}
