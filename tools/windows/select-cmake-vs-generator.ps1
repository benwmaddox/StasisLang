[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"

$vsWhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio/Installer/vswhere.exe"
if (-not (Test-Path -LiteralPath $vsWhere)) {
  throw "vswhere.exe was not found; install Visual Studio 2022 or 2026 with MSBuild and the C++ toolchain."
}

$installationVersion = @(
  & $vsWhere -latest -products * `
    -requires Microsoft.Component.MSBuild Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
    -property installationVersion
)[0]
if ($LASTEXITCODE -ne 0 -or -not $installationVersion) {
  throw "Visual Studio 2022 or 2026 with MSBuild and the C++ toolchain is not installed."
}

$generator = switch -Regex ($installationVersion.Trim()) {
  '^18\.' { "Visual Studio 18 2026"; break }
  '^17\.' { "Visual Studio 17 2022"; break }
  default { throw "Unsupported Visual Studio version: $installationVersion" }
}

$cmakeCapabilities = cmake -E capabilities | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) {
  throw "Unable to query CMake generator capabilities."
}
if ($cmakeCapabilities.generators.name -notcontains $generator) {
  throw "Installed $generator is not supported by this CMake version."
}

$generator
