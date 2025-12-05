@echo off
setlocal enabledelayedexpansion

if "%~1"=="" goto :usage
set CMD=%~1
shift
if "%~1"=="" goto :usage
set FILE=%~1
shift
set EXTRA=%*
set PROJ=Stasis.Cli\Stasis.Cli.csproj

where lli >nul 2>&1
if errorlevel 1 (
  echo error: lli not found on PATH>&2
  exit /b 1
)

set TMP=%TEMP%\stasis_%RANDOM%%RANDOM%.ll

if /I "%CMD%"=="run" (
  dotnet run --project "%PROJ%" -- "%FILE%" %EXTRA% > "%TMP%"
  if errorlevel 1 goto :fail
  lli "%TMP%"
  set EXITCODE=!errorlevel!
  del "%TMP%"
  exit /b !EXITCODE!
)

if /I "%CMD%"=="test" (
  dotnet run --project "%PROJ%" -- "%FILE%" --with-tests %EXTRA% > "%TMP%"
  if errorlevel 1 goto :fail
  lli -entry-function=run_tests "%TMP%"
  set EXITCODE=!errorlevel!
  del "%TMP%"
  exit /b !EXITCODE!
)

:usage
echo Usage: stasis run ^<file^> [extra cli args...]
echo        stasis test ^<file^> [extra cli args...] (adds --with-tests automatically)
exit /b 1

:fail
set EXITCODE=%ERRORLEVEL%
if exist "%TMP%" del "%TMP%"
exit /b %EXITCODE%
