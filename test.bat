@echo off
setlocal

set STASIS_CRANELIFT_AOT=%CD%\tools\cranelift-aot\target\release\stasis-cranelift-aot.exe

dotnet test
if errorlevel 1 exit /b 1

dotnet run --project Stasis.Cli -- test --all
if errorlevel 1 exit /b 1

endlocal
