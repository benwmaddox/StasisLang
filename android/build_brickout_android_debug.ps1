$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Resolve-RepoRoot {
    return (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
}

function Resolve-VcpkgRoot {
    param([string]$RepoRoot)

    $vendored = Join-Path $RepoRoot "codevcpkg"
    if (Test-Path (Join-Path $vendored "vcpkg.exe")) {
        return $vendored
    }

    $envRoot = $env:VCPKG_ROOT
    if ($envRoot -and (Test-Path (Join-Path $envRoot "vcpkg.exe"))) {
        return $envRoot
    }

    $default = "C:\\vcpkg"
    if (Test-Path (Join-Path $default "vcpkg.exe")) {
        return $default
    }

    throw "vcpkg not found (set VCPKG_ROOT or install to C:\\vcpkg)"
}

function Resolve-AndroidNdk {
    $ndk = $env:ANDROID_NDK_HOME
    if (-not $ndk) { $ndk = $env:ANDROID_NDK_ROOT }
    if (-not $ndk) { throw "ANDROID_NDK_HOME (or ANDROID_NDK_ROOT) must be set" }
    if (-not (Test-Path $ndk)) { throw "Android NDK path not found: $ndk" }
    return (Resolve-Path $ndk).Path
}

function Require-Command {
    param([string]$Name)
    if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) {
        throw "$Name not found in PATH"
    }
}

function Find-LibSDL2 {
    param(
        [string]$VcpkgRoot,
        [string]$Triplet
    )

    $installed = Join-Path $VcpkgRoot "installed\\$Triplet"
    if (-not (Test-Path $installed)) {
        return $null
    }

    $candidates = @(
        (Join-Path $installed "lib\\libSDL2.so"),
        (Join-Path $installed "bin\\libSDL2.so"),
        (Join-Path $installed "lib\\libSDL2-2.0.so"),
        (Join-Path $installed "bin\\libSDL2-2.0.so")
    )

    foreach ($c in $candidates) {
        if (Test-Path $c) {
            return $c
        }
    }

    $found = Get-ChildItem -Path $installed -Recurse -File -ErrorAction SilentlyContinue -Filter "libSDL2*.so" | Select-Object -First 1
    if ($found) { return $found.FullName }

    return $null
}

$repoRoot = Resolve-RepoRoot
$ndk = Resolve-AndroidNdk

Require-Command "cmake"
Require-Command "ninja"
Require-Command "dotnet"

$vcpkgRoot = Resolve-VcpkgRoot -RepoRoot $repoRoot
$vcpkgExe = Join-Path $vcpkgRoot "vcpkg.exe"
$triplet = "arm64-android"
$emitHeapLimitBytes = 2147483648

Write-Host "Repo:  $repoRoot"
Write-Host "NDK:   $ndk"
Write-Host "vcpkg: $vcpkgExe"

# vcpkg expects ANDROID_NDK_HOME for Android triplets.
$env:ANDROID_NDK_HOME = $ndk

& $vcpkgExe install "sdl2:$triplet" "libiconv:$triplet" --recurse

$outDir = Join-Path $repoRoot "android\\out"
New-Item -ItemType Directory -Force -Path $outDir | Out-Null

$game = Join-Path $repoRoot "samples\\brickout_revenge\\brickout_revenge.stasis"
$structMetaPath = Join-Path $repoRoot "samples\\brickout_revenge\\data\\brickout_revenge.struct-meta.json"
$stamp = Get-Date -Format "yyyyMMdd_HHmmss"
$llPath = Join-Path $outDir "brickout_revenge_$stamp.ll"

Write-Host ""
Write-Host "Emitting LLVM IR to: $llPath"

$stasisBat = Join-Path $repoRoot "stasis.bat"
if (-not (Test-Path $stasisBat)) {
    throw "stasis.bat not found at $stasisBat"
}

$emitCmd = @(
    "`"$stasisBat`"",
    "run",
    "`"$game`"",
    "--backend", "llvm",
    "--graphics",
    "--emit-ir",
    "--llvm-target", "aarch64-linux-android21",
    "--out", "`"$llPath`"",
    "--emit-struct-meta", "`"$structMetaPath`""
) -join " "

$prevGcServer = $env:DOTNET_gcServer
$prevHeapLimit = $env:DOTNET_GCHeapHardLimit
$env:DOTNET_gcServer = "0"
$env:DOTNET_GCHeapHardLimit = "$emitHeapLimitBytes"
try {
    cmd /c "$emitCmd"
}
finally {
    $env:DOTNET_gcServer = $prevGcServer
    $env:DOTNET_GCHeapHardLimit = $prevHeapLimit
}
if ($LASTEXITCODE -ne 0) { throw "stasis IR emit failed with exit code $LASTEXITCODE" }

$toolchain = Join-Path $vcpkgRoot "scripts\\buildsystems\\vcpkg.cmake"
if (-not (Test-Path $toolchain)) {
    throw "vcpkg toolchain not found: $toolchain"
}

$ndkToolchain = Join-Path $ndk "build\\cmake\\android.toolchain.cmake"
if (-not (Test-Path $ndkToolchain)) {
    throw "Android NDK toolchain not found: $ndkToolchain"
}

$nativeBuildDir = Join-Path $repoRoot "android\\build\\brickout-android-arm64-debug"
New-Item -ItemType Directory -Force -Path $nativeBuildDir | Out-Null

Write-Host ""
Write-Host "Building native libmain.so..."

& cmake -S (Join-Path $repoRoot "android\\brickout-revenge\\native") -B $nativeBuildDir -G Ninja `
    -DCMAKE_BUILD_TYPE=Debug `
    -DCMAKE_TOOLCHAIN_FILE="$toolchain" `
    -DVCPKG_CHAINLOAD_TOOLCHAIN_FILE="$ndkToolchain" `
    -DANDROID_ABI="arm64-v8a" `
    -DANDROID_PLATFORM="android-21" `
    -DVCPKG_TARGET_TRIPLET="$triplet" `
    -DSTASIS_REPO_ROOT="$repoRoot" `
    -DSTASIS_GAME_LL="$llPath"
if ($LASTEXITCODE -ne 0) { throw "cmake configure failed with exit code $LASTEXITCODE" }

& cmake --build $nativeBuildDir
if ($LASTEXITCODE -ne 0) { throw "cmake build failed with exit code $LASTEXITCODE" }

$libMain = Join-Path $nativeBuildDir "libmain.so"
if (-not (Test-Path $libMain)) {
    throw "Expected output not found: $libMain"
}

$libSdl = Find-LibSDL2 -VcpkgRoot $vcpkgRoot -Triplet $triplet
if (-not $libSdl) {
    Write-Host "libSDL2.so not found under vcpkg installed/$triplet; assuming SDL2 is statically linked into libmain.so."
}

$jniDir = Join-Path $repoRoot "android\\brickout-revenge\\app\\src\\main\\jniLibs\\arm64-v8a"
New-Item -ItemType Directory -Force -Path $jniDir | Out-Null

Copy-Item -Force $libMain (Join-Path $jniDir "libmain.so")
if ($libSdl) {
    Copy-Item -Force $libSdl (Join-Path $jniDir "libSDL2.so")
}

Write-Host ""
Write-Host "Packaging APK (Gradle)..."
Push-Location (Join-Path $repoRoot "android\\brickout-revenge")
try {
    cmd /c "gradlew.bat assembleDebug"
    if ($LASTEXITCODE -ne 0) { throw "gradle assembleDebug failed with exit code $LASTEXITCODE" }
}
finally {
    Pop-Location
}

$apk = Join-Path $repoRoot "android\\brickout-revenge\\app\\build\\outputs\\apk\\debug\\app-debug.apk"
if (Test-Path $apk) {
    Write-Host ""
    Write-Host "APK: $apk"
    exit 0
}

throw "APK not found at expected path: $apk"
