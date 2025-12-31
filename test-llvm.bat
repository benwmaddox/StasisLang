@echo off
setlocal

call "%~dp0env.bat"

set "SCRIPT_DIR=%~dp0"
set "STASIS_SUPPRESS_WARNINGS=1"
set "STASIS_BACKEND=llvm"
set "STASIS_COMMAND=%SCRIPT_DIR%stasis.bat"

powershell -NoProfile -Command ^
  "$ErrorActionPreference='Stop';" ^
  "$stasisCommand = '%STASIS_COMMAND%';" ^
  "$arguments = @('test','samples','--all','--backend','%STASIS_BACKEND%');" ^
  "function Invoke-TimedRun([string]$label) {" ^
  "  $stopwatch = [Diagnostics.Stopwatch]::StartNew();" ^
  "  & $stasisCommand @arguments;" ^
  "  $exitCode = $LASTEXITCODE;" ^
  "  $stopwatch.Stop();" ^
  "  if ($exitCode -ne 0) { Write-Error \"$label run failed with exit code $exitCode\"; exit $exitCode }" ^
  "  $seconds = $stopwatch.Elapsed.TotalSeconds.ToString('0.000');" ^
  "  Write-Host (\"{0} run time: {1}s\" -f $label, $seconds);" ^
  "}" ^
  "Invoke-TimedRun 'Cold';" ^
  "Invoke-TimedRun 'Warm';"
if errorlevel 1 exit /b 1

endlocal
