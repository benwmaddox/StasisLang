@echo off
setlocal

set STASIS_CRANELIFT_AOT=%CD%\tools\cranelift-aot\target\release\stasis-cranelift-aot.exe

dotnet test
if errorlevel 1 exit /b 1

set STASIS_SUPPRESS_WARNINGS=1

dotnet run --project Stasis.Cli -- test --all --backend llvm
if errorlevel 1 exit /b 1

dotnet run --project Stasis.Cli -- test samples\fib_tests.stasis --backend cranelift
if errorlevel 1 exit /b 1

powershell -NoProfile -Command ^
  "$samples = @('samples\\tests.stasis','samples\\fib_tests.stasis','samples\\strings.stasis');" ^
  "$env:STASIS_SUPPRESS_WARNINGS='1';" ^
  "$build = Measure-Command { & .\\build.bat | Out-Host };" ^
  "$llvm = Measure-Command { foreach ($s in $samples) { & dotnet run --project Stasis.Cli -- test $s --backend llvm | Out-Host } };" ^
  "$cranelift = Measure-Command { foreach ($s in $samples) { & dotnet run --project Stasis.Cli -- test $s --backend cranelift | Out-Host } };" ^
  "$buildMs = [math]::Round($build.TotalMilliseconds, 0);" ^
  "$llvmMs = [math]::Round($llvm.TotalMilliseconds, 0);" ^
  "$craneliftMs = [math]::Round($cranelift.TotalMilliseconds, 0);" ^
  "Write-Host ('Build ms=' + $buildMs);" ^
  "Write-Host ('LLVM subset ms=' + $llvmMs);" ^
  "Write-Host ('Cranelift subset ms=' + $craneliftMs);" ^
  "Write-Host ('LLVM total ms=' + ($buildMs + $llvmMs));" ^
  "Write-Host ('Cranelift total ms=' + ($buildMs + $craneliftMs));"
if errorlevel 1 exit /b 1

endlocal
