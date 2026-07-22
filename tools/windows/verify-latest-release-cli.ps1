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

$projectRoot = Join-Path $extractDir "smoke_project"
$runFile = Join-Path $projectRoot "src/main.stasis"
$buildOut = Join-Path $projectRoot "build/smoke_project.exe"

$runContent = @'
function main(): i32 {
    return 7;
}
'@

Push-Location $extractDir
try {
    Invoke-CheckedCommand -Description "Creating a project with stasis new" -Action {
        & $stasisExePath new smoke_project --dir $projectRoot
    }
    Set-Content -Path $runFile -Value $runContent -Encoding ASCII
    Invoke-CheckedCommand -Description "Checking from a project subdirectory" -Action {
        & $stasisExePath --workspace (Join-Path $projectRoot "src") check --json
    }
    Invoke-CheckedCommand -Description "Running project tests" -Action {
        & $stasisExePath --workspace $projectRoot test --json
    }
    Invoke-CheckedCommand -Description "Running project main through JIT" -ExpectedExitCode 7 -Action {
        & $stasisExePath --workspace $projectRoot run
    }
    Invoke-CheckedCommand -Description "Building an offline AOT executable" -Action {
        & $stasisExePath --workspace $projectRoot build --mode release
    }
    if (-not (Test-Path $buildOut)) {
        throw "stasis build did not produce expected output: $buildOut"
    }
    Invoke-CheckedCommand -Description "Running stasis.exe probe-graphics-runtime" -Action {
        Remove-Item Env:STASIS_RUNTIME_DLL_PATH -ErrorAction SilentlyContinue
        & $stasisExePath probe-graphics-runtime
    }
}
finally {
    Pop-Location
}

Write-Host "Release CLI bundle validation succeeded for tag $tag."
Write-Host "Bundle location: $extractDir"
Write-Host "Launcher path: $stasisExePath"
