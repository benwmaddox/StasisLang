param(
    [string]$Version = "20.1.2"
)

$ErrorActionPreference = "Stop"

function Ensure-Dir([string]$Path) {
    if (!(Test-Path -LiteralPath $Path)) {
        New-Item -ItemType Directory -Path $Path | Out-Null
    }
}

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
$toolsRoot = Join-Path $repoRoot ".tools"
Ensure-Dir $toolsRoot

$installDir = Join-Path $toolsRoot ("llvm-" + $Version)
$clang = Join-Path $installDir "bin\\clang.exe"
$llvmAr = Join-Path $installDir "bin\\llvm-ar.exe"

if ((Test-Path -LiteralPath $clang) -and (Test-Path -LiteralPath $llvmAr)) {
    Write-Host ("LLVM already installed at: " + $installDir)
    Write-Host ("clang: " + $clang)
    Write-Host ("llvm-ar: " + $llvmAr)
    exit 0
}

$installer = Join-Path $toolsRoot ("LLVM-" + $Version + "-win64.exe")
$url = "https://github.com/llvm/llvm-project/releases/download/llvmorg-$Version/LLVM-$Version-win64.exe"

if (!(Test-Path -LiteralPath $installer)) {
    Write-Host ("Downloading: " + $url)
    Write-Host ("To: " + $installer)
    curl.exe -L -o $installer $url
}

Write-Host ("Installing LLVM " + $Version + " to: " + $installDir)
Ensure-Dir $installDir

& $installer /S /D=$installDir

if (!(Test-Path -LiteralPath $clang)) {
    throw ("Install failed: missing " + $clang)
}
if (!(Test-Path -LiteralPath $llvmAr)) {
    throw ("Install failed: missing " + $llvmAr)
}

Write-Host ("Installed:")
Write-Host ("clang: " + $clang)
Write-Host ("llvm-ar: " + $llvmAr)
