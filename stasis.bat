@echo off
setlocal
call "%~dp0env.bat"
set "SCRIPT_DIR=%~dp0"
set "CONFIG=Release"
set "DOTNET_ARGS=--no-restore --configuration %CONFIG%"
if "%DOTNET_GCHeapHardLimit%"=="" set "DOTNET_GCHeapHardLimit=2147483648"
if "%1"=="test" (
    set "DOTNET_ARGS=--no-build --configuration %CONFIG%"
)
dotnet run %DOTNET_ARGS% --project "%SCRIPT_DIR%Stasis.Cli\Stasis.Cli.csproj" -- %*
set "CODE=%ERRORLEVEL%"
endlocal & exit /b %CODE%
