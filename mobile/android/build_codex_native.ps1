param(
    [string]$AndroidHome = "",
    [string]$NdkVersion = "",
    [int]$MinSdk = 26,
    [switch]$Release
)

$ErrorActionPreference = "Stop"

$codexRevision = "5c19155cbd93bfa099016e7487259f61669823ff"
$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Split-Path -Parent (Split-Path -Parent $scriptRoot)
$buildRoot = Join-Path $repoRoot "build\android_codex_native"
$upstreamRoot = Join-Path $buildRoot "codex"
$codexRustRoot = Join-Path $upstreamRoot "codex-rs"
$wrapperRoot = Join-Path $codexRustRoot "stasis-codex-android"
$patchPath = Join-Path $scriptRoot "patches\codex-android-rustls.patch"

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
    $ndks = Get-ChildItem -Directory (Join-Path $AndroidHome "ndk") -ErrorAction SilentlyContinue |
        Sort-Object Name -Descending
    if (-not $ndks) { throw "No Android NDK found under $AndroidHome\ndk" }
    $ndks[0].FullName
}
$toolchain = Join-Path $ndkRoot "toolchains\llvm\prebuilt\windows-x86_64\bin"
$linker = Join-Path $toolchain "aarch64-linux-android$MinSdk-clang.cmd"
if (-not (Test-Path $linker)) { throw "Android linker not found: $linker" }

if (-not (Test-Path (Join-Path $upstreamRoot ".git"))) {
    New-Item -ItemType Directory -Force $buildRoot | Out-Null
    git clone --filter=blob:none https://github.com/openai/codex.git $upstreamRoot
    if ($LASTEXITCODE -ne 0) { throw "Codex clone failed with exit code $LASTEXITCODE" }
}

git -C $upstreamRoot fetch origin $codexRevision
if ($LASTEXITCODE -ne 0) { throw "Codex fetch failed with exit code $LASTEXITCODE" }
git -C $upstreamRoot checkout --detach --force $codexRevision
if ($LASTEXITCODE -ne 0) { throw "Codex checkout failed with exit code $LASTEXITCODE" }
git -C $upstreamRoot apply --ignore-space-change --check $patchPath
if ($LASTEXITCODE -eq 0) {
    git -C $upstreamRoot apply --ignore-space-change $patchPath
    if ($LASTEXITCODE -ne 0) { throw "Codex Android TLS patch failed with exit code $LASTEXITCODE" }
} else {
    git -C $upstreamRoot apply --ignore-space-change --reverse --check $patchPath
    if ($LASTEXITCODE -ne 0) { throw "Codex checkout does not match the pinned Android TLS patch" }
}

New-Item -ItemType Directory -Force (Join-Path $wrapperRoot "src") | Out-Null
Copy-Item -Force (Join-Path $scriptRoot "codex_native\Cargo.toml") (Join-Path $wrapperRoot "Cargo.toml")
Copy-Item -Force (Join-Path $scriptRoot "codex_native\src\lib.rs") (Join-Path $wrapperRoot "src\lib.rs")

$installedTargets = & rustup target list --installed --toolchain 1.95.0
if ($installedTargets -notcontains "aarch64-linux-android") {
    rustup target add aarch64-linux-android --toolchain 1.95.0
    if ($LASTEXITCODE -ne 0) { throw "Rust Android target installation failed" }
}

$env:CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER = $linker
$env:CC_aarch64_linux_android = $linker
$env:AR_aarch64_linux_android = Join-Path $toolchain "llvm-ar.exe"
$env:CARGO_INCREMENTAL = "0"
$profileArgs = @()
$profileDir = "debug"
if ($Release) {
    $profileArgs += "--release"
    $profileDir = "release"
}

cargo +1.95.0 build --manifest-path (Join-Path $wrapperRoot "Cargo.toml") --target aarch64-linux-android @profileArgs
if ($LASTEXITCODE -ne 0) { throw "Codex Android native build failed with exit code $LASTEXITCODE" }

$source = Join-Path $codexRustRoot "target\aarch64-linux-android\$profileDir\libstasis_codex_android.so"
if (-not (Test-Path $source)) { throw "Codex Android library was not produced: $source" }
$destDir = Join-Path $scriptRoot "app\src\workshop\jniLibs\arm64-v8a"
New-Item -ItemType Directory -Force $destDir | Out-Null
$dest = Join-Path $destDir "libstasis_codex_android.so"
Copy-Item -Force $source $dest

$metadata = cargo +1.95.0 metadata --format-version 1 --filter-platform aarch64-linux-android `
    --manifest-path (Join-Path $wrapperRoot "Cargo.toml") | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) { throw "Codex Android dependency discovery failed" }
$verifierPackage = $metadata.packages | Where-Object { $_.name -eq "rustls-platform-verifier-android" } |
    Select-Object -First 1
if (-not $verifierPackage) { throw "Rustls Android verifier package was not found" }
$verifierRoot = Split-Path -Parent $verifierPackage.manifest_path
$verifierAar = Get-ChildItem -Recurse -Filter "rustls-platform-verifier-*.aar" (Join-Path $verifierRoot "maven") |
    Select-Object -First 1
if (-not $verifierAar) { throw "Rustls Android verifier AAR was not found" }
$aarDir = Join-Path $scriptRoot "app\src\workshop\libs"
New-Item -ItemType Directory -Force $aarDir | Out-Null
Copy-Item -Force $verifierAar.FullName (Join-Path $aarDir "rustls-platform-verifier.aar")
Write-Host "Packaged phone-native Codex login bridge: $dest"
