@echo off
setlocal

set SCRIPT_DIR=%~dp0
"%SCRIPT_DIR%stasis.bat" run --backend cranelift --graphics %*
