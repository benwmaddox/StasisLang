[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string] $ReleaseId,
    [Parameter(Mandatory = $true)][string] $AssetName,
    [string] $Repository = 'benwmaddox/StasisLang',
    [string] $ProjectRoot = (Get-Location).Path,
    [string] $Destination = '.stasis/toolchain'
)

$ErrorActionPreference = 'Stop'
if ($ReleaseId -notmatch '^nightly-[0-9]{8}-[0-9]+$') { throw "Invalid Stasis nightly release ID '$ReleaseId'." }
if (-not $Repository) { throw 'The official Stasis release repository is required.' }

$project = [IO.Path]::GetFullPath($ProjectRoot).TrimEnd([IO.Path]::DirectorySeparatorChar, [IO.Path]::AltDirectorySeparatorChar)
$destinationPath = if ([IO.Path]::IsPathRooted($Destination)) { [IO.Path]::GetFullPath($Destination) } else { [IO.Path]::GetFullPath((Join-Path $project $Destination)) }
$prefix = $project + [IO.Path]::DirectorySeparatorChar
$pathComparison = if ([Runtime.InteropServices.RuntimeInformation]::IsOSPlatform([Runtime.InteropServices.OSPlatform]::Windows)) { [StringComparison]::OrdinalIgnoreCase } else { [StringComparison]::Ordinal }
if (-not $destinationPath.StartsWith($prefix, $pathComparison)) {
    throw "Toolchain destination must be contained under project root '$project'."
}
if (Get-Item -LiteralPath $destinationPath -Force -ErrorAction SilentlyContinue) { throw "Refusing to overwrite existing toolchain destination '$destinationPath'." }

$relative = [IO.Path]::GetRelativePath($project, $destinationPath)
$cursor = $project
foreach ($part in ($relative -split '[\\/]')) {
    $cursor = Join-Path $cursor $part
    $item = Get-Item -LiteralPath $cursor -Force -ErrorAction SilentlyContinue
    if ($item) {
        if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { throw "Refusing toolchain destination through reparse point '$cursor'." }
    }
}

$parent = Split-Path -Parent $destinationPath
New-Item -ItemType Directory -Force -Path $parent | Out-Null
$token = [Guid]::NewGuid().ToString('N')
$staging = Join-Path $parent ".stasis-restore-$token"
$archive = Join-Path $parent ".stasis-download-$token"

try {
    $releaseJson = @(& gh api "repos/$Repository/releases/tags/$ReleaseId")
    if ($LASTEXITCODE -ne 0) { throw "Unable to query release '$ReleaseId' from $Repository." }
    $release = ($releaseJson -join [Environment]::NewLine) | ConvertFrom-Json
    if ($release.draft -or $release.tag_name -ne $ReleaseId) { throw "Release '$ReleaseId' is not an immutable published release." }
    $matches = @($release.assets | Where-Object { $_.name -eq $AssetName })
    if ($matches.Count -ne 1) { throw "Release '$ReleaseId' must contain exactly one '$AssetName' asset; found $($matches.Count)." }
    $asset = $matches[0]
    if ($asset.digest -notmatch '^sha256:([0-9a-fA-F]{64})$') { throw "GitHub did not publish a SHA-256 digest for '$AssetName'." }
    $expectedDigest = $Matches[1].ToLowerInvariant()
    $headers = @{ Accept = 'application/octet-stream' }
    if ($env:GH_TOKEN) { $headers.Authorization = "Bearer $env:GH_TOKEN" }
    Invoke-WebRequest -Uri $asset.url -Headers $headers -OutFile $archive
    $actualDigest = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actualDigest -ne $expectedDigest) { throw "SHA-256 digest mismatch for '$AssetName'." }

    $entries = @(tar -tf $archive)
    if ($LASTEXITCODE -ne 0 -or $entries.Count -eq 0) { throw "Archive '$AssetName' is empty or unreadable." }
    foreach ($entry in $entries) {
        $normalized = $entry.Replace('\\', '/')
        if ($normalized.StartsWith('/') -or $normalized -match '(^|/)\.\.(/|$)' -or $normalized -match '^[A-Za-z]:') {
            throw "Archive '$AssetName' contains unsafe path '$entry'."
        }
    }
    New-Item -ItemType Directory -Path $staging | Out-Null
    tar -xf $archive -C $staging
    if ($LASTEXITCODE -ne 0) { throw "Failed to extract '$AssetName'." }
    $linked = @(Get-ChildItem -LiteralPath $staging -Recurse -Force | Where-Object { ($_.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 })
    if ($linked.Count -ne 0) { throw "Archive '$AssetName' contains links or reparse points." }
    $candidateNames = if ($AssetName.EndsWith('.zip')) { @('stasis.exe', 'bin/stasis.exe') } else { @('stasis', 'bin/stasis') }
    $allExecutables = @(Get-ChildItem -LiteralPath $staging -Recurse -File | Where-Object { $_.Name -in @('stasis', 'stasis.exe') })
    $candidates = @($candidateNames | ForEach-Object { Join-Path $staging $_ } | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf })
    $wrapper = $null
    if ($candidates.Count -eq 0) {
        $topLevel = @(Get-ChildItem -LiteralPath $staging -Force)
        if ($topLevel.Count -eq 1 -and $topLevel[0].PSIsContainer) {
            $wrapper = $topLevel[0].FullName
            $candidates = @($candidateNames | ForEach-Object { Join-Path $wrapper $_ } | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf })
        }
    }
    if ($candidates.Count -ne 1 -or $allExecutables.Count -ne 1) { throw "Archive '$AssetName' must contain exactly one Stasis executable at its root, in bin, or under one shared wrapper directory; found $($allExecutables.Count)." }

    if ($wrapper) {
        $candidateRelative = [IO.Path]::GetRelativePath($wrapper, $candidates[0])
        Get-ChildItem -LiteralPath $wrapper -Force | ForEach-Object { Move-Item -LiteralPath $_.FullName -Destination $staging }
        Remove-Item -LiteralPath $wrapper -Force
        $candidates = @(Join-Path $staging $candidateRelative)
    }

    $info = & $candidates[0] --json editor-info | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0 -or $info.result.release_id -ne $ReleaseId) { throw "Restored toolchain identity does not match '$ReleaseId'." }
    Move-Item -LiteralPath $staging -Destination $destinationPath
    $installedExecutable = Join-Path $destinationPath ([IO.Path]::GetRelativePath($staging, $candidates[0]))
    $binDirectory = Split-Path -Parent $installedExecutable
    if ($env:GITHUB_PATH) { Add-Content -LiteralPath $env:GITHUB_PATH -Value $binDirectory }
    Write-Output $installedExecutable
}
finally {
    if (Test-Path -LiteralPath $archive) { Remove-Item -LiteralPath $archive -Force }
    if (Test-Path -LiteralPath $staging) { Remove-Item -LiteralPath $staging -Recurse -Force }
}
