param(
    [switch]$Install,
    [switch]$RenderAcceptance,
    [switch]$SkipCodexNative,
    [switch]$SkipRustBridgeBuild,
    [switch]$NoGradleDaemon,
    [string]$GradlePath = "",
    [string]$CompileSdk = "",
    [string]$TargetSdk = ""
)

$ErrorActionPreference = "Stop"

$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
Push-Location $scriptRoot
try {
    $gradle = Join-Path $scriptRoot "gradlew.bat"
    if ($GradlePath) {
        if (-not (Test-Path $GradlePath)) { throw "Gradle was not found: $GradlePath" }
        $gradleCmd = $GradlePath
    } elseif (Test-Path $gradle) {
        $gradleCmd = $gradle
    } elseif (Get-Command gradle -ErrorAction SilentlyContinue) {
        $gradleCmd = "gradle"
    } else {
        throw "Gradle was not found. Install Gradle or open mobile/android in Android Studio."
    }

    if ($SkipRustBridgeBuild) {
        & (Join-Path $scriptRoot "rust_bridge_provenance.ps1") -Mode Verify -Profile release
    } else {
        & (Join-Path $scriptRoot "build_rust_bridge.ps1") -Release
    }
    if (-not $SkipCodexNative) {
        & (Join-Path $scriptRoot "build_codex_native.ps1") -Release
    }

    $task = if ($Install) { ":app:installWorkshopDebug" } else { ":app:assembleWorkshopDebug" }
    $args = @($task)
    if ($NoGradleDaemon) { $args += "--no-daemon" }
    if ($RenderAcceptance) { $args += "-Pstasis.renderAcceptance=true" }
    if ($CompileSdk) { $args += "-Pstasis.compileSdk=$CompileSdk" }
    if ($TargetSdk) { $args += "-Pstasis.targetSdk=$TargetSdk" }

    & $gradleCmd @args
    if ($LASTEXITCODE -ne 0) { throw "Workshop Android Gradle build failed with exit code $LASTEXITCODE" }
}
finally {
    Pop-Location
}
