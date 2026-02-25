@echo off
setlocal EnableExtensions

if "%~1"=="" (
  echo [stasis-sign-runner] missing executable path>&2
  exit /b 2
)

set "TARGET=%~1"
shift

set "SIGN_TOOL=%STASIS_AOT_SIGN_TOOL%"
if not "%SIGN_TOOL%"=="" (
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

"%TARGET%" %*
exit /b %ERRORLEVEL%
