@echo off
setlocal
set "SCRIPT_DIR=%~dp0"
cd /d "%SCRIPT_DIR%" || exit /b 1

rem Dev loop for Brickout Revenge (original sample).
rem Usage: .\dev_brickout_revenge.bat [extra stasis args...]

call "%SCRIPT_DIR%stasis.bat" run "samples\brickout_revenge\brickout_revenge.stasis" --watch --backend cranelift --graphics --module brick --fps 60 %*
exit /b %ERRORLEVEL%

