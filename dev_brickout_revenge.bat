@echo off
setlocal EnableExtensions

rem Brickout Revenge dev loop (Windows).
rem - Keeps the game running and hot-swaps between tick() calls.
rem - Uses Cranelift AOT + runner (graphics-capable path).

cd /d "%~dp0"

set "ROOT=%CD%"
set "CONFIG=Release"
set "GAME=samples\brickout_revenge\brickout_revenge.stasis"
set "MODULE=brick"
set "FPS=60"

set "CLI_EXE=%ROOT%\build\stasis_release.exe"
if not exist "%CLI_EXE%" set "CLI_EXE=%ROOT%\Stasis.Cli\bin\%CONFIG%\net9.0\Stasis.Cli.exe"

set "RUNNER_EXE=%ROOT%\runtime\build\bin\%CONFIG%\stasis_runner.exe"
set "AOT_EXE=%ROOT%\tools\cranelift-aot\target\%CONFIG%\stasis-cranelift-aot.exe"

if not exist "%GAME%" (
  echo error: game not found: %GAME%
  exit /b 1
)

if not exist "%CLI_EXE%" (
  echo info: building CLI...
  dotnet build -c %CONFIG% "%ROOT%\Stasis.sln"
  if errorlevel 1 exit /b %ERRORLEVEL%
)

if not exist "%RUNNER_EXE%" (
  echo info: building runtime runner...
  call "%ROOT%\runtime\build.bat"
  if errorlevel 1 exit /b %ERRORLEVEL%
)

if not exist "%AOT_EXE%" (
  echo info: building cranelift aot tool...
  pushd "%ROOT%\tools\cranelift-aot" >nul
  cargo build --release
  if errorlevel 1 (popd >nul & exit /b %ERRORLEVEL%)
  popd >nul
)

set "STASIS_CRANELIFT_RUNNER_EXE=%RUNNER_EXE%"
set "STASIS_CRANELIFT_AOT=%AOT_EXE%"
set "STASIS_CRANELIFT_AOT_SERVER=1"
set "STASIS_CRANELIFT_RUNNER_SERVER=1"
set "STASIS_HOTSWAP_KEEP_OLD=1"

echo Root:   %ROOT%
echo CLI:    %CLI_EXE%
echo Game:   %GAME%
echo Module: %MODULE%
echo FPS:    %FPS%
echo.

rem Pass extra CLI args after defaults, e.g.:
rem   dev_brickout_revenge.bat --fps 30
rem   dev_brickout_revenge.bat --backend llvm
call "%CLI_EXE%" run "%GAME%" --graphics --watch --backend cranelift --module "%MODULE%" --fps "%FPS%" %*
exit /b %ERRORLEVEL%
