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
if "%LLVM_NATIVE_PATH%"=="" (
  set "LLVM_NATIVE_PATH=%USERPROFILE%\.nuget\packages\libllvm.runtime.win-x64\20.1.2\runtimes\win-x64\native"
)

set LLI=
for %%I in (lli.exe) do @for %%J in ("%ProgramFiles%\\LLVM\\bin\\%%I" "%%~$PATH:I") do @if exist %%~J set LLI=%%~fJ

set CLANG=
for %%I in (clang.exe) do @for %%J in ("%ProgramFiles%\\LLVM\\bin\\%%I" "%%~$PATH:I") do @if exist %%~J set CLANG=%%~fJ

if "%LLI%"=="" if "%CLANG%"=="" (
  echo error: neither lli nor clang found. Install LLVM or add to PATH.&goto :fail
)

set OUTLL=%TEMP%\stasis_%RANDOM%%RANDOM%.ll
set TMPEXE=%TEMP%\stasis_%RANDOM%%RANDOM%.exe

if /I "%CMD%"=="run" (
  dotnet run --project "%PROJ%" -- "%FILE%" %EXTRA% > "%TMP%"
  if errorlevel 1 goto :fail
  if not "%LLI%"=="" (
    "%LLI%" "%OUTLL%"
  ) else (
    "%CLANG%" "%OUTLL%" -o "%TMPEXE%"
    "%TMPEXE%"
  )
  set EXITCODE=!errorlevel!
  del "%OUTLL%"
  if exist "%TMPEXE%" del "%TMPEXE%"
  exit /b !EXITCODE!
)

if /I "%CMD%"=="test" (
  dotnet run --project "%PROJ%" -- "%FILE%" --with-tests %EXTRA% > "%OUTLL%"
  if errorlevel 1 goto :fail
  if not "%LLI%"=="" (
    "%LLI%" -entry-function=run_tests "%OUTLL%"
  ) else (
    "%CLANG%" "%OUTLL%" -o "%TMPEXE%" -Wl,/entry:run_tests -Wl,/subsystem:console
    "%TMPEXE%"
  )
  set EXITCODE=!errorlevel!
  del "%OUTLL%"
  if exist "%TMPEXE%" del "%TMPEXE%"
  exit /b !EXITCODE!
)

:usage
echo Usage: stasis run ^<file^> [extra cli args...]
echo        stasis test ^<file^> [extra cli args...] (adds --with-tests automatically)
exit /b 1

:fail
set EXITCODE=%ERRORLEVEL%
if exist "%OUTLL%" del "%OUTLL%"
if exist "%TMPEXE%" del "%TMPEXE%"
exit /b %EXITCODE%
