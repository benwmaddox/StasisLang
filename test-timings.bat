@echo off
setlocal

call "%~dp0env.bat"

set "SCRIPT_DIR=%~dp0"
set "STASIS_SUPPRESS_WARNINGS=1"
set "AOT_CLI=%SCRIPT_DIR%build\\aot\\Stasis.Cli.exe"

set "STASIS_CRANELIFT_AOT=%SCRIPT_DIR%tools\\cranelift-aot\\target\\release\\stasis-cranelift-aot.exe"
set "STASIS_CRANELIFT_AOT_SERVER=1"
set "STASIS_CRANELIFT_RUNNER_SERVER=1"

powershell -NoProfile -Command ^
  "$ErrorActionPreference='Stop';" ^
  "$root = Resolve-Path '%SCRIPT_DIR%';" ^
  "Set-Location $root;" ^
  "$aot = Join-Path $root 'build\\aot\\Stasis.Cli.exe';" ^
  "if (!(Test-Path $aot)) { Write-Error 'AOT CLI not found. Run build.bat first.'; exit 1 }" ^
  "$env:STASIS_SUPPRESS_WARNINGS='1';" ^
  "$env:STASIS_CRANELIFT_AOT = (Join-Path $root 'tools\\cranelift-aot\\target\\release\\stasis-cranelift-aot.exe');" ^
  "$env:STASIS_CRANELIFT_AOT_SERVER='1';" ^
  "$env:STASIS_CRANELIFT_RUNNER_SERVER='1';" ^
  "function Clear-TestCache() {" ^
  "  $cache = Join-Path $root '.stasis_cache\\test';" ^
  "  if (!(Test-Path $cache)) { return }" ^
  "  try {" ^
  "    Remove-Item -Recurse -Force $cache -ErrorAction Stop;" ^
  "    return" ^
  "  } catch {" ^
  "    $parent = Split-Path $cache -Parent;" ^
  "    $stamp = Get-Date -Format 'yyyyMMdd_HHmmss';" ^
  "    $backup = Join-Path $parent (\"test_old_{0}\" -f $stamp);" ^
  "    try { Move-Item -Force $cache $backup -ErrorAction Stop } catch { }" ^
  "    New-Item -ItemType Directory -Force $cache | Out-Null" ^
  "  }" ^
  "}" ^
  "function Invoke-TimedRun([string]$label, [string]$backend) {" ^
  "  $args = @('test','--all','--backend',$backend);" ^
  "  $sw = [Diagnostics.Stopwatch]::StartNew();" ^
  "  & $aot @args;" ^
  "  $exitCode = $LASTEXITCODE;" ^
  "  $sw.Stop();" ^
  "  if ($exitCode -ne 0) { Write-Error \"$label failed with exit code $exitCode\"; exit $exitCode }" ^
  "  $seconds = $sw.Elapsed.TotalSeconds.ToString('0.000');" ^
  "  Write-Host (\"{0} run time: {1}s\" -f $label, $seconds);" ^
  "}" ^
  "function Invoke-Backend([string]$backend) {" ^
  "  Write-Host (\"\\n=== backend={0} ===\" -f $backend);" ^
  "  Clear-TestCache;" ^
  "  Invoke-TimedRun \"$backend Cold\" $backend;" ^
  "  Invoke-TimedRun \"$backend Warm\" $backend;" ^
  "}" ^
  "Invoke-Backend 'llvm';" ^
  "Invoke-Backend 'cranelift';"
if errorlevel 1 exit /b 1

endlocal
