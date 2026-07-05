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

    $task = if ($Install) { ":app:installDebug" } else { ":app:assembleDebug" }
    $args = @($task)
    if ($CompileSdk) { $args += "-Pstasis.compileSdk=$CompileSdk" }
    if ($TargetSdk) { $args += "-Pstasis.targetSdk=$TargetSdk" }

    & $gradleCmd @args
}
finally {
    Pop-Location
}