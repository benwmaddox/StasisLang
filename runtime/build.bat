@echo off
setlocal

echo Building Stasis Graphics Runtime Library...

:: Check for vcpkg - try common locations
if not defined VCPKG_ROOT (
    if exist "C:\code\vcpkg\vcpkg.exe" (
        set VCPKG_ROOT=C:\code\vcpkg
    ) else if exist "C:\vcpkg\vcpkg.exe" (
        set VCPKG_ROOT=C:\vcpkg
    ) else (
        echo Error: VCPKG_ROOT environment variable not set and vcpkg not found.
        echo Please install vcpkg and set VCPKG_ROOT.
        echo Example: set VCPKG_ROOT=C:\code\vcpkg
        exit /b 1
    )
)

:: Install SDL2 if not present
echo Checking for SDL2...
%VCPKG_ROOT%\vcpkg install sdl2:x64-windows --recurse

:: Create build directory
if not exist build mkdir build
cd build

:: Configure with CMake
echo Configuring...
cmake .. -DCMAKE_TOOLCHAIN_FILE=%VCPKG_ROOT%\scripts\buildsystems\vcpkg.cmake -DVCPKG_TARGET_TRIPLET=x64-windows

if %ERRORLEVEL% neq 0 (
    echo CMake configuration failed.
    exit /b 1
)

:: Build
echo Building...
cmake --build . --config Release

if %ERRORLEVEL% neq 0 (
    echo Build failed.
    exit /b 1
)

echo.
echo Build successful!
echo Library at: %CD%\Release\stasis_graphics.lib
echo DLL at: %CD%\bin\Release\stasis_graphics.dll
echo.
echo Copy DLLs to StasisLang root for runtime:
copy /Y "%CD%\bin\Release\*.dll" "%CD%\..\..\"
echo.
echo To run Asteroids demo:
echo   cd ..\..
echo   dotnet run --project Stasis.Cli -- run samples\asteroids.stasis --graphics --graphics-lib runtime\build\Release\stasis_graphics.lib

endlocal
