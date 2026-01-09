param(
  [string]$Configuration = "Release"
)

$ErrorActionPreference = "Stop"

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
Set-Location $repoRoot

$cli = Join-Path $repoRoot "Stasis.Cli\bin\$Configuration\net9.0\Stasis.Cli.exe"
if (!(Test-Path $cli)) {
  Write-Host "Building CLI ($Configuration)..."
  dotnet build -c $Configuration .\Stasis.sln | Out-Host
}

$clang = Join-Path $repoRoot ".tools\llvm-18.1.8\bin\clang.exe"
if (!(Test-Path $clang)) {
  throw "clang not found: $clang"
}

$source = Join-Path $repoRoot "samples\brickout_revenge\brickout_revenge_v1.stasis"
if (!(Test-Path $source)) {
  throw "source not found: $source"
}

$outDir = Join-Path $repoRoot "examples\wasm\brickout_revenge_v1"
New-Item -ItemType Directory -Force -Path $outDir | Out-Null

$tmpDir = Join-Path $repoRoot "build\wasm"
New-Item -ItemType Directory -Force -Path $tmpDir | Out-Null

$irPath = Join-Path $tmpDir "brickout_revenge_v1.ll"
$wasmPath = Join-Path $outDir "brickout_revenge_v1.wasm"

Write-Host "Emitting LLVM IR..."
$psi = New-Object System.Diagnostics.ProcessStartInfo
$psi.FileName = $cli
$psi.Arguments = "run `"$source`" --backend llvm --emit-ir --llvm-target wasm32-unknown-unknown"
$psi.RedirectStandardOutput = $true
$psi.RedirectStandardError = $true
$psi.UseShellExecute = $false
$psi.CreateNoWindow = $true

$proc = [System.Diagnostics.Process]::Start($psi)
$irText = $proc.StandardOutput.ReadToEnd()
$errText = $proc.StandardError.ReadToEnd()
$proc.WaitForExit()

if ($proc.ExitCode -ne 0) {
  throw "stasisc failed ($($proc.ExitCode)):`n$errText"
}

# The CLI prints a timing footer on stdout; strip it so clang can parse the IR.
$irLines = $irText -split "`r?`n"
$irLines = $irLines | Where-Object { $_ -notmatch '^Total time=' }
$irTextClean = ($irLines -join "`n")

$utf8NoBom = New-Object System.Text.UTF8Encoding -ArgumentList @($false)
[System.IO.File]::WriteAllText($irPath, $irTextClean, $utf8NoBom)

Write-Host "Compiling to wasm32..."
& $clang @(
  "--target=wasm32-unknown-unknown",
  "-O0",
  "-nostdlib",
  "-Wl,--no-entry",
  "-Wl,--export=main",
  "-Wl,--export=tick",
  "-Wl,--export=memory",
  "-Wl,--allow-undefined",
  "-Wl,--initial-memory=67108864",
  "-Wl,--max-memory=67108864",
  "-Wl,-z,stack-size=8388608",
  $irPath,
  "-o",
  $wasmPath
) | Out-Host

Write-Host "Wrote: $wasmPath"
