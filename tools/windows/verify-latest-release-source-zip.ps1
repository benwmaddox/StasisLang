param(
    [string] $Owner = "benwmaddox",
    [string] $Repo = "StasisLang",
    [string] $OutputRoot = "",
    [switch] $StableOnly
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function New-TemporaryRoot {
    $shortBase = Join-Path $env:SystemDrive "stasis_release_verify"
    New-Item -ItemType Directory -Path $shortBase -Force | Out-Null
    $suffix = [System.Guid]::NewGuid().ToString("N").Substring(0, 8)
    return Join-Path $shortBase ("srv-" + $suffix)
}

function Invoke-CheckedCommand {
    param(
        [string] $Description,
        [scriptblock] $Action
    )

    Write-Host $Description
    & $Action
    if ($LASTEXITCODE -ne 0) {
        throw "$Description failed with exit code $LASTEXITCODE."
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
    "User-Agent" = "$Owner-$Repo-release-verify"
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
$zipUrl = $release.zipball_url
$publishedAt = $release.published_at

if ([string]::IsNullOrWhiteSpace($tag)) {
    throw "Latest release payload did not include tag_name."
}

if ([string]::IsNullOrWhiteSpace($zipUrl)) {
    throw "Latest release payload did not include zipball_url."
}

Write-Host "Validating release source zip:"
Write-Host "  Repo: $Owner/$Repo"
Write-Host "  Tag: $tag"
Write-Host "  Published: $publishedAt"
Write-Host "  Source Zip URL: $zipUrl"
Write-Host "  Working Directory: $OutputRoot"

$downloadDir = Join-Path $OutputRoot "download"
$extractDir = Join-Path $OutputRoot "extract"
$zipPath = Join-Path $downloadDir "$tag-source.zip"

New-Item -ItemType Directory -Path $downloadDir -Force | Out-Null
New-Item -ItemType Directory -Path $extractDir -Force | Out-Null

Invoke-WebRequest -Headers $headers -Uri $zipUrl -OutFile $zipPath
Expand-Archive -Path $zipPath -DestinationPath $extractDir -Force

$sourceRoot = Get-ChildItem -Path $extractDir -Directory | Select-Object -First 1
if (-not $sourceRoot) {
    throw "Source archive extracted without a root directory."
}

$cargoToml = Join-Path $sourceRoot.FullName "Cargo.toml"
$legacyBuildBat = Join-Path $sourceRoot.FullName "build.bat"
$legacyTestBat = Join-Path $sourceRoot.FullName "test.bat"
$legacySolution = Join-Path $sourceRoot.FullName "Stasis.sln"
$validationMode = ""

Push-Location $sourceRoot.FullName
try {
    if (Test-Path $cargoToml) {
        $validationMode = "cargo"
        Invoke-CheckedCommand -Description "Running cargo test --workspace --all-targets" -Action {
            cargo test --workspace --all-targets
        }
        Invoke-CheckedCommand -Description "Running cargo build --workspace --all-targets" -Action {
            cargo build --workspace --all-targets
        }
    }
    elseif ((Test-Path $legacyBuildBat) -and (Test-Path $legacyTestBat)) {
        $validationMode = "legacy-source"
        $env:STASIS_CLEAN_RUNTIME_BUILD = "1"

        Invoke-CheckedCommand -Description "Running runtime\\build.bat" -Action {
            cmd /c runtime\build.bat
        }
        Invoke-CheckedCommand -Description "Running cargo build -p stasis-cranelift-aot --release --manifest-path tools\\cranelift-aot\\Cargo.toml" -Action {
            cargo build -p stasis-cranelift-aot --release --manifest-path tools\cranelift-aot\Cargo.toml
        }
        Invoke-CheckedCommand -Description "Running dotnet build Stasis.sln -c Release -m:1" -Action {
            dotnet build Stasis.sln -c Release -m:1
        }
        Invoke-CheckedCommand -Description "Running dotnet test Stasis.sln -c Release -- RunConfiguration.MaxCpuCount=1" -Action {
            dotnet test Stasis.sln -c Release -- RunConfiguration.MaxCpuCount=1
        }
    }
    elseif (Test-Path $legacySolution) {
        $validationMode = "legacy-dotnet-fallback"
        Invoke-CheckedCommand -Description "Running dotnet build Stasis.sln -c Release" -Action {
            dotnet build Stasis.sln -c Release
        }
        Invoke-CheckedCommand -Description "Running dotnet test Stasis.sln -c Release -- RunConfiguration.MaxCpuCount=1" -Action {
            dotnet test Stasis.sln -c Release -- RunConfiguration.MaxCpuCount=1
        }
    }
    else {
        throw "Unable to determine build/test entrypoints in extracted source root: $($sourceRoot.FullName)"
    }
}
finally {
    Pop-Location
}

Write-Host "Release source zip validation succeeded for tag $tag (mode: $validationMode)."
