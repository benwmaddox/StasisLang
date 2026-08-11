param(
    [string]$Serial = $env:ANDROID_SERIAL,
    [string]$OutputPath = "",
    [int]$TotalTimeoutSeconds = 900
)

$ErrorActionPreference = "Stop"
$startedAt = [System.Diagnostics.Stopwatch]::StartNew()
$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Split-Path -Parent (Split-Path -Parent $scriptRoot)
$androidHome = if ($env:ANDROID_HOME) {
    $env:ANDROID_HOME
} elseif ($env:ANDROID_SDK_ROOT) {
    $env:ANDROID_SDK_ROOT
} else {
    "C:\Android\Sdk"
}
$adb = Join-Path $androidHome "platform-tools\adb.exe"
if (-not (Test-Path $adb)) { throw "adb.exe was not found: $adb" }
if (-not $Serial) { throw "Pass -Serial or set ANDROID_SERIAL to one arm64 Android device" }

$stamp = (Get-Date).ToUniversalTime().ToString("yyyyMMddTHHmmssZ")
if (-not $OutputPath) {
    $OutputPath = Join-Path $repoRoot "target\it017\$stamp"
}
$artifactRoot = [System.IO.Path]::GetFullPath($OutputPath)
$workspaceRoot = Join-Path $artifactRoot "w"
$packageRoot = Join-Path $workspaceRoot "d"
$evidenceRoot = Join-Path $artifactRoot "e"
New-Item -ItemType Directory -Force -Path $artifactRoot | Out-Null
Copy-Item -LiteralPath (Join-Path $repoRoot "samples\android_aot_seam") `
    -Destination $workspaceRoot -Recurse
$vendorRoot = Join-Path $workspaceRoot "vendor\stasis\src"
New-Item -ItemType Directory -Force -Path $vendorRoot | Out-Null
Copy-Item -LiteralPath (Join-Path $repoRoot "src\stdlib") `
    -Destination (Join-Path $vendorRoot "stdlib") -Recurse

function Assert-In-Time([string]$Step) {
    if ($startedAt.Elapsed.TotalSeconds -gt $TotalTimeoutSeconds) {
        throw "Android release-shell seam exceeded ${TotalTimeoutSeconds}s after $Step"
    }
}

function Resolve-Gradle {
    $command = Get-Command gradle -ErrorAction SilentlyContinue
    if ($command) { return $command.Source }
    if ($env:ChocolateyInstall) {
        $installed = Get-ChildItem (Join-Path $env:ChocolateyInstall "lib\gradle\tools") `
            -Recurse -Filter gradle.bat -ErrorAction SilentlyContinue |
            Sort-Object FullName -Descending | Select-Object -First 1
        if ($installed) { return $installed.FullName }
    }
    throw "Gradle was not found; install Gradle 8.9 or newer"
}

$abiOutput = @(& $adb -s $Serial shell getprop ro.product.cpu.abi)
if ($LASTEXITCODE -ne 0 -or $abiOutput.Count -eq 0) {
    throw "Unable to inspect Android device $Serial"
}
$abi = ($abiOutput -join "").Trim()
$abiList = (& $adb -s $Serial shell getprop ro.product.cpu.abilist).Trim() -split ','
if ($LASTEXITCODE -ne 0 -or "arm64-v8a" -notin $abiList) {
    throw "IT-017 requires arm64-v8a support; $Serial reports '$abi' ($($abiList -join ','))"
}
if (-not $env:STASIS_SDL3_SOURCE -or -not $env:STASIS_SDL3_IMAGE_SOURCE) {
    throw "Set STASIS_SDL3_SOURCE and STASIS_SDL3_IMAGE_SOURCE to the pinned source trees"
}

Push-Location $repoRoot
try {
    python tools/cargo_cache.py run -- cargo build -p stasis
    if ($LASTEXITCODE -ne 0) { throw "IT-017 compiler build failed with exit code $LASTEXITCODE" }
    $commonGit = (& git rev-parse --path-format=absolute --git-common-dir).Trim()
    if ($LASTEXITCODE -ne 0) { throw "Unable to resolve the shared Cargo target" }
    $compiler = Join-Path (Split-Path -Parent $commonGit) "build\codex-cargo-target\debug\stasis.exe"
    if (-not (Test-Path $compiler)) { throw "Built Stasis compiler is missing: $compiler" }
    & $compiler --workspace $workspaceRoot package-mobile `
        --target android-arm64 --out d --development-build
    if ($LASTEXITCODE -ne 0) { throw "IT-017 package-mobile failed with exit code $LASTEXITCODE" }
    Assert-In-Time "package-mobile"

    $gradle = Resolve-Gradle
    & $gradle -p (Join-Path $packageRoot "android") `
        :app:assembleDebug -PstasisSeamTests=true --no-daemon --max-workers=2 --console=plain
    if ($LASTEXITCODE -ne 0) { throw "IT-017 Gradle build failed with exit code $LASTEXITCODE" }
    Assert-In-Time "Gradle build"

    $apk = Join-Path $packageRoot "android\app\build\outputs\apk\debug\app-debug.apk"
    if (-not (Test-Path $apk)) { throw "Generated Android APK is missing: $apk" }
    python tools/ci/run_android_release_shell_seam.py `
        --adb $adb `
        --serial $Serial `
        --apk $apk `
        --package-manifest (Join-Path $packageRoot "stasis_mobile_package.json") `
        --expectations samples/android_aot_seam/android_seam_expectations.json `
        --output $evidenceRoot `
        --timeout-seconds ([math]::Max(15, $TotalTimeoutSeconds - [math]::Floor($startedAt.Elapsed.TotalSeconds)))
    if ($LASTEXITCODE -ne 0) { throw "IT-017 device acceptance failed with exit code $LASTEXITCODE" }
    Assert-In-Time "device acceptance"
} finally {
    Pop-Location
}
