param(
    [string]$Game = "pong",
    [switch]$Install,
    [switch]$ValidateAot,
    [string]$PackageName = "",
    [string]$Serial = "",
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

    $task = ":app:assemblePublishedRelease"
    $args = @($task, "-Pstasis.publishedGame=$Game")
    if ($CompileSdk) { $args += "-Pstasis.compileSdk=$CompileSdk" }
    if ($TargetSdk) { $args += "-Pstasis.targetSdk=$TargetSdk" }

    & $gradleCmd @args
    if ($LASTEXITCODE -ne 0) { throw "Published Android Gradle build failed with exit code $LASTEXITCODE" }

    $unsignedApk = Join-Path $scriptRoot "app\build\outputs\apk\published\release\app-published-release-unsigned.apk"
    $apk = $unsignedApk
    if ($Install) {
        $sdkRoot = @($env:ANDROID_HOME, $env:ANDROID_SDK_ROOT, "C:\Android\Sdk", (Join-Path $env:LOCALAPPDATA "Android\Sdk")) |
            Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
        if (-not $sdkRoot) { throw "Android SDK was not found; set ANDROID_HOME or ANDROID_SDK_ROOT." }
        $buildTools = Get-ChildItem (Join-Path $sdkRoot "build-tools") -Directory |
            Sort-Object Name -Descending | Select-Object -First 1
        if (-not $buildTools) { throw "Android SDK build-tools were not found under $sdkRoot" }
        $zipalign = Join-Path $buildTools.FullName "zipalign.exe"
        $apksigner = Join-Path $buildTools.FullName "apksigner.bat"
        if (-not (Test-Path $zipalign) -or -not (Test-Path $apksigner)) { throw "Android signing tools were not found in $($buildTools.FullName)" }
        $alignedApk = Join-Path $scriptRoot "app\build\outputs\apk\published\release\app-published-release-aligned.apk"
        $apk = Join-Path $scriptRoot "app\build\outputs\apk\published\release\app-published-release.apk"
        & $zipalign -f 4 $unsignedApk $alignedApk
        if ($LASTEXITCODE -ne 0) { throw "zipalign failed with exit code $LASTEXITCODE" }
        $debugKeystore = Join-Path $env:USERPROFILE ".android\debug.keystore"
        if (-not (Test-Path $debugKeystore)) { throw "Debug keystore was not found: $debugKeystore" }
        & $apksigner sign --ks $debugKeystore --ks-key-alias androiddebugkey --ks-pass pass:android --key-pass pass:android --out $apk $alignedApk
        if ($LASTEXITCODE -ne 0) { throw "apksigner failed with exit code $LASTEXITCODE" }
        & $apksigner verify --verbose $apk
        if ($LASTEXITCODE -ne 0) { throw "APK signature verification failed with exit code $LASTEXITCODE" }
    }
    & python (Join-Path $scriptRoot "..\..\tools\ci\check_android_published_apk.py") $apk
    if ($LASTEXITCODE -ne 0) { throw "Published APK validation failed with exit code $LASTEXITCODE" }
    if ($Install) {
        if (-not $PackageName) { throw "-PackageName is required when -Install is used." }
        $adb = Join-Path $scriptRoot "adb.ps1"
        $adbArgs = @()
        if ($Serial) { $adbArgs += "-s"; $adbArgs += $Serial }
        & $adb @adbArgs install -r $apk
        if ($LASTEXITCODE -ne 0) { throw "adb install failed with exit code $LASTEXITCODE" }
        & $adb @adbArgs shell am force-stop $PackageName
        & $adb @adbArgs shell am start -W -n "$PackageName/com.stasislang.workshop.MainActivity"
        if ($LASTEXITCODE -ne 0) { throw "adb launch failed with exit code $LASTEXITCODE" }
    }
}
finally {
    Pop-Location
}
