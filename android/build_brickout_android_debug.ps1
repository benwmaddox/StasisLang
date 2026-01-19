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

    $fDrive = "F:\\vcpkg"
    if (Test-Path (Join-Path $fDrive "vcpkg.exe")) {
        return $fDrive
    }

    $default = "C:\\vcpkg"
    if (Test-Path (Join-Path $default "vcpkg.exe")) {
        return $default
    }

    throw "vcpkg not found (set VCPKG_ROOT or install to C:\\vcpkg)"
}

function Resolve-AndroidNdk {
    param([string]$RepoRoot)

    function Resolve-NdkVersionDir {
        param([string]$NdkRoot)
        if (-not $NdkRoot -or -not (Test-Path $NdkRoot)) { return $null }

        # If $NdkRoot already looks like an NDK (contains build/cmake/android.toolchain.cmake),
        # return it. Otherwise assume it's the parent of versioned ndk folders.
        $toolchain = Join-Path $NdkRoot "build\\cmake\\android.toolchain.cmake"
        if (Test-Path $toolchain) { return (Resolve-Path $NdkRoot).Path }

        $versions = Get-ChildItem -Path $NdkRoot -Directory -ErrorAction SilentlyContinue |
            Sort-Object Name -Descending
        foreach ($v in $versions) {
            $t = Join-Path $v.FullName "build\\cmake\\android.toolchain.cmake"
            if (Test-Path $t) { return $v.FullName }
        }
        return $null
    }

    $ndkEnv = $env:ANDROID_NDK_HOME
    if (-not $ndkEnv) { $ndkEnv = $env:ANDROID_NDK_ROOT }

    # Prefer an NDK on F:\ (or inside the repo) if available, to keep the toolchain on the F drive.
    $preferred = @(
        (Join-Path $RepoRoot ".tools\\android-sdk\\ndk"),
        "F:\\Android\\Sdk\\ndk"
    )

    foreach ($p in $preferred) {
        $resolved = Resolve-NdkVersionDir -NdkRoot $p
        if ($resolved) { return (Resolve-Path $resolved).Path }
    }

    if ($ndkEnv) {
        if (-not (Test-Path $ndkEnv)) { throw "Android NDK path not found: $ndkEnv" }
        return (Resolve-Path $ndkEnv).Path
    }

    # Fallback auto-detect (may be on C:\ depending on the machine).
    $fallback = @(
        "C:\\Android\\Sdk\\ndk",
        (Join-Path $env:LOCALAPPDATA "Android\\Sdk\\ndk")
    )
    foreach ($p in $fallback) {
        $resolved = Resolve-NdkVersionDir -NdkRoot $p
        if ($resolved) { return (Resolve-Path $resolved).Path }
    }

    throw "ANDROID_NDK_HOME (or ANDROID_NDK_ROOT) must be set (or install an NDK under F:\\Android\\Sdk\\ndk)"
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
$ndk = Resolve-AndroidNdk -RepoRoot $repoRoot

Require-Command "cmake"
Require-Command "ninja"
Require-Command "dotnet"

$vcpkgRoot = Resolve-VcpkgRoot -RepoRoot $repoRoot
$vcpkgExe = Join-Path $vcpkgRoot "vcpkg.exe"
$triplet = "arm64-android-dynamic"
$androidPlatform = "android-28"
$emitHeapLimitBytes = 2147483648

Write-Host "Repo:  $repoRoot"
Write-Host "NDK:   $ndk"
Write-Host "vcpkg: $vcpkgExe"

# vcpkg expects ANDROID_NDK_HOME for Android triplets.
$env:ANDROID_NDK_HOME = $ndk
$env:VCPKG_OVERLAY_TRIPLETS = Join-Path $repoRoot "android\\vcpkg-triplets"

& $vcpkgExe install "sdl2:$triplet" --recurse

$outDir = Join-Path $repoRoot "android\\out"
New-Item -ItemType Directory -Force -Path $outDir | Out-Null

$game = Join-Path $repoRoot "samples\\brickout_revenge\\brickout_revenge_v1.stasis"
$stamp = Get-Date -Format "yyyyMMdd_HHmmss"
$llPath = Join-Path $outDir "brickout_revenge_$stamp.ll"

Write-Host ""
Write-Host "Emitting LLVM IR to: $llPath"

$cliProject = Join-Path $repoRoot "Stasis.Cli\\Stasis.Cli.csproj"
if (-not (Test-Path $cliProject)) {
    throw "Stasis CLI project not found at $cliProject"
}

& dotnet build -c Release $cliProject
if ($LASTEXITCODE -ne 0) { throw "dotnet build failed with exit code $LASTEXITCODE" }

$cliDll = Join-Path $repoRoot "Stasis.Cli\\bin\\Release\\net9.0\\Stasis.Cli.dll"
if (-not (Test-Path $cliDll)) {
    throw "Stasis CLI dll not found at $cliDll"
}

$prevGcServer = $env:DOTNET_gcServer
$prevHeapLimit = $env:DOTNET_GCHeapHardLimit
$env:DOTNET_gcServer = "0"
$env:DOTNET_GCHeapHardLimit = "$emitHeapLimitBytes"
try {
    # Invoke the dll directly to avoid Application Control blocking `Stasis.Cli.exe`.
    & dotnet $cliDll run $game --backend llvm --graphics --emit-ir --llvm-target aarch64-linux-android28 --out $llPath
}
finally {
    $env:DOTNET_gcServer = $prevGcServer
    $env:DOTNET_GCHeapHardLimit = $prevHeapLimit
}
if ($LASTEXITCODE -ne 0) { throw "stasis IR emit failed with exit code $LASTEXITCODE" }

$androidToolchain = Join-Path $ndk "build\\cmake\\android.toolchain.cmake"
if (-not (Test-Path $androidToolchain)) {
    throw "Android CMake toolchain not found: $androidToolchain"
}

$vcpkgInstalled = Join-Path $vcpkgRoot "installed\\$triplet"
if (-not (Test-Path $vcpkgInstalled)) {
    throw "vcpkg installed triplet not found: $vcpkgInstalled (did vcpkg install succeed?)"
}

$sdl2Dir = Join-Path $vcpkgInstalled "share\\sdl2"
if (-not (Test-Path $sdl2Dir)) {
    throw "SDL2Config.cmake directory not found: $sdl2Dir"
}

$nativeBuildDir = Join-Path $repoRoot "android\\build\\brickout-android-arm64-debug"
if (Test-Path $nativeBuildDir) {
    Remove-Item -Recurse -Force $nativeBuildDir
}
New-Item -ItemType Directory -Force -Path $nativeBuildDir | Out-Null

Write-Host ""
Write-Host "Building native libmain.so..."

& cmake -S (Join-Path $repoRoot "android\\brickout-revenge\\native") -B $nativeBuildDir -G Ninja `
    -DCMAKE_BUILD_TYPE=Debug `
    -DCMAKE_TOOLCHAIN_FILE="$androidToolchain" `
    -DANDROID_ABI="arm64-v8a" `
    -DANDROID_PLATFORM="$androidPlatform" `
    -DCMAKE_TRY_COMPILE_TARGET_TYPE="STATIC_LIBRARY" `
    -DCMAKE_PREFIX_PATH="$vcpkgInstalled" `
    -DSDL2_DIR="$sdl2Dir" `
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
    throw "libSDL2.so not found under vcpkg installed/$triplet (check your vcpkg triplet linkage)"
}

$jniDir = Join-Path $repoRoot "android\\brickout-revenge\\app\\src\\main\\jniLibs\\arm64-v8a"
New-Item -ItemType Directory -Force -Path $jniDir | Out-Null

Copy-Item -Force $libMain (Join-Path $jniDir "libmain.so")
Copy-Item -Force $libSdl (Join-Path $jniDir "libSDL2.so")

Write-Host ""
Write-Host "Packaging APK (Gradle)..."
Push-Location (Join-Path $repoRoot "android\\brickout-revenge")
try {
    # Keep gradle caches on the repo drive.
    $env:GRADLE_USER_HOME = Join-Path $repoRoot "android\\.gradle"
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
