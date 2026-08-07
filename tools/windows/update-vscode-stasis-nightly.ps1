[CmdletBinding()]
param(
    [string] $Owner = "benwmaddox",
    [string] $Repo = "StasisLang",
    [string] $StateRoot = "",
    [string] $DownloadRoot = "",
    [string] $CodePath = "",
    [string] $ExtensionRoot = "",
    [switch] $CheckOnly
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Get-LocalAppDataRoot {
    $root = [Environment]::GetFolderPath("LocalApplicationData")
    if ([string]::IsNullOrWhiteSpace($root)) {
        $root = [Environment]::GetEnvironmentVariable("TEMP")
    }
    if ([string]::IsNullOrWhiteSpace($root)) {
        throw "Unable to determine a writable local application-data directory."
    }
    return $root
}

function Convert-ToAbsolutePath {
    param([string] $Path)

    return [IO.Path]::GetFullPath($Path)
}

function Write-UpdateLog {
    param([string] $Message)

    $timestamp = [DateTimeOffset]::Now.ToString("o")
    $line = "$timestamp $Message"
    Add-Content -LiteralPath $script:LogPath -Value $line -Encoding UTF8
    Write-Host $line
}

function Get-GitHubHeaders {
    $headers = @{
        "Accept" = "application/vnd.github+json"
        "User-Agent" = "$Owner-$Repo-vscode-nightly-updater"
    }
    if (-not [string]::IsNullOrWhiteSpace($env:GITHUB_TOKEN)) {
        $headers["Authorization"] = "Bearer $($env:GITHUB_TOKEN)"
    }
    return $headers
}

function Get-LatestNightlyRelease {
    $headers = Get-GitHubHeaders
    $releaseApiBase = "https://api.github.com/repos/$Owner/$Repo/releases"
    $releases = @(Invoke-RestMethod -Headers $headers -Uri "${releaseApiBase}?per_page=100" -Method Get)
    $release = $releases |
        Where-Object {
            -not $_.draft -and
            $_.published_at -and
            $_.tag_name -match "^nightly-[A-Za-z0-9._-]+$"
        } |
        Sort-Object { [DateTimeOffset]::Parse($_.published_at) } -Descending |
        Select-Object -First 1

    if (-not $release) {
        return $null
    }

    $tag = [string] $release.tag_name
    $releaseWithAssets = Invoke-RestMethod -Headers $headers -Uri "${releaseApiBase}/tags/$([uri]::EscapeDataString($tag))" -Method Get
    $asset = $releaseWithAssets.assets |
        Where-Object { $_.name -eq "stasis-editor-release-win32-x64.zip" } |
        Select-Object -First 1

    if (-not $asset) {
        throw "Nightly release '$tag' does not contain stasis-editor-release-win32-x64.zip."
    }
    if ([string]::IsNullOrWhiteSpace([string] $asset.browser_download_url)) {
        throw "Nightly asset '$($asset.name)' does not contain a browser download URL."
    }
    if ([string] $asset.digest -notmatch "^sha256:[0-9a-fA-F]{64}$") {
        throw "Nightly asset '$($asset.name)' does not contain a SHA-256 digest."
    }

    return [pscustomobject] @{
        Tag = $tag
        PublishedAt = [string] $releaseWithAssets.published_at
        AssetName = [string] $asset.name
        AssetUrl = [string] $asset.browser_download_url
        AssetSha256 = ([string] $asset.digest).Substring(7).ToLowerInvariant()
    }
}

function Get-InstalledExtensionInfo {
    if (-not (Test-Path -LiteralPath $ExtensionRoot -PathType Container)) {
        return $null
    }

    $extensionDirectories = Get-ChildItem -LiteralPath $ExtensionRoot -Directory -Filter "stasislang.stasis-*" |
        Sort-Object LastWriteTime -Descending
    foreach ($extensionDirectory in $extensionDirectories) {
        $manifestPath = Join-Path $extensionDirectory.FullName "dist\toolchain-manifest.json"
        if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
            continue
        }

        try {
            $manifest = Get-Content -LiteralPath $manifestPath -Raw -Encoding UTF8 | ConvertFrom-Json
            $releaseId = [string] $manifest.identity.release_id
            if ([string]::IsNullOrWhiteSpace($releaseId)) {
                continue
            }
            return [pscustomobject] @{
                Directory = $extensionDirectory.FullName
                ReleaseId = $releaseId
                Version = [string] $manifest.identity.version
            }
        }
        catch {
            Write-UpdateLog "Ignoring unreadable installed extension manifest '$manifestPath': $($_.Exception.Message)"
        }
    }

    return $null
}

function Resolve-CodePath {
    if (-not [string]::IsNullOrWhiteSpace($CodePath)) {
        if (Test-Path -LiteralPath $CodePath -PathType Leaf) {
            return (Get-Item -LiteralPath $CodePath).FullName
        }
        throw "The configured VS Code CLI does not exist: $CodePath"
    }

    foreach ($commandName in @("code.cmd", "code")) {
        $command = Get-Command $commandName -CommandType Application -ErrorAction SilentlyContinue |
            Select-Object -First 1
        if ($command) {
            return $command.Source
        }
    }

    $localAppData = Get-LocalAppDataRoot
    $programFiles = [Environment]::GetEnvironmentVariable("ProgramFiles")
    $programFilesX86 = [Environment]::GetEnvironmentVariable("ProgramFiles(x86)")
    $candidateRoots = @(
        (Join-Path $localAppData "Programs\Microsoft VS Code\bin"),
        (Join-Path $programFiles "Microsoft VS Code\bin"),
        (Join-Path $programFilesX86 "Microsoft VS Code\bin")
    )
    foreach ($candidateRoot in $candidateRoots) {
        foreach ($candidateName in @("code.cmd", "code.exe")) {
            $candidate = Join-Path $candidateRoot $candidateName
            if (Test-Path -LiteralPath $candidate -PathType Leaf) {
                return (Get-Item -LiteralPath $candidate).FullName
            }
        }
    }

    throw "VS Code CLI was not found. Put code.cmd on PATH or pass -CodePath."
}

function Get-FileSha256 {
    param([string] $Path)

    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Assert-PathInside {
    param(
        [string] $Path,
        [string] $Root,
        [string] $Description
    )

    $absolutePath = Convert-ToAbsolutePath $Path
    $absoluteRoot = (Convert-ToAbsolutePath $Root).TrimEnd([IO.Path]::DirectorySeparatorChar, [IO.Path]::AltDirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
    if (-not $absolutePath.StartsWith($absoluteRoot, [StringComparison]::OrdinalIgnoreCase)) {
        throw "$Description is outside its expected root: $absolutePath"
    }
    return $absolutePath
}

function Write-InstalledState {
    param(
        [pscustomobject] $Release,
        [string] $VsixSha256
    )

    $statePath = Join-Path $StateRoot "state.json"
    $temporaryPath = "$statePath.$PID.tmp"
    $state = [ordered] @{
        schema = 1
        owner = $Owner
        repo = $Repo
        release_tag = $Release.Tag
        published_at = $Release.PublishedAt
        asset_name = $Release.AssetName
        asset_sha256 = $Release.AssetSha256
        vscode_extension_sha256 = $VsixSha256
        installed_at_utc = [DateTimeOffset]::UtcNow.ToString("o")
    }
    $state | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $temporaryPath -Encoding UTF8
    Move-Item -LiteralPath $temporaryPath -Destination $statePath -Force
}

$localAppDataRoot = Get-LocalAppDataRoot
if ([string]::IsNullOrWhiteSpace($StateRoot)) {
    $StateRoot = Join-Path $localAppDataRoot "StasisLang\vscode-nightly"
}
if ([string]::IsNullOrWhiteSpace($DownloadRoot)) {
    $DownloadRoot = Join-Path $StateRoot "downloads"
}
if ([string]::IsNullOrWhiteSpace($ExtensionRoot)) {
    $userProfile = [Environment]::GetFolderPath("UserProfile")
    if ([string]::IsNullOrWhiteSpace($userProfile)) {
        throw "Unable to determine the current user's profile directory."
    }
    $ExtensionRoot = Join-Path $userProfile ".vscode\extensions"
}

$StateRoot = Convert-ToAbsolutePath $StateRoot
$DownloadRoot = Convert-ToAbsolutePath $DownloadRoot
$ExtensionRoot = Convert-ToAbsolutePath $ExtensionRoot
$script:LogPath = Join-Path $StateRoot "update.log"
New-Item -ItemType Directory -Path $StateRoot -Force | Out-Null

try {
    Write-UpdateLog "Checking published nightly releases for $Owner/$Repo."
    $release = Get-LatestNightlyRelease
    if (-not $release) {
        Write-UpdateLog "No published nightly release exists; exiting."
        return
    }

    $installed = Get-InstalledExtensionInfo
    if ($installed) {
        Write-UpdateLog "Installed Stasis extension release: $($installed.ReleaseId) ($($installed.Directory))."
    }
    else {
        Write-UpdateLog "No installed Stasis extension with a toolchain manifest was found."
    }

    if ($installed -and $installed.ReleaseId -eq $release.Tag) {
        Write-UpdateLog "No new nightly release; installed release '$($release.Tag)' is current."
        return
    }

    Write-UpdateLog "New nightly release available: $($release.Tag), published $($release.PublishedAt)."
    if ($CheckOnly) {
        Write-UpdateLog "Check-only mode; no download or installation performed."
        return
    }

    New-Item -ItemType Directory -Path $DownloadRoot -Force | Out-Null
    $archivePath = Assert-PathInside (Join-Path $DownloadRoot $release.AssetName) $DownloadRoot "Nightly archive path"
    $partialArchivePath = "$archivePath.partial"
    $archiveIsValid = $false
    if (Test-Path -LiteralPath $archivePath -PathType Leaf) {
        $archiveIsValid = (Get-FileSha256 $archivePath) -eq $release.AssetSha256
        if ($archiveIsValid) {
            Write-UpdateLog "Using cached nightly archive '$archivePath'."
        }
        else {
            Write-UpdateLog "Cached nightly archive failed its SHA-256 check; downloading a fresh copy."
            Remove-Item -LiteralPath $archivePath -Force
        }
    }

    if (-not $archiveIsValid) {
        if (Test-Path -LiteralPath $partialArchivePath) {
            Remove-Item -LiteralPath $partialArchivePath -Force
        }
        Write-UpdateLog "Downloading $($release.AssetUrl)."
        $headers = Get-GitHubHeaders
        Invoke-WebRequest -Headers $headers -Uri $release.AssetUrl -OutFile $partialArchivePath -UseBasicParsing
        $downloadedSha256 = Get-FileSha256 $partialArchivePath
        if ($downloadedSha256 -ne $release.AssetSha256) {
            Remove-Item -LiteralPath $partialArchivePath -Force
            throw "Downloaded nightly archive SHA-256 mismatch. Expected $($release.AssetSha256), got $downloadedSha256."
        }
        Move-Item -LiteralPath $partialArchivePath -Destination $archivePath -Force
        Write-UpdateLog "Nightly archive SHA-256 verified: $downloadedSha256."
    }

    $extractRoot = Assert-PathInside (Join-Path $DownloadRoot (Join-Path "extracted" $release.Tag)) $DownloadRoot "Nightly extraction path"
    if (Test-Path -LiteralPath $extractRoot) {
        Remove-Item -LiteralPath $extractRoot -Recurse -Force
    }
    New-Item -ItemType Directory -Path $extractRoot -Force | Out-Null
    Expand-Archive -LiteralPath $archivePath -DestinationPath $extractRoot -Force

    $releaseManifestPath = Join-Path $extractRoot "stasis-editor-release.json"
    if (-not (Test-Path -LiteralPath $releaseManifestPath -PathType Leaf)) {
        throw "Nightly editor archive is missing stasis-editor-release.json."
    }
    $releaseManifest = Get-Content -LiteralPath $releaseManifestPath -Raw -Encoding UTF8 | ConvertFrom-Json
    if ([int] $releaseManifest.schema -ne 1) {
        throw "Unsupported Stasis editor release manifest schema: $($releaseManifest.schema)"
    }
    if ([string] $releaseManifest.release_id -ne $release.Tag) {
        throw "Editor archive release identity '$($releaseManifest.release_id)' does not match tag '$($release.Tag)'."
    }
    if ([string] $releaseManifest.platform -ne "win32-x64") {
        throw "Editor archive platform '$($releaseManifest.platform)' is not win32-x64."
    }

    $vsixEntry = $releaseManifest.files |
        Where-Object { $_.role -eq "vscode_extension" } |
        Select-Object -First 1
    if (-not $vsixEntry) {
        throw "Editor archive manifest has no VSIX entry."
    }
    if ([string] $vsixEntry.name -notmatch "\.vsix$") {
        throw "Editor archive manifest contains an invalid VSIX name: $($vsixEntry.name)"
    }
    $vsixPath = Assert-PathInside (Join-Path $extractRoot ([string] $vsixEntry.name)) $extractRoot "VSIX path"
    if (-not (Test-Path -LiteralPath $vsixPath -PathType Leaf)) {
        throw "Editor archive is missing the VSIX: $vsixPath"
    }
    if ([string] $vsixEntry.sha256 -notmatch "^[0-9a-fA-F]{64}$") {
        throw "Editor archive manifest contains an invalid VSIX SHA-256 value."
    }
    $vsixSha256 = Get-FileSha256 $vsixPath
    if ($vsixSha256 -ne ([string] $vsixEntry.sha256).ToLowerInvariant()) {
        throw "VSIX SHA-256 mismatch. Expected $($vsixEntry.sha256), got $vsixSha256."
    }
    Write-UpdateLog "VSIX SHA-256 verified: $vsixSha256."

    $resolvedCodePath = Resolve-CodePath
    New-Item -ItemType Directory -Path $ExtensionRoot -Force | Out-Null
    Write-UpdateLog "Installing VSIX with '$resolvedCodePath'."
    & $resolvedCodePath --install-extension $vsixPath --force --extensions-dir $ExtensionRoot
    if ($LASTEXITCODE -ne 0) {
        throw "VS Code VSIX installation failed with exit code $LASTEXITCODE."
    }

    $installedAfterUpdate = Get-InstalledExtensionInfo
    if (-not $installedAfterUpdate -or $installedAfterUpdate.ReleaseId -ne $release.Tag) {
        throw "VSIX installation completed but the installed toolchain release is not '$($release.Tag)'."
    }
    Write-InstalledState -Release $release -VsixSha256 $vsixSha256
    Write-UpdateLog "Installed Stasis nightly release '$($release.Tag)' successfully."
}
catch {
    Write-UpdateLog "ERROR: $($_.Exception.Message)"
    throw
}
