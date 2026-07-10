param(
    [string]$AndroidHome = "",
    [string]$NdkVersion = "",
    [int]$MinSdk = 26,
    [switch]$Release
)

$ErrorActionPreference = "Stop"

$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Split-Path -Parent (Split-Path -Parent $scriptRoot)

if (-not $AndroidHome) {
    if ($env:ANDROID_HOME) {
        $AndroidHome = $env:ANDROID_HOME
    } elseif ($env:ANDROID_SDK_ROOT) {
        $AndroidHome = $env:ANDROID_SDK_ROOT
    } else {
        $AndroidHome = "C:\Android\Sdk"
    }
}

$ndkRoot = if ($NdkVersion) {
    Join-Path $AndroidHome "ndk\$NdkVersion"
} else {
    $ndkParent = Join-Path $AndroidHome "ndk"
    $ndks = Get-ChildItem -Directory $ndkParent -ErrorAction SilentlyContinue | Sort-Object Name -Descending
    if (-not $ndks) {
        throw "No Android NDK found under $ndkParent. Install an NDK with Android Studio or set ANDROID_HOME."
    }
    $ndks[0].FullName
}

$prebuilt = Join-Path $ndkRoot "toolchains\llvm\prebuilt\windows-x86_64"
$linker = Join-Path $prebuilt "bin\aarch64-linux-android$MinSdk-clang.cmd"
if (-not (Test-Path $linker)) {
    throw "Android linker not found: $linker"
}

$installedTargets = & rustup target list --installed
if ($LASTEXITCODE -ne 0) { throw "rustup target discovery failed with exit code $LASTEXITCODE" }
if ($installedTargets -notcontains "aarch64-linux-android") {
    throw "Rust target aarch64-linux-android is not installed. Run: rustup target add aarch64-linux-android"
}

$env:CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER = $linker
$env:CARGO_INCREMENTAL = "0"
$profileArgs = @()
$profileDir = "debug"
if ($Release) {
    $profileArgs += "--release"
    $profileDir = "release"
}

Push-Location $repoRoot
try {
    cargo build -p stasis_android_bridge --target aarch64-linux-android @profileArgs
    if ($LASTEXITCODE -ne 0) { throw "Rust Android bridge build failed with exit code $LASTEXITCODE" }
} finally {
    Pop-Location
}

$source = Join-Path $repoRoot "target\aarch64-linux-android\$profileDir\libstasis_android_bridge.so"
if (-not (Test-Path $source)) {
    throw "Rust bridge output was not produced: $source"
}

$destDir = Join-Path $scriptRoot "app\src\workshop\jniLibs\arm64-v8a"
New-Item -ItemType Directory -Force $destDir | Out-Null
$dest = Join-Path $destDir "libstasis_android_bridge.so"
Copy-Item -Force $source $dest
Write-Host "Packaged Rust Android bridge: $dest"
