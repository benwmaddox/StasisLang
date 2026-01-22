@echo off
setlocal
set "SCRIPT_DIR=%~dp0"
cd /d "%SCRIPT_DIR%" || exit /b 1

rem Dev loop for Brickout Revenge (original sample).
rem Usage: .\dev_brickout_revenge.bat [extra stasis args...]

rem Dev defaults: prefer no-disk Cranelift JIT hot-swap for faster iteration.
set "STASIS_CRANELIFT_JIT_RUNNER=1"
if not defined STASIS_CRANELIFT_JIT_RUNNER_EXE (
  if exist "%SCRIPT_DIR%tools\cranelift-jit-runner\target\release\stasis-cranelift-jit-runner.exe" (
    set "STASIS_CRANELIFT_JIT_RUNNER_EXE=%SCRIPT_DIR%tools\cranelift-jit-runner\target\release\stasis-cranelift-jit-runner.exe"
  ) else if exist "%SCRIPT_DIR%tools\cranelift-jit-runner\target\debug\stasis-cranelift-jit-runner.exe" (
    set "STASIS_CRANELIFT_JIT_RUNNER_EXE=%SCRIPT_DIR%tools\cranelift-jit-runner\target\debug\stasis-cranelift-jit-runner.exe"
  )
)

call "%SCRIPT_DIR%stasis.bat" run "samples\brickout_revenge\brickout_revenge.stasis" --watch --backend cranelift --graphics --module brick --fps 60 %*
exit /b %ERRORLEVEL%

