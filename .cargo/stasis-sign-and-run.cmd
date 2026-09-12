@echo off
setlocal EnableExtensions DisableDelayedExpansion
if "%~1"=="" (
  echo [stasis-sign-runner] missing executable path>&2
  exit /b 2
)
set "REQUIRED=0"
if "%STASIS_REQUIRE_SIGNED_EXECUTION%"=="1" set "REQUIRED=1"
if /I "%STASIS_SIGNING_MODE%"=="required" set "REQUIRED=1"
set "CONFIGURED=%REQUIRED%"
if defined STASIS_AOT_SIGN_TOOL set "CONFIGURED=1"
if defined STASIS_SIGNING_CERTIFICATE set "CONFIGURED=1"
if defined STASIS_SIGNING_CERT_THUMBPRINT set "CONFIGURED=1"
if /I "%STASIS_SIGNING_MODE%"=="production" goto configuration_ready
if /I "%STASIS_SIGNING_PROFILE%"=="production" goto configuration_ready
if defined STASIS_SIGNING_LOCAL_RECORD if exist "%STASIS_SIGNING_LOCAL_RECORD%" set "CONFIGURED=1"
if not defined STASIS_SIGNING_LOCAL_RECORD if defined LOCALAPPDATA if exist "%LOCALAPPDATA%\Stasis\signing\development-thumbprint.txt" set "CONFIGURED=1"
:configuration_ready
if "%CONFIGURED%"=="0" goto execute
if not exist "%~1" (
  echo [stasis-sign-runner] target does not exist: %~1>&2
  exit /b 3
)
powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "%~dp0..\tools\windows\stasis-signing.ps1" sign -Artifact "%~1"
if not errorlevel 1 goto execute
if "%REQUIRED%"=="1" (
  echo [stasis-sign-runner] required repository signing failed; target was not executed: %~1>&2
  exit /b 5
)
echo [stasis-sign-runner] ignoring optional repository signing failure for: %~1>&2
:execute
%*
exit /b %ERRORLEVEL%
