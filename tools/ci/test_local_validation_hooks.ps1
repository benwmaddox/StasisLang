[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$sourceRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$templateRoot = Join-Path $sourceRoot 'apps/stasis/templates/local-validation'
$gitCommand = Get-Command git -CommandType Application -ErrorAction Stop | Select-Object -First 1
$pwshCommand = Get-Command pwsh -CommandType Application -ErrorAction Stop | Select-Object -First 1
$gitExe = [IO.Path]::GetFullPath($gitCommand.Source)
$tempRoot = Join-Path ([IO.Path]::GetTempPath()) ('stasis-local-hook-test-' + [Guid]::NewGuid().ToString('N'))
$repo = Join-Path $tempRoot 'game'
$bare = Join-Path $tempRoot 'remote.git'
$oldPath = $env:PATH
$oldMode = $env:STASIS_HOOK_TEST_MODE
$env:PATH = (Split-Path -Parent $pwshCommand.Source) + [IO.Path]::PathSeparator + (Split-Path -Parent $gitExe) + [IO.Path]::PathSeparator + $oldPath

function Invoke-Git {
    param([string] $Directory, [string[]] $Arguments, [int[]] $ExpectedExit = @(0))
    $output = @(& $gitExe -C $Directory @Arguments 2>&1 | ForEach-Object { [string]$_ })
    $exit = $LASTEXITCODE
    if ($exit -notin $ExpectedExit) { throw "git -C '$Directory' $($Arguments -join ' ') returned $exit; expected $($ExpectedExit -join ','): $($output -join [Environment]::NewLine)" }
    return [pscustomobject]@{ exit_code = $exit; output = $output }
}

function Invoke-Push {
    param([string[]] $Arguments, [int[]] $ExpectedExit = @(0), [string] $Mode = 'success')
    $env:STASIS_HOOK_TEST_MODE = $Mode
    $output = @(& $gitExe -C $repo push origin @Arguments 2>&1 | ForEach-Object { [string]$_ })
    $exit = $LASTEXITCODE
    if ($exit -notin $ExpectedExit) { throw "git push $($Arguments -join ' ') returned $exit; expected $($ExpectedExit -join ','): $($output -join [Environment]::NewLine)" }
    return [pscustomobject]@{ exit_code = $exit; output = $output }
}

function Get-CallCount {
    $countFile = Join-Path $repo 'build/hook-validator-calls.txt'
    if (-not (Test-Path -LiteralPath $countFile -PathType Leaf)) { return 0 }
    return @(Get-Content -LiteralPath $countFile).Count
}

try {
    New-Item -ItemType Directory -Path $repo, (Join-Path $repo '.githooks'), (Join-Path $repo 'tools') -Force | Out-Null
    New-Item -ItemType Directory -Path $bare -Force | Out-Null
    Invoke-Git $bare @('init', '--bare') | Out-Null
    Copy-Item -LiteralPath (Join-Path $templateRoot 'pre-push') -Destination (Join-Path $repo '.githooks/pre-push')
    Copy-Item -LiteralPath (Join-Path $templateRoot 'pre-push.ps1') -Destination (Join-Path $repo '.githooks/pre-push.ps1')
    $hookScriptPath = Join-Path $repo '.githooks/pre-push.ps1'
    $parseTokens = $null
    $parseErrors = $null
    $hookAst = [System.Management.Automation.Language.Parser]::ParseFile($hookScriptPath, [ref]$parseTokens, [ref]$parseErrors)
    if ($parseErrors.Count -gt 0) { throw "Could not parse generated pre-push script: $($parseErrors -join '; ')" }
    $timestampConverter = $hookAst.Find({
        param($node)
        $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Convert-ReceiptTimestamp'
    }, $true)
    if (-not $timestampConverter) { throw 'Pre-push timestamp conversion helper is missing.' }
    . ([scriptblock]::Create($timestampConverter.Extent.Text))
    $timestampThreshold = [DateTimeOffset]::Parse('2026-10-05T14:12:47.1234567+00:00', [Globalization.CultureInfo]::InvariantCulture, [Globalization.DateTimeStyles]::None)
    $freshTimestampJson = '{"started_at":"2026-10-05T10:12:47.1234568-04:00"}'
    $staleTimestampJson = '{"started_at":"2026-10-05T10:12:47.1234566-04:00"}'
    $freshTimestampReceipt = $freshTimestampJson | ConvertFrom-Json -ErrorAction Stop
    $staleTimestampReceipt = $staleTimestampJson | ConvertFrom-Json -ErrorAction Stop
    foreach ($receiptValue in @($freshTimestampReceipt.started_at, $staleTimestampReceipt.started_at)) {
        if ($receiptValue -isnot [DateTime] -and $receiptValue -isnot [DateTimeOffset] -and $receiptValue -isnot [string]) {
            throw "Unsupported ConvertFrom-Json timestamp value type: $($receiptValue.GetType().FullName)"
        }
    }
    $freshTimestamp = Convert-ReceiptTimestamp $freshTimestampReceipt.started_at
    $staleTimestamp = Convert-ReceiptTimestamp $staleTimestampReceipt.started_at
    $stringTimestamp = Convert-ReceiptTimestamp '2026-10-05T10:12:47.1234568-04:00'
    $offsetTimestamp = Convert-ReceiptTimestamp $timestampThreshold
    if ($freshTimestamp -le $timestampThreshold) { throw 'Fractional offset receipt timestamp lost its fresh tick during JSON conversion.' }
    if ($staleTimestamp -ge $timestampThreshold) { throw 'Fractional offset stale receipt timestamp was rounded forward.' }
    if ($stringTimestamp -ne $freshTimestamp) { throw 'String timestamp round trip changed its instant.' }
    if ($offsetTimestamp -ne $timestampThreshold) { throw 'DateTimeOffset timestamp round trip changed its instant.' }
    Set-Content -LiteralPath (Join-Path $repo '.gitignore') -Value "build/`n"
    Set-Content -LiteralPath (Join-Path $repo 'fake-stasis.exe') -Value 'hook identity fixture'
    [ordered]@{ vendor = @{ stasis = @{ release_id = 'nightly-20260929-747'; sha256 = 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'; hash_version = 2 } } } |
        ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $repo 'stasis.json') -Encoding utf8
    @'
[CmdletBinding()]
param([string] $ProjectRoot, [string] $GitPath, [string] $InvocationId, [switch] $Quiet)
$ErrorActionPreference = 'Stop'
$calls = Join-Path $ProjectRoot 'build/hook-validator-calls.txt'
New-Item -ItemType Directory -Path (Split-Path -Parent $calls) -Force | Out-Null
Add-Content -LiteralPath $calls -Value ([DateTimeOffset]::UtcNow.ToString('o'))
if ($env:STASIS_HOOK_TEST_MODE -eq 'fail') { exit 17 }
if ($env:STASIS_HOOK_TEST_MODE -eq 'dirty-after') { Set-Content -LiteralPath (Join-Path $ProjectRoot 'validation-created.txt') -Value 'dirty' }
if ($env:STASIS_HOOK_TEST_MODE -eq 'move-head') {
    & $GitPath -C $ProjectRoot reset --hard HEAD^ | Out-Null
    if ($LASTEXITCODE -ne 0) { exit 18 }
}
$head = (& $GitPath -C $ProjectRoot rev-parse HEAD).Trim()
$identityFile = Join-Path $ProjectRoot 'fake-stasis.exe'
$identitySha = (Get-FileHash -LiteralPath $identityFile -Algorithm SHA256).Hash.ToLowerInvariant()
$receiptRelease = if ($env:STASIS_HOOK_TEST_MODE -eq 'wrong-pin') { 'nightly-20260929-748' } else { 'nightly-20260929-747' }
$receiptStartedAt = [DateTimeOffset]::UtcNow
if ($env:STASIS_HOOK_TEST_MODE -eq 'stale-timestamp') { $receiptStartedAt = $receiptStartedAt.AddMinutes(-1) }
$receiptStartedAt = $receiptStartedAt.ToOffset([TimeSpan]::FromHours(-4))
$receiptRoot = Join-Path $ProjectRoot 'build/local-validation'
New-Item -ItemType Directory -Path $receiptRoot -Force | Out-Null
$receiptInvocation = if ($env:STASIS_HOOK_TEST_MODE -eq 'stale') { 'old-invocation' } else { $InvocationId }
$receipt = [ordered]@{
    ok = $true
    source_head = $head
    started_at = $receiptStartedAt.ToString('o')
    invocation_id = $receiptInvocation
    stasis = @{
        path = $identityFile
        binary_sha256 = $identitySha
        release_id = $receiptRelease
        vendor_sha256 = 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'
        hash_version = 2
    }
}
[IO.File]::WriteAllText((Join-Path $receiptRoot 'local-validation.json'), (($receipt | ConvertTo-Json -Depth 5) + [Environment]::NewLine), [Text.UTF8Encoding]::new($false))
'@ | Set-Content -LiteralPath (Join-Path $repo 'tools/local-validation.ps1') -Encoding utf8

    Invoke-Git $repo @('init', '--initial-branch=main') | Out-Null
    Invoke-Git $repo @('config', 'user.name', 'Local Hook Test') | Out-Null
    Invoke-Git $repo @('config', 'user.email', 'local-hook-test@example.invalid') | Out-Null
    Invoke-Git $repo @('config', 'stasis.gitExecutable', $gitExe) | Out-Null
    Invoke-Git $repo @('config', 'core.hooksPath', '.githooks') | Out-Null
    Invoke-Git $repo @('remote', 'add', 'origin', $bare) | Out-Null
    Set-Content -LiteralPath (Join-Path $repo 'README.md') -Value 'first'
    Invoke-Git $repo @('add', '--all') | Out-Null
    Invoke-Git $repo @('update-index', '--chmod=+x', '.githooks/pre-push') | Out-Null
    Invoke-Git $repo @('commit', '-m', 'first commit') | Out-Null
    $baseCommit = (Invoke-Git $repo @('rev-parse', 'HEAD')).output[0].Trim()

    Invoke-Push @('HEAD:refs/heads/main') @(0) 'fresh-fractional-offset' | Out-Null
    $firstCount = Get-CallCount
    if ($firstCount -ne 1) { throw "Expected one validator call after the first push, got $firstCount." }

    Add-Content -LiteralPath (Join-Path $repo 'README.md') -Value 'second'
    Invoke-Git $repo @('add', 'README.md') | Out-Null
    Invoke-Git $repo @('commit', '-m', 'second commit') | Out-Null
    $headCommit = (Invoke-Git $repo @('rev-parse', 'HEAD')).output[0].Trim()
    Invoke-Git $repo @('tag', '-a', 'release-1', '-m', 'annotated HEAD', $headCommit) | Out-Null
    Invoke-Push @('HEAD:refs/heads/main', 'refs/tags/release-1') | Out-Null
    if ((Get-CallCount) -ne 2) { throw 'A multiple-ref push matching HEAD did not invoke validation exactly once.' }

    Invoke-Push @("${baseCommit}:refs/heads/old") @(1) | Out-Null
    if ((Get-CallCount) -ne 2) { throw 'A non-HEAD ref incorrectly invoked local validation.' }
    Invoke-Push @('HEAD:refs/heads/mixed', "${baseCommit}:refs/heads/mixed-old") @(1) | Out-Null
    if ((Get-CallCount) -ne 2) { throw 'A mixed HEAD and non-HEAD push incorrectly ran validation.' }

    Invoke-Push @('HEAD:refs/heads/validator-failure') @(1) 'fail' | Out-Null
    if ((Get-CallCount) -ne 3) { throw 'Validator failure was not propagated by the hook.' }
    Invoke-Push @('HEAD:refs/heads/wrong-pin') @(1) 'wrong-pin' | Out-Null
    if ((Get-CallCount) -ne 4) { throw 'A validation receipt for a different pin was accepted.' }
    $staleTimestampPush = Invoke-Push @('HEAD:refs/heads/stale-timestamp') @(1) 'stale-timestamp'
    if ((Get-CallCount) -ne 5) { throw 'A stale timestamp was accepted or validation was not invoked.' }
    if (($staleTimestampPush.output -join [Environment]::NewLine) -notmatch 'started_at=') {
        throw 'The stale timestamp rejection did not report receipt freshness details.'
    }
    Invoke-Push @('HEAD:refs/heads/stale-receipt') @(1) 'stale' | Out-Null
    if ((Get-CallCount) -ne 6) { throw 'A stale invocation receipt was accepted or validation was not invoked.' }
    Invoke-Push @('HEAD:refs/heads/dirty-after') @(1) 'dirty-after' | Out-Null
    if ((Get-CallCount) -ne 7) { throw 'Post-validation dirty state was not checked.' }
    Remove-Item -LiteralPath (Join-Path $repo 'validation-created.txt') -Force

    Invoke-Push @('HEAD:refs/heads/head-moved') @(1) 'move-head' | Out-Null
    if ((Get-CallCount) -ne 8) { throw 'HEAD movement did not invoke validation.' }
    Invoke-Git $repo @('reset', '--hard', $headCommit) | Out-Null

    Set-Content -LiteralPath (Join-Path $repo 'dirty-before.txt') -Value 'dirty'
    Invoke-Push @('HEAD:refs/heads/dirty-before') @(1) | Out-Null
    if ((Get-CallCount) -ne 8) { throw 'The hook ran validation before rejecting a dirty starting tree.' }

    $blobInput = Join-Path $tempRoot 'blob.txt'
    Set-Content -LiteralPath $blobInput -Value 'not a commit'
    $blob = (Invoke-Git $repo @('hash-object', '-w', $blobInput)).output[0].Trim()
    Invoke-Git $repo @('tag', '-a', 'blob-tag', '-m', 'tag to blob', $blob) | Out-Null
    Invoke-Push @('refs/tags/blob-tag') @(1) | Out-Null
    if ((Get-CallCount) -ne 8) { throw 'An annotated tag that does not peel to HEAD ran validation.' }

    $env:STASIS_HOOK_TEST_MODE = 'success'
    Invoke-Git $repo @('push', 'origin', ':refs/heads/main') | Out-Null
    if ((Get-CallCount) -ne 8) { throw 'A deletion-only push invoked validation.' }
    Write-Output 'PASS: 13 real Git pushes verified exact-HEAD, fractional timezone receipt freshness, stale timestamp/invocation rejection, wrong pins, clean-tree enforcement, and failure propagation; timestamp helper passed fresh/stale/string round-trip probes.'
} finally {
    $env:PATH = $oldPath
    if ($null -eq $oldMode) { Remove-Item Env:STASIS_HOOK_TEST_MODE -ErrorAction SilentlyContinue } else { $env:STASIS_HOOK_TEST_MODE = $oldMode }
    if (Test-Path -LiteralPath $tempRoot) {
        $entries = @(Get-ChildItem -LiteralPath $tempRoot -Recurse -Force)
        if ($entries | Where-Object { ($_.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 }) {
            throw "Test temporary tree contains a reparse point; leaving it in place: $tempRoot"
        }
        Remove-Item -LiteralPath $tempRoot -Recurse -Force
    }
}
