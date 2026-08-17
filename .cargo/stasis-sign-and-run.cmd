@echo off
setlocal EnableExtensions EnableDelayedExpansion

if "%~1"=="" (
  echo [stasis-sign-runner] missing executable path>&2
  exit /b 2
)

set "TARGET=%~1"
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
  "%SIGN_TOOL%" "%TARGET%"
  if errorlevel 1 (
    echo [stasis-sign-runner] signer failed for: %TARGET%>&2
    exit /b 4
  )
) else (
  if "%STASIS_REQUIRE_SIGNED_EXECUTION%"=="1" (
    echo [stasis-sign-runner] STASIS_REQUIRE_SIGNED_EXECUTION=1 but STASIS_AOT_SIGN_TOOL is not set>&2
    exit /b 5
  )
)

:run_unsigned
"%TARGET%" %FORWARD_ARGS%
exit /b %ERRORLEVEL%
