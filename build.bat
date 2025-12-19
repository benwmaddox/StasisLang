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

endlocal
