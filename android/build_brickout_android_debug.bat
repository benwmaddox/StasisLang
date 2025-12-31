@echo off
setlocal
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0build_brickout_android_debug.ps1"
exit /b %ERRORLEVEL%

