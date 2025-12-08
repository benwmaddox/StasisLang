@echo off
setlocal
set "SCRIPT_DIR=%~dp0"
set "DOTNET_ARGS=--no-restore"
if "%1"=="test" (
    set "DOTNET_ARGS=--no-build"
)
dotnet run %DOTNET_ARGS% --project "%SCRIPT_DIR%Stasis.Cli\Stasis.Cli.csproj" -- %*
set "CODE=%ERRORLEVEL%"
endlocal & exit /b %CODE%
