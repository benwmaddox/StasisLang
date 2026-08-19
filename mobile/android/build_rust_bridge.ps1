param(
    [string]$AndroidHome = "",
    [string]$NdkVersion = "",
    [int]$MinSdk = 26,
    [string[]]$Abis = @("arm64-v8a", "x86_64"),
    [switch]$Release,
    [switch]$Debug
)

$ErrorActionPreference = "Stop"

$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Split-Path -Parent (Split-Path -Parent $scriptRoot)
$runningOnWindows = [System.IO.Path]::DirectorySeparatorChar -eq [char]'\'
$executableSuffix = if ($runningOnWindows) { ".exe" } else { "" }

if (-not $AndroidHome) {
    if ($env:ANDROID_HOME) {
        $AndroidHome = $env:ANDROID_HOME
    } elseif ($env:ANDROID_SDK_ROOT) {
        $AndroidHome = $env:ANDROID_SDK_ROOT
    } else {
        $AndroidHome = if ($runningOnWindows) {
            "C:\Android\Sdk"
        } else {
            Join-Path ([Environment]::GetFolderPath("UserProfile")) "Android/sdk"
        }
    }
}

$ndkRoot = if ($NdkVersion) {
    Join-Path (Join-Path $AndroidHome "ndk") $NdkVersion
} else {
    $ndkParent = Join-Path $AndroidHome "ndk"
    $ndks = Get-ChildItem -Directory $ndkParent -ErrorAction SilentlyContinue | Sort-Object Name -Descending
    if (-not $ndks) {
        throw "No Android NDK found under $ndkParent. Install an NDK with Android Studio or set ANDROID_HOME."
    }
    $ndks[0].FullName
}

$prebuiltName = if ($runningOnWindows) { "windows-x86_64" } else { "linux-x86_64" }
$prebuilt = Join-Path (Join-Path (Join-Path $ndkRoot "toolchains") "llvm") (Join-Path "prebuilt" $prebuiltName)
$linkerSuffix = if ($runningOnWindows) { ".cmd" } else { "" }
$installedTargets = & rustup target list --installed
if ($LASTEXITCODE -ne 0) { throw "rustup target discovery failed with exit code $LASTEXITCODE" }
$env:CARGO_INCREMENTAL = "0"
$useRelease = -not $Debug
if ($Release -and $Debug) {
    throw "Choose either -Release or -Debug, not both."
}
if ($Release) {
    $useRelease = $true
}
$profileArgs = @()
$profileDir = "debug"
if ($useRelease) {
    $profileArgs += "--release"
    $profileDir = "release"
}

$targets = @{
    "arm64-v8a" = @{ Rust = "aarch64-linux-android"; Clang = "aarch64-linux-android" }
    "x86_64" = @{ Rust = "x86_64-linux-android"; Clang = "x86_64-linux-android" }
}
$requiredAbis = @("arm64-v8a", "x86_64")
$requestedAbis = @($Abis | Sort-Object -Unique)
if ($requestedAbis.Count -ne $requiredAbis.Count -or
        (Compare-Object ($requiredAbis | Sort-Object) $requestedAbis)) {
    throw "Workshop Rust bridge packaging requires both ABIs: $($requiredAbis -join ', ')."
}

foreach ($abi in $Abis) {
    $target = $targets[$abi]
    if (-not $target) { throw "Unsupported Android ABI: $abi" }
    $rustTarget = $target.Rust
    $linker = Join-Path (Join-Path $prebuilt "bin") "$($target.Clang)$MinSdk-clang$linkerSuffix"
    if (-not (Test-Path $linker)) { throw "Android linker not found: $linker" }
    if ($installedTargets -notcontains $rustTarget) {
        throw "Rust target $rustTarget is not installed. Run: rustup target add $rustTarget"
    }

    $targetEnvName = $rustTarget.Replace('-', '_')
    $linkerVariable = "CARGO_TARGET_$($rustTarget.ToUpperInvariant().Replace('-', '_'))_LINKER"
    Set-Item -Path "Env:$linkerVariable" -Value $linker
    Set-Item -Path "Env:CC_$targetEnvName" -Value $linker
    Set-Item -Path "Env:AR_$targetEnvName" -Value (Join-Path (Join-Path $prebuilt "bin") "llvm-ar$executableSuffix")

    Push-Location $repoRoot
    try {
        $cargoCache = Join-Path (Join-Path $repoRoot "tools") "cargo_cache.py"
        & python $cargoCache run -- cargo build -p stasis_android_bridge --target $rustTarget @profileArgs
        if ($LASTEXITCODE -ne 0) { throw "Rust Android bridge build failed with exit code $LASTEXITCODE" }
    } finally {
        Pop-Location
    }

    $targetRoot = if ($env:CARGO_TARGET_DIR) {
        if ([System.IO.Path]::IsPathRooted($env:CARGO_TARGET_DIR)) {
            [System.IO.Path]::GetFullPath($env:CARGO_TARGET_DIR)
        } else {
            [System.IO.Path]::GetFullPath((Join-Path $repoRoot $env:CARGO_TARGET_DIR))
        }
    } else {
        $commonGitDir = (& git -C $repoRoot rev-parse --path-format=absolute --git-common-dir).Trim()
        Join-Path (Split-Path $commonGitDir -Parent) (Join-Path "build" "codex-cargo-target")
    }
    $source = Join-Path (Join-Path (Join-Path $targetRoot $rustTarget) $profileDir) "libstasis_android_bridge.so"
    if (-not (Test-Path $source)) {
        $source = Join-Path (Join-Path (Join-Path $repoRoot "target") $rustTarget) (Join-Path $profileDir "libstasis_android_bridge.so")
    }
    if (-not (Test-Path $source)) { throw "Rust bridge output was not produced: $source" }
    $destDir = Join-Path (Join-Path (Join-Path (Join-Path (Join-Path $scriptRoot "app") "src") "workshop") "jniLibs") $abi
    New-Item -ItemType Directory -Force $destDir | Out-Null
    $dest = Join-Path $destDir "libstasis_android_bridge.so"
    Copy-Item -Force $source $dest
    Write-Host "Packaged Rust Android bridge: $dest"
}

& (Join-Path $scriptRoot "rust_bridge_provenance.ps1") -Mode Write -Profile $profileDir
if ($LASTEXITCODE -ne 0) { throw "Rust bridge provenance write failed with exit code $LASTEXITCODE" }
