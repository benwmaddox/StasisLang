[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string] $StasisPath,
    [Parameter(Mandatory = $true)][string] $ExpectedRelease,
    [string] $ProjectRoot = (Get-Location).Path
)

$ErrorActionPreference = 'Stop'
if ($ExpectedRelease -notmatch '^nightly-[0-9]{8}-[0-9]+$') {
    throw "Invalid expected Stasis release '$ExpectedRelease'."
}

$project = [IO.Path]::GetFullPath($ProjectRoot).TrimEnd([IO.Path]::DirectorySeparatorChar, [IO.Path]::AltDirectorySeparatorChar)
$stasis = if ([IO.Path]::IsPathRooted($StasisPath)) { [IO.Path]::GetFullPath($StasisPath) } else { [IO.Path]::GetFullPath((Join-Path $project $StasisPath)) }
if (-not (Test-Path -LiteralPath $stasis -PathType Leaf)) { throw "Stasis executable does not exist: $stasis" }

$manifestPath = Join-Path $project 'stasis.json'
$vendorPath = Join-Path $project 'vendor/stasis'
if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) { throw 'stasis.json is missing.' }
if (-not (Test-Path -LiteralPath $vendorPath -PathType Container)) { throw 'vendor/stasis is missing.' }

Push-Location $project
try {
    & $stasis vendor update
    if ($LASTEXITCODE -ne 0) { throw "stasis vendor update failed with exit code $LASTEXITCODE." }

    $statusJson = @(& $stasis --json vendor status --workspace .)
    if ($LASTEXITCODE -ne 0) { throw 'stasis vendor status failed after the pin update.' }
    try { $status = ($statusJson -join [Environment]::NewLine) | ConvertFrom-Json -ErrorAction Stop }
    catch { throw 'stasis vendor status returned invalid JSON after the pin update.' }
    if ($status.ok -ne $true -or $status.result.current -ne $true) {
        throw 'The updated vendor snapshot is not current for the restored Stasis release.'
    }

    $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
    $pin = $manifest.vendor.stasis
    if ($null -eq $pin -or [string]$pin.release_id -ne $ExpectedRelease) {
        throw "stasis vendor update recorded '$($pin.release_id)' instead of '$ExpectedRelease'."
    }
    if ([string]$pin.sha256 -notmatch '^[0-9a-f]{64}$') {
        throw 'stasis.json vendor.stasis.sha256 is not a lowercase SHA-256 digest.'
    }
    [ordered]@{
        ok = $true
        release_id = [string]$pin.release_id
        vendor_sha256 = [string]$pin.sha256
        changed_files = @('stasis.json', 'vendor/stasis')
    } | ConvertTo-Json -Compress
}
finally {
    Pop-Location
}
