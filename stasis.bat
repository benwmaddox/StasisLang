@echo off
setlocal
call "%~dp0env.bat"
set "SCRIPT_DIR=%~dp0"
set "CONFIG=Release"
set "DOTNET_ARGS=--no-restore --configuration %CONFIG%"
if "%DOTNET_GCHeapHardLimit%"=="" set "DOTNET_GCHeapHardLimit=2147483648"
if "%1"=="test" (
    set "DOTNET_ARGS=--no-build --configuration %CONFIG%"
    if "%STASIS_WINDOW_START_MINIMIZED%"=="" set "STASIS_WINDOW_START_MINIMIZED=1"
    if "%STASIS_TEST_TIMEOUT_MS%"=="" set "STASIS_TEST_TIMEOUT_MS=30000"
)
dotnet run %DOTNET_ARGS% --project "%SCRIPT_DIR%Stasis.Cli\Stasis.Cli.csproj" -- %*
set "CODE=%ERRORLEVEL%"
endlocal & exit /b %CODE%
