param(
    [ValidateSet("Write", "Verify")]
    [string]$Mode = "Verify",
    [ValidateSet("release", "debug")]
    [string]$Profile = "release"
)

$ErrorActionPreference = "Stop"

$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Split-Path -Parent (Split-Path -Parent $scriptRoot)
$jniRoot = Join-Path (Join-Path (Join-Path (Join-Path $scriptRoot "app") "src") "workshop") "jniLibs"
$manifestPath = Join-Path $jniRoot "stasis-rust-bridge.json"
$requiredTargets = [ordered]@{
    "arm64-v8a" = "aarch64-linux-android"
    "x86_64" = "x86_64-linux-android"
}

function Get-BridgeInputFingerprint([string]$Root) {
    $rootFiles = @(
        Get-Item -LiteralPath (Join-Path $Root "Cargo.toml"), (Join-Path $Root "Cargo.lock")
    )
    $crateFiles = Get-ChildItem -LiteralPath (Join-Path $Root "crates") -Recurse -File |
        Where-Object {
            $_.Name -eq "Cargo.toml" -or
            $_.Name -eq "build.rs" -or
            $_.Extension -in @(".rs", ".stasis", ".json", ".c", ".h", ".txt")
        }
    $records = foreach ($file in @($rootFiles) + @($crateFiles) | Sort-Object FullName) {
        $relative = $file.FullName.Substring($Root.Length).TrimStart("\", "/").Replace("\", "/")
        $fileHash = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
        "$relative`t$fileHash`n"
    }
    $bytes = [System.Text.Encoding]::UTF8.GetBytes(($records -join ""))
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        return ([BitConverter]::ToString($sha.ComputeHash($bytes))).Replace("-", "").ToLowerInvariant()
    } finally {
        $sha.Dispose()
    }
}

function Get-BridgeEntries([string]$ExpectedProfile) {
    $entries = @()
    foreach ($pair in $requiredTargets.GetEnumerator()) {
        $abi = $pair.Key
        $bridge = Join-Path (Join-Path $jniRoot $abi) "libstasis_android_bridge.so"
        if (-not (Test-Path -LiteralPath $bridge)) {
            throw "Workshop Rust bridge is missing ABI $abi."
        }
        $item = Get-Item -LiteralPath $bridge
        $entries += [ordered]@{
            abi = $abi
            rustTarget = $pair.Value
            profile = $ExpectedProfile
            file = "$abi/libstasis_android_bridge.so"
            bytes = [long]$item.Length
            sha256 = (Get-FileHash -LiteralPath $bridge -Algorithm SHA256).Hash.ToLowerInvariant()
        }
    }
    return $entries
}

if ($Mode -eq "Write") {
    $manifest = [ordered]@{
        schemaVersion = 1
        crate = "stasis_android_bridge"
        profile = $Profile
        inputFingerprint = Get-BridgeInputFingerprint $repoRoot
        entries = @(Get-BridgeEntries $Profile)
    }
    $manifest | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $manifestPath -Encoding UTF8
    Write-Host "Recorded Rust Android bridge provenance: $manifestPath"
    return
}

if (-not (Test-Path -LiteralPath $manifestPath)) {
    throw "Workshop Rust bridge provenance is missing. Run build_rust_bridge.ps1 -Release."
}
$manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
if ($manifest.schemaVersion -ne 1 -or $manifest.crate -ne "stasis_android_bridge") {
    throw "Workshop Rust bridge provenance is invalid."
}
if ($manifest.profile -ne $Profile) {
    throw "Workshop Rust bridge must use the $Profile profile. Use -Pstasis.allowDebugRustBridge=true only with a -Debug bridge build."
}
$entries = @($manifest.entries)
if ($entries.Count -ne $requiredTargets.Count -or
        (@($entries.abi | Sort-Object -Unique).Count -ne $requiredTargets.Count)) {
    throw "Workshop Rust bridge provenance must contain exactly one entry for each required ABI."
}
$inputFingerprint = Get-BridgeInputFingerprint $repoRoot
if ($manifest.inputFingerprint -ne $inputFingerprint) {
    throw "Workshop Rust bridge is stale for the current Rust/Cargo inputs (manifest=$($manifest.inputFingerprint), current=$inputFingerprint). Rebuild the bridge before assembling the APK."
}
foreach ($pair in $requiredTargets.GetEnumerator()) {
    $abi = $pair.Key
    $entry = @($entries | Where-Object abi -eq $abi)
    $expectedFile = "$abi/libstasis_android_bridge.so"
    if ($entry.Count -ne 1 -or $entry[0].rustTarget -ne $pair.Value -or
            $entry[0].file -ne $expectedFile -or $entry[0].profile -ne $Profile) {
        throw "Workshop Rust bridge provenance is invalid for ABI $abi."
    }
    $bridge = Join-Path (Join-Path $jniRoot $abi) "libstasis_android_bridge.so"
    if (-not (Test-Path -LiteralPath $bridge)) {
        throw "Workshop Rust bridge is missing ABI $abi."
    }
    $item = Get-Item -LiteralPath $bridge
    $hash = (Get-FileHash -LiteralPath $bridge -Algorithm SHA256).Hash.ToLowerInvariant()
    if ([long]$entry[0].bytes -ne $item.Length -or $entry[0].sha256 -ne $hash) {
        throw "Workshop Rust bridge provenance mismatch for $abi. Rebuild the bridge before assembling the APK."
    }
}
Write-Host "Verified $Profile Rust Android bridge provenance for both Workshop ABIs."
