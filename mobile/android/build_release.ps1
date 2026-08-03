param(
    [switch]$Install,
    [string]$ProjectDir = "",
    [string]$OutputDir = "build/android-release",
    [string]$RequiredAsset = "assets/ball.svg",
    [string]$StasisPath = "",
    [switch]$DevelopmentBuild,
    [string]$GradlePath = "",
    [string]$Sdl2Source = "",
    [string]$Sdl2ImageSource = ""
)

$ErrorActionPreference = "Stop"

$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Split-Path -Parent (Split-Path -Parent $scriptRoot)
if (-not $ProjectDir) {
    $ProjectDir = Join-Path $scriptRoot "app/src/main/assets/workshop_sample"
}
$project = [IO.Path]::GetFullPath($ProjectDir)
if (-not (Test-Path -LiteralPath (Join-Path $project "stasis.json") -PathType Leaf)) {
    throw "Release project does not contain stasis.json: $project"
}

$relativeOutput = $OutputDir.Replace('\', '/')
if ([IO.Path]::IsPathRooted($OutputDir) -or $relativeOutput -notmatch '^build/[^/].*') {
    throw "Release output must be a project-relative child of build/: $OutputDir"
}
$output = [IO.Path]::GetFullPath((Join-Path $project $OutputDir))
$allowedOutputRoot = [IO.Path]::GetFullPath((Join-Path $project "build")) + [IO.Path]::DirectorySeparatorChar
if (-not $output.StartsWith($allowedOutputRoot, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Release output escaped the project build directory: $output"
}
if (Test-Path -LiteralPath $output) {
    $deletePath = if ($output.StartsWith('\\?\')) { $output } else { "\\?\$output" }
    [IO.Directory]::Delete($deletePath, $true)
}

Push-Location $repoRoot
try {
    if ($StasisPath) {
        $stasis = [IO.Path]::GetFullPath($StasisPath)
        if (-not (Test-Path -LiteralPath $stasis -PathType Leaf)) {
            throw "Stasis executable was not found: $stasis"
        }
        $packageArguments = @(
            "--workspace", $project,
            "package-mobile", "--target", "android-arm64", "--out", $relativeOutput
        )
        if ($DevelopmentBuild) { $packageArguments += "--development-build" }
        & $stasis @packageArguments
    } else {
        cargo run -p stasis -- --workspace $project package-mobile --target android-arm64 `
            --out $relativeOutput --development-build
    }
    if ($LASTEXITCODE -ne 0) {
        throw "Stasis release packaging failed with exit code $LASTEXITCODE"
    }
} finally {
    Pop-Location
}

$androidRoot = Join-Path $output "android"
$gradle = if ($GradlePath) {
    [IO.Path]::GetFullPath($GradlePath)
} elseif (Get-Command gradle -ErrorAction SilentlyContinue) {
    (Get-Command gradle).Source
} else {
    ""
}
if (-not $gradle -or -not (Test-Path -LiteralPath $gradle -PathType Leaf)) {
    throw "Gradle was not found; pass -GradlePath or install Gradle"
}

$resolvedSdl2 = if ($Sdl2Source) { $Sdl2Source } else { $env:STASIS_SDL2_SOURCE }
$resolvedSdl2Image = if ($Sdl2ImageSource) { $Sdl2ImageSource } else { $env:STASIS_SDL2_IMAGE_SOURCE }
if (-not $resolvedSdl2 -or -not (Test-Path -LiteralPath $resolvedSdl2 -PathType Container)) {
    throw "SDL2 source was not found; pass -Sdl2Source or set STASIS_SDL2_SOURCE"
}
if (-not $resolvedSdl2Image -or -not (Test-Path -LiteralPath $resolvedSdl2Image -PathType Container)) {
    throw "SDL2_image source was not found; pass -Sdl2ImageSource or set STASIS_SDL2_IMAGE_SOURCE"
}
$env:STASIS_SDL2_SOURCE = [IO.Path]::GetFullPath($resolvedSdl2)
$env:STASIS_SDL2_IMAGE_SOURCE = [IO.Path]::GetFullPath($resolvedSdl2Image)

Push-Location $androidRoot
try {
    $task = if ($Install) { ":app:installDebug" } else { ":app:bundleRelease" }
    & $gradle $task --no-daemon --max-workers=2
    if ($LASTEXITCODE -ne 0) {
        throw "Android release build failed with exit code $LASTEXITCODE"
    }
} finally {
    Pop-Location
}

$package = if ($Install) {
    Join-Path $androidRoot "app/build/outputs/apk/debug/app-debug.apk"
} else {
    Join-Path $androidRoot "app/build/outputs/bundle/release/app-release.aab"
}
python (Join-Path $repoRoot "tools/ci/check_android_release_package.py") `
    $package --abi arm64-v8a --required-asset $RequiredAsset
if ($LASTEXITCODE -ne 0) {
    throw "Android release package validation failed with exit code $LASTEXITCODE"
}
Write-Output "Android release package: $package"
