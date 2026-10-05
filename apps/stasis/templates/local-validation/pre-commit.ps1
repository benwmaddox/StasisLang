[CmdletBinding()]
param([string] $ProjectRoot = (Get-Location).Path, [string] $GitPath = '')

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$project = [IO.Path]::GetFullPath($ProjectRoot).TrimEnd([IO.Path]::DirectorySeparatorChar, [IO.Path]::AltDirectorySeparatorChar)
Push-Location $project
try {
$git = $GitPath
if (-not $git) { $git = $env:STASIS_GIT_EXE }
if (-not $git) {
    $git = (Get-Command git -CommandType Application -ErrorAction Stop | Select-Object -First 1).Source
    $configured = @(& $git -C $project config --local --get stasis.gitExecutable 2>$null)
    if ($LASTEXITCODE -eq 0 -and $configured.Count -gt 0 -and $configured[0].Trim()) { $git = $configured[0].Trim() }
    elseif ($LASTEXITCODE -gt 1) { throw "Could not read git stasis.gitExecutable (exit $LASTEXITCODE)." }
}
if (-not [IO.Path]::IsPathRooted($git) -and ($git.Contains('/') -or $git.Contains('\'))) { $git = Join-Path $project $git }
elseif (-not [IO.Path]::IsPathRooted($git)) { $git = (Get-Command $git -CommandType Application -ErrorAction Stop | Select-Object -First 1).Source }
$git = [IO.Path]::GetFullPath($git)
if (-not (Test-Path -LiteralPath $git -PathType Leaf)) { throw "Git executable does not exist: $git" }

$resolver = Join-Path $project 'tools/resolve-pinned-stasis.ps1'
if (-not (Test-Path -LiteralPath $resolver -PathType Leaf)) { throw "Pinned Stasis resolver is missing: $resolver" }
$resolvedOutput = @(& $resolver -ProjectRoot $project -GitPath $git)
if ($resolvedOutput.Count -eq 0) { throw 'Pinned Stasis resolver returned no identity.' }
try { $identity = ($resolvedOutput -join [Environment]::NewLine) | ConvertFrom-Json -ErrorAction Stop }
catch { throw "Pinned Stasis resolver returned invalid JSON: $($_.Exception.Message)" }
if (-not $identity.executable -or [string]$identity.release_id -notmatch '^nightly-[0-9]{8}-[0-9]+$' -or
    [string]$identity.vendor_sha256 -cnotmatch '^[0-9a-f]{64}$' -or [int]$identity.hash_version -ne 2) {
    throw 'Pinned Stasis resolver did not prove the exact project vendor identity.'
}

Write-Output "Stasis pre-commit: checking format with pinned release $($identity.release_id)."
$checkOutput = @(& $identity.executable fmt --check 2>&1 | ForEach-Object { [string]$_ })
$checkExit = $LASTEXITCODE
if ($checkExit -ne 0) {
    $formatOutput = @(& $identity.executable format 2>&1 | ForEach-Object { [string]$_ })
    $formatExit = $LASTEXITCODE
    if ($formatExit -ne 0) { throw "Pinned 'stasis format' failed with exit code ${formatExit}: $($formatOutput -join [Environment]::NewLine)" }
    $diffOutput = @(& $git -C $project diff --quiet -- ':(glob)**/*.stasis' 2>&1)
    $diffExit = $LASTEXITCODE
    if ($diffExit -eq 1) { throw "Commit blocked: pinned Stasis formatting changed source files. Review and stage the formatting changes, then commit again." }
    if ($diffExit -ne 0) { throw "git diff --quiet failed with exit code ${diffExit}: $($diffOutput -join [Environment]::NewLine)" }
    throw "Pinned 'stasis fmt --check' failed without producing a source diff: $($checkOutput -join [Environment]::NewLine)"
}

$diffOutput = @(& $git -C $project diff --quiet -- ':(glob)**/*.stasis' 2>&1)
$diffExit = $LASTEXITCODE
if ($diffExit -eq 1) { throw "Commit blocked: Stasis source has unstaged changes. Review and stage the source before committing." }
if ($diffExit -ne 0) { throw "git diff --quiet failed with exit code ${diffExit}: $($diffOutput -join [Environment]::NewLine)" }
Write-Output "Stasis pre-commit: source formatting is clean (release $($identity.release_id))."
} finally { Pop-Location }
