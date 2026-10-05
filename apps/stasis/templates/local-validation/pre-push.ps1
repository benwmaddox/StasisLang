[CmdletBinding()]
param([string] $ProjectRoot = (Get-Location).Path, [string] $GitPath = '')

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$project = [IO.Path]::GetFullPath($ProjectRoot).TrimEnd([IO.Path]::DirectorySeparatorChar, [IO.Path]::AltDirectorySeparatorChar)

function Resolve-Git {
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
    if (-not [IO.Path]::IsPathRooted($candidate) -and ($candidate.Contains('/') -or $candidate.Contains('\'))) { $candidate = Join-Path $project $candidate }
    elseif (-not [IO.Path]::IsPathRooted($candidate)) { $candidate = (Get-Command $candidate -CommandType Application -ErrorAction Stop | Select-Object -First 1).Source }
    $candidate = [IO.Path]::GetFullPath($candidate)
    if (-not (Test-Path -LiteralPath $candidate -PathType Leaf)) { throw "Git executable does not exist: $candidate" }
    return $candidate
}

function Invoke-Git {
    param([string] $Executable, [string[]] $Arguments, [switch] $AllowFailure)
    $output = @(& $Executable -C $project @Arguments 2>&1 | ForEach-Object { [string]$_ })
    $exit = $LASTEXITCODE
    if (-not $AllowFailure -and $exit -ne 0) { throw "git $($Arguments -join ' ') failed with exit code ${exit}: $($output -join [Environment]::NewLine)" }
    return [pscustomobject]@{ output = $output; exit_code = $exit }
}

function Get-PowerShellHost {
    foreach ($name in @('pwsh', 'powershell.exe', 'powershell')) {
        $found = Get-Command $name -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($found) { return $found.Source }
    }
    throw 'The pre-push hook requires PowerShell (pwsh or Windows PowerShell).'
}

function Get-Porcelain {
    param([string] $Executable)
    $result = Invoke-Git $Executable @('status', '--porcelain=v1', '--untracked-files=all')
    return @($result.output | Where-Object { $_ -ne '' })
}

function Convert-ReceiptTimestamp {
    param([object] $Value)
    if ($Value -is [DateTimeOffset]) { return $Value }
    if ($Value -is [DateTime]) { return [DateTimeOffset]$Value }
    if ($Value -isnot [string]) { throw 'Local validation receipt has an invalid started_at value.' }
    $startedAt = [DateTimeOffset]::MinValue
    $parsed = [DateTimeOffset]::TryParse(
        $Value,
        [Globalization.CultureInfo]::InvariantCulture,
        [Globalization.DateTimeStyles]::None,
        [ref]$startedAt
    )
    if (-not $parsed) { throw 'Local validation receipt has an invalid started_at timestamp.' }
    return $startedAt
}

try {
    $stdinText = [Console]::In.ReadToEnd()
    $refs = [System.Collections.Generic.List[object]]::new()
    foreach ($line in ($stdinText -split "`r?`n")) {
        if (-not $line.Trim()) { continue }
        $columns = @($line.Trim() -split '\s+')
        if ($columns.Count -ne 4) { throw "Git pre-push supplied a malformed ref line: $line" }
        $refs.Add([pscustomobject]@{ local_ref = $columns[0]; local_sha = $columns[1]; remote_ref = $columns[2]; remote_sha = $columns[3] })
    }
    $nonDeletionCount = @($refs | Where-Object { $_.local_sha -notmatch '^0+$' }).Count
    if ($nonDeletionCount -eq 0) {
        Write-Output 'Stasis pre-push: deletion-only or empty push; local validation not required.'
        exit 0
    }

    $git = Resolve-Git $GitPath
    $headResult = Invoke-Git $git @('rev-parse', '--verify', 'HEAD')
    $head = ($headResult.output -join '').Trim().ToLowerInvariant()
    $matchingRefs = [System.Collections.Generic.List[string]]::new()
    foreach ($ref in $refs) {
        if ($ref.local_sha -match '^0+$') { continue }
        $peeled = Invoke-Git $git @('rev-parse', '--verify', "$($ref.local_sha)^{commit}") -AllowFailure
        if ($peeled.exit_code -ne 0) { throw "Pushed ref '$($ref.local_ref)' does not peel to a commit; refusing to push without local validation." }
        $peeledHead = ($peeled.output -join '').Trim().ToLowerInvariant()
        if ($peeledHead -cne $head) { throw "Pushed ref '$($ref.local_ref)' resolves to $peeledHead, not exact HEAD $head; refusing the mixed or non-HEAD push." }
        $matchingRefs.Add($ref.local_ref)
    }
    $before = @(Get-Porcelain $git)
    if ($before.Count -gt 0) {
        throw "Stasis pre-push requires a clean index and worktree (including untracked files) before validation:`n$($before -join [Environment]::NewLine)"
    }

    $validation = Join-Path $project 'tools/local-validation.ps1'
    if (-not (Test-Path -LiteralPath $validation -PathType Leaf)) { throw "Local validation entrypoint is missing: $validation" }
    $powerShell = Get-PowerShellHost
    $invocationId = [Guid]::NewGuid().ToString('N')
    $validationStartedAt = [DateTimeOffset]::UtcNow
    $validationOutput = @(& $powerShell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $validation -ProjectRoot $project -GitPath $git -InvocationId $invocationId -Quiet 2>&1 | ForEach-Object { [string]$_ })
    $validationExit = $LASTEXITCODE

    $after = @()
    $afterError = $null
    try { $after = @(Get-Porcelain $git) } catch { $afterError = $_.Exception.Message }
    $headAfterResult = Invoke-Git $git @('rev-parse', '--verify', 'HEAD') -AllowFailure
    $headAfter = if ($headAfterResult.exit_code -eq 0) { ($headAfterResult.output -join '').Trim().ToLowerInvariant() } else { '' }
    if ($afterError) { throw "Could not verify the worktree after validation: $afterError" }
    if ($headAfterResult.exit_code -ne 0 -or $headAfter -cne $head) { throw "HEAD changed during pre-push validation (before=$head after=$headAfter); refusing the push." }
    if ($after.Count -gt 0) { throw "Stasis pre-push validation left staged, unstaged, or untracked changes:`n$($after -join [Environment]::NewLine)" }
    if ($validationExit -ne 0) { throw "Local validation failed with exit code ${validationExit}: $($validationOutput | Select-Object -Last 30 | Out-String)" }

    $receiptPath = Join-Path $project 'build/local-validation/local-validation.json'
    if (-not (Test-Path -LiteralPath $receiptPath -PathType Leaf)) { throw "Local validation completed without its receipt: $receiptPath" }
    $receipt = Get-Content -LiteralPath $receiptPath -Raw | ConvertFrom-Json -ErrorAction Stop
    $receiptStartedAt = Convert-ReceiptTimestamp $receipt.started_at
    $manifestPath = Join-Path $project 'stasis.json'
    if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) { throw "Pinned project manifest is missing: $manifestPath" }
    $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json -ErrorAction Stop
    $pin = $manifest.vendor.stasis
    if ($receipt.ok -ne $true -or [string]$receipt.source_head -cne $head -or
        [string]$receipt.invocation_id -cne $invocationId -or
        $receiptStartedAt -lt $validationStartedAt -or
        [string]$receipt.stasis.release_id -notmatch '^nightly-[0-9]{8}-[0-9]+$' -or
        [string]$receipt.stasis.vendor_sha256 -cnotmatch '^[0-9a-f]{64}$' -or
        [string]$receipt.stasis.binary_sha256 -cnotmatch '^[0-9a-f]{64}$' -or
        [int]$receipt.stasis.hash_version -ne 2) {
        throw "Local validation receipt does not prove this successful run for the exact pushed HEAD and pinned release (ok=$($receipt.ok), receipt_head=$($receipt.source_head), push_head=$head, invocation=$($receipt.invocation_id), expected_invocation=$invocationId, started_at=$($receiptStartedAt.ToString('o')), validator_started_at=$($validationStartedAt.ToString('o')), release=$($receipt.stasis.release_id), vendor_sha256=$($receipt.stasis.vendor_sha256), hash_version=$($receipt.stasis.hash_version), binary_sha256=$($receipt.stasis.binary_sha256))."
    }
    if ([string]$pin.release_id -notmatch '^nightly-[0-9]{8}-[0-9]+$' -or
        [string]$pin.sha256 -cnotmatch '^[0-9a-f]{64}$' -or [int]$pin.hash_version -ne 2) {
        throw 'stasis.json does not contain an immutable nightly release pin with hash_version 2.'
    }
    if ([string]$receipt.stasis.release_id -cne [string]$pin.release_id -or
        [string]$receipt.stasis.vendor_sha256 -cne [string]$pin.sha256 -or
        [int]$receipt.stasis.hash_version -ne [int]$pin.hash_version) {
        throw "Local validation receipt pin does not match stasis.json (receipt=$($receipt.stasis.release_id)/$($receipt.stasis.vendor_sha256)/$($receipt.stasis.hash_version), manifest=$($pin.release_id)/$($pin.sha256)/$($pin.hash_version))."
    }
    $binaryAfter = (Get-FileHash -LiteralPath ([string]$receipt.stasis.path) -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($binaryAfter -cne [string]$receipt.stasis.binary_sha256) {
        throw 'The selected Stasis executable changed while local validation was running.'
    }
    $matched = $matchingRefs -join ', '
    Write-Output "Stasis pre-push validation passed for HEAD $head via $matched (Stasis $($receipt.stasis.release_id))."
    exit 0
} catch {
    [Console]::Error.WriteLine("Stasis pre-push: $($_.Exception.Message)")
    exit 1
}
