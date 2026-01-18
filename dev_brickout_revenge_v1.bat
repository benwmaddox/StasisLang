@echo off
setlocal
set "SCRIPT_DIR=%~dp0"
cd /d "%SCRIPT_DIR%" || exit /b 1

rem Dev loop for Brickout Revenge v1.
rem Usage: .\dev_brickout_revenge_v1.bat [extra stasis args...]

rem Prefer toolchains configured by env.bat (LLVM, CMake, Rust, vcpkg).
call "%SCRIPT_DIR%env.bat" >nul 2>nul

rem If clang still isn't discoverable, you can point the CLI at it:
rem   set STASIS_CLANG=C:\path\to\clang.exe
if not defined STASIS_CLANG (
  where clang.exe >nul 2>nul
  if errorlevel 1 (
    echo warning: clang.exe not found in PATH and STASIS_CLANG is not set. 1>&2
    echo warning: stasis build/test/run may fail until clang is available. 1>&2
  )
)

call "%SCRIPT_DIR%stasis.bat" run "samples\brickout_revenge\brickout_revenge_v1.stasis" --watch --backend cranelift --graphics --module brick --fps 60 %*
exit /b %ERRORLEVEL%

