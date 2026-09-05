$ErrorActionPreference = "Stop"

param(
    [string]$Triplet = "arm64-android",
    [string]$Configuration = "Release"
)

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
Write-Host "Triplet: $Triplet"

$buildDir = Join-Path $PSScriptRoot ("build-android-" + $Triplet)
New-Item -ItemType Directory -Force -Path $buildDir | Out-Null

Push-Location $buildDir
try {
    & cmake .. -G Ninja `
        -DCMAKE_BUILD_TYPE="$Configuration" `
        -DCMAKE_TOOLCHAIN_FILE="$ndk\\build\\cmake\\android.toolchain.cmake" `
        -DANDROID_ABI=arm64-v8a `
        -DANDROID_PLATFORM=android-26 `
        -DSTASIS_GRAPHICS_BUNDLE_SDL=ON `
        `
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

