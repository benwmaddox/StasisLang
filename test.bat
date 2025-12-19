@echo off
setlocal

set STASIS_CRANELIFT_AOT=%CD%\tools\cranelift-aot\target\release\stasis-cranelift-aot.exe
set STASIS_CRANELIFT_AOT_SERVER=1
set STASIS_CRANELIFT_RUNNER_SERVER=1

dotnet test
if errorlevel 1 exit /b 1

set STASIS_SUPPRESS_WARNINGS=1

set AOT_CLI=%CD%\build\aot\Stasis.Cli.exe
powershell -NoProfile -Command ^
  "$env:STASIS_SUPPRESS_WARNINGS='1';" ^
  "$env:STASIS_CRANELIFT_AOT='%STASIS_CRANELIFT_AOT%';" ^
  "$aot = '%AOT_CLI%';" ^
  "if (!(Test-Path $aot)) { Write-Error 'AOT CLI not found. Run build.bat first.'; exit 1 }" ^
  "& $aot test --all --backend cranelift | Out-Host;" ^
  "$timing = Measure-Command { & $aot test --all --backend cranelift | Out-Host };" ^
  "Write-Host ('Cranelift all-tests AOT ms=' + [math]::Round($timing.TotalMilliseconds, 0));"
if errorlevel 1 exit /b 1

endlocal
