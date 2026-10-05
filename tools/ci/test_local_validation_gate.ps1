[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$repoRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$templateRoot = Join-Path $repoRoot 'apps/stasis/templates/local-validation'
$gitCommand = Get-Command git -CommandType Application -ErrorAction Stop | Select-Object -First 1
$powerShellCommand = Get-Command pwsh -CommandType Application -ErrorAction Stop | Select-Object -First 1
$gitExe = [IO.Path]::GetFullPath($gitCommand.Source)
$pwshExe = [IO.Path]::GetFullPath($powerShellCommand.Source)
$tempRoot = Join-Path ([IO.Path]::GetTempPath()) ('stasis-local-gate-test-' + [Guid]::NewGuid().ToString('N'))
$project = Join-Path $tempRoot 'game'
$outside = Join-Path $tempRoot 'outside'
$oldStasis = $env:STASIS_BIN
$oldGit = $env:STASIS_GIT_EXE
$oldProject = $env:STASIS_FIXTURE_PROJECT
$oldFailure = $env:STASIS_FIXTURE_FAIL_STAGE
$oldWrongPin = $env:STASIS_FIXTURE_WRONG_PIN
$oldLinkTarget = $env:STASIS_FIXTURE_LINK_TARGET
$oldStderrDiagnostic = $env:STASIS_FIXTURE_STDERR_DIAGNOSTIC
$oldFixtureRelease = $env:STASIS_FIXTURE_RELEASE
$oldFixtureVendorSha = $env:STASIS_FIXTURE_VENDOR_SHA
$oldVendorStatusMode = $env:STASIS_FIXTURE_VENDOR_STATUS_MODE
$release = 'nightly-20261005-816'
$vendorSha = 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'
$sourceCommit = '99a5b3943f1759b7abbdbf0e37634965749c1337'

function Invoke-Git {
    param([string] $Directory, [string[]] $Arguments, [int[]] $ExpectedExit = @(0))
    $output = @(& $gitExe -C $Directory @Arguments 2>&1 | ForEach-Object { [string]$_ })
    $exitCode = $LASTEXITCODE
    if ($exitCode -notin $ExpectedExit) {
        throw "git -C '$Directory' $($Arguments -join ' ') returned $exitCode; expected $($ExpectedExit -join ','): $($output -join [Environment]::NewLine)"
    }
    return [pscustomobject]@{ exit_code = $exitCode; output = $output }
}

function Write-JsonFile {
    param([string] $Path, $Value)
    [IO.File]::WriteAllText($Path, (($Value | ConvertTo-Json -Depth 12) + [Environment]::NewLine), [Text.UTF8Encoding]::new($false))
}

function Invoke-Gate {
    param([string] $Label, [int[]] $ExpectedExit = @(0), [switch] $SkipReceiptRead)
    $invocation = [Guid]::NewGuid().ToString('N')
    $entrypoint = Join-Path $project 'tools/local-validation.ps1'
    $output = @(& $pwshExe -NoProfile -NonInteractive -File $entrypoint -ProjectRoot $project -InvocationId $invocation -Quiet 2>&1 | ForEach-Object { [string]$_ })
    $exitCode = $LASTEXITCODE
    if ($exitCode -notin $ExpectedExit) {
        throw "local-validation $Label returned $exitCode; expected $($ExpectedExit -join ','): $($output | Select-Object -Last 40 | Out-String)"
    }
    $receiptPath = Join-Path $project 'build/local-validation/local-validation.json'
    $receipt = if (-not $SkipReceiptRead -and (Test-Path -LiteralPath $receiptPath -PathType Leaf)) {
        Get-Content -LiteralPath $receiptPath -Raw | ConvertFrom-Json -ErrorAction Stop
    } else { $null }
    return [pscustomobject]@{ exit_code = $exitCode; output = $output; receipt = $receipt; invocation_id = $invocation }
}

function Invoke-Resolver {
    param([string[]] $ExtraArguments = @(), [int[]] $ExpectedExit = @(0))
    $resolver = Join-Path $project 'tools/resolve-pinned-stasis.ps1'
    $output = @(& $pwshExe -NoProfile -NonInteractive -File $resolver -ProjectRoot $project -GitPath $gitExe @ExtraArguments 2>&1 | ForEach-Object { [string]$_ })
    $exitCode = $LASTEXITCODE
    if ($exitCode -notin $ExpectedExit) {
        throw "Pinned Stasis resolver returned $exitCode; expected $($ExpectedExit -join ','): $($output | Select-Object -Last 20 | Out-String)"
    }
    return [pscustomobject]@{ exit_code = $exitCode; output = $output }
}

function Assert-Step {
    param($Receipt, [string] $Name, [bool] $ExpectedOk, [bool] $ExpectedSkipped = $false)
    if ($null -eq $Receipt) { throw "Expected a receipt for step '$Name'." }
    $step = @($Receipt.steps | Where-Object { $_.name -eq $Name }) | Select-Object -First 1
    if ($null -eq $step -or [bool]$step.ok -ne $ExpectedOk -or [bool]$step.skipped -ne $ExpectedSkipped) {
        throw "Unexpected '$Name' result; expected ok=$ExpectedOk skipped=$ExpectedSkipped. Receipt: $($Receipt | ConvertTo-Json -Depth 10 -Compress)"
    }
}

function Remove-SafeTestTree {
    param([string] $Path)
    if (-not (Test-Path -LiteralPath $Path)) { return }
    $pending = [System.Collections.Generic.Stack[string]]::new()
    $entries = [System.Collections.Generic.List[object]]::new()
    $pending.Push($Path)
    while ($pending.Count -gt 0) {
        $directory = $pending.Pop()
        foreach ($item in Get-ChildItem -LiteralPath $directory -Force -ErrorAction Stop) {
            if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
                $entries.Add($item)
            } elseif ($item.PSIsContainer) {
                $entries.Add($item)
                $pending.Push($item.FullName)
            } else {
                $entries.Add($item)
            }
        }
    }
    foreach ($item in ($entries | Sort-Object { $_.FullName.Length } -Descending)) {
        if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
            Remove-Item -LiteralPath $item.FullName -Force
        }
    }
    Remove-Item -LiteralPath $Path -Recurse -Force
}

try {
    New-Item -ItemType Directory -Path $project, (Join-Path $project 'tools'), (Join-Path $project 'src'), (Join-Path $project 'tests'), (Join-Path $project 'vendor/stasis'), $outside -Force | Out-Null
    foreach ($name in @('local-validation.ps1', 'resolve-pinned-stasis.ps1')) {
        Copy-Item -LiteralPath (Join-Path $templateRoot $name) -Destination (Join-Path $project 'tools' $name)
    }
    $manifest = @{
        name = 'local_gate_fixture'
        vendor = @{ stasis = @{ release_id = $release; sha256 = $vendorSha; hash_version = 2 } }
    }
    Write-JsonFile (Join-Path $project 'stasis.json') $manifest
    $originalManifestContent = [IO.File]::ReadAllText((Join-Path $project 'stasis.json'))
    Set-Content -LiteralPath (Join-Path $project 'src/main.stasis') -Value 'function main(): i32 { return 0; }'
    Set-Content -LiteralPath (Join-Path $project 'tests/main.test.stasis') -Value 'test `fixture`(): bool { return true; }'
    Set-Content -LiteralPath (Join-Path $project '.gitignore') -Value "build/`ntools/local-validation.json`n"
    $fakeStasis = Join-Path $project 'tools/fake-stasis.ps1'
    @'
param([Parameter(ValueFromRemainingArguments = $true)][string[]] $ToolArgs)
$ErrorActionPreference = 'Stop'
$global:LASTEXITCODE = 0
$release = if ($env:STASIS_FIXTURE_RELEASE) { $env:STASIS_FIXTURE_RELEASE } else { 'nightly-20261005-816' }
$vendorSha = if ($env:STASIS_FIXTURE_VENDOR_SHA) { $env:STASIS_FIXTURE_VENDOR_SHA } else { 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' }
$sourceCommit = '99a5b3943f1759b7abbdbf0e37634965749c1337'
if ($env:STASIS_FIXTURE_STDERR_DIAGNOSTIC -eq 'true') { Write-Error 'cache_cleanup_removed_files=0 cache_cleanup_removed_dirs=1 cache_cleanup_ttl_days=7' -ErrorAction Continue }
$manifestPath = Join-Path $env:STASIS_FIXTURE_PROJECT 'stasis.json'
$manifestSha = (Get-FileHash -LiteralPath $manifestPath -Algorithm SHA256).Hash.ToLowerInvariant()
$binarySha = (Get-FileHash -LiteralPath $PSCommandPath -Algorithm SHA256).Hash.ToLowerInvariant()
if ($ToolArgs -contains 'editor-info') {
    $reportedRelease = if ($env:STASIS_FIXTURE_WRONG_PIN -eq 'true') { 'nightly-20261004-815' } else { $release }
    [ordered]@{ ok = $true; result = @{ release_id = $reportedRelease; source_commit = $sourceCommit; build_fingerprint = 'fixture-fingerprint'; executable = @{ sha256 = $binarySha } } } | ConvertTo-Json -Depth 6 -Compress
    return
}
if ($ToolArgs -contains 'vendor') {
    if ($env:STASIS_FIXTURE_VENDOR_STATUS_MODE -eq 'malformed') { Write-Output '{"ok":'; return }
    $vendorMutated = Test-Path -LiteralPath (Join-Path $env:STASIS_FIXTURE_PROJECT 'vendor/stasis/mutated.txt') -PathType Leaf
    $dirtyStatus = $env:STASIS_FIXTURE_VENDOR_STATUS_MODE -eq 'dirty'
    $wrongRelease = $env:STASIS_FIXTURE_VENDOR_STATUS_MODE -eq 'wrong-release'
    $wrongHash = $env:STASIS_FIXTURE_VENDOR_STATUS_MODE -eq 'wrong-hash'
    $stringBoolean = $env:STASIS_FIXTURE_VENDOR_STATUS_MODE -eq 'string-boolean'
    $floatHashVersion = $env:STASIS_FIXTURE_VENDOR_STATUS_MODE -eq 'float-hash-version'
    $actualSha = if ($vendorMutated -or $dirtyStatus -or $wrongHash) { 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb' } else { $vendorSha }
    $installedRelease = if ($wrongRelease) { 'local-mismatched' } else { $release }
    $installedSha = if ($wrongHash) { $actualSha } else { $vendorSha }
    $installed = @{ release_id = $installedRelease; sha256 = $installedSha; hash_version = 2 }
    $recorded = @{ release_id = $release; sha256 = $vendorSha; hash_version = 2 }
    $result = @{ current = -not ($vendorMutated -or $dirtyStatus); update_available = $dirtyStatus; pin_verified = -not ($vendorMutated -or $dirtyStatus); legacy_pin_unverified = $false; local_changes = $vendorMutated -or $dirtyStatus; actual_hash_version = 2; expected_hash_version = 2; recorded_hash_version = 2; installed = $installed; recorded = $recorded; recorded_sha256 = $vendorSha; expected_sha256 = $vendorSha; actual_sha256 = $actualSha }
    if ($stringBoolean) { $result.current = 'true' }
    if ($floatHashVersion) { $result.actual_hash_version = [double]2.0 }
    [ordered]@{ ok = $true; command = 'vendor'; result = $result } | ConvertTo-Json -Depth 8 -Compress
    return
}
if ($ToolArgs -contains '--fixture-suite') {
    $releaseIndex = [Array]::IndexOf([string[]]$ToolArgs, '--fixture-release')
    $vendorIndex = [Array]::IndexOf([string[]]$ToolArgs, '--fixture-vendor')
    if ($releaseIndex -lt 0 -or $vendorIndex -lt 0 -or
        $releaseIndex + 1 -ge $ToolArgs.Count -or $vendorIndex + 1 -ge $ToolArgs.Count -or
        $ToolArgs[$releaseIndex + 1] -cne $release -or $ToolArgs[$vendorIndex + 1] -cne $vendorSha) {
        $global:LASTEXITCODE = 20
        Write-Output 'configured test did not receive the selected Stasis pin identity'
        return
    }
    if ($env:STASIS_FIXTURE_FAIL_STAGE -eq 'configured-test') { $global:LASTEXITCODE = 21; Write-Output 'fixture configured test failure'; return }
    Write-Output 'configured test pin arguments verified'
    return
}
if ($ToolArgs -contains 'test' -and $env:STASIS_FIXTURE_FAIL_STAGE -eq 'test') { $global:LASTEXITCODE = 17; Write-Output 'fixture test failure'; return }
if ($ToolArgs -contains 'package') {
    if ($env:STASIS_FIXTURE_FAIL_STAGE -eq 'package') { $global:LASTEXITCODE = 18; Write-Output 'fixture package failure'; return }
    $outIndex = [Array]::IndexOf([string[]]$ToolArgs, '--out')
    if ($outIndex -lt 0 -or $outIndex + 1 -ge $ToolArgs.Count) { $global:LASTEXITCODE = 19; return }
    $output = Join-Path $env:STASIS_FIXTURE_PROJECT $ToolArgs[$outIndex + 1]
    New-Item -ItemType Directory -Path $output -Force | Out-Null
    Set-Content -LiteralPath (Join-Path $output 'index.html') -Value '<!doctype html>'
    [IO.File]::WriteAllBytes((Join-Path $output 'game.wasm'), [byte[]](0, 97, 115, 109, 1, 0, 0, 0))
    $vendor = @{ release_id = $release; recorded_sha256 = $vendorSha; actual_sha256 = $vendorSha; recorded_hash_version = 2; actual_hash_version = 2 }
    $provenance = @{ schema = 'stasis.release_provenance.v1'; release_tag = $release; source_commit = $sourceCommit; compiler = @{ sha256 = $binarySha }; development_build = $false; dirty_state = $false; web_package = @{ project = @{ manifest = @{ sha256 = $manifestSha }; vendor = $vendor } } }
    $provenance | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath (Join-Path $output 'stasis_provenance.json') -Encoding utf8
    if ($env:STASIS_FIXTURE_FAIL_STAGE -eq 'package-link') {
        New-Item -ItemType Junction -Path (Join-Path $output 'linked-outside') -Target $env:STASIS_FIXTURE_LINK_TARGET | Out-Null
    }
    return
}
return
'@ | Set-Content -LiteralPath $fakeStasis -Encoding utf8
    Set-Content -LiteralPath (Join-Path $outside 'sentinel.txt') -Value 'keep this target untouched'
    $configPath = Join-Path $project 'tools/local-validation.json'
    $config = @{ schema_version = 1; deterministic_tests = @(); stasis_tests = $null; web_package = $null; after_web_package = @(); restore_toolchain = $null }
    Write-JsonFile $configPath $config

    Invoke-Git $project @('init', '--initial-branch=main') | Out-Null
    Invoke-Git $project @('config', 'user.name', 'Local Gate Test') | Out-Null
    Invoke-Git $project @('config', 'user.email', 'local-gate-test@example.invalid') | Out-Null
    Invoke-Git $project @('config', 'stasis.gitExecutable', $gitExe) | Out-Null
    Invoke-Git $project @('add', '--all') | Out-Null
    Invoke-Git $project @('commit', '-m', 'create validation fixture') | Out-Null
    $env:STASIS_BIN = $fakeStasis
    $env:STASIS_GIT_EXE = $gitExe
    $env:STASIS_FIXTURE_PROJECT = $project
    $env:STASIS_FIXTURE_FAIL_STAGE = ''
    $env:STASIS_FIXTURE_WRONG_PIN = 'false'
    $env:STASIS_FIXTURE_LINK_TARGET = $outside
    $env:STASIS_FIXTURE_STDERR_DIAGNOSTIC = 'true'

    $developmentSha = 'cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc'
    foreach ($localReleaseId in @('development', 'local-format-fixture')) {
        $env:STASIS_FIXTURE_RELEASE = $localReleaseId
        $env:STASIS_FIXTURE_VENDOR_SHA = $developmentSha
        $env:STASIS_FIXTURE_VENDOR_STATUS_MODE = ''
        $manifest.vendor.stasis.release_id = $localReleaseId
        $manifest.vendor.stasis.sha256 = $developmentSha
        Write-JsonFile (Join-Path $project 'stasis.json') $manifest

        $localFormat = Invoke-Resolver @('-LocalFormatOnly')
        $localFormatIdentity = ($localFormat.output -join [Environment]::NewLine) | ConvertFrom-Json -ErrorAction Stop
        if ($localFormatIdentity.validation_scope -cne 'vendor-match-format-only' -or
            $localFormatIdentity.release_id -cne $localReleaseId -or
            $localFormatIdentity.vendor_sha256 -cne $developmentSha -or
            $localFormatIdentity.hash_version -ne 2 -or
            $localFormatIdentity.binary_sha256 -notmatch '^[0-9a-f]{64}$' -or
            $null -ne $localFormatIdentity.source_commit -or $null -ne $localFormatIdentity.build_fingerprint) {
            throw "The local-format resolver did not return a vendor-matched format-only identity for '$localReleaseId'."
        }
    }

    $env:STASIS_FIXTURE_RELEASE = 'development'
    $manifest.vendor.stasis.release_id = 'development'
    Write-JsonFile (Join-Path $project 'stasis.json') $manifest
    $strictDevelopment = Invoke-Resolver @() @(1)
    if (-not (($strictDevelopment.output -join [Environment]::NewLine).Contains('immutable nightly release'))) {
        throw 'The default resolver accepted or misreported a development pin.'
    }
    $strictDevelopmentGate = Invoke-Gate 'development pin stays rejected by the full gate' @(1)
    if ($strictDevelopmentGate.receipt.ok) { throw 'The full local gate accepted a development-only formatting identity.' }
    Assert-Step $strictDevelopmentGate.receipt 'validation-setup' $false

    $manifest.vendor.stasis.hash_version = '2'
    Write-JsonFile (Join-Path $project 'stasis.json') $manifest
    $stringManifestVersion = Invoke-Resolver @('-LocalFormatOnly') @(1)
    if (-not (($stringManifestVersion.output -join [Environment]::NewLine).Contains('hash_version must be the integer 2'))) {
        throw 'The local-format resolver accepted or misreported a string manifest hash version.'
    }
    $manifest.vendor.stasis.hash_version = 2
    Write-JsonFile (Join-Path $project 'stasis.json') $manifest

    foreach ($mode in @('wrong-release', 'wrong-hash', 'dirty', 'string-boolean', 'float-hash-version', 'malformed')) {
        $env:STASIS_FIXTURE_VENDOR_STATUS_MODE = $mode
        $invalidLocalFormat = Invoke-Resolver @('-LocalFormatOnly') @(1)
        $expectedDiagnostic = switch ($mode) {
            'malformed' { 'returned invalid JSON' }
            'string-boolean' { 'strict boolean identity fields' }
            'float-hash-version' { 'integer hash-version fields' }
            default { 'does not prove exact pin fidelity' }
        }
        if (-not (($invalidLocalFormat.output -join [Environment]::NewLine).Contains($expectedDiagnostic))) {
            throw "The local-format resolver did not report the expected rejection for '$mode'."
        }
    }

    $env:STASIS_FIXTURE_VENDOR_STATUS_MODE = ''
    $env:STASIS_FIXTURE_RELEASE = $release
    $env:STASIS_FIXTURE_VENDOR_SHA = $vendorSha
    [IO.File]::WriteAllText((Join-Path $project 'stasis.json'), $originalManifestContent, [Text.UTF8Encoding]::new($false))
    $cleanAfterResolverFixtures = Invoke-Git $project @('diff', '--quiet') @(0, 1)
    if ($cleanAfterResolverFixtures.exit_code -ne 0) { throw 'The local-format resolver fixtures left stasis.json modified.' }

    $success = Invoke-Gate 'success control'
    if (-not $success.receipt.ok -or $success.receipt.invocation_id -cne $success.invocation_id -or $success.receipt.web_package.wasm_files.Count -ne 1) {
        throw 'The real local-validation entrypoint did not accept the fixture control package or emit fresh evidence.'
    }
    Assert-Step $success.receipt 'test' $true
    $defaultTest = @($success.receipt.steps | Where-Object { $_.name -eq 'test' }) | Select-Object -First 1
    if ($defaultTest.command -cne $fakeStasis -or @($defaultTest.arguments).Count -ne 1 -or $defaultTest.arguments[0] -cne 'test') {
        throw 'An absent stasis_tests configuration did not retain the default pinned Stasis test command.'
    }
    if (Test-Path -LiteralPath (Join-Path $project $success.receipt.web_output)) { throw 'The successful fixture Web output was not removed after evidence capture.' }
    Assert-Step $success.receipt 'toolchain-identity-stable' $true
    Assert-Step $success.receipt 'source-head-stable' $true

    [IO.File]::WriteAllText((Join-Path $project 'src/staged-whitespace.stasis'), "staged content `t`n", [Text.UTF8Encoding]::new($false))
    Invoke-Git $project @('add', 'src/staged-whitespace.stasis') | Out-Null
    $stagedWhitespace = Invoke-Gate 'staged whitespace' @(1)
    Assert-Step $stagedWhitespace.receipt 'git-diff-check' $false
    if (-not ([string](@($stagedWhitespace.receipt.steps | Where-Object { $_.name -eq 'git-diff-check' })[0].output_tail).Contains('trailing whitespace'))) {
        throw 'The git diff check did not report staged whitespace.'
    }
    Invoke-Git $project @('reset', '--hard', 'HEAD') | Out-Null
    if (Test-Path -LiteralPath (Join-Path $project 'src/staged-whitespace.stasis')) { Remove-Item -LiteralPath (Join-Path $project 'src/staged-whitespace.stasis') -Force }

    $env:STASIS_FIXTURE_WRONG_PIN = 'true'
    $wrongPin = Invoke-Gate 'wrong pin' @(1)
    if ($wrongPin.receipt.ok) { throw 'The real local-validation entrypoint accepted a toolchain with the wrong release pin.' }
    Assert-Step $wrongPin.receipt 'validation-setup' $false
    $env:STASIS_FIXTURE_WRONG_PIN = 'false'

    $config.stasis_tests = @{ command = '{stasis}'; args = @('test', '--fixture-suite', '--fixture-release', '{release_id}', '--fixture-vendor', '{vendor_sha256}') }
    Write-JsonFile $configPath $config
    $configuredTest = Invoke-Gate 'configured Stasis test'
    if (-not $configuredTest.receipt.ok) { throw 'The real local-validation entrypoint rejected a configured Stasis test command using the selected pin.' }
    Assert-Step $configuredTest.receipt 'test' $true
    $configuredTestStep = @($configuredTest.receipt.steps | Where-Object { $_.name -eq 'test' }) | Select-Object -First 1
    if ($configuredTestStep.command -cne $fakeStasis -or -not ([string]$configuredTestStep.output_tail).Contains('configured test pin arguments verified')) {
        throw 'The configured Stasis test did not resolve {stasis} or pass the selected release and vendor identity.'
    }

    $env:STASIS_FIXTURE_FAIL_STAGE = 'configured-test'
    $configuredTestFailure = Invoke-Gate 'configured Stasis test failure' @(1)
    if ($configuredTestFailure.receipt.ok) { throw 'The real local-validation entrypoint accepted a failed configured Stasis test.' }
    Assert-Step $configuredTestFailure.receipt 'test' $false
    Assert-Step $configuredTestFailure.receipt 'web-package' $false $true
    if (-not (($configuredTestFailure.output -join [Environment]::NewLine).Contains('fixture configured test failure'))) {
        throw 'Quiet failure output omitted the configured Stasis test diagnostic.'
    }

    $env:STASIS_FIXTURE_FAIL_STAGE = ''
    $config.stasis_tests = @{ command = '{stasis}'; args = 'test' }
    Write-JsonFile $configPath $config
    $badConfiguredTests = Invoke-Gate 'malformed configured Stasis tests' @(1)
    if ($badConfiguredTests.receipt.ok) { throw 'The real local-validation entrypoint accepted malformed stasis_tests arguments.' }
    Assert-Step $badConfiguredTests.receipt 'validation-setup' $false
    if (-not ([string](@($badConfiguredTests.receipt.steps | Where-Object { $_.name -eq 'validation-setup' })[0].output_tail).Contains('stasis_tests must have command and args array fields'))) {
        throw 'Malformed stasis_tests configuration did not report its schema error.'
    }
    $config.stasis_tests = $null
    Write-JsonFile $configPath $config

    $env:STASIS_FIXTURE_FAIL_STAGE = 'test'
    $testFailure = Invoke-Gate 'test failure' @(1)
    if ($testFailure.receipt.ok) { throw 'The real local-validation entrypoint accepted a failed required test command.' }
    Assert-Step $testFailure.receipt 'test' $false
    Assert-Step $testFailure.receipt 'web-package' $false $true
    if (-not (($testFailure.output -join [Environment]::NewLine).Contains('fixture test failure'))) { throw 'Quiet failure output omitted the required test diagnostic.' }

    $env:STASIS_FIXTURE_FAIL_STAGE = 'package'
    $packageFailure = Invoke-Gate 'package failure' @(1)
    if ($packageFailure.receipt.ok) { throw 'The real local-validation entrypoint accepted a failed fresh package command.' }
    Assert-Step $packageFailure.receipt 'web-package' $false

    $callback = Join-Path $project 'tools/fail-callback.ps1'
    Set-Content -LiteralPath $callback -Value "throw 'configured callback failure'" -Encoding utf8
    $env:STASIS_FIXTURE_FAIL_STAGE = ''
    $config.after_web_package = @(@{ name = 'configured-callback'; command = 'pwsh'; args = @('-NoProfile', '-NonInteractive', '-File', 'tools/fail-callback.ps1', '{web_output}') })
    Write-JsonFile $configPath $config
    $callbackFailure = Invoke-Gate 'configured callback failure' @(1)
    if ($callbackFailure.receipt.ok) { throw 'The real local-validation entrypoint accepted a failed configured post-package callback.' }
    Assert-Step $callbackFailure.receipt 'configured-callback' $false
    if (-not (($callbackFailure.output -join [Environment]::NewLine).Contains('configured callback failure'))) { throw 'Quiet failure output omitted the configured callback diagnostic.' }
    if (Test-Path -LiteralPath (Join-Path $project $callbackFailure.receipt.web_output)) { throw 'The callback-failure fixture output was not cleaned up safely.' }

    $mutator = Join-Path $project 'tools/mutate-vendor.ps1'
    @'
$mutationPath = Join-Path $env:STASIS_FIXTURE_PROJECT 'vendor/stasis/mutated.txt'
$ErrorActionPreference = 'Stop'
Set-Content -LiteralPath $mutationPath -Value 'mutated'
'@ | Set-Content -LiteralPath $mutator -Encoding utf8
    $config.after_web_package = @(@{ name = 'mutate-vendor-snapshot'; command = 'pwsh'; args = @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', 'tools/mutate-vendor.ps1', '{web_output}') })
    Write-JsonFile $configPath $config
    $vendorMutation = Invoke-Gate 'post-callback vendor mutation' @(1)
    if ($vendorMutation.receipt.ok) { throw 'The local gate accepted a callback that changed vendor files after packaging.' }
    Assert-Step $vendorMutation.receipt 'toolchain-identity-stable' $false
    if (-not (($vendorMutation.output -join [Environment]::NewLine).Contains('exact pin fidelity'))) {
        throw 'The final toolchain check did not report the post-callback vendor mutation.'
    }
    Remove-Item -LiteralPath (Join-Path $project 'vendor/stasis/mutated.txt') -Force

    $config.after_web_package = @()
    Write-JsonFile $configPath $config
    $env:STASIS_FIXTURE_FAIL_STAGE = 'package-link'
    $linkedOutput = Invoke-Gate 'linked output' @(1)
    if ($linkedOutput.receipt.ok) { throw 'The real local-validation entrypoint accepted a linked directory inside fresh output.' }
    if (-not (Test-Path -LiteralPath (Join-Path $outside 'sentinel.txt') -PathType Leaf)) { throw 'Linked output handling removed or altered the external target.' }
    $linkedError = @($linkedOutput.receipt.steps | Where-Object { $_.name -eq 'validation-setup' }) | Select-Object -First 1
    if ($null -eq $linkedError -or -not ([string]$linkedError.output_tail).Contains('Fresh Web package contains a reparse point')) {
        throw 'The linked output was not rejected during provenance inspection.'
    }
    $linkedPath = Join-Path $project $linkedOutput.receipt.web_output
    $junctionPath = Join-Path $linkedPath 'linked-outside'
    if (Test-Path -LiteralPath $junctionPath) { Remove-Item -LiteralPath $junctionPath -Force }
    if (Test-Path -LiteralPath $linkedPath) { Remove-Item -LiteralPath $linkedPath -Recurse -Force }

    $receiptTarget = Join-Path $outside 'receipt-target.json'
    $summaryTarget = Join-Path $outside 'summary-target.md'
    Set-Content -LiteralPath $receiptTarget -Value 'keep receipt target' -NoNewline
    Set-Content -LiteralPath $summaryTarget -Value 'keep summary target' -NoNewline
    $receiptPath = Join-Path $project 'build/local-validation/local-validation.json'
    if (Test-Path -LiteralPath $receiptPath) { Remove-Item -LiteralPath $receiptPath -Force }
    New-Item -ItemType SymbolicLink -Path $receiptPath -Target $receiptTarget | Out-Null
    $linkedReceipt = Invoke-Gate 'linked receipt' @(1) -SkipReceiptRead
    if ((Test-Path -LiteralPath $receiptTarget -PathType Leaf) -and (Get-Content -LiteralPath $receiptTarget -Raw) -ne 'keep receipt target') {
        throw 'Local validation followed a linked receipt path and changed its target.'
    }
    Remove-Item -LiteralPath $receiptPath -Force

    $summaryPath = Join-Path $project 'build/local-validation/local-validation.md'
    if (Test-Path -LiteralPath $summaryPath) { Remove-Item -LiteralPath $summaryPath -Force }
    New-Item -ItemType SymbolicLink -Path $summaryPath -Target $summaryTarget | Out-Null
    $linkedSummary = Invoke-Gate 'linked summary' @(1)
    if ((Get-Content -LiteralPath $summaryTarget -Raw) -ne 'keep summary target') {
        throw 'Local validation followed a linked summary path and changed its target.'
    }
    Remove-Item -LiteralPath $summaryPath -Force

    $env:STASIS_FIXTURE_FAIL_STAGE = ''
    $buildRoot = Join-Path $project 'build'
    if (Test-Path -LiteralPath $buildRoot) { Remove-Item -LiteralPath $buildRoot -Recurse -Force }
    $linkedParent = Join-Path $project 'build'
    New-Item -ItemType Junction -Path $linkedParent -Target $outside | Out-Null
    $parentFailure = Invoke-Gate 'linked output parent' @(1)
    if ($parentFailure.receipt) { throw 'The linked output-parent fixture unexpectedly wrote its receipt through the junction.' }
    if (-not (Test-Path -LiteralPath (Join-Path $outside 'sentinel.txt') -PathType Leaf)) { throw 'Linked output-parent handling removed or altered the external target.' }
    Remove-Item -LiteralPath $linkedParent -Force
    Write-Output 'PASS: the real local-validation entrypoint rejected wrong pins, failed required commands/callbacks, and linked output paths; fixture success verifies evidence flow only, not compiler acceptance.'
} finally {
    if ($null -eq $oldStasis) { Remove-Item Env:STASIS_BIN -ErrorAction SilentlyContinue } else { $env:STASIS_BIN = $oldStasis }
    if ($null -eq $oldGit) { Remove-Item Env:STASIS_GIT_EXE -ErrorAction SilentlyContinue } else { $env:STASIS_GIT_EXE = $oldGit }
    if ($null -eq $oldProject) { Remove-Item Env:STASIS_FIXTURE_PROJECT -ErrorAction SilentlyContinue } else { $env:STASIS_FIXTURE_PROJECT = $oldProject }
    if ($null -eq $oldFailure) { Remove-Item Env:STASIS_FIXTURE_FAIL_STAGE -ErrorAction SilentlyContinue } else { $env:STASIS_FIXTURE_FAIL_STAGE = $oldFailure }
    if ($null -eq $oldWrongPin) { Remove-Item Env:STASIS_FIXTURE_WRONG_PIN -ErrorAction SilentlyContinue } else { $env:STASIS_FIXTURE_WRONG_PIN = $oldWrongPin }
    if ($null -eq $oldLinkTarget) { Remove-Item Env:STASIS_FIXTURE_LINK_TARGET -ErrorAction SilentlyContinue } else { $env:STASIS_FIXTURE_LINK_TARGET = $oldLinkTarget }
    if ($null -eq $oldStderrDiagnostic) { Remove-Item Env:STASIS_FIXTURE_STDERR_DIAGNOSTIC -ErrorAction SilentlyContinue } else { $env:STASIS_FIXTURE_STDERR_DIAGNOSTIC = $oldStderrDiagnostic }
    if ($null -eq $oldFixtureRelease) { Remove-Item Env:STASIS_FIXTURE_RELEASE -ErrorAction SilentlyContinue } else { $env:STASIS_FIXTURE_RELEASE = $oldFixtureRelease }
    if ($null -eq $oldFixtureVendorSha) { Remove-Item Env:STASIS_FIXTURE_VENDOR_SHA -ErrorAction SilentlyContinue } else { $env:STASIS_FIXTURE_VENDOR_SHA = $oldFixtureVendorSha }
    if ($null -eq $oldVendorStatusMode) { Remove-Item Env:STASIS_FIXTURE_VENDOR_STATUS_MODE -ErrorAction SilentlyContinue } else { $env:STASIS_FIXTURE_VENDOR_STATUS_MODE = $oldVendorStatusMode }
    if (Test-Path -LiteralPath $tempRoot) { Remove-SafeTestTree $tempRoot }
}
