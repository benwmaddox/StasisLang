@echo off
setlocal

pushd tools\cranelift-aot
cargo build -p stasis-cranelift-aot --release
if errorlevel 1 (
  popd
  exit /b 1
)
popd

dotnet build Stasis.sln
if errorlevel 1 exit /b 1

set AOT_DIR=%CD%\build\aot
dotnet publish Stasis.Cli\Stasis.Cli.csproj -c Release -r win-x64 -p:PublishAot=true -p:SelfContained=true -o "%AOT_DIR%"
if errorlevel 1 exit /b 1

endlocal
