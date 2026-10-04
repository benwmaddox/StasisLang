param(
    [string]$ArchiveConsumerRoot = "",
    [string]$ArtifactRoot = ""
)

$ErrorActionPreference = "Stop"
$repoRoot = if ($env:GITHUB_WORKSPACE) {
    [System.IO.Path]::GetFullPath($env:GITHUB_WORKSPACE)
} else {
    [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot "../.."))
}
if (-not $ArchiveConsumerRoot) {
    $ArchiveConsumerRoot = Join-Path $repoRoot "target/staged-android-acceptance/consumers"
}
if (-not $ArtifactRoot) {
    $ArtifactRoot = Join-Path $repoRoot "target/android-runtime"
}
$ArchiveConsumerRoot = [System.IO.Path]::GetFullPath($ArchiveConsumerRoot)
$ArtifactRoot = [System.IO.Path]::GetFullPath($ArtifactRoot)
$emulatorScript = Join-Path $repoRoot "mobile/android/test_release_shell_emulator.ps1"

if (-not $env:STASIS_CLI_EXECUTABLE -or
        -not (Test-Path -LiteralPath $env:STASIS_CLI_EXECUTABLE -PathType Leaf)) {
    throw "Staged Android acceptance requires the CLI from the extracted release archive"
}
if (-not $env:STASIS_RUNTIME_LIBRARY_PATH -or
        -not (Test-Path -LiteralPath $env:STASIS_RUNTIME_LIBRARY_PATH -PathType Leaf)) {
    throw "Staged Android acceptance requires the matching archive graphics runtime"
}

foreach ($consumer in @("bundled-generics", "generated-generics")) {
    $label = if ($consumer -eq "bundled-generics") { "bundled" } else { "generated" }
    $project = Join-Path $ArchiveConsumerRoot $consumer
    $expectations = Join-Path $project "android_seam_expectations.json"
    $artifacts = Join-Path (Join-Path $ArtifactRoot $label) "test"
    if (-not (Test-Path -LiteralPath $project -PathType Container)) {
        throw "Staged Android $label archive consumer is missing: $project"
    }
    if (-not (Test-Path -LiteralPath $expectations -PathType Leaf)) {
        throw "Staged Android $label archive sample omitted its seam expectations: $expectations"
    }
    & $emulatorScript `
        -TestId ANDROID-GENERICS `
        -ProjectPath $project `
        -GenericsExpectationsPath $expectations `
        -ArtifactRoot $artifacts
    if ($LASTEXITCODE -ne 0) {
        throw "Android $label archive consumer failed with exit code $LASTEXITCODE"
    }
}
