@echo off
setlocal
set "SCRIPT_DIR=%~dp0"
cd /d "%SCRIPT_DIR%" || exit /b 1

rem Dev loop for Brickout Revenge v1.
rem Usage: .\dev_brickout_revenge_v1.bat [extra stasis args...]

call "%SCRIPT_DIR%stasis.bat" run "samples\brickout_revenge\brickout_revenge_v1.stasis" --watch --backend cranelift --graphics --module brick --fps 60 %*
exit /b %ERRORLEVEL%

