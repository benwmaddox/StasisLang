param(
    [string]$Serial = $env:ANDROID_SERIAL,
    [string]$OutputPath = "",
    [string]$ProjectPath = "samples/android_aot_seam",
    [string]$ExpectationsPath = "",
    [ValidateSet("android-arm64", "android-x86_64")]
    [string]$Target = "android-arm64",
    [int]$TotalTimeoutSeconds = 900
)

$ErrorActionPreference = "Stop"
$startedAt = [System.Diagnostics.Stopwatch]::StartNew()
$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Split-Path -Parent (Split-Path -Parent $scriptRoot)
$isWindows = [System.IO.Path]::DirectorySeparatorChar -eq [char]'\'
$executableSuffix = if ($isWindows) { ".exe" } else { "" }
$androidHome = if ($env:ANDROID_HOME) {
    $env:ANDROID_HOME
} elseif ($env:ANDROID_SDK_ROOT) {
    $env:ANDROID_SDK_ROOT
} elseif (-not $isWindows) {
    Join-Path ([Environment]::GetFolderPath("UserProfile")) "Library/Android/sdk"
} else {
    "C:\Android\Sdk"
}
$adb = Join-Path (Join-Path $androidHome "platform-tools") "adb$executableSuffix"
if (-not (Test-Path $adb)) { throw "adb was not found: $adb" }
if (-not $Serial) { throw "Pass -Serial or set ANDROID_SERIAL to one Android target" }
$requiredAbi = if ($Target -eq "android-x86_64") { "x86_64" } else { "arm64-v8a" }
$projectRoot = if ([System.IO.Path]::IsPathRooted($ProjectPath)) {
    [System.IO.Path]::GetFullPath($ProjectPath)
} else {
    [System.IO.Path]::GetFullPath((Join-Path $repoRoot $ProjectPath))
}
if (-not (Test-Path (Join-Path $projectRoot "stasis.json"))) {
    throw "Android seam project is missing stasis.json: $projectRoot"
}
if (-not $ExpectationsPath) {
    $ExpectationsPath = Join-Path $projectRoot "android_seam_expectations.json"
} elseif (-not [System.IO.Path]::IsPathRooted($ExpectationsPath)) {
    $ExpectationsPath = Join-Path $repoRoot $ExpectationsPath
}
$ExpectationsPath = [System.IO.Path]::GetFullPath($ExpectationsPath)
if (-not (Test-Path $ExpectationsPath)) {
    throw "Android seam expectations are missing: $ExpectationsPath"
}
$testId = (Get-Content -Raw $ExpectationsPath | ConvertFrom-Json).test_id
if (-not $testId) { throw "Android seam expectations do not name test_id" }

$stamp = (Get-Date).ToUniversalTime().ToString("yyyyMMddTHHmmssZ")
if (-not $OutputPath) {
    $OutputPath = Join-Path $repoRoot "target\$($testId.ToLowerInvariant().Replace('-', ''))\$stamp"
}
$artifactRoot = [System.IO.Path]::GetFullPath($OutputPath)
$workspaceRoot = Join-Path $artifactRoot "w"
$packageRoot = Join-Path $workspaceRoot "d"
$evidenceRoot = Join-Path $artifactRoot "e"
New-Item -ItemType Directory -Force -Path $artifactRoot | Out-Null
Copy-Item -LiteralPath $projectRoot -Destination $workspaceRoot -Recurse
$vendorRoot = [System.IO.Path]::Combine($workspaceRoot, "vendor", "stasis", "src")
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
if ($LASTEXITCODE -ne 0 -or $requiredAbi -notin $abiList) {
    throw "$testId requires $requiredAbi support; $Serial reports '$abi' ($($abiList -join ','))"
}
if (-not $env:STASIS_SDL3_SOURCE -or -not $env:STASIS_SDL3_IMAGE_SOURCE) {
    throw "Set STASIS_SDL3_SOURCE and STASIS_SDL3_IMAGE_SOURCE to the pinned source trees"
}

Push-Location $repoRoot
try {
    python tools/cargo_cache.py run -- cargo build -p stasis
    if ($LASTEXITCODE -ne 0) { throw "$testId compiler build failed with exit code $LASTEXITCODE" }
    $commonGit = (& git rev-parse --path-format=absolute --git-common-dir).Trim()
    if ($LASTEXITCODE -ne 0) { throw "Unable to resolve the shared Cargo target" }
    $compiler = [System.IO.Path]::Combine(
        (Split-Path -Parent $commonGit),
        "build",
        "codex-cargo-target",
        "debug",
        "stasis$executableSuffix"
    )
    if (-not (Test-Path $compiler)) { throw "Built Stasis compiler is missing: $compiler" }
    & $compiler --workspace $workspaceRoot package-mobile `
        --target $Target --out d --development-build
    if ($LASTEXITCODE -ne 0) { throw "$testId package-mobile failed with exit code $LASTEXITCODE" }
    Assert-In-Time "package-mobile"

    $gradle = Resolve-Gradle
    & $gradle -p (Join-Path $packageRoot "android") `
        :app:assembleDebug -PstasisSeamTests=true --no-daemon --max-workers=2 --console=plain
    if ($LASTEXITCODE -ne 0) { throw "$testId Gradle build failed with exit code $LASTEXITCODE" }
    Assert-In-Time "Gradle build"

    $apk = [System.IO.Path]::Combine(
        $packageRoot,
        "android",
        "app",
        "build",
        "outputs",
        "apk",
        "debug",
        "app-debug.apk"
    )
    if (-not (Test-Path $apk)) { throw "Generated Android APK is missing: $apk" }
    python tools/ci/run_android_release_shell_seam.py `
        --adb $adb `
        --serial $Serial `
        --apk $apk `
        --package-manifest (Join-Path $packageRoot "stasis_mobile_package.json") `
        --expectations $ExpectationsPath `
        --output $evidenceRoot `
        --timeout-seconds ([math]::Max(15, $TotalTimeoutSeconds - [math]::Floor($startedAt.Elapsed.TotalSeconds)))
    if ($LASTEXITCODE -ne 0) { throw "$testId device acceptance failed with exit code $LASTEXITCODE" }
    Assert-In-Time "device acceptance"
} finally {
    Pop-Location
}
