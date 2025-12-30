@echo off
setlocal

call "%~dp0env.bat"

set STASIS_CRANELIFT_AOT=%CD%\tools\cranelift-aot\target\release\stasis-cranelift-aot.exe
set STASIS_CRANELIFT_AOT_SERVER=1
set STASIS_CRANELIFT_RUNNER_SERVER=1

dotnet test -- RunConfiguration.MaxCpuCount=1
if errorlevel 1 exit /b 1

set STASIS_SUPPRESS_WARNINGS=1

set AOT_CLI=%CD%\build\aot\Stasis.Cli.exe
powershell -NoProfile -Command ^
  "$env:STASIS_SUPPRESS_WARNINGS='1';" ^
  "$env:STASIS_CRANELIFT_AOT='%STASIS_CRANELIFT_AOT%';" ^
  "$aot = '%AOT_CLI%';" ^
  "if (!(Test-Path $aot)) { Write-Error 'AOT CLI not found. Run build.bat first.'; exit 1 }" ^
  "& $aot test --all --backend cranelift;"
if errorlevel 1 exit /b 1

endlocal
