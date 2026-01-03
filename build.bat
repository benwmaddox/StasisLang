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
  exit /b 1
)
popd

dotnet build Stasis.sln
if errorlevel 1 exit /b 1

set LSP_DIR=%CD%\vscode-stasis\server
dotnet publish Stasis.LanguageServer\Stasis.LanguageServer.csproj -c Release -r win-x64 -o "%LSP_DIR%"
if errorlevel 1 exit /b 1

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
