[CmdletBinding()]
param(
    [string] $ProjectRoot = (Get-Location).Path,
    [string] $GitPath = '',
    [string] $ConfigPath = 'tools/local-validation.json',
    [string] $ReceiptPath = 'build/local-validation/local-validation.json',
    [string] $SummaryPath = 'build/local-validation/local-validation.md',
    [string] $InvocationId = '',
    [switch] $RestoreToolchain,
    [switch] $Quiet
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$project = [IO.Path]::GetFullPath($ProjectRoot).TrimEnd([IO.Path]::DirectorySeparatorChar, [IO.Path]::AltDirectorySeparatorChar)

function Resolve-PathUnderProject {
    param([string] $Value, [string] $Label)
    $path = if ([IO.Path]::IsPathRooted($Value)) { [IO.Path]::GetFullPath($Value) } else { [IO.Path]::GetFullPath((Join-Path $project $Value)) }
    $prefix = $project + [IO.Path]::DirectorySeparatorChar
    $comparison = if ($env:OS -eq 'Windows_NT') { [StringComparison]::OrdinalIgnoreCase } else { [StringComparison]::Ordinal }
    if (-not $path.Equals($project, $comparison) -and -not $path.StartsWith($prefix, $comparison)) {
        throw "$Label must be contained under project root '$project'."
    }
    return $path
}

function Get-RelativePathPortable {
    param([string] $Directory, [string] $Path)
    $directoryPath = [IO.Path]::GetFullPath($Directory)
    $pathValue = [IO.Path]::GetFullPath($Path)
    if (-not $directoryPath.EndsWith([IO.Path]::DirectorySeparatorChar.ToString()) -and
        -not $directoryPath.EndsWith([IO.Path]::AltDirectorySeparatorChar.ToString())) {
        $directoryPath += [IO.Path]::DirectorySeparatorChar
    }
    $directoryUri = New-Object System.Uri($directoryPath)
    $pathUri = New-Object System.Uri($pathValue)
    return [Uri]::UnescapeDataString($directoryUri.MakeRelativeUri($pathUri).ToString()).Replace('/', [IO.Path]::DirectorySeparatorChar)
}

function Get-GitExecutable {
    param([string] $Requested)
    $candidate = $Requested
    if (-not $candidate) { $candidate = $env:STASIS_GIT_EXE }
    if (-not $candidate) {
        $found = Get-Command git -CommandType Application -ErrorAction Stop | Select-Object -First 1
        $candidate = $found.Source
        $configured = @(& $candidate -C $project config --local --get stasis.gitExecutable 2>$null)
        if ($LASTEXITCODE -eq 0 -and $configured.Count -gt 0 -and $configured[0].Trim()) { $candidate = $configured[0].Trim() }
        elseif ($LASTEXITCODE -gt 1) { throw "Could not read git stasis.gitExecutable (exit $LASTEXITCODE)." }
    }
    if (-not [IO.Path]::IsPathRooted($candidate) -and ($candidate.Contains('/') -or $candidate.Contains('\'))) {
        $candidate = Join-Path $project $candidate
    } elseif (-not [IO.Path]::IsPathRooted($candidate)) {
        $found = Get-Command $candidate -CommandType Application -ErrorAction Stop | Select-Object -First 1
        $candidate = $found.Source
    }
    $candidate = [IO.Path]::GetFullPath($candidate)
    if (-not (Test-Path -LiteralPath $candidate -PathType Leaf)) { throw "Git executable does not exist: $candidate" }
    return $candidate
}

$steps = [System.Collections.Generic.List[object]]::new()
$packageEvidence = $null
$toolchain = $null
$git = $null
$head = $null
$webOutputRelative = $null
$webOutput = $null
$config = $null
$startedAt = [DateTimeOffset]::UtcNow
if (-not $InvocationId) { $InvocationId = [Guid]::NewGuid().ToString('N') }
$success = $false
$cleanupError = $null

$main = {
Push-Location $project
try {
    $git = Get-GitExecutable $GitPath
    $headLines = @(& $git -C $project rev-parse --verify HEAD 2>&1)
    $headExit = $LASTEXITCODE
    if ($headExit -ne 0) { throw "git rev-parse HEAD failed with exit code ${headExit}: $($headLines -join [Environment]::NewLine)" }
    $head = ($headLines -join '').Trim()

    $configFile = Resolve-PathUnderProject $ConfigPath 'Configuration path'
    if (Test-Path -LiteralPath $configFile -PathType Leaf) {
        try { $config = Get-Content -LiteralPath $configFile -Raw | ConvertFrom-Json -ErrorAction Stop }
        catch { throw "Could not parse local validation config '$configFile': $($_.Exception.Message)" }
        if ($null -eq $config.PSObject.Properties['schema_version'] -or [int]$config.schema_version -ne 1) { throw "Unsupported local validation config schema in '$configFile'; expected schema_version 1." }
    } else {
        $config = [pscustomobject]@{ schema_version = 1; deterministic_tests = @(); stasis_tests = $null; web_package = $null; after_web_package = @(); restore_toolchain = $null }
    }
    foreach ($default in @(
        @{ name = 'deterministic_tests'; value = @() },
        @{ name = 'stasis_tests'; value = $null },
        @{ name = 'web_package'; value = $null },
        @{ name = 'after_web_package'; value = @() },
        @{ name = 'restore_toolchain'; value = $null }
    )) {
        if ($null -eq $config.PSObject.Properties[$default.name]) { $config | Add-Member -NotePropertyName $default.name -NotePropertyValue $default.value }
        elseif ($null -eq $config.($default.name) -and $default.name -in @('deterministic_tests', 'after_web_package')) { $config.($default.name) = @() }
    }
    foreach ($key in @('deterministic_tests', 'after_web_package')) {
        if ($null -eq $config.$key) { continue }
        if ($config.$key -isnot [System.Array]) { throw "Local validation config '$key' must be an array." }
        foreach ($entry in $config.$key) {
            if (-not $entry.name -or -not $entry.command -or $entry.args -isnot [System.Array]) {
                throw "Every '$key' entry must have name, command, and args array fields."
            }
        }
    }
    if ($null -ne $config.web_package -and ($null -eq $config.web_package.PSObject.Properties['command'] -or
        [string]::IsNullOrWhiteSpace([string]$config.web_package.command) -or
        $null -eq $config.web_package.PSObject.Properties['args'] -or $config.web_package.args -isnot [System.Array])) {
        throw 'Local validation config web_package must have command and args array fields.'
    }
    if ($null -ne $config.stasis_tests -and ($null -eq $config.stasis_tests.PSObject.Properties['command'] -or
        [string]::IsNullOrWhiteSpace([string]$config.stasis_tests.command) -or
        $null -eq $config.stasis_tests.PSObject.Properties['args'] -or $config.stasis_tests.args -isnot [System.Array])) {
        throw 'Local validation config stasis_tests must have command and args array fields.'
    }
    if ($null -ne $config.restore_toolchain -and ($null -eq $config.restore_toolchain.PSObject.Properties['command'] -or
        [string]::IsNullOrWhiteSpace([string]$config.restore_toolchain.command) -or
        $null -eq $config.restore_toolchain.PSObject.Properties['args'] -or $config.restore_toolchain.args -isnot [System.Array])) {
        throw 'Local validation config restore_toolchain must have command and args array fields.'
    }

    $resolver = Join-Path $PSScriptRoot 'resolve-pinned-stasis.ps1'
    if (-not (Test-Path -LiteralPath $resolver -PathType Leaf)) { throw "Pinned Stasis resolver is missing: $resolver" }
    $resolverArgs = @{ ProjectRoot = $project; GitPath = $git }
    if ($RestoreToolchain) {
        $resolverArgs.RestoreToolchain = $true
        if ($null -ne $config.restore_toolchain) {
            $resolverArgs.RestoreCommand = [string]$config.restore_toolchain.command
            $resolverArgs.RestoreArguments = @($config.restore_toolchain.args | ForEach-Object { [string]$_ })
        }
    }
    $resolutionText = @(& $resolver @resolverArgs)
    if ($resolutionText.Count -eq 0) { throw 'Pinned Stasis resolver returned no identity.' }
    $toolchain = ($resolutionText -join [Environment]::NewLine) | ConvertFrom-Json -ErrorAction Stop
    if (-not $toolchain.executable -or [string]$toolchain.release_id -notmatch '^nightly-[0-9]{8}-[0-9]+$' -or
        [string]$toolchain.vendor_sha256 -cnotmatch '^[0-9a-f]{64}$' -or [string]$toolchain.binary_sha256 -cnotmatch '^[0-9a-f]{64}$') {
        throw 'Pinned Stasis resolver returned an incomplete or malformed identity.'
    }

    $steps.Add((Invoke-ValidationStep 'fmt-check' $toolchain.executable @('fmt', '--check')))
    $steps.Add((Invoke-ValidationStep 'check' $toolchain.executable @('check')))
    if ($null -ne $config.stasis_tests) {
        $steps.Add((Invoke-ConfiguredStep 'test' ([string]$config.stasis_tests.command) @($config.stasis_tests.args) $toolchain $null))
    } else {
        $steps.Add((Invoke-ValidationStep 'test' $toolchain.executable @('test')))
    }
    foreach ($entry in @($config.deterministic_tests)) {
        if ($null -eq $entry) { continue }
        $steps.Add((Invoke-ConfiguredStep ([string]$entry.name) ([string]$entry.command) @($entry.args) $toolchain $null))
    }
    $steps.Add((Invoke-ValidationStep 'git-diff-check' $git @('-C', $project, 'diff', '--check', 'HEAD', '--')))

    if (@($steps | Where-Object { -not $_.ok }).Count -eq 0) {
        $buildRoot = Join-Path $project 'build'
        $validationRoot = Join-Path $buildRoot 'local-validation'
        Assert-SafeDirectoryChain $project $validationRoot
        if (-not (Test-Path -LiteralPath $validationRoot -PathType Container)) { New-Item -ItemType Directory -Path $validationRoot -Force | Out-Null }
        Assert-SafeDirectoryChain $project $validationRoot
        $leaf = 'web-' + [Guid]::NewGuid().ToString('N')
        $webOutput = Join-Path $validationRoot $leaf
        if (Test-Path -LiteralPath $webOutput) { throw "Fresh Web output path already exists: $webOutput" }
        $webOutputRelative = ('build/local-validation/' + $leaf)

        if ($null -ne $config.web_package) {
            $packageStep = Invoke-ConfiguredStep 'web-package' ([string]$config.web_package.command) @($config.web_package.args) $toolchain $webOutputRelative
        } else {
            $packageStep = Invoke-ValidationStep 'web-package' $toolchain.executable @('--json', 'package', '--target', 'web', '--out', $webOutputRelative)
        }
        $steps.Add($packageStep)
        if ($packageStep.ok) {
            $packageEvidence = Get-WebPackageEvidence $webOutput $toolchain
            foreach ($entry in @($config.after_web_package)) {
                if ($null -eq $entry) { continue }
                $steps.Add((Invoke-ConfiguredStep ([string]$entry.name) ([string]$entry.command) @($entry.args) $toolchain $webOutputRelative))
            }
            $packageEvidence = Get-WebPackageEvidence $webOutput $toolchain
            if (@($steps | Where-Object { -not $_.ok }).Count -eq 0) {
                $steps.Add((Invoke-ToolchainIdentityCheck $toolchain))
            }
        }
    } else {
        $steps.Add([pscustomobject]@{ name = 'web-package'; ok = $false; exit_code = $null; skipped = $true; output_tail = 'Skipped because an earlier validation step failed.' })
    }
    if ($null -ne $head -and $null -ne $git) {
        $headAfter = @(& $git -C $project rev-parse --verify HEAD 2>&1 | ForEach-Object { [string]$_ })
        $headExit = $LASTEXITCODE
        $headAfterValue = ($headAfter -join '').Trim()
        $steps.Add([pscustomobject]@{
            name = 'source-head-stable'
            ok = $headExit -eq 0 -and $headAfterValue -ceq $head
            skipped = $false
            exit_code = if ($headExit -ne 0) { $headExit } elseif ($headAfterValue -cne $head) { 1 } else { 0 }
            output_tail = if ($headExit -ne 0) { $headAfter -join [Environment]::NewLine } elseif ($headAfterValue -cne $head) { "HEAD changed during validation (before=$head after=$headAfterValue)." } else { '' }
        })
    }
    $success = @($steps | Where-Object { -not $_.ok }).Count -eq 0 -and $null -ne $packageEvidence
} catch {
    $steps.Add([pscustomobject]@{ name = 'validation-setup'; ok = $false; skipped = $false; exit_code = 1; output_tail = $_.Exception.Message })
} finally {
    try {
        if ($null -ne $webOutput -and (Test-Path -LiteralPath $webOutput)) {
            try { Remove-SafeFreshOutput $project $webOutput }
            catch { $cleanupError = $_.Exception.Message; $success = $false }
        }
    } finally { Pop-Location }
}

if ($cleanupError) {
    $steps.Add([pscustomobject]@{ name = 'temporary-output-cleanup'; ok = $false; skipped = $false; exit_code = 1; output_tail = $cleanupError })
}

$receipt = Resolve-PathUnderProject $ReceiptPath 'Receipt path'
$summary = Resolve-PathUnderProject $SummaryPath 'Summary path'
foreach ($path in @($receipt, $summary)) {
    $parent = Split-Path -Parent $path
    Assert-SafeDirectoryChain $project $parent
    if (-not (Test-Path -LiteralPath $parent -PathType Container)) { New-Item -ItemType Directory -Path $parent -Force | Out-Null }
    Assert-SafeDirectoryChain $project $parent
    Assert-SafeDirectoryChain $project $path
}
$result = [ordered]@{
    schema = 'stasis-local-validation'
    version = 1
    ok = $success
    generated_at = [DateTimeOffset]::UtcNow.ToString('o')
    started_at = $startedAt.ToString('o')
    invocation_id = $InvocationId
    project_root = $project
    source_head = $head
    stasis = if ($null -ne $toolchain) { [ordered]@{ path = $toolchain.executable; binary_sha256 = $toolchain.binary_sha256; release_id = $toolchain.release_id; vendor_sha256 = $toolchain.vendor_sha256; hash_version = $toolchain.hash_version; source_commit = $toolchain.source_commit; build_fingerprint = $toolchain.build_fingerprint } } else { $null }
    web_output = $webOutputRelative
    web_package = $packageEvidence
    steps = @($steps)
}
[IO.File]::WriteAllText($receipt, (($result | ConvertTo-Json -Depth 12) + [Environment]::NewLine), [Text.UTF8Encoding]::new($false))

$lines = [System.Collections.Generic.List[string]]::new()
$lines.Add('# Stasis local validation')
$lines.Add('')
$lines.Add("- Result: **$(if ($success) { 'PASS' } else { 'FAIL' })**")
$lines.Add("- Source HEAD: ``$head``")
$lines.Add("- Stasis: ``$(if ($null -ne $toolchain) { $toolchain.executable } else { 'unresolved' })``")
$lines.Add("- Release and vendor SHA-256: ``$(if ($null -ne $toolchain) { $toolchain.release_id + ' / ' + $toolchain.vendor_sha256 } else { 'unverified' })``")
$lines.Add("- Web provenance SHA-256: ``$(if ($null -ne $packageEvidence) { $packageEvidence.provenance_sha256 } else { 'unavailable' })``")
$lines.Add("- WebAssembly: $(if ($null -ne $packageEvidence) { (@($packageEvidence.wasm_files | ForEach-Object { $_.path + ' ' + $_.sha256 }) -join '; ') } else { 'unavailable' })")
$lines.Add("- JSON receipt: ``$ReceiptPath``")
$lines.Add('')
$lines.Add('| Step | Result | Exit code |')
$lines.Add('| --- | --- | ---: |')
foreach ($step in $steps) { $lines.Add("| $($step.name) | $(if ($step.ok) { 'PASS' } elseif ($step.skipped) { 'SKIP' } else { 'FAIL' }) | $($step.exit_code) |") }
[IO.File]::WriteAllText($summary, (($lines -join [Environment]::NewLine) + [Environment]::NewLine), [Text.UTF8Encoding]::new($false))
if (-not $Quiet) {
    Get-Content -LiteralPath $summary
} elseif (-not $success) {
    [Console]::Error.WriteLine("Stasis local validation failed. Receipt: $ReceiptPath")
    foreach ($step in @($steps | Where-Object { -not $_.ok })) {
        [Console]::Error.WriteLine("- $($step.name) (exit $($step.exit_code))")
        $diagnostic = ([string]$step.output_tail -split "`r?`n" | Select-Object -Last 20)
        foreach ($line in $diagnostic) {
            if (-not [string]::IsNullOrWhiteSpace($line)) { [Console]::Error.WriteLine("  $line") }
        }
    }
}
if (-not $success) { exit 1 }
}

function Invoke-ValidationStep {
    param([string] $Name, [string] $Executable, [string[]] $Arguments)
    $started = [DateTimeOffset]::UtcNow
    $output = @()
    $exit = 0
    try {
        $output = @(& $Executable @Arguments 2>&1 | ForEach-Object { [string]$_ })
        $exit = $LASTEXITCODE
    } catch { $output += $_.Exception.Message; $exit = 1 }
    return [pscustomobject]@{ name = $Name; command = $Executable; arguments = @($Arguments); ok = $exit -eq 0; skipped = $false; exit_code = $exit; started_at = $started.ToString('o'); finished_at = [DateTimeOffset]::UtcNow.ToString('o'); output_tail = (($output | Select-Object -Last 60) -join [Environment]::NewLine) }
}

function Resolve-ConfiguredCommand {
    param([string] $Command)
    if ([IO.Path]::IsPathRooted($Command)) { return [IO.Path]::GetFullPath($Command) }
    if ($Command.Contains('/') -or $Command.Contains('\')) { return [IO.Path]::GetFullPath((Join-Path $project $Command)) }
    $found = Get-Command $Command -CommandType Application -ErrorAction Stop | Select-Object -First 1
    return $found.Source
}

function Invoke-ConfiguredStep {
    param([string] $Name, [string] $Command, [string[]] $Arguments, $Identity, [string] $WebOutput)
    $expandedCommand = ([string]$Command).Replace('{stasis}', [string]$Identity.executable).Replace('{web_output}', [string]$WebOutput).Replace('{project_root}', $project).Replace('{release_id}', [string]$Identity.release_id).Replace('{vendor_sha256}', [string]$Identity.vendor_sha256)
    $resolved = Resolve-ConfiguredCommand $expandedCommand
    $expanded = @($Arguments | ForEach-Object {
        ([string]$_).Replace('{stasis}', [string]$Identity.executable).Replace('{web_output}', [string]$WebOutput).Replace('{project_root}', $project).Replace('{release_id}', [string]$Identity.release_id).Replace('{vendor_sha256}', [string]$Identity.vendor_sha256)
    })
    return Invoke-ValidationStep $Name $resolved $expanded
}

function Invoke-ToolchainIdentityCheck {
    param($Identity)
    $started = [DateTimeOffset]::UtcNow
    $exit = 0
    $errorText = ''
    try {
        $resolver = Join-Path $PSScriptRoot 'resolve-pinned-stasis.ps1'
        $output = @(& $resolver -WarningAction SilentlyContinue -ProjectRoot $project -GitPath $Identity.git_executable)
        if ($output.Count -eq 0) { throw 'final resolver returned no identity.' }
        $current = ($output -join [Environment]::NewLine) | ConvertFrom-Json -ErrorAction Stop
        foreach ($key in @('executable', 'binary_sha256', 'release_id', 'vendor_sha256', 'hash_version', 'source_commit', 'build_fingerprint')) {
            if ([string]$current.$key -cne [string]$Identity.$key) { throw "selected Stasis identity changed at '$key'" }
        }
    } catch {
        $exit = 1
        $errorText = $_.Exception.Message
    }
    return [pscustomobject]@{
        name = 'toolchain-identity-stable'
        ok = $exit -eq 0
        skipped = $false
        exit_code = $exit
        started_at = $started.ToString('o')
        finished_at = [DateTimeOffset]::UtcNow.ToString('o')
        output_tail = if ($errorText) { $errorText } else { 'Pinned release, binary hash, vendor snapshot, source commit, and build fingerprint stayed unchanged.' }
    }
}

function Assert-SafeDirectoryChain {
    param([string] $Root, [string] $Path)
    $fullRoot = [IO.Path]::GetFullPath($Root)
    $fullPath = [IO.Path]::GetFullPath($Path)
    $prefix = $fullRoot + [IO.Path]::DirectorySeparatorChar
    $comparison = if ($env:OS -eq 'Windows_NT') { [StringComparison]::OrdinalIgnoreCase } else { [StringComparison]::Ordinal }
    if (-not $fullPath.Equals($fullRoot, $comparison) -and -not $fullPath.StartsWith($prefix, $comparison)) { throw "Refusing path outside project root: $fullPath" }
    $relative = Get-RelativePathPortable $fullRoot $fullPath
    $cursor = $fullRoot
    foreach ($part in ($relative -split '[\\/]')) {
        if (-not $part -or $part -eq '.') { continue }
        $cursor = Join-Path $cursor $part
        $item = Get-Item -LiteralPath $cursor -Force -ErrorAction SilentlyContinue
        if ($item -and (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0)) { throw "Refusing to use path through reparse point '$cursor'." }
    }
}

function Get-WebPackageEvidence {
    param([string] $OutputPath, $Identity)
    Assert-SafeDirectoryChain $project $OutputPath
    if (-not (Test-Path -LiteralPath $OutputPath -PathType Container)) { throw "Web package output is missing: $OutputPath" }
    $provenancePath = Join-Path $OutputPath 'stasis_provenance.json'
    if (-not (Test-Path -LiteralPath $provenancePath -PathType Leaf)) { throw 'Fresh Web package has no stasis_provenance.json.' }
    if (-not (Test-Path -LiteralPath (Join-Path $OutputPath 'index.html') -PathType Leaf)) { throw 'Fresh Web package has no index.html.' }
    try { $provenance = Get-Content -LiteralPath $provenancePath -Raw | ConvertFrom-Json -ErrorAction Stop }
    catch { throw "Web package provenance is invalid JSON: $($_.Exception.Message)" }
    $manifestSha = (Get-FileHash -LiteralPath (Join-Path $project 'stasis.json') -Algorithm SHA256).Hash.ToLowerInvariant()
    $projectProvenance = $provenance.web_package.project
    $vendorProvenance = $projectProvenance.vendor
    if ([string]$provenance.schema -cne 'stasis.release_provenance.v1' -or
        [string]$provenance.release_tag -cne [string]$Identity.release_id -or
        [string]$provenance.source_commit -cne [string]$Identity.source_commit -or
        [string]$provenance.compiler.sha256 -cne [string]$Identity.binary_sha256 -or
        $provenance.development_build -ne $false -or $provenance.dirty_state -ne $false -or
        [string]$projectProvenance.manifest.sha256 -cne $manifestSha -or
        [string]$vendorProvenance.release_id -cne [string]$Identity.release_id -or
        [string]$vendorProvenance.recorded_sha256 -cne [string]$Identity.vendor_sha256 -or
        [string]$vendorProvenance.actual_sha256 -cne [string]$Identity.vendor_sha256 -or
        [int]$vendorProvenance.recorded_hash_version -ne 2 -or [int]$vendorProvenance.actual_hash_version -ne 2) {
        throw 'Fresh Web package provenance does not match the selected official Stasis binary, immutable pin, and project manifest.'
    }
    $descendants = @(Get-SafeDescendants $OutputPath 'Fresh Web package')
    $wasmFiles = @($descendants | Where-Object { -not $_.PSIsContainer -and $_.Extension -ieq '.wasm' } | Sort-Object FullName)
    if ($wasmFiles.Count -eq 0) { throw 'Fresh Web package contains no WebAssembly module.' }
    $records = @()
    foreach ($file in $wasmFiles) {
        $records += [pscustomobject]@{ path = (Get-RelativePathPortable $OutputPath $file.FullName).Replace('\', '/'); sha256 = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant(); bytes = $file.Length }
    }
    return [pscustomobject]@{
        provenance_sha256 = (Get-FileHash -LiteralPath $provenancePath -Algorithm SHA256).Hash.ToLowerInvariant()
        schema = [string]$provenance.schema
        release_tag = [string]$provenance.release_tag
        source_commit = [string]$provenance.source_commit
        compiler_sha256 = [string]$provenance.compiler.sha256
        project_manifest_sha256 = [string]$projectProvenance.manifest.sha256
        vendor_sha256 = [string]$vendorProvenance.actual_sha256
        wasm_files = $records
    }
}

function Get-SafeDescendants {
    param([string] $Directory, [string] $Label)
    $rootItem = Get-Item -LiteralPath $Directory -Force -ErrorAction Stop
    if (($rootItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { throw "$Label contains a reparse point: $Directory" }
    $pending = [System.Collections.Generic.Stack[string]]::new()
    $pending.Push($Directory)
    $items = [System.Collections.Generic.List[object]]::new()
    while ($pending.Count -gt 0) {
        $current = $pending.Pop()
        foreach ($item in Get-ChildItem -LiteralPath $current -Force -ErrorAction Stop) {
            if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { throw "$Label contains a reparse point: $($item.FullName)" }
            $items.Add($item)
            if ($item.PSIsContainer) { $pending.Push($item.FullName) }
        }
    }
    return $items.ToArray()
}

function Remove-SafeFreshOutput {
    param([string] $Root, [string] $OutputPath)
    $expected = [IO.Path]::GetFullPath((Join-Path $Root 'build/local-validation'))
    $full = [IO.Path]::GetFullPath($OutputPath)
    $prefix = $expected + [IO.Path]::DirectorySeparatorChar
    $comparison = if ($env:OS -eq 'Windows_NT') { [StringComparison]::OrdinalIgnoreCase } else { [StringComparison]::Ordinal }
    if (-not $full.StartsWith($prefix, $comparison)) { throw "Refusing to remove Web output outside its unique local-validation directory: $full" }
    Assert-SafeDirectoryChain $Root $full
    $null = Get-SafeDescendants $full 'Fresh Web output; leaving it in place'
    Remove-Item -LiteralPath $full -Recurse -Force
}

. $main
