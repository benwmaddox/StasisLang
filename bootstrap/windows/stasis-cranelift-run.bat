@echo off
setlocal

set "SCRIPT_DIR=%~dp0"
for %%I in ("%SCRIPT_DIR%..\..") do set "REPO_ROOT=%%~fI"
set "AOT_EXE=%REPO_ROOT%\tools\cranelift-aot\target\debug\stasis-cranelift-aot.exe"

if "%~1"=="" (
  echo Usage: bootstrap\windows\stasis-cranelift-run.bat ^<file.stasis^> [additional stasisc run args]
  exit /b 1
)

if not exist "%AOT_EXE%" (
  echo [stasis-cranelift-run] Building Cranelift AOT helper...
  cargo build --manifest-path "%REPO_ROOT%\tools\cranelift-aot\Cargo.toml"
  if errorlevel 1 exit /b 1
)

if not defined STASIS_CRANELIFT_AOT (
  set "STASIS_CRANELIFT_AOT=%AOT_EXE%"
)

if exist "C:\Program Files\LLVM\bin\clang.exe" (
  set "PATH=C:\Program Files\LLVM\bin;%PATH%"
)
if exist "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\Llvm\x64\bin\clang.exe" (
  set "PATH=C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\Llvm\x64\bin;%PATH%"
)

call "%SCRIPT_DIR%stasisc.bat" run %* --backend cranelift --no-cranelift-runner
exit /b %ERRORLEVEL%
