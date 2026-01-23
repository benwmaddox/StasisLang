@echo off
setlocal
set "SCRIPT_DIR=%~dp0"
cd /d "%SCRIPT_DIR%" || exit /b 1

rem Dev loop for Brickout Revenge (original sample).
rem Usage: .\dev_brickout_revenge.bat [extra stasis args...]

rem Dev defaults: use the stable AOT hot-swap path. Set STASIS_CRANELIFT_JIT_RUNNER=1 to opt into the no-disk Cranelift JIT runner.
set "STASIS_CRANELIFT_JIT_RUNNER=0"
rem If the JIT runner becomes unresponsive, exit so the watch loop can restart it.
if not defined STASIS_JIT_WATCHDOG_MS set "STASIS_JIT_WATCHDOG_MS=15000"
if not defined STASIS_CRANELIFT_JIT_RUNNER_EXE (
  if exist "%SCRIPT_DIR%tools\cranelift-jit-runner\target\release\stasis-cranelift-jit-runner.exe" (
    set "STASIS_CRANELIFT_JIT_RUNNER_EXE=%SCRIPT_DIR%tools\cranelift-jit-runner\target\release\stasis-cranelift-jit-runner.exe"
  ) else if exist "%SCRIPT_DIR%tools\cranelift-jit-runner\target\debug\stasis-cranelift-jit-runner.exe" (
    set "STASIS_CRANELIFT_JIT_RUNNER_EXE=%SCRIPT_DIR%tools\cranelift-jit-runner\target\debug\stasis-cranelift-jit-runner.exe"
  )
)

call "%SCRIPT_DIR%stasis.bat" run "samples\brickout_revenge\brickout_revenge.stasis" --watch --backend cranelift --graphics --module brick --fps 60 %*
exit /b %ERRORLEVEL%

