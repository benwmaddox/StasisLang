$ErrorActionPreference = "Stop"

param(
    [string]$Triplet = "arm64-android",
    [string]$Configuration = "Release"
)

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
$vcpkgRoot = Join-Path $repoRoot "codevcpkg"
$vcpkgExe = Join-Path $vcpkgRoot "vcpkg.exe"

if (-not (Test-Path $vcpkgExe)) {
    throw "vcpkg.exe not found at $vcpkgExe (expected vendored vcpkg under codevcpkg/)"
}

$ndk = $env:ANDROID_NDK_HOME
if (-not $ndk) { $ndk = $env:ANDROID_NDK_ROOT }
if (-not $ndk) {
    throw "ANDROID_NDK_HOME (or ANDROID_NDK_ROOT) must be set to build for Android."
}
if (-not (Test-Path $ndk)) {
    throw "Android NDK path not found: $ndk"
}

if (-not (Get-Command cmake -ErrorAction SilentlyContinue)) {
    throw "cmake not found in PATH"
}

if (-not (Get-Command ninja -ErrorAction SilentlyContinue)) {
    throw "ninja not found in PATH (recommended generator for Android builds)"
}

Write-Host "NDK: $ndk"
Write-Host "vcpkg: $vcpkgExe"
Write-Host "Triplet: $Triplet"

# vcpkg expects ANDROID_NDK_HOME for Android triplets.
$env:ANDROID_NDK_HOME = $ndk

& $vcpkgExe install "sdl2:$Triplet" --recurse

$buildDir = Join-Path $PSScriptRoot ("build-android-" + $Triplet)
New-Item -ItemType Directory -Force -Path $buildDir | Out-Null

Push-Location $buildDir
try {
    & cmake .. -G Ninja `
        -DCMAKE_BUILD_TYPE="$Configuration" `
        -DCMAKE_TOOLCHAIN_FILE="$vcpkgRoot\\scripts\\buildsystems\\vcpkg.cmake" `
        -DVCPKG_TARGET_TRIPLET="$Triplet" `
        -DSTASIS_GRAPHICS_SDL_ONLY=ON `
        -DSTASIS_GRAPHICS_BUILD_SHARED=ON `
        -DSTASIS_GRAPHICS_BUILD_STATIC=OFF `
        -DSTASIS_BUILD_RUNNER=OFF

    & cmake --build .
}
finally {
    Pop-Location
}

Write-Host ""
Write-Host "Android runtime build complete:"
Write-Host "  $buildDir"

