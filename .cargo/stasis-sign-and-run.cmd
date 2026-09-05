@echo off
setlocal EnableExtensions EnableDelayedExpansion

if "%~1"=="" (
  echo [stasis-sign-runner] missing executable path>&2
  exit /b 2
)

set "TARGET=%~1"
set "RUNNER_DIR=%~dp0"
shift
set "FORWARD_ARGS="
:collect_args
if "%~1"=="" goto sign_and_run
set "FORWARD_ARGS=!FORWARD_ARGS! "%~1""
shift
goto collect_args
:sign_and_run

set "SIGN_TOOL=%STASIS_AOT_SIGN_TOOL%"
if not "%SIGN_TOOL%"=="" (
  where "%SIGN_TOOL%" >nul 2>nul
  if errorlevel 1 if not exist "%SIGN_TOOL%" (
    if "%STASIS_REQUIRE_SIGNED_EXECUTION%"=="1" (
      echo [stasis-sign-runner] configured signer does not exist: %SIGN_TOOL%>&2
      exit /b 5
    )
    echo [stasis-sign-runner] ignoring unavailable optional signer: %SIGN_TOOL%>&2
    goto run_unsigned
  )
  if not exist "%TARGET%" (
    echo [stasis-sign-runner] target does not exist: %TARGET%>&2
    exit /b 3
  )
  for %%S in ("%SIGN_TOOL%") do set "SIGNER_NAME=%%~nxS"
  for %%P in ("%RUNNER_DIR%..\tools\windows\stasis-signing.ps1") do set "POLICY_SCRIPT=%%~fP"
  if /I "!SIGNER_NAME!"=="signtool.exe" (
    if exist "!POLICY_SCRIPT!" (
      powershell.exe -NoProfile -ExecutionPolicy Bypass -File "!POLICY_SCRIPT!" sign -Tool "%SIGN_TOOL%" -Artifact "%TARGET%"
    ) else (
      call "%SIGN_TOOL%" "%TARGET%"
    )
  ) else (
    call "%SIGN_TOOL%" "%TARGET%"
  )
  if errorlevel 1 (
    if "%STASIS_REQUIRE_SIGNED_EXECUTION%"=="1" (
      echo [stasis-sign-runner] required signer failed for: %TARGET%>&2
      exit /b 4
    )
    echo [stasis-sign-runner] ignoring optional signer failure for: %TARGET%>&2
    goto run_unsigned
  )
) else (
  set "POLICY_CONFIGURED=0"
  if defined STASIS_SIGNING_CERTIFICATE set "POLICY_CONFIGURED=1"
  if defined STASIS_SIGNING_CERT_THUMBPRINT set "POLICY_CONFIGURED=1"
  if defined STASIS_SIGNING_LOCAL_RECORD if exist "%STASIS_SIGNING_LOCAL_RECORD%" set "POLICY_CONFIGURED=1"
  if defined LOCALAPPDATA if exist "%LOCALAPPDATA%\Stasis\signing\development-thumbprint.txt" set "POLICY_CONFIGURED=1"
  if "%STASIS_SIGNING_MODE%"=="optional" set "POLICY_CONFIGURED=1"
  if "%STASIS_SIGNING_MODE%"=="required" set "POLICY_CONFIGURED=1"
  if "%STASIS_REQUIRE_SIGNED_EXECUTION%"=="1" set "POLICY_CONFIGURED=1"
  if "!POLICY_CONFIGURED!"=="0" goto run_unsigned
  for %%P in ("%RUNNER_DIR%..\tools\windows\stasis-signing.ps1") do set "POLICY_SCRIPT=%%~fP"
  if exist "!POLICY_SCRIPT!" (
    powershell.exe -NoProfile -ExecutionPolicy Bypass -File "!POLICY_SCRIPT!" sign -Artifact "%TARGET%"
    if errorlevel 1 (
      if "%STASIS_REQUIRE_SIGNED_EXECUTION%"=="1" (
        echo [stasis-sign-runner] required repository signing policy failed for: %TARGET%>&2
        exit /b 5
      )
      if "%STASIS_SIGNING_MODE%"=="required" (
        echo [stasis-sign-runner] required repository signing policy failed for: %TARGET%>&2
        exit /b 5
      )
      echo [stasis-sign-runner] ignoring optional repository signing failure for: %TARGET%>&2
    )
  ) else (
    if "%STASIS_REQUIRE_SIGNED_EXECUTION%"=="1" if not "%STASIS_SIGNING_MODE%"=="optional" exit /b 5
    if "%STASIS_SIGNING_MODE%"=="required" exit /b 5
    echo [stasis-sign-runner] ignoring optional signing because repository policy entrypoint is unavailable>&2
  )
)

:run_unsigned
"%TARGET%" %FORWARD_ARGS%
exit /b %ERRORLEVEL%
