[CmdletBinding()]
param(
    [Parameter(Position = 0)]
    [ValidateSet('status', 'provision', 'sign', 'verify')]
    [string] $Command = 'status',
    [string[]] $Artifact,
    [string] $Tool,
    [string] $Certificate,
    [string] $Thumbprint,
    [string] $TimestampUrl
)

$ErrorActionPreference = 'Stop'
$developmentSubject = 'CN=StasisLang Development Signing'
$codeSigningOid = '1.3.6.1.5.5.7.3.3'

function Test-ProductionMode {
    return ($env:STASIS_SIGNING_MODE -eq 'production' -or $env:STASIS_SIGNING_PROFILE -eq 'production')
}

function Get-LocalRecordPath {
    if ($env:STASIS_SIGNING_LOCAL_RECORD) { return $env:STASIS_SIGNING_LOCAL_RECORD }
    if ($env:LOCALAPPDATA) { return (Join-Path $env:LOCALAPPDATA 'Stasis\signing\development-thumbprint.txt') }
    return $null
}

function Normalize-Thumbprint([string] $Value) {
    if (-not $Value) { return $null }
    $normalized = $Value.Replace(' ', '').ToUpperInvariant()
    if ($normalized -notmatch '^[0-9A-F]+$') { throw "invalid certificate thumbprint: $Value" }
    return $normalized
}

function Get-PersonalCertificate([string] $CertificateThumbprint) {
    if (-not $CertificateThumbprint) { return $null }
    return Get-Item -LiteralPath "Cert:\CurrentUser\My\$(Normalize-Thumbprint $CertificateThumbprint)" -ErrorAction SilentlyContinue
}

function Test-NonExportablePrivateKey($Cert) {
    $key = $null
    try {
        $key = [Security.Cryptography.X509Certificates.RSACertificateExtensions]::GetRSAPrivateKey($Cert)
        if ($key -is [Security.Cryptography.RSACng]) {
            return ($key.Key.ExportPolicy -eq [Security.Cryptography.CngExportPolicies]::None)
        }
        if ($key -is [Security.Cryptography.RSACryptoServiceProvider]) { return (-not $key.CspKeyContainerInfo.Exportable) }
        return $false
    } catch { return $false }
    finally { if ($key) { $key.Dispose() } }
}

function Test-CodeSigningCertificate($Cert, [switch] $RequireNonExportable) {
    if (-not $Cert -or -not $Cert.HasPrivateKey) { return $false }
    $now = Get-Date
    if ($Cert.NotBefore -gt $now -or $Cert.NotAfter -le $now) { return $false }
    foreach ($extension in $Cert.Extensions) {
        if ($extension.Oid.Value -eq '2.5.29.37') {
            foreach ($usage in $extension.EnhancedKeyUsages) {
                if ($usage.Value -eq $codeSigningOid) {
                    return (-not $RequireNonExportable -or (Test-NonExportablePrivateKey $Cert))
                }
            }
        }
    }
    return $false
}

function Test-CurrentUserRootTrust([string] $CertificateThumbprint) {
    return [bool](Get-CurrentUserRootCertificate $CertificateThumbprint)
}

function Get-CurrentUserRootCertificate([string] $CertificateThumbprint) {
    return Get-Item -LiteralPath "Cert:\CurrentUser\Root\$(Normalize-Thumbprint $CertificateThumbprint)" -ErrorAction SilentlyContinue
}

function Get-LocalDevelopmentCertificate {
    if (Test-ProductionMode) { return $null }
    $record = Get-LocalRecordPath
    if (-not $record -or -not (Test-Path -LiteralPath $record -PathType Leaf)) { return $null }
    $value = (Get-Content -LiteralPath $record -Raw).Trim()
    if (-not $value -or $value -notmatch '^[0-9A-Fa-f ]+$') { return $null }
    $cert = Get-PersonalCertificate $value
    if (-not (Test-CodeSigningCertificate $cert -RequireNonExportable)) { return $null }
    if ($cert.Subject -ne $developmentSubject -or -not (Test-CurrentUserRootTrust $cert.Thumbprint)) { return $null }
    return $cert
}

function Get-LocalDevelopmentThumbprint {
    $cert = Get-LocalDevelopmentCertificate
    if ($cert) { return (Normalize-Thumbprint $cert.Thumbprint) }
    return $null
}

function Get-RecordedDevelopmentThumbprint {
    if (Test-ProductionMode) { return $null }
    $record = Get-LocalRecordPath
    if (-not $record -or -not (Test-Path -LiteralPath $record -PathType Leaf)) { return $null }
    $value = (Get-Content -LiteralPath $record -Raw).Trim()
    if (-not $value) { throw "local development signing selection record is empty: $record" }
    return (Normalize-Thumbprint $value)
}

function Resolve-SignTool {
    param([switch] $Verification)
    if ($Tool) { return [pscustomobject]@{ Path = $Tool; Source = 'explicit' } }
    if ($env:STASIS_AOT_SIGN_TOOL -and (-not $Verification -or [IO.Path]::GetFileNameWithoutExtension($env:STASIS_AOT_SIGN_TOOL) -eq 'signtool')) {
        return [pscustomobject]@{ Path = $env:STASIS_AOT_SIGN_TOOL; Source = 'explicit' }
    }
    $pathCandidates = @($env:PATH -split [IO.Path]::PathSeparator |
        Where-Object { -not [string]::IsNullOrWhiteSpace($_) } |
        ForEach-Object { Join-Path $_ 'signtool.exe' } |
        Sort-Object)
    foreach ($candidate in $pathCandidates) {
        if (Test-Path -LiteralPath $candidate -PathType Leaf) { return [pscustomobject]@{ Path = $candidate; Source = 'path' } }
    }
    $roots = @()
    if (${env:ProgramFiles(x86)}) { $roots += Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin' }
    if ($env:ProgramFiles) { $roots += Join-Path $env:ProgramFiles 'Windows Kits\10\bin' }
    $architectures = @('x64', 'x86', 'arm64', 'arm')
    $candidates = foreach ($root in $roots) {
        if (Test-Path -LiteralPath $root) {
            foreach ($version in (Get-ChildItem -LiteralPath $root -Directory | ForEach-Object {
                try { [pscustomobject]@{ Name = $_.Name; FullName = $_.FullName; Version = [version]$_.Name } } catch { }
            } | Sort-Object @{Expression = 'Version'; Descending = $true}, Name)) {
                foreach ($architecture in $architectures) {
                    $candidate = Join-Path $version.FullName "$architecture\signtool.exe"
                    if (Test-Path -LiteralPath $candidate -PathType Leaf) {
                        [pscustomobject]@{ Version = $version.Version; Rank = [array]::IndexOf($architectures, $architecture); Path = $candidate }
                    }
                }
            }
        }
    }
    $selected = $candidates | Sort-Object @{Expression = 'Version'; Descending = $true}, Rank, Path | Select-Object -First 1
    if ($selected) { return [pscustomobject]@{ Path = $selected.Path; Source = 'windows-kits' } }
    return $null
}

function Get-ConfiguredCertificate {
    $certificatePath = if ($Certificate) { $Certificate } else { $env:STASIS_SIGNING_CERTIFICATE }
    $certificateThumbprint = if ($Thumbprint) { $Thumbprint } else { $env:STASIS_SIGNING_CERT_THUMBPRINT }
    if ($certificatePath) {
        if (-not (Test-Path -LiteralPath $certificatePath -PathType Leaf)) { throw "signing certificate does not exist: $certificatePath" }
        $password = if ($env:STASIS_SIGNING_PFX_PASSWORD) { $env:STASIS_SIGNING_PFX_PASSWORD } else { $null }
        $flags = [Security.Cryptography.X509Certificates.X509KeyStorageFlags]::EphemeralKeySet
        $cert = [Security.Cryptography.X509Certificates.X509Certificate2]::new($certificatePath, $password, $flags)
        try {
            if (-not (Test-CodeSigningCertificate $cert)) { throw "signing certificate is not currently usable for code signing: $certificatePath" }
            $arguments = @('/f', $certificatePath)
            if ($password) { $arguments += @('/p', $password) }
            return [pscustomobject]@{ Arguments = $arguments; Thumbprint = (Normalize-Thumbprint $cert.Thumbprint) }
        } finally { $cert.Dispose() }
    }
    if ($certificateThumbprint) {
        $normalized = Normalize-Thumbprint $certificateThumbprint
        $cert = Get-PersonalCertificate $normalized
        if (-not (Test-CodeSigningCertificate $cert)) { throw "signing certificate $normalized was not found as a usable certificate in CurrentUser\\My" }
        return [pscustomobject]@{ Arguments = @('/sha1', $normalized); Thumbprint = $normalized }
    }
    $local = Get-LocalDevelopmentCertificate
    if ($local) {
        $normalized = Normalize-Thumbprint $local.Thumbprint
        return [pscustomobject]@{ Arguments = @('/sha1', $normalized); Thumbprint = $normalized }
    }
    throw "Windows signing requires STASIS_SIGNING_CERTIFICATE or STASIS_SIGNING_CERT_THUMBPRINT; run 'powershell -NoProfile -ExecutionPolicy Bypass -File tools/windows/stasis-signing.ps1 provision' from the checkout for explicit local development signing."
}

function Assert-AuthenticodeIdentity([string] $Path, [string] $ExpectedThumbprint, [switch] $AllowUntrusted) {
    $signature = Get-AuthenticodeSignature -LiteralPath $Path
    if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid -and
        -not ($AllowUntrusted -and $signature.Status -eq [System.Management.Automation.SignatureStatus]::Unknown)) {
        throw "Authenticode verification failed for $Path with status $($signature.Status): $($signature.StatusMessage)"
    }
    $actual = if ($signature.SignerCertificate) { Normalize-Thumbprint $signature.SignerCertificate.Thumbprint } else { $null }
    if ($ExpectedThumbprint) {
        $expected = Normalize-Thumbprint $ExpectedThumbprint
        if ($actual -ne $expected) { throw "Authenticode signer mismatch for $Path; expected thumbprint $expected but found $actual" }
    }
}

function Get-VerificationThumbprint {
    $certificatePath = if ($Certificate) { $Certificate } else { $env:STASIS_SIGNING_CERTIFICATE }
    $certificateThumbprint = if ($Thumbprint) { $Thumbprint } else { $env:STASIS_SIGNING_CERT_THUMBPRINT }
    if ($certificatePath) {
        if (-not (Test-Path -LiteralPath $certificatePath -PathType Leaf)) { throw "verification certificate does not exist: $certificatePath" }
        $password = if ($env:STASIS_SIGNING_PFX_PASSWORD) { $env:STASIS_SIGNING_PFX_PASSWORD } else { $null }
        $flags = [Security.Cryptography.X509Certificates.X509KeyStorageFlags]::EphemeralKeySet
        $cert = [Security.Cryptography.X509Certificates.X509Certificate2]::new($certificatePath, $password, $flags)
        try { return (Normalize-Thumbprint $cert.Thumbprint) }
        finally { $cert.Dispose() }
    }
    if ($certificateThumbprint) { return (Normalize-Thumbprint $certificateThumbprint) }
    return (Get-RecordedDevelopmentThumbprint)
}

function Test-PinnedSelfSignedVerificationFailure([int] $ExitCode, [string] $Output) {
    if (-not (Test-ProductionMode) -or $env:STASIS_SIGNING_ALLOW_PINNED_SELF_SIGNED_VERIFY -ne '1') { return $false }
    if ($ExitCode -ne 1) { return $false }
    if ($Output -match '(?i)No signature found|TRUST_E_BAD_DIGEST|0x80096010|digital signature[^\r\n]*not valid') { return $false }
    if ($Output -notmatch '(?i)0x800B0109|terminated in a root\s+certificate which is not trusted by the trust provider') { return $false }
    if ($Output -notmatch '(?im)^\s*Number of errors:\s*1\s*$') { return $false }
    if ($Output -notmatch '(?im)^\s*Number of warnings:\s*0\s*$') { return $false }
    return $true
}

function Invoke-BoundedSignTool([string] $Executable, [string[]] $Arguments, [switch] $AllowPinnedSelfSigned) {
    $timeoutText = $env:STASIS_SIGNING_TIMEOUT_SECONDS
    if (-not $timeoutText) {
        $commandOutput = @(& $Executable @Arguments 2>&1)
        $exitCode = $LASTEXITCODE
        $commandOutput | ForEach-Object { Write-Host "$_" }
        $outputText = $commandOutput | Out-String
        if ($exitCode -ne 0 -and -not ($AllowPinnedSelfSigned -and (Test-PinnedSelfSignedVerificationFailure $exitCode $outputText))) {
            throw "signtool failed with exit code $exitCode"
        }
        return ($exitCode -eq 0)
    }
    $timeoutSeconds = 0
    $validTimeout = [int]::TryParse($timeoutText, [ref]$timeoutSeconds)
    if (-not $validTimeout -or $timeoutSeconds -lt 1 -or $timeoutSeconds -gt 600) {
        throw 'STASIS_SIGNING_TIMEOUT_SECONDS must be an integer from 1 through 600'
    }
    $startInfo = [Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $Executable
    $startInfo.UseShellExecute = $false
    if ($AllowPinnedSelfSigned) {
        $startInfo.RedirectStandardOutput = $true
        $startInfo.RedirectStandardError = $true
    }
    if ($null -eq $startInfo.ArgumentList) {
        throw 'bounded signing requires PowerShell 7 or newer'
    }
    foreach ($argument in $Arguments) { $startInfo.ArgumentList.Add($argument) }
    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    try {
        if (-not $process.Start()) { throw 'signtool process did not start' }
        if ($AllowPinnedSelfSigned) {
            $standardOutput = $process.StandardOutput.ReadToEndAsync()
            $standardError = $process.StandardError.ReadToEndAsync()
        }
        if (-not $process.WaitForExit($timeoutSeconds * 1000)) {
            # SignTool does not launch a process tree that must be supervised. Killing the
            # direct process also avoids the Windows process-tree enumeration path hanging.
            $process.Kill()
            if (-not $process.WaitForExit(5000)) {
                throw "signtool timed out after $timeoutSeconds seconds and did not terminate within 5 seconds"
            }
            throw "signtool timed out after $timeoutSeconds seconds"
        }
        $outputText = ''
        if ($AllowPinnedSelfSigned) {
            $outputText = "$($standardOutput.Result)$($standardError.Result)"
            if ($outputText) { Write-Host $outputText.TrimEnd() }
        }
        if ($process.ExitCode -ne 0 -and -not ($AllowPinnedSelfSigned -and (Test-PinnedSelfSignedVerificationFailure $process.ExitCode $outputText))) {
            throw "signtool failed with exit code $($process.ExitCode)"
        }
        return ($process.ExitCode -eq 0)
    } finally {
        $process.Dispose()
    }
}

function Invoke-SignArtifact([string] $Path) {
    $signer = Resolve-SignTool
    if (-not $signer) { throw 'signtool.exe was not found. Set STASIS_AOT_SIGN_TOOL, add it to PATH, or install the Windows SDK.' }
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { throw "signing input does not exist: $Path" }
    $pageHashArgument = switch ([IO.Path]::GetExtension($Path).ToLowerInvariant()) {
        '.exe' { '/ph' }
        '.dll' { '/nph' }
        default { throw "unsupported Authenticode artifact type for ${Path}; expected .exe or .dll" }
    }
    if ([IO.Path]::GetFileNameWithoutExtension($signer.Path) -ne 'signtool') {
        $expectedThumbprint = Get-VerificationThumbprint
        $previousPageHashes = $env:STASIS_SIGN_PAGE_HASHES
        try {
            $env:STASIS_SIGN_PAGE_HASHES = if ($pageHashArgument -eq '/ph') { '1' } else { $null }
            & $signer.Path $Path
            if ($LASTEXITCODE -ne 0) { throw "signer failed for $Path with exit code $LASTEXITCODE" }
        } finally { $env:STASIS_SIGN_PAGE_HASHES = $previousPageHashes }
        Assert-AuthenticodeIdentity $Path $expectedThumbprint
        return
    }
    $configured = Get-ConfiguredCertificate
    $timestamps = if ($TimestampUrl) {
        @($TimestampUrl)
    } elseif ($env:STASIS_SIGNING_TIMESTAMP_URLS) {
        @($env:STASIS_SIGNING_TIMESTAMP_URLS -split ';' | Where-Object { $_ })
    } elseif ($env:STASIS_SIGNING_TIMESTAMP_URL) {
        @($env:STASIS_SIGNING_TIMESTAMP_URL)
    } else {
        @('')
    }
    $errors = @()
    foreach ($timestamp in $timestamps) {
        $arguments = @('sign', '/fd', 'SHA256', $pageHashArgument) + $configured.Arguments
        if ($timestamp) { $arguments += @('/tr', $timestamp, '/td', 'SHA256') }
        try {
            Invoke-BoundedSignTool $signer.Path ($arguments + $Path)
            Assert-AuthenticodeIdentity $Path $configured.Thumbprint
            return
        } catch {
            $errors += if ($timestamp) {
                "$timestamp`: $($_.Exception.Message)"
            } else {
                $_.Exception.Message
            }
        }
    }
    throw "signer failed for $Path ($($errors -join '; '))"
}

function Add-CurrentUserRootTrust($Cert) {
    $store = [Security.Cryptography.X509Certificates.X509Store]::new('Root', 'CurrentUser')
    try {
        $store.Open([Security.Cryptography.X509Certificates.OpenFlags]::ReadWrite)
        $publicCertificate = [Security.Cryptography.X509Certificates.X509Certificate2]::new($Cert.RawData)
        try { $store.Add($publicCertificate) }
        finally { $publicCertificate.Dispose() }
    } finally { $store.Dispose() }
}

function Remove-CurrentUserCertificate([string] $StoreName, [string] $CertificateThumbprint) {
    $item = Get-Item -LiteralPath "Cert:\CurrentUser\$StoreName\$(Normalize-Thumbprint $CertificateThumbprint)" -ErrorAction SilentlyContinue
    if ($item) { Remove-Item -LiteralPath $item.PSPath -Force }
}

function Write-LocalDevelopmentRecord([string] $Record, [string] $CertificateThumbprint) {
    $parent = Split-Path -Parent $Record
    if ($parent) { New-Item -ItemType Directory -Force -Path $parent | Out-Null }
    $temporary = "$Record.tmp-$PID"
    try {
        Set-Content -LiteralPath $temporary -Value $CertificateThumbprint -Encoding ascii
        Move-Item -LiteralPath $temporary -Destination $Record -Force
    } finally {
        if (Test-Path -LiteralPath $temporary) { Remove-Item -LiteralPath $temporary -Force }
    }
}

function Invoke-Provision {
    if (Test-ProductionMode) { throw 'production signing never provisions certificates' }
    $record = Get-LocalRecordPath
    if (-not $record) { throw 'LOCALAPPDATA is not set; set STASIS_SIGNING_LOCAL_RECORD to persist the CurrentUser certificate selection' }
    $previousRecord = if (Test-Path -LiteralPath $record -PathType Leaf) { Get-Content -LiteralPath $record -Raw } else { $null }
    $cert = Get-LocalDevelopmentCertificate
    $created = $false
    $trustAdded = $false
    $recordChanged = $false
    $recordWriteAttempted = $false
    try {
        if (-not $cert) {
            $cert = Get-ChildItem -LiteralPath 'Cert:\CurrentUser\My' | Where-Object {
                $_.Subject -eq $developmentSubject -and (Test-CodeSigningCertificate $_ -RequireNonExportable)
            } | Sort-Object NotAfter -Descending | Select-Object -First 1
        }
        if (-not $cert) {
            $cert = New-SelfSignedCertificate -Type CodeSigningCert -Subject $developmentSubject -CertStoreLocation 'Cert:\CurrentUser\My' -KeyExportPolicy NonExportable -KeyLength 2048 -HashAlgorithm SHA256
            $created = $true
        }
        if (-not (Test-CodeSigningCertificate $cert -RequireNonExportable)) { throw 'provisioned development certificate is not usable with a nonexportable code-signing key' }
        if (-not (Test-CurrentUserRootTrust $cert.Thumbprint)) {
            # An insertion can fail after changing the store.
            $trustAdded = $true
            Add-CurrentUserRootTrust $cert
        }
        $trustedCertificate = Get-CurrentUserRootCertificate $cert.Thumbprint
        if (-not $trustedCertificate) { throw 'development certificate was not added to CurrentUser\Root' }
        if ($trustedCertificate.HasPrivateKey) { throw 'CurrentUser\Root development trust unexpectedly contains a private key' }
        $recordWriteAttempted = $true
        Write-LocalDevelopmentRecord $record (Normalize-Thumbprint $cert.Thumbprint)
        $recordChanged = (($previousRecord | Out-String).Trim().ToUpperInvariant() -ne (Normalize-Thumbprint $cert.Thumbprint))
        [ordered]@{
            subject = $developmentSubject
            thumbprint = (Normalize-Thumbprint $cert.Thumbprint)
            store = 'CurrentUser\My'
            personal_store = 'CurrentUser\My'
            trust_store = 'CurrentUser\Root'
            certificate = if ($created) { 'created' } else { 'reused' }
            root_trust = if ($trustAdded) { 'added' } else { 'already-present' }
            record = if ($recordChanged) { 'updated' } else { 'unchanged' }
        } | ConvertTo-Json -Compress
    } catch {
        $failure = $_.Exception.Message
        $rollback = @()
        if ($trustAdded -and $cert) {
            try { Remove-CurrentUserCertificate 'Root' $cert.Thumbprint; $rollback += 'removed CurrentUser\Root trust' }
            catch { $rollback += "CurrentUser\Root rollback failed: $($_.Exception.Message)" }
        }
        if ($created -and $cert) {
            try { Remove-CurrentUserCertificate 'My' $cert.Thumbprint; $rollback += 'removed CurrentUser\My certificate' }
            catch { $rollback += "CurrentUser\My rollback failed: $($_.Exception.Message)" }
        }
        try {
            if ($recordWriteAttempted) {
                if ($null -ne $previousRecord) { Set-Content -LiteralPath $record -Value $previousRecord -NoNewline -Encoding ascii; $rollback += 'restored selection record' }
                elseif (Test-Path -LiteralPath $record) { Remove-Item -LiteralPath $record -Force; $rollback += 'removed selection record' }
            }
        } catch { $rollback += "selection record rollback failed: $($_.Exception.Message)" }
        $detail = if ($rollback.Count) { $rollback -join '; ' } else { 'no changes required rollback' }
        throw "development signing provisioning failed: $failure. Rollback: $detail"
    }
}

switch ($Command) {
    'status' {
        $signer = Resolve-SignTool
        $localCertificate = [bool](Get-LocalDevelopmentCertificate)
        $certificateDiagnostic = $null
        try { $null = Get-ConfiguredCertificate; $certificateConfigured = $true }
        catch { $certificateConfigured = $false; $certificateDiagnostic = $_.Exception.Message }
        $productionCredentials = [bool]($Certificate -or $Thumbprint -or $env:STASIS_SIGNING_CERTIFICATE -or $env:STASIS_SIGNING_CERT_THUMBPRINT -or $env:STASIS_SIGNING_PFX_BASE64)
        [ordered]@{
            platform = 'windows'
            signer = if ($signer) { $signer.Path } else { $null }
            signer_source = if ($signer) { $signer.Source } else { $null }
            certificate_configured = $certificateConfigured
            local_development_certificate_configured = $localCertificate
            certificate_diagnostic = $certificateDiagnostic
            production_credentials_configured = $productionCredentials
            required = ($env:STASIS_REQUIRE_SIGNED_EXECUTION -eq '1' -or $env:STASIS_SIGNING_MODE -eq 'required')
        } | ConvertTo-Json -Compress
    }
    'provision' { Invoke-Provision }
    'sign' {
        if (-not $Artifact) { throw 'sign requires at least one explicit artifact path' }
        foreach ($path in $Artifact) { Invoke-SignArtifact $path }
    }
    'verify' {
        if (-not $Artifact) { throw 'verify requires at least one explicit artifact path' }
        $signer = Resolve-SignTool -Verification
        if (-not $signer) { throw 'signtool.exe was not found; install the Windows SDK or set STASIS_AOT_SIGN_TOOL.' }
        if ([IO.Path]::GetFileNameWithoutExtension($signer.Path) -ne 'signtool') { throw "verification requires a real signtool.exe; configured legacy hook $($signer.Path) only supports signing" }
        $expectedThumbprint = Get-VerificationThumbprint
        foreach ($path in $Artifact) {
            if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw "verification input does not exist: $path" }
            try {
                $signtoolVerified = Invoke-BoundedSignTool $signer.Path @('verify', '/pa', '/all', '/tw', '/v', $path) -AllowPinnedSelfSigned
                if ($signtoolVerified) {
                    Assert-AuthenticodeIdentity $path $expectedThumbprint
                } else {
                    Assert-AuthenticodeIdentity $path $expectedThumbprint -AllowUntrusted
                }
            } catch {
                if (-not (Test-PinnedSelfSignedVerificationFailure $LASTEXITCODE $_.Exception.Message)) {
                    throw "signature verification failed for $path`: $($_.Exception.Message)"
                }
            }
        }
    }
}
