param(
  [switch]$All,
  [switch]$VscodeSyntax,
  [switch]$VscodeLsp,
  [switch]$BrowserTests,
  [switch]$CheckOnly,
  [switch]$Force,
  [string]$LspRuntime = "win-x64",
  [string]$LspConfiguration = "Release"
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $repoRoot

function Have([string]$name) {
  return [bool](Get-Command $name -ErrorAction SilentlyContinue)
}

function Require([string]$name, [string]$hint) {
  if (Have $name) {
    Write-Host "OK  $name"
    return $true
  }
  Write-Host "MISS $name - $hint"
  return $false
}

if ($All) {
  $VscodeSyntax = $true
  $VscodeLsp = $true
  $BrowserTests = $true
}

Write-Host "Repo: $repoRoot"

$okDotnet = Require "dotnet" ".NET 9 SDK required (see README.md)"
$okNode = Require "node" "Node.js required (for VSCode extension build / Playwright tests)"
$okNpm = Require "npm" "npm required (bundled with Node.js)"
$okNpx = Require "npx" "npx required (bundled with Node.js)"

$okCode = $true
if ($VscodeSyntax -or $VscodeLsp) {
  $okCode = Require "code" "VS Code CLI required (Cmd+Shift+P: 'Shell Command: Install code command in PATH')"
}

if ($CheckOnly) {
  Write-Host "Check-only: done."
  exit 0
}

if ($VscodeSyntax) {
  if (-not $okNpx -or -not $okCode) {
    throw "Cannot install vscode syntax extension without npx + code."
  }
  & powershell -ExecutionPolicy Bypass -File (Join-Path $repoRoot "scripts\\install_vscode_stasis_syntax.ps1") -Force:$Force
}

if ($VscodeLsp) {
  if (-not $okDotnet -or -not $okNpm -or -not $okNpx -or -not $okCode) {
    throw "Cannot install vscode LSP extension without dotnet + npm/npx + code."
  }
  & powershell -ExecutionPolicy Bypass -File (Join-Path $repoRoot "scripts\\install_vscode_stasis_lsp.ps1") -Runtime $LspRuntime -Configuration $LspConfiguration -Force:$Force
}

if ($BrowserTests) {
  if (-not $okNpm -or -not $okNpx) {
    throw "Cannot install browser test deps without npm/npx."
  }

  $browserDir = Join-Path $repoRoot "tests\\browser"
  if (-not (Test-Path $browserDir)) {
    Write-Host "Skipping browser tests (missing folder): $browserDir"
  } else {
    Push-Location $browserDir
    try {
      & npm install
      if ($LASTEXITCODE -ne 0) { throw "npm install failed (tests/browser)" }
      & npx playwright install chromium
      if ($LASTEXITCODE -ne 0) { throw "playwright install failed (tests/browser)" }
    } finally {
      Pop-Location
    }
  }
}

Write-Host "Done."

