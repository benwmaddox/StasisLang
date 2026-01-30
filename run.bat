@echo off
setlocal

set SCRIPT_DIR=%~dp0
"%SCRIPT_DIR%stasis.bat" run --watch --backend cranelift --graphics %*
