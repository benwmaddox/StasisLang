@echo off
setlocal EnableDelayedExpansion

call "%~dp0..\env.bat"

echo Building Stasis Graphics Runtime Library (static+shared)...

:: Allow skipping runtime build for environments without native toolchain/vcpkg.
if /I "%STASIS_SKIP_RUNTIME%"=="1" (
    echo NOTE: STASIS_SKIP_RUNTIME=1 set; skipping runtime build.
    exit /b 0
)

:: Preflight: CMake must be available for runtime builds.
where cmake >nul 2>nul
if errorlevel 1 (
    echo Error: cmake not found in PATH.
    echo Install CMake and ensure it is available on PATH, or set STASIS_SKIP_RUNTIME=1.
    exit /b 1
)

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
if %ERRORLEVEL% neq 0 (
    echo Error: vcpkg install failed.
    exit /b 1
)
:: Default generator: Visual Studio 2022. Override via STASIS_CMAKE_GENERATOR (e.g. "Ninja").
if "%STASIS_CMAKE_GENERATOR%"=="" (
    set "STASIS_CMAKE_GENERATOR=Visual Studio 17 2022"
)

:: Create build directory
if /I "%STASIS_CLEAN_RUNTIME_BUILD%"=="1" (
    if exist build (
        echo Cleaning runtime build directory...
        rmdir /s /q build
    )
)
:: CMake caches are not relocatable: if runtime/build was created under a different checkout path,
:: CMake will error. Detect mismatches and auto-clean when needed.
if exist build\CMakeCache.txt call :maybe_clean_stale_cmake_cache
if not exist build mkdir build
cd build

:: If there's an existing cache with a different generator, configure will fail. Emit a helpful hint.
if exist CMakeCache.txt (
    for /f "tokens=2 delims==" %%G in ('findstr /b /c:"CMAKE_GENERATOR:INTERNAL=" CMakeCache.txt 2^>nul') do (
        set "CACHE_GEN=%%G"
    )
    if defined CACHE_GEN (
        if not "%CACHE_GEN%"=="" if /I not "%CACHE_GEN%"=="%STASIS_CMAKE_GENERATOR%" (
            echo NOTE: runtime/build was configured with generator "%CACHE_GEN%".
            echo NOTE: current generator is "%STASIS_CMAKE_GENERATOR%".
            echo NOTE: Delete runtime/build or set STASIS_CLEAN_RUNTIME_BUILD=1 to reconfigure.
        )
    )
)

:: Configure with CMake
echo Configuring...
if /I "%STASIS_CMAKE_GENERATOR%"=="Ninja" (
    cmake .. -G "%STASIS_CMAKE_GENERATOR%" -DCMAKE_TOOLCHAIN_FILE=%VCPKG_ROOT%\scripts\buildsystems\vcpkg.cmake -DVCPKG_TARGET_TRIPLET=%VCPKG_TRIPLET% -DSTASIS_GRAPHICS_BUILD_STATIC=ON -DSTASIS_GRAPHICS_BUILD_SHARED=ON
) else (
    cmake .. -G "%STASIS_CMAKE_GENERATOR%" -A x64 -DCMAKE_TOOLCHAIN_FILE=%VCPKG_ROOT%\scripts\buildsystems\vcpkg.cmake -DVCPKG_TARGET_TRIPLET=%VCPKG_TRIPLET% -DSTASIS_GRAPHICS_BUILD_STATIC=ON -DSTASIS_GRAPHICS_BUILD_SHARED=ON
)

if %ERRORLEVEL% neq 0 (
    echo CMake configuration failed.
    echo Hint: delete runtime\build or set STASIS_CLEAN_RUNTIME_BUILD=1.
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
echo Copying sys runtime lib to repo root and build/ for auto-discovery...
if exist "%CD%\\Release\\stasis_sys_static.lib" (
    if /I "%STASIS_OVERWRITE_CHECKED_IN_RUNTIME_LIBS%"=="1" (
        copy /Y "%CD%\\Release\\stasis_sys_static.lib" "%CD%\\..\\.." >NUL 2>&1
        if not exist "%CD%\\..\\..\\build" (
            mkdir "%CD%\\..\\..\\build" >NUL 2>&1
        )
        copy /Y "%CD%\\Release\\stasis_sys_static.lib" "%CD%\\..\\..\\build" >NUL 2>&1
    ) else (
        if not exist "%CD%\\..\\..\\stasis_sys_static.lib" (
            copy "%CD%\\Release\\stasis_sys_static.lib" "%CD%\\..\\.." >NUL 2>&1
        )
        if not exist "%CD%\\..\\..\\build" (
            mkdir "%CD%\\..\\..\\build" >NUL 2>&1
        )
        if not exist "%CD%\\..\\..\\build\\stasis_sys_static.lib" (
            copy "%CD%\\Release\\stasis_sys_static.lib" "%CD%\\..\\..\\build" >NUL 2>&1
        )
    )
)
echo.
echo To run Asteroids demo with static runtime:
echo   cd ..\..
echo   .\\stasis.bat run samples\\asteroids.stasis --graphics
echo (the CLI will pick up runtime\\build\\Release\\stasis_graphics_static.lib by default)

endlocal

goto :eof

:maybe_clean_stale_cmake_cache
set "CACHE_HOME="
set "CACHE_DIR="
for /f "tokens=2* delims==" %%A in ('findstr /b /c:"CMAKE_HOME_DIRECTORY:INTERNAL=" "build\\CMakeCache.txt"') do set "CACHE_HOME=%%B"
for /f "tokens=2* delims==" %%A in ('findstr /b /c:"CMAKE_CACHEFILE_DIR:INTERNAL=" "build\\CMakeCache.txt"') do set "CACHE_DIR=%%B"

set "RUNTIME_DIR=%CD%"
set "BUILD_DIR=%CD%\\build"

if defined CACHE_HOME (
    if /I not "!CACHE_HOME!"=="!RUNTIME_DIR!" goto :do_clean_cache
)
if defined CACHE_DIR (
    if /I not "!CACHE_DIR!"=="!BUILD_DIR!" goto :do_clean_cache
)
goto :eof

:do_clean_cache
echo NOTE: Detected stale CMake cache (previous root: "!CACHE_HOME!").
echo NOTE: Cleaning runtime build directory...
rmdir /s /q build
goto :eof
