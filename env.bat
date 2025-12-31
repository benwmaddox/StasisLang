@echo off

REM Shared environment setup for local scripts (build/test).
REM Keeps the repo self-contained by preferring toolchains in well-known locations.

set "REPO_ROOT=%~dp0"
if "%REPO_ROOT:~-1%"=="\" set "REPO_ROOT=%REPO_ROOT:~0,-1%"

REM Prefer the repo-pinned LLVM toolchain if present (pick the newest llvm-* folder).
set "LLVM_BIN="
for /f "delims=" %%D in ('dir /b /ad "%REPO_ROOT%\.tools\llvm-*" 2^>NUL') do (
  if exist "%REPO_ROOT%\.tools\%%D\bin\clang.exe" (
    set "LLVM_BIN=%REPO_ROOT%\.tools\%%D\bin"
  )
)
if defined LLVM_BIN (
  set "PATH=%LLVM_BIN%;%PATH%"
)

REM CMake (installed via winget/MSI by default).
set "CMAKE_BIN=%ProgramFiles%\CMake\bin"
if exist "%CMAKE_BIN%\cmake.exe" (
  set "PATH=%CMAKE_BIN%;%PATH%"
)

REM Rust (installed via rustup).
set "CARGO_BIN=%USERPROFILE%\.cargo\bin"
if exist "%CARGO_BIN%\cargo.exe" (
  set "PATH=%CARGO_BIN%;%PATH%"
)

REM vcpkg (used by runtime/build.bat). Prefer the conventional C:\vcpkg location.
if not defined VCPKG_ROOT (
  if exist "C:\vcpkg\vcpkg.exe" (
    set "VCPKG_ROOT=C:\vcpkg"
  ) else if exist "C:\code\vcpkg\vcpkg.exe" (
    set "VCPKG_ROOT=C:\code\vcpkg"
  )
)
