@echo off
setlocal

pushd "%~dp0"

set "ENTRY_FILE=samples\bucket_catcher.stasis"
set "DEFAULT_WATCH_DIR=samples"
set "RUNTIME_DLL=stasis_graphics.dll"
set "TARGET_DIR=target\release"
set "TARGET_EXE=%TARGET_DIR%\stasis.exe"
set "TARGET_DLL=%TARGET_DIR%\%RUNTIME_DLL%"

where cargo >nul 2>nul
if errorlevel 1 (
    echo Error: cargo was not found in PATH.
    popd
    exit /b 1
)

if not exist "%ENTRY_FILE%" (
    echo Error: sample entry file not found: %ENTRY_FILE%
    popd
    exit /b 1
)

echo Building Stasis release CLI...
cargo build -p stasis --release
if errorlevel 1 (
    echo Error: cargo build failed.
    popd
    exit /b 1
)

if not exist "%TARGET_EXE%" (
    echo Error: built executable not found: %TARGET_EXE%
    popd
    exit /b 1
)

if not exist "%TARGET_DLL%" (
    if exist "%RUNTIME_DLL%" (
        echo Staging %RUNTIME_DLL% next to the built executable...
        copy /Y "%RUNTIME_DLL%" "%TARGET_DLL%" >nul
    ) else (
        echo Graphics runtime DLL was not found in the repo root.
        echo Attempting to build the runtime...
        call runtime\build.bat
        if errorlevel 1 (
            echo Error: runtime build failed.
            popd
            exit /b 1
        )
        if exist "%RUNTIME_DLL%" (
            copy /Y "%RUNTIME_DLL%" "%TARGET_DLL%" >nul
        )
    )
)

if not exist "%TARGET_DLL%" (
    echo Error: graphics runtime DLL not found: %TARGET_DLL%
    popd
    exit /b 1
)

echo Starting Bucket Catcher...
"%TARGET_EXE%" play "%ENTRY_FILE%" --watch-dir "%DEFAULT_WATCH_DIR%" %*
set "EXIT_CODE=%ERRORLEVEL%"

popd
exit /b %EXIT_CODE%
