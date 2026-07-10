param(
    [switch]$Install,
    [string]$CompileSdk = "",
    [string]$TargetSdk = ""
)

$ErrorActionPreference = "Stop"

$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
Push-Location $scriptRoot
try {
    $gradle = Join-Path $scriptRoot "gradlew.bat"
    if (Test-Path $gradle) {
        $gradleCmd = $gradle
    } elseif (Get-Command gradle -ErrorAction SilentlyContinue) {
        $gradleCmd = "gradle"
    } else {
        throw "Gradle was not found. Install Gradle or open mobile/android in Android Studio."
    }

    & (Join-Path $scriptRoot "build_rust_bridge.ps1")

    $task = if ($Install) { ":app:installWorkshopDebug" } else { ":app:assembleWorkshopDebug" }
    $args = @($task)
    if ($CompileSdk) { $args += "-Pstasis.compileSdk=$CompileSdk" }
    if ($TargetSdk) { $args += "-Pstasis.targetSdk=$TargetSdk" }

    & $gradleCmd @args
    if ($LASTEXITCODE -ne 0) { throw "Workshop Android Gradle build failed with exit code $LASTEXITCODE" }
}
finally {
    Pop-Location
}
