param(
    [string] $Owner = "benwmaddox",
    [string] $Repo = "StasisLang",
    [string] $OutputRoot = "",
    [switch] $StableOnly
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function New-TemporaryRoot {
    $shortBase = Join-Path $env:SystemDrive "stasis_release_cli_verify"
    New-Item -ItemType Directory -Path $shortBase -Force | Out-Null
    $suffix = [System.Guid]::NewGuid().ToString("N").Substring(0, 8)
    return Join-Path $shortBase ("cli-" + $suffix)
}

function Invoke-CheckedCommand {
    param(
        [string] $Description,
        [scriptblock] $Action,
        [int] $ExpectedExitCode = 0
    )

    Write-Host $Description
    & $Action
    if ($LASTEXITCODE -ne $ExpectedExitCode) {
        throw "$Description failed with exit code $LASTEXITCODE (expected $ExpectedExitCode)."
    }
}

if ([string]::IsNullOrWhiteSpace($OutputRoot)) {
    $OutputRoot = New-TemporaryRoot
}

if (Test-Path $OutputRoot) {
    throw "OutputRoot already exists: $OutputRoot"
}

$headers = @{
    "Accept" = "application/vnd.github+json"
    "User-Agent" = "$Owner-$Repo-release-cli-verify"
}

if (-not [string]::IsNullOrWhiteSpace($env:GITHUB_TOKEN)) {
    $headers["Authorization"] = "Bearer $($env:GITHUB_TOKEN)"
}

$releaseApiBase = "https://api.github.com/repos/$Owner/$Repo/releases"

if ($StableOnly) {
    $release = Invoke-RestMethod -Headers $headers -Uri "${releaseApiBase}/latest" -Method Get
}
else {
    $releases = Invoke-RestMethod -Headers $headers -Uri "${releaseApiBase}?per_page=20" -Method Get
    $release = $releases |
        Where-Object { -not $_.draft -and $_.published_at } |
        Sort-Object { [DateTimeOffset]::Parse($_.published_at) } -Descending |
        Select-Object -First 1

    if (-not $release) {
        throw "No published release found for $Owner/$Repo."
    }
}

$tag = $release.tag_name
if ([string]::IsNullOrWhiteSpace($tag)) {
    throw "Latest release payload did not include tag_name."
}

# Fetch by tag to ensure assets are fully populated for selection.
$releaseWithAssets = Invoke-RestMethod -Headers $headers -Uri "${releaseApiBase}/tags/$tag" -Method Get

$windowsZipAsset = $releaseWithAssets.assets |
    Where-Object { $_.name -match "(?i)win.*\.zip$" } |
    Select-Object -First 1

if (-not $windowsZipAsset) {
    throw "Latest release tag '$tag' has no Windows zip asset."
}

$publishedAt = $releaseWithAssets.published_at
$assetName = $windowsZipAsset.name
$assetUrl = $windowsZipAsset.browser_download_url

if ([string]::IsNullOrWhiteSpace($assetUrl)) {
    throw "Windows zip asset '$assetName' did not include browser_download_url."
}

Write-Host "Validating release CLI bundle:"
Write-Host "  Repo: $Owner/$Repo"
Write-Host "  Tag: $tag"
Write-Host "  Published: $publishedAt"
Write-Host "  Asset: $assetName"
Write-Host "  Asset URL: $assetUrl"
Write-Host "  Working Directory: $OutputRoot"

$downloadDir = Join-Path $OutputRoot "download"
$extractDir = Join-Path $OutputRoot "extract"
$zipPath = Join-Path $downloadDir $assetName

New-Item -ItemType Directory -Path $downloadDir -Force | Out-Null
New-Item -ItemType Directory -Path $extractDir -Force | Out-Null

Invoke-WebRequest -Headers $headers -Uri $assetUrl -OutFile $zipPath
Expand-Archive -Path $zipPath -DestinationPath $extractDir -Force

$stasisExePath = Join-Path $extractDir "stasis.exe"
if (-not (Test-Path $stasisExePath)) {
    throw "Extracted bundle does not contain stasis.exe at $stasisExePath"
}

$graphicsDllPath = Join-Path $extractDir "stasis_graphics.dll"
if (-not (Test-Path $graphicsDllPath)) {
    throw "Extracted bundle does not contain stasis_graphics.dll at $graphicsDllPath"
}

$runFile = Join-Path $extractDir "smoke_run.stasis"
$testFile = Join-Path $extractDir "smoke_test.stasis"
$buildOut = Join-Path $extractDir "smoke_run.exe"
$craneliftAotExe = Join-Path $extractDir "bin\stasis-cranelift-aot.exe"

$runContent = @'
function main(): i32 {
    return 7;
}
'@
Set-Content -Path $runFile -Value $runContent -Encoding ASCII

$testContent = @'
test `one equals one`(): bool {
    return 1 == 1;
}
'@
Set-Content -Path $testFile -Value $testContent -Encoding ASCII

if (-not (Test-Path $craneliftAotExe)) {
    throw "Extracted bundle does not contain Cranelift AOT helper at $craneliftAotExe"
}

$env:STASIS_CRANELIFT_AOT = $craneliftAotExe

Push-Location $extractDir
try {
    Invoke-CheckedCommand -Description "Running stasis run smoke_run.stasis" -ExpectedExitCode 7 -Action {
        & $stasisExePath run $runFile --backend cranelift --no-cranelift-runner
    }
    Invoke-CheckedCommand -Description "Running stasis build smoke_run.stasis" -Action {
        & $stasisExePath build $runFile --backend cranelift --out $buildOut
    }
    if (-not (Test-Path $buildOut)) {
        throw "stasis build did not produce expected output: $buildOut"
    }
    Invoke-CheckedCommand -Description "Running stasis.exe probe-graphics-runtime" -Action {
        Remove-Item Env:STASIS_RUNTIME_DLL_PATH -ErrorAction SilentlyContinue
        & $stasisExePath probe-graphics-runtime
    }
    Invoke-CheckedCommand -Description "Running stasis test smoke_test.stasis" -Action {
        & $stasisExePath test $testFile --backend cranelift --no-cranelift-runner
    }
}
finally {
    Pop-Location
}

Write-Host "Release CLI bundle validation succeeded for tag $tag."
Write-Host "Bundle location: $extractDir"
Write-Host "Launcher path: $stasisExePath"
