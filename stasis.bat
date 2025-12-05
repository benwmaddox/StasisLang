@echo off
setlocal
set "SCRIPT_DIR=%~dp0"
dotnet run --project "%SCRIPT_DIR%Stasis.Cli\Stasis.Cli.csproj" -- %*
set "CODE=%ERRORLEVEL%"
endlocal & exit /b %CODE%
