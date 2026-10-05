[CmdletBinding()]
param(
    [string] $ProjectRoot = (Get-Location).Path,
    [string] $GitPath = '',
    [switch] $RestoreToolchain,
    [string] $RestoreCommand = '',
    [string[]] $RestoreArguments = @(),
    [string] $RestoreAsset = '',
    [switch] $LocalFormatOnly
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Test-JsonIntegerValue {
    param($Value)
    if ($null -eq $Value) { return $false }
    $type = $Value.GetType()
    return $type -eq [sbyte] -or $type -eq [byte] -or $type -eq [int16] -or $type -eq [uint16] -or
        $type -eq [int32] -or $type -eq [uint32] -or $type -eq [int64] -or $type -eq [uint64]
}

$project = [IO.Path]::GetFullPath($ProjectRoot).TrimEnd([IO.Path]::DirectorySeparatorChar, [IO.Path]::AltDirectorySeparatorChar)
$manifestPath = Join-Path $project 'stasis.json'
if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) { throw "stasis.json is missing from '$project'." }
try { $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json -ErrorAction Stop }
catch { throw "Could not parse stasis.json: $($_.Exception.Message)" }

$pin = $manifest.vendor.stasis
if ($null -eq $pin -or [string]$pin.sha256 -cnotmatch '^[0-9a-f]{64}$' -or [int]$pin.hash_version -ne 2) {
    throw 'stasis.json vendor.stasis must contain an immutable nightly release, lowercase SHA-256, and hash_version 2.'
}
$pinHashVersionIsInteger = Test-JsonIntegerValue $pin.hash_version
if (-not $pinHashVersionIsInteger -or $pin.hash_version -ne 2) {
    throw 'stasis.json vendor.stasis hash_version must be the integer 2.'
}
$releaseId = [string]$pin.release_id
$vendorSha = [string]$pin.sha256
$officialRelease = $releaseId -match '^nightly-[0-9]{8}-[0-9]+$'
$localFormatRelease = $releaseId -ceq 'development' -or $releaseId -cmatch '^local-[A-Za-z0-9][A-Za-z0-9._-]*$'
if (-not $officialRelease -and (-not $LocalFormatOnly -or -not $localFormatRelease)) {
    throw 'stasis.json vendor.stasis must contain an immutable nightly release, lowercase SHA-256, and hash_version 2.'
}
$formatOnlyIdentity = -not $officialRelease

function Resolve-GitExecutable {
    param([string] $Requested)
    $candidate = $Requested
    if ([string]::IsNullOrWhiteSpace($candidate)) { $candidate = $env:STASIS_GIT_EXE }
    if ([string]::IsNullOrWhiteSpace($candidate)) {
        $found = Get-Command git -CommandType Application -ErrorAction Stop | Select-Object -First 1
        $candidate = $found.Source
        $configured = @(& $candidate -C $project config --local --get stasis.gitExecutable 2>$null)
        if ($LASTEXITCODE -eq 0 -and $configured.Count -gt 0 -and $configured[0].Trim()) { $candidate = $configured[0].Trim() }
        elseif ($LASTEXITCODE -gt 1) { throw "Could not read git stasis.gitExecutable (exit $LASTEXITCODE)." }
    } elseif (-not [IO.Path]::IsPathRooted($candidate) -and $candidate.Contains([IO.Path]::DirectorySeparatorChar)) {
        $candidate = Join-Path $project $candidate
    } elseif (-not [IO.Path]::IsPathRooted($candidate)) {
        $found = Get-Command $candidate -CommandType Application -ErrorAction Stop | Select-Object -First 1
        $candidate = $found.Source
    }
    $candidate = [IO.Path]::GetFullPath($candidate)
    if (-not (Test-Path -LiteralPath $candidate -PathType Leaf)) { throw "Git executable does not exist: $candidate" }
    return $candidate
}

$git = Resolve-GitExecutable $GitPath
function Get-LocalGitConfig {
    param([string] $Name)
    $value = @(& $git -C $project config --local --get $Name 2>$null)
    $exit = $LASTEXITCODE
    if ($exit -eq 0) { return (($value -join [Environment]::NewLine).Trim()) }
    if ($exit -eq 1) { return '' }
    throw "git config --local --get $Name failed with exit code $exit."
}

function Resolve-ProjectPath {
    param([string] $Value)
    if ([IO.Path]::IsPathRooted($Value)) { return [IO.Path]::GetFullPath($Value) }
    return [IO.Path]::GetFullPath((Join-Path $project $Value))
}

function Get-StasisCandidates {
    $candidates = [System.Collections.Generic.List[object]]::new()
    if (-not [string]::IsNullOrWhiteSpace($env:STASIS_BIN)) {
        $candidates.Add([pscustomobject]@{ path = (Resolve-ProjectPath $env:STASIS_BIN); required = $true; source = 'STASIS_BIN' })
    }
    $configured = Get-LocalGitConfig 'stasis.executable'
    if ($configured) {
        $candidates.Add([pscustomobject]@{ path = (Resolve-ProjectPath $configured); required = $true; source = 'git config stasis.executable' })
    }
    if ($LocalFormatOnly) {
        $pathStasis = Get-Command stasis -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($pathStasis) {
            $candidates.Add([pscustomobject]@{ path = [IO.Path]::GetFullPath($pathStasis.Source); required = $false; source = 'PATH Stasis executable' })
        }
    }

    $installRoots = [System.Collections.Generic.List[string]]::new()
    if ($env:LOCALAPPDATA) { $installRoots.Add((Join-Path $env:LOCALAPPDATA 'Stasis/releases')) }
    if ($env:USERPROFILE) { $installRoots.Add((Join-Path $env:USERPROFILE 'Stasis/releases')) }
    if ($env:HOME) {
        $installRoots.Add((Join-Path $env:HOME 'Stasis/releases'))
        $installRoots.Add((Join-Path $env:HOME '.local/share/stasis/releases'))
    }
    if ($env:XDG_DATA_HOME) { $installRoots.Add((Join-Path $env:XDG_DATA_HOME 'stasis/releases')) }
    if ($env:OS -eq 'Windows_NT') { $installRoots.Insert(0, 'D:/Stasis/releases') }
    foreach ($root in $installRoots) {
        foreach ($name in @('stasis.exe', 'bin/stasis.exe', 'stasis', 'bin/stasis')) {
            $candidates.Add([pscustomobject]@{ path = Join-Path (Join-Path $root $releaseId) $name; required = $false; source = 'per-user release install' })
        }
    }

    foreach ($root in @('.release-toolchain', '.stasis/toolchain')) {
        foreach ($name in @('stasis.exe', 'bin/stasis.exe', 'stasis', 'bin/stasis')) {
            $candidates.Add([pscustomobject]@{ path = (Join-Path (Join-Path $project $root) $name); required = $false; source = 'project toolchain cache' })
        }
    }
    return $candidates
}

function Get-JsonCommand {
    param([string] $Executable, [string[]] $Arguments, [string] $Description)
    Push-Location $project
    $stderrFile = $null
    $stderr = ''
    try {
        $stderrFile = New-TemporaryFile
        if (($stderrFile.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { throw "Refusing stderr capture reparse point '$($stderrFile.FullName)'" }
        $output = @(& $Executable @Arguments 2> $stderrFile.FullName | ForEach-Object { [string]$_ })
        $exit = $LASTEXITCODE
        $stderr = Get-Content -LiteralPath $stderrFile.FullName -Raw -ErrorAction SilentlyContinue
    } finally {
        try {
            if ($null -ne $stderrFile -and (Test-Path -LiteralPath $stderrFile.FullName -PathType Leaf)) {
                $tempItem = Get-Item -LiteralPath $stderrFile.FullName -Force
                if (($tempItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { throw "Refusing to remove replaced stderr capture path '$($stderrFile.FullName)'" }
                Remove-Item -LiteralPath $stderrFile.FullName -Force
            }
        } finally { Pop-Location }
    }
    if ($exit -ne 0) { throw "$Description failed with exit code ${exit}: stdout: $($output -join [Environment]::NewLine) stderr: $stderr" }
    try { return (($output -join [Environment]::NewLine) | ConvertFrom-Json -ErrorAction Stop) }
    catch {
        $rawOutput = ($output -join [Environment]::NewLine)
        if ($rawOutput.Length -gt 4000) { $rawOutput = $rawOutput.Substring(0, 4000) + '...' }
        if ($stderr.Length -gt 4000) { $stderr = $stderr.Substring(0, 4000) + '...' }
        throw "$Description returned invalid JSON: $($_.Exception.Message) Stdout: $rawOutput Stderr: $stderr"
    }
}

function Test-StasisCandidate {
    param([string] $Executable)
    $binarySha = (Get-FileHash -LiteralPath $Executable -Algorithm SHA256).Hash.ToLowerInvariant()
    $sourceCommit = $null
    $buildFingerprint = $null
    if (-not $formatOnlyIdentity) {
        $identity = Get-JsonCommand $Executable @('--json', 'editor-info') 'Stasis editor-info'
        if ($identity.ok -ne $true -or [string]$identity.result.release_id -cne $releaseId) {
            throw "Stasis executable '$Executable' is not the pinned release '$releaseId'."
        }
        if ([string]$identity.result.executable.sha256 -cne $binarySha -or
            [string]$identity.result.source_commit -notmatch '^[0-9a-f]{40}$' -or
            [string]::IsNullOrWhiteSpace([string]$identity.result.build_fingerprint)) {
            throw "Stasis editor-info for '$Executable' does not prove its binary and source identity."
        }
        $sourceCommit = [string]$identity.result.source_commit
        $buildFingerprint = [string]$identity.result.build_fingerprint
    }

    $status = Get-JsonCommand $Executable @('--json', 'vendor', 'status', '--workspace', $project) 'Stasis vendor status'
    $result = $status.result
    if ($null -eq $result -or $null -eq $result.recorded -or $null -eq $result.installed) {
        throw "Stasis vendor status for '$Executable' did not return recorded and installed identities."
    }
    $recorded = $result.recorded
    $installed = $result.installed
    if ($status.ok -isnot [bool] -or $result.current -isnot [bool] -or
        $result.update_available -isnot [bool] -or $result.pin_verified -isnot [bool] -or
        $result.legacy_pin_unverified -isnot [bool] -or $result.local_changes -isnot [bool]) {
        throw "Stasis vendor status for '$Executable' did not return strict boolean identity fields."
    }
    $hashVersions = @(
        $result.actual_hash_version, $result.expected_hash_version, $result.recorded_hash_version,
        $installed.hash_version, $recorded.hash_version
    )
    if (@($hashVersions | Where-Object { -not (Test-JsonIntegerValue $_) }).Count -gt 0) {
        throw "Stasis vendor status for '$Executable' did not return integer hash-version fields."
    }
    $requiredChecks = @(
        ($status.ok -eq $true),
        ([string]$status.command -ceq 'vendor'),
        ($result.current -eq $true),
        ($result.update_available -eq $false),
        ($result.pin_verified -eq $true),
        ($result.legacy_pin_unverified -eq $false),
        ($result.local_changes -eq $false),
        ([int]$result.actual_hash_version -eq 2),
        ([int]$result.expected_hash_version -eq 2),
        ([int]$result.recorded_hash_version -eq 2),
        ([int]$installed.hash_version -eq 2),
        ([int]$recorded.hash_version -eq 2),
        ([string]$installed.release_id -ceq $releaseId),
        ([string]$recorded.release_id -ceq $releaseId),
        ([string]$installed.sha256 -ceq $vendorSha),
        ([string]$recorded.sha256 -ceq $vendorSha),
        ([string]$result.recorded_sha256 -ceq $vendorSha),
        ([string]$result.expected_sha256 -ceq $vendorSha),
        ([string]$result.actual_sha256 -ceq $vendorSha)
    )
    if ($requiredChecks -contains $false) {
        throw "Stasis vendor status for '$Executable' does not prove exact pin fidelity (release=$releaseId hash=$vendorSha hash_version=2)."
    }
    return [pscustomobject]@{
        executable = [IO.Path]::GetFullPath($Executable)
        binary_sha256 = $binarySha
        release_id = $releaseId
        vendor_sha256 = $vendorSha
        hash_version = 2
        validation_scope = if ($formatOnlyIdentity) { 'vendor-match-format-only' } else { 'official-release' }
        source_commit = $sourceCommit
        build_fingerprint = $buildFingerprint
        git_executable = $git
    }
}

$selected = $null
foreach ($entry in (Get-StasisCandidates)) {
    if (-not (Test-Path -LiteralPath $entry.path -PathType Leaf)) {
        if ($entry.required) { throw "$($entry.source) points to a missing Stasis executable: $($entry.path)" }
        continue
    }
    try {
        $selected = Test-StasisCandidate $entry.path
        break
    } catch {
        if ($entry.required) { throw }
        Write-Warning "Skipping stale or invalid $($entry.source) candidate '$($entry.path)': $($_.Exception.Message)"
    }
}

if ($null -eq $selected -and $RestoreToolchain) {
    $restoreScript = Join-Path $project 'tools/restore-stasis-release.ps1'
    if (-not $RestoreCommand -and -not (Test-Path -LiteralPath $restoreScript -PathType Leaf)) { throw "Explicit restore requested but '$restoreScript' is missing." }
    $asset = $RestoreAsset
    if (-not $asset) {
        if ($env:OS -eq 'Windows_NT') { $asset = 'stasis-nightly-win-x64.zip' }
        else { $asset = 'stasis-nightly-linux-x64.tar.gz' }
    }
    if ($RestoreCommand) {
        $restoreExecutable = $RestoreCommand
        if ([IO.Path]::IsPathRooted($restoreExecutable)) { $restoreExecutable = [IO.Path]::GetFullPath($restoreExecutable) }
        elseif ($restoreExecutable.Contains('/') -or $restoreExecutable.Contains('\')) { $restoreExecutable = [IO.Path]::GetFullPath((Join-Path $project $restoreExecutable)) }
        else { $restoreExecutable = (Get-Command $restoreExecutable -CommandType Application -ErrorAction Stop | Select-Object -First 1).Source }
        $expandedArguments = @($RestoreArguments | ForEach-Object {
            ([string]$_).Replace('{release_id}', $releaseId).Replace('{asset_name}', $asset).Replace('{project_root}', $project).Replace('{destination}', '.stasis/toolchain')
        })
        $restoreOutput = @(& $restoreExecutable @expandedArguments 2>&1 | ForEach-Object { [string]$_ })
        $restoreExit = $LASTEXITCODE
        if ($restoreExit -ne 0) { throw "Explicit Stasis restore failed with exit code ${restoreExit}: $($restoreOutput -join [Environment]::NewLine)" }
    } else {
        & $restoreScript -ReleaseId $releaseId -AssetName $asset -ProjectRoot $project -Destination '.stasis/toolchain' | Out-Null
    }
    foreach ($candidate in (Get-StasisCandidates | Where-Object { $_.source -eq 'project toolchain cache' })) {
        if (Test-Path -LiteralPath $candidate.path -PathType Leaf) {
            try { $selected = Test-StasisCandidate $candidate.path; break }
            catch { Write-Warning "Restored toolchain candidate '$($candidate.path)' failed exact pin validation: $($_.Exception.Message)" }
        }
    }
}
if ($null -eq $selected) {
    throw "No verified Stasis executable for '$releaseId' was found. Set STASIS_BIN, configure git stasis.executable, install the release in the per-user release directory, or pass -RestoreToolchain explicitly."
}

$selected | ConvertTo-Json -Compress
