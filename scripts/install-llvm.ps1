param(
  [string]$Version = "18.1.8"
)

$ErrorActionPreference = "Stop"

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
Set-Location $repoRoot

$toolsRoot = Join-Path $repoRoot ".tools"
$installDir = Join-Path $toolsRoot ("llvm-" + $Version)
$binDir = Join-Path $installDir "bin"
$clang = Join-Path $binDir "clang.exe"

if (Test-Path $clang) {
  Write-Host "OK: clang already present: $clang"
  exit 0
}

New-Item -ItemType Directory -Force -Path $toolsRoot | Out-Null

# LLVM release artifacts live under llvmorg-<version>.
# We prefer the Windows MSVC tarball so installation is just extraction (no admin/MSI).
$tag = "llvmorg-$Version"
$archiveName = "clang+llvm-$Version-x86_64-pc-windows-msvc.tar.xz"
$url = "https://github.com/llvm/llvm-project/releases/download/$tag/$archiveName"
$archivePath = Join-Path $toolsRoot $archiveName

Write-Host "Downloading: $url"
Invoke-WebRequest -Uri $url -OutFile $archivePath

Write-Host "Extracting to: $installDir"
if (Test-Path $installDir) {
  Remove-Item -Recurse -Force $installDir
}
New-Item -ItemType Directory -Force -Path $installDir | Out-Null

# The tarball contains a top-level folder named like:
#   clang+llvm-<version>-x86_64-pc-windows-msvc/
# Extract to a temp dir and then move contents into .tools/llvm-<version>/ for stable paths.
$tmp = Join-Path $toolsRoot ("tmp-llvm-" + [Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force -Path $tmp | Out-Null

tar -xJf $archivePath -C $tmp

$extractedRoot = Get-ChildItem -Path $tmp -Directory | Select-Object -First 1
if (-not $extractedRoot) {
  throw "Extraction failed: no top-level directory found in $archivePath"
}

Move-Item -Force -Path (Join-Path $extractedRoot.FullName "*") -Destination $installDir
Remove-Item -Recurse -Force $tmp

if (!(Test-Path $clang)) {
  throw "clang not found after install: $clang"
}

Write-Host "OK: installed clang: $clang"
Write-Host "Tip: run `".\\env.bat`" in cmd.exe to add it to PATH, or restart your shell."

