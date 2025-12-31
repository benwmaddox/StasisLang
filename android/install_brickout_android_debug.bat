@echo off
setlocal
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0install_brickout_android_debug.ps1"
exit /b %ERRORLEVEL%

