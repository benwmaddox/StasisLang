@echo off
setlocal

call "%~dp0env.bat"

pushd tools\cranelift-aot
cargo build -p stasis-cranelift-aot --release
if errorlevel 1 (
  popd
  exit /b 1
)
popd

pushd runtime
call build.bat
if errorlevel 1 (
  popd
  echo.
  echo Runtime build failed.
  echo - Ensure CMake, vcpkg, and VS Build Tools are installed ^(see README.md^).
  echo - If runtime/build was configured with a different generator, set STASIS_CLEAN_RUNTIME_BUILD=1.
  echo - To skip runtime build ^(no graphics^), set STASIS_SKIP_RUNTIME=1.
  exit /b 1
)
popd

dotnet build Stasis.sln
if errorlevel 1 exit /b 1

set LSP_DIR=%CD%\vscode-stasis\server
if exist "%LSP_DIR%" rmdir /s /q "%LSP_DIR%"
mkdir "%LSP_DIR%"
type nul > "%LSP_DIR%\.gitkeep"
dotnet publish Stasis.LanguageServer\Stasis.LanguageServer.csproj -c Release -o "%LSP_DIR%" -p:SelfContained=false -p:PublishSingleFile=false -p:PublishReadyToRun=false -p:UseAppHost=false
if errorlevel 1 exit /b 1

REM Keep the VSCode extension in sync with the repo build.
REM Default behavior:
REM - If VS Code CLI (code) is available and we're not on CI, build+install the latest VSIX.
REM - If code is missing, skip (unless STASIS_INSTALL_VSCODE=1 is set).
REM - Set STASIS_SKIP_VSCODE=1 to opt out.
if "%STASIS_SKIP_VSCODE%"=="1" goto :skip_vscode
if not "%CI%"=="" if not "%STASIS_INSTALL_VSCODE%"=="1" goto :skip_vscode

where code >nul 2>nul
if errorlevel 1 (
  if "%STASIS_INSTALL_VSCODE%"=="1" (
    echo ERROR: VS Code CLI ^(code^) not found in PATH, but STASIS_INSTALL_VSCODE=1 was set.
    echo        In VS Code: Ctrl+Shift+P ^> "Shell Command: Install 'code' command in PATH"
    exit /b 1
  )
  echo NOTE: VS Code CLI ^(code^) not found in PATH; skipping VSCode extension install.
  goto :skip_vscode
)

where npm >nul 2>nul
if errorlevel 1 (
  echo ERROR: npm not found in PATH; cannot build VSCode extension.
  exit /b 1
)

where npx >nul 2>nul
if errorlevel 1 (
  echo ERROR: npx not found in PATH; cannot package VSCode extension.
  exit /b 1
)

set VSIX_SKIP_INSTALL=
tasklist /fi "imagename eq Code.exe" 2>nul | find /i "Code.exe" >nul
if not errorlevel 1 (
  if "%STASIS_INSTALL_VSCODE%"=="1" (
    echo ERROR: VS Code is currently running; close it to update the extension.
    exit /b 1
  )
  echo NOTE: VS Code is running; building VSIX but skipping install. Close VS Code and rerun to install.
  set VSIX_SKIP_INSTALL=-SkipInstall
)

powershell -ExecutionPolicy Bypass -File "%~dp0scripts\install_vscode_stasis_lsp.ps1" -Configuration Release -Force %VSIX_SKIP_INSTALL%
if errorlevel 1 exit /b 1

:skip_vscode

REM Validate SVG assets (Rule 20: fail pipeline on violations)
if exist assets_src (
  for /r assets_src %%f in (*.svg) do (
    dotnet run --project Stasis.SvgValidator\Stasis.SvgValidator.csproj -c Release -- --dir assets_src
    if errorlevel 1 exit /b 1
    goto :svg_done
  )
)
:svg_done

set AOT_DIR=%CD%\build\aot
dotnet publish Stasis.Cli\Stasis.Cli.csproj -c Release -r win-x64 -p:PublishAot=true -p:SelfContained=true -o "%AOT_DIR%"
if errorlevel 1 exit /b 1

endlocal
