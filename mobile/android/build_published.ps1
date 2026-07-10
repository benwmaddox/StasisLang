param(
    [switch]$Install,
    [switch]$ValidateAot,
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

    if ($ValidateAot) {
        Push-Location (Join-Path $scriptRoot "..\..")
        try {
            cargo test -p stasis_compiler backend::aot::tests::aot_engine_bundle_writes_manifest_and_required_entrypoints
            if ($LASTEXITCODE -ne 0) { throw "Published AOT validation failed with exit code $LASTEXITCODE" }
        }
        finally {
            Pop-Location
        }
    }

    $task = if ($Install) { ":app:installPublishedDebug" } else { ":app:assemblePublishedRelease" }
    $args = @($task)
    if ($CompileSdk) { $args += "-Pstasis.compileSdk=$CompileSdk" }
    if ($TargetSdk) { $args += "-Pstasis.targetSdk=$TargetSdk" }

    & $gradleCmd @args
    if ($LASTEXITCODE -ne 0) { throw "Published Android Gradle build failed with exit code $LASTEXITCODE" }

    $apk = if ($Install) {
        Join-Path $scriptRoot "app\build\outputs\apk\published\debug\app-published-debug.apk"
    } else {
        Join-Path $scriptRoot "app\build\outputs\apk\published\release\app-published-release-unsigned.apk"
    }
    & python (Join-Path $scriptRoot "..\..\tools\ci\check_android_published_apk.py") $apk
    if ($LASTEXITCODE -ne 0) { throw "Published APK validation failed with exit code $LASTEXITCODE" }
}
finally {
    Pop-Location
}
