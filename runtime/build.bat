@echo off
setlocal

call "%~dp0..\env.bat"

echo Building Stasis Graphics Runtime Library (static+shared)...

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

:: Default to static triplet for single-exe bundling; allow override
if "%VCPKG_TRIPLET%"=="" (
    set VCPKG_TRIPLET=x64-windows-static
)

:: Install SDL2 + GLEW for the chosen triplet
echo Checking for SDL2/GLEW with triplet %VCPKG_TRIPLET%...
%VCPKG_ROOT%\vcpkg install sdl2:%VCPKG_TRIPLET% glew:%VCPKG_TRIPLET% --recurse

:: Create build directory
if not exist build mkdir build
cd build

:: Configure with CMake
echo Configuring...
cmake .. -DCMAKE_TOOLCHAIN_FILE=%VCPKG_ROOT%\scripts\buildsystems\vcpkg.cmake -DVCPKG_TARGET_TRIPLET=%VCPKG_TRIPLET% -DSTASIS_GRAPHICS_BUILD_STATIC=ON -DSTASIS_GRAPHICS_BUILD_SHARED=ON

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
echo Static lib at: %CD%\Release\stasis_graphics_static.lib
echo Shared lib at: %CD%\bin\Release\stasis_graphics.dll
echo.
echo Copying shared DLLs to StasisLang root for legacy runs...
copy /Y "%CD%\bin\Release\*.dll" "%CD%\..\..\" >NUL 2>&1
echo Copying shared DLLs next to builds...
copy /Y "%CD%\bin\Release\*.dll" "%CD%\\bin\\Release" >NUL 2>&1
if not exist "%CD%\\..\\..\\build" (
    mkdir "%CD%\\..\\..\\build" >NUL 2>&1
)
copy /Y "%CD%\\bin\\Release\\*.dll" "%CD%\\..\\..\\build" >NUL 2>&1
echo Copying stasis_runner to repo root and build/ for auto-discovery...
if exist "%CD%\\bin\\Release\\stasis_runner.exe" (
    copy /Y "%CD%\\bin\\Release\\stasis_runner.exe" "%CD%\\..\\.." >NUL 2>&1
    copy /Y "%CD%\\bin\\Release\\stasis_runner.exe" "%CD%\\..\\..\\build" >NUL 2>&1
)

echo.
echo Copying static dependency libs to build\\Release for single-exe links...
set "STATIC_LIB_DIR=%VCPKG_ROOT%\\installed\\%VCPKG_TRIPLET%\\lib"
set "MANUAL_LIB_DIR=%STATIC_LIB_DIR%\\manual-link"
for %%F in (SDL2-static.lib libglew32.lib OpenGL32.Lib GlU32.Lib) do (
    if exist "%STATIC_LIB_DIR%\\%%F" (
        copy /Y "%STATIC_LIB_DIR%\\%%F" "%CD%\\Release" >NUL
    ) else (
        echo   warning: missing %%F in %STATIC_LIB_DIR%
    )
)
if exist "%MANUAL_LIB_DIR%\\SDL2main.lib" (
    copy /Y "%MANUAL_LIB_DIR%\\SDL2main.lib" "%CD%\\Release" >NUL
) else (
    echo   warning: missing SDL2main.lib in %MANUAL_LIB_DIR%
)
echo Copying static graphics lib to repo root and build/ for auto-discovery...
if /I "%STASIS_OVERWRITE_CHECKED_IN_RUNTIME_LIBS%"=="1" (
    copy /Y "%CD%\\Release\\stasis_graphics_static.lib" "%CD%\\..\\.." >NUL 2>&1
    if not exist "%CD%\\..\\..\\build" (
        mkdir "%CD%\\..\\..\\build" >NUL 2>&1
    )
    copy /Y "%CD%\\Release\\stasis_graphics_static.lib" "%CD%\\..\\..\\build" >NUL 2>&1
) else (
    if not exist "%CD%\\..\\..\\stasis_graphics_static.lib" (
        copy "%CD%\\Release\\stasis_graphics_static.lib" "%CD%\\..\\.." >NUL 2>&1
    )
    if not exist "%CD%\\..\\..\\build" (
        mkdir "%CD%\\..\\..\\build" >NUL 2>&1
    )
    if not exist "%CD%\\..\\..\\build\\stasis_graphics_static.lib" (
        copy "%CD%\\Release\\stasis_graphics_static.lib" "%CD%\\..\\..\\build" >NUL 2>&1
    )
)
echo.
echo To run Asteroids demo with static runtime:
echo   cd ..\..
echo   .\\stasis.bat run samples\\asteroids.stasis --graphics
echo (the CLI will pick up runtime\\build\\Release\\stasis_graphics_static.lib by default)

endlocal
