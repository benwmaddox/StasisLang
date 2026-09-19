[CmdletBinding()]
param(
    [string] $ProjectRoot = (Get-Location).Path,
    [string] $StasisPath = '',
    [string] $ReceiptPath = 'artifacts/validation/pre-pr.json',
    [string] $SummaryPath = 'artifacts/validation/pre-pr.md'
)

$ErrorActionPreference = 'Stop'
$project = [IO.Path]::GetFullPath($ProjectRoot).TrimEnd([IO.Path]::DirectorySeparatorChar, [IO.Path]::AltDirectorySeparatorChar)
$receipt = if ([IO.Path]::IsPathRooted($ReceiptPath)) { [IO.Path]::GetFullPath($ReceiptPath) } else { [IO.Path]::GetFullPath((Join-Path $project $ReceiptPath)) }
$summary = if ([IO.Path]::IsPathRooted($SummaryPath)) { [IO.Path]::GetFullPath($SummaryPath) } else { [IO.Path]::GetFullPath((Join-Path $project $SummaryPath)) }
$receiptParent = Split-Path -Parent $receipt
$summaryParent = Split-Path -Parent $summary
New-Item -ItemType Directory -Force -Path $receiptParent, $summaryParent | Out-Null

function Resolve-StasisExecutable {
    if (-not [string]::IsNullOrWhiteSpace($StasisPath)) {
        $candidate = if ([IO.Path]::IsPathRooted($StasisPath)) {
            [IO.Path]::GetFullPath($StasisPath)
        } else {
            [IO.Path]::GetFullPath((Join-Path $project $StasisPath))
        }
    } else {
        $command = Get-Command stasis -CommandType Application -ErrorAction Stop
        $candidate = [IO.Path]::GetFullPath($command.Source)
    }
    if (-not (Test-Path -LiteralPath $candidate -PathType Leaf)) {
        throw "Stasis executable does not exist: $candidate"
    }
    return $candidate
}

$stasis = $null
$toolchainError = $null
try { $stasis = Resolve-StasisExecutable }
catch { $toolchainError = $_.Exception.Message }

function Invoke-ValidationStep {
    param([string] $Name, [string[]] $Arguments)
    $started = [DateTimeOffset]::UtcNow
    $output = @()
    $exitCode = 0
    try {
        $output = @(& $stasis @Arguments 2>&1 | ForEach-Object { [string]$_ })
        $exitCode = $LASTEXITCODE
    } catch {
        $output += $_.Exception.Message
        $exitCode = 1
    }
    $finished = [DateTimeOffset]::UtcNow
    $tail = ($output | Select-Object -Last 80) -join [Environment]::NewLine
    [pscustomobject]@{
        name = $Name
        command = ('& "' + $stasis + '" ' + ($Arguments -join ' '))
        ok = $exitCode -eq 0
        exit_code = $exitCode
        started_at = $started.ToString('o')
        finished_at = $finished.ToString('o')
        output_tail = $tail
    }
}

$manifest = $null
$pin = $null
$manifestError = $null
try {
    $manifest = Get-Content -LiteralPath (Join-Path $project 'stasis.json') -Raw | ConvertFrom-Json -ErrorAction Stop
    $pin = $manifest.vendor.stasis
    if ($null -eq $pin -or [string]$pin.release_id -notmatch '^nightly-[0-9]{8}-[0-9]+$' -or [string]$pin.sha256 -notmatch '^[0-9a-f]{64}$') {
        throw 'stasis.json vendor.stasis must contain a nightly release and lowercase SHA-256 digest.'
    }
} catch {
    $manifestError = $_.Exception.Message
}

$steps = @()
if ($null -eq $manifestError -and $null -eq $toolchainError) {
    Push-Location $project
    try {
        # These are intentionally one ordered local pass. The hosted PR gate
        # stays cheap while this receipt captures the broad pre-PR evidence.
        $vendorStep = Invoke-ValidationStep 'pinned-vendor-status' @('--json', 'vendor', 'status', '--workspace', '.')
        $toolchainIdentity = $null
        if ($vendorStep.ok) {
            try {
                $status = $vendorStep.output_tail | ConvertFrom-Json -ErrorAction Stop
                $installed = $status.result.installed
                if ($status.ok -ne $true -or $status.result.current -ne $true) {
                    throw 'checked-in vendor snapshot is not current for the immutable pin'
                }
                if ([string]$installed.release_id -ne [string]$pin.release_id -or
                    [string]$installed.sha256 -ne [string]$pin.sha256) {
                    throw "selected Stasis executable identity does not match the checked-in pin ($($pin.release_id))."
                }
                $toolchainIdentity = [ordered]@{
                    release_id = [string]$installed.release_id
                    vendor_sha256 = [string]$installed.sha256
                }
            } catch {
                $vendorStep.ok = $false
                $vendorStep.exit_code = 1
                $vendorStep.error = $_.Exception.Message
            }
        }
        $steps += $vendorStep
        $steps += Invoke-ValidationStep 'format' @('fmt', '--check')
        $steps += Invoke-ValidationStep 'check' @('check')
        $steps += Invoke-ValidationStep 'test' @('test')
        $steps += Invoke-ValidationStep 'package-desktop' @('package', '--target', 'desktop')
    }
    finally {
        Pop-Location
    }
}
elseif ($null -ne $toolchainError) {
    $steps += [pscustomobject]@{
        name = 'pinned-vendor-status'
        command = if ($StasisPath) { $StasisPath } else { 'stasis' }
        ok = $false
        exit_code = 1
        started_at = [DateTimeOffset]::UtcNow.ToString('o')
        finished_at = [DateTimeOffset]::UtcNow.ToString('o')
        output_tail = $toolchainError
        error = $toolchainError
    }
}

$gitHead = $null
$gitBranch = $null
try {
    $gitHead = (git -C $project rev-parse HEAD 2>$null).Trim()
    $gitBranch = (git -C $project branch --show-current 2>$null).Trim()
} catch { }
$ok = $null -eq $manifestError -and $null -eq $toolchainError -and @($steps | Where-Object { -not $_.ok }).Count -eq 0
$result = [ordered]@{
    schema = 'stasis-pre-pr-validation'
    version = 1
    ok = $ok
    generated_at = [DateTimeOffset]::UtcNow.ToString('o')
    project = [IO.Path]::GetFileName($project)
    stasis_executable = $stasis
    git = [ordered]@{ head = $gitHead; branch = $gitBranch }
    pin = if ($null -ne $pin) { [ordered]@{ release_id = [string]$pin.release_id; vendor_sha256 = [string]$pin.sha256 } } else { $null }
    manifest_error = $manifestError
    toolchain_error = $toolchainError
    toolchain_identity = $toolchainIdentity
    steps = @($steps)
}
[IO.File]::WriteAllText($receipt, (($result | ConvertTo-Json -Depth 8) + [Environment]::NewLine), [Text.UTF8Encoding]::new($false))

$summaryLines = [System.Collections.Generic.List[string]]::new()
$summaryLines.Add('# Stasis pre-PR validation')
$summaryLines.Add('')
$summaryLines.Add("- Result: **$(if ($ok) { 'PASS' } else { 'FAIL' })**")
$summaryLines.Add("- Checked-in Stasis release: ``$([string]$pin.release_id)``")
$summaryLines.Add("- Checked-in vendor SHA-256: ``$([string]$pin.sha256)``")
$summaryLines.Add("- Validated executable: ``$stasis``")
$summaryLines.Add("- Identity check: $(if ($null -ne $toolchainIdentity) { 'exact checked-in pin' } else { 'not established' })")
$summaryLines.Add("- Git head: ``$gitHead``")
$summaryLines.Add("- Machine-readable receipt: ``$ReceiptPath``")
$summaryLines.Add('')
$summaryLines.Add('| Step | Result | Exit code |')
$summaryLines.Add('| --- | --- | ---: |')
foreach ($step in $steps) {
    $summaryLines.Add("| $($step.name) | $(if ($step.ok) { 'PASS' } else { 'FAIL' }) | $($step.exit_code) |")
}
if ($manifestError) { $summaryLines.Add("| manifest | FAIL | 1 |") }
$summaryLines.Add('')
$summaryLines.Add('Paste this summary into the pull request description and attach the JSON receipt when requesting review.')
[IO.File]::WriteAllText($summary, (($summaryLines -join [Environment]::NewLine) + [Environment]::NewLine), [Text.UTF8Encoding]::new($false))
Get-Content -LiteralPath $summary
if (-not $ok) { exit 1 }
