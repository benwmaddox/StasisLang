param(
    [string]$AndroidHome = "",
    [string]$NdkVersion = "",
    [int]$MinSdk = 26,
    [string[]]$Abis = @("arm64-v8a", "x86_64"),
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
$sharedAiRoot = Join-Path $codexRustRoot "stasis-ai"
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
New-Item -ItemType Directory -Force (Join-Path $sharedAiRoot "src") | Out-Null
Copy-Item -Force (Join-Path $repoRoot "crates\stasis_ai\Cargo.toml") (Join-Path $sharedAiRoot "Cargo.toml")
Copy-Item -Force (Join-Path $repoRoot "crates\stasis_ai\src\lib.rs") (Join-Path $sharedAiRoot "src\lib.rs")
$sharedManifest = Join-Path $sharedAiRoot "Cargo.toml"
$sharedCargo = Get-Content -Raw $sharedManifest
$sharedCargo = $sharedCargo.Replace('version.workspace = true', 'version = "0.1.0"')
$sharedCargo = $sharedCargo.Replace('edition.workspace = true', 'edition = "2021"')
$sharedCargo = $sharedCargo.Replace('license.workspace = true', 'license = "MIT"')
$sharedCargo = $sharedCargo.Replace('serde.workspace = true', 'serde = { version = "1", features = ["derive"] }')
$sharedCargo = $sharedCargo.Replace('serde_json.workspace = true', 'serde_json = "1"')
Set-Content -NoNewline -Path $sharedManifest -Value $sharedCargo
$wrapperManifest = Join-Path $wrapperRoot "Cargo.toml"
$wrapperCargo = Get-Content -Raw $wrapperManifest
$wrapperCargo = $wrapperCargo.Replace('../../../crates/stasis_ai', '../stasis-ai')
Set-Content -NoNewline -Path $wrapperManifest -Value $wrapperCargo

$env:CARGO_INCREMENTAL = "0"
$profileArgs = @()
$profileDir = "debug"
if ($Release) {
    $profileArgs += "--release"
    $profileDir = "release"
}

$targets = @{
    "arm64-v8a" = @{ Rust = "aarch64-linux-android"; Clang = "aarch64-linux-android" }
    "x86_64" = @{ Rust = "x86_64-linux-android"; Clang = "x86_64-linux-android" }
}
$installedTargets = & rustup target list --installed --toolchain 1.95.0
foreach ($abi in $Abis) {
    $target = $targets[$abi]
    if (-not $target) { throw "Unsupported Android ABI: $abi" }
    $rustTarget = $target.Rust
    $linker = Join-Path $toolchain "$($target.Clang)$MinSdk-clang.cmd"
    if (-not (Test-Path $linker)) { throw "Android linker not found: $linker" }
    if ($installedTargets -notcontains $rustTarget) {
        rustup target add $rustTarget --toolchain 1.95.0
        if ($LASTEXITCODE -ne 0) { throw "Rust Android target installation failed: $rustTarget" }
    }

    $targetEnvName = $rustTarget.Replace('-', '_')
    $linkerVariable = "CARGO_TARGET_$($rustTarget.ToUpperInvariant().Replace('-', '_'))_LINKER"
    Set-Item -Path "Env:$linkerVariable" -Value $linker
    Set-Item -Path "Env:CC_$targetEnvName" -Value $linker
    Set-Item -Path "Env:AR_$targetEnvName" -Value (Join-Path $toolchain "llvm-ar.exe")

    cargo +1.95.0 build --manifest-path (Join-Path $wrapperRoot "Cargo.toml") --target $rustTarget @profileArgs
    if ($LASTEXITCODE -ne 0) { throw "Codex Android native build failed with exit code $LASTEXITCODE" }

    $source = Join-Path $codexRustRoot "target\$rustTarget\$profileDir\libstasis_codex_android.so"
    if (-not (Test-Path $source)) { throw "Codex Android library was not produced: $source" }
    $destDir = Join-Path $scriptRoot "app\src\workshop\jniLibs\$abi"
    New-Item -ItemType Directory -Force $destDir | Out-Null
    $dest = Join-Path $destDir "libstasis_codex_android.so"
    Copy-Item -Force $source $dest
    Write-Host "Packaged phone-native Codex login bridge: $dest"
}

$metadataTarget = $targets[$Abis[0]].Rust
$metadata = cargo +1.95.0 metadata --format-version 1 --filter-platform $metadataTarget `
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
