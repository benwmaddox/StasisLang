[CmdletBinding()]
param(
    [string] $Repository = 'benwmaddox/StasisLang'
)

$ErrorActionPreference = 'Stop'
if (-not $Repository) { throw 'The official Stasis release repository is required.' }

$requiredAssets = @(
    'stasis-nightly-linux-x64.tar.gz',
    'stasis-nightly-win-x64.zip',
    'stasis-nightly-osx-arm64.tar.gz'
)

for ($page = 1; $page -le 10; $page++) {
    $releaseJson = @(& gh api "repos/$Repository/releases?per_page=100&page=$page")
    if ($LASTEXITCODE -ne 0) { throw "Unable to query releases for $Repository." }
    $releases = ($releaseJson -join [Environment]::NewLine) | ConvertFrom-Json
    foreach ($release in $releases) {
        if ($release.draft -or $release.tag_name -notmatch '^nightly-[0-9]{8}-[0-9]+$') { continue }
        $assets = @($release.assets)
        $complete = $true
        foreach ($name in $requiredAssets) {
            $matches = @($assets | Where-Object { $_.name -eq $name -and $_.digest -match '^sha256:[0-9a-fA-F]{64}$' })
            if ($matches.Count -ne 1) { $complete = $false; break }
        }
        if ($complete) {
            Write-Output $release.tag_name
            exit 0
        }
    }
    if (@($releases).Count -lt 100) { break }
}

throw "No complete non-draft nightly release was found for $Repository."
