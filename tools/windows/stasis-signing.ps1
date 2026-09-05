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

function Test-ProductionMode {
    return ($env:STASIS_SIGNING_MODE -eq 'production' -or $env:STASIS_SIGNING_PROFILE -eq 'production')
}

function Get-LocalRecordPath {
    if ($env:STASIS_SIGNING_LOCAL_RECORD) { return $env:STASIS_SIGNING_LOCAL_RECORD }
    if ($env:LOCALAPPDATA) { return (Join-Path $env:LOCALAPPDATA 'Stasis\signing\development-thumbprint.txt') }
    return $null
}

function Get-LocalDevelopmentThumbprint {
    if (Test-ProductionMode) { return $null }
    $record = Get-LocalRecordPath
    if (-not $record -or -not (Test-Path -LiteralPath $record -PathType Leaf)) { return $null }
    $value = (Get-Content -LiteralPath $record -Raw).Trim()
    if ($value -and $value -match '^[0-9A-Fa-f]+$') { return $value }
    return $null
}

function Resolve-SignTool {
    param([switch] $Verification)
    if ($Tool) {
        return [pscustomobject]@{ Path = $Tool; Source = 'explicit' }
    }
    if ($env:STASIS_AOT_SIGN_TOOL -and (-not $Verification -or [IO.Path]::GetFileNameWithoutExtension($env:STASIS_AOT_SIGN_TOOL) -eq 'signtool')) {
        return [pscustomobject]@{ Path = $env:STASIS_AOT_SIGN_TOOL; Source = 'explicit' }
    }
    $pathCandidates = @($env:PATH -split [IO.Path]::PathSeparator |
        ForEach-Object { Join-Path $_ 'signtool.exe' } |
        Sort-Object)
    foreach ($candidate in $pathCandidates) {
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            return [pscustomobject]@{ Path = $candidate; Source = 'path' }
        }
    }
    $roots = @(
        (Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin'),
        (Join-Path $env:ProgramFiles 'Windows Kits\10\bin')
    )
    $architectures = @('x64', 'x86', 'arm64', 'arm')
    $candidates = foreach ($root in $roots) {
        if (Test-Path -LiteralPath $root) {
            foreach ($version in (Get-ChildItem -LiteralPath $root -Directory | ForEach-Object {
                try { [pscustomobject]@{ Name = $_.Name; FullName = $_.FullName; Version = [version]$_.Name } } catch { }
            } | Sort-Object @{Expression = 'Version'; Descending = $true}, Name)) {
                foreach ($architecture in $architectures) {
                    $candidate = Join-Path $version.FullName "$architecture\signtool.exe"
                    if (Test-Path -LiteralPath $candidate -PathType Leaf) {
                        [pscustomobject]@{ Version = $version.Name; Rank = [array]::IndexOf($architectures, $architecture); Path = $candidate }
                    }
                }
            }
        }
    }
    $selected = $candidates | Sort-Object @{Expression = 'Version'; Descending = $true}, Rank, Path | Select-Object -First 1
    if ($selected) { return [pscustomobject]@{ Path = $selected.Path; Source = 'windows-kits' } }
    return $null
}

function Get-CertificateArguments {
    $certificatePath = if ($Certificate) { $Certificate } else { $env:STASIS_SIGNING_CERTIFICATE }
    $certificateThumbprint = if ($Thumbprint) { $Thumbprint } else { $env:STASIS_SIGNING_CERT_THUMBPRINT }
    if ($certificatePath) {
        $certificateArgs = @('/f', $certificatePath)
        if ($env:STASIS_SIGNING_PFX_PASSWORD) { $certificateArgs += @('/p', $env:STASIS_SIGNING_PFX_PASSWORD) }
        return $certificateArgs
    }
    if ($certificateThumbprint) { return @('/sha1', $certificateThumbprint) }
    $localThumbprint = Get-LocalDevelopmentThumbprint
    if ($localThumbprint) { return @('/sha1', $localThumbprint) }
    throw "Windows signing requires STASIS_SIGNING_CERTIFICATE or STASIS_SIGNING_CERT_THUMBPRINT; run 'stasis signing provision' only for explicit local development signing."
}

function Invoke-SignArtifact([string] $Path) {
    $signer = Resolve-SignTool
    if (-not $signer) { throw 'signtool.exe was not found. Set STASIS_AOT_SIGN_TOOL, add it to PATH, or install the Windows SDK.' }
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { throw "signing input does not exist: $Path" }
    if ([IO.Path]::GetFileNameWithoutExtension($signer.Path) -ne 'signtool') {
        & $signer.Path $Path
    } else {
        $arguments = @('sign', '/fd', 'SHA256', '/ph') + (Get-CertificateArguments)
        $timestamp = if ($TimestampUrl) { $TimestampUrl } else { $env:STASIS_SIGNING_TIMESTAMP_URL }
        if ($timestamp) { $arguments += @('/tr', $timestamp, '/td', 'SHA256') }
        & $signer.Path @arguments $Path
    }
    if ($LASTEXITCODE -ne 0) { throw "signer failed for $Path with exit code $LASTEXITCODE" }
}

switch ($Command) {
    'status' {
        $signer = Resolve-SignTool
        $localCertificate = [bool](Get-LocalDevelopmentThumbprint)
        $certificate = [bool]($Certificate -or $Thumbprint -or $env:STASIS_SIGNING_CERTIFICATE -or $env:STASIS_SIGNING_CERT_THUMBPRINT -or $localCertificate)
        $productionCredentials = [bool]($Certificate -or $Thumbprint -or $env:STASIS_SIGNING_CERTIFICATE -or $env:STASIS_SIGNING_CERT_THUMBPRINT -or $env:STASIS_SIGNING_PFX_BASE64)
        [ordered]@{
            platform = 'windows'
            signer = if ($signer) { $signer.Path } else { $null }
            signer_source = if ($signer) { $signer.Source } else { $null }
            certificate_configured = $certificate
            local_development_certificate_configured = $localCertificate
            production_credentials_configured = $productionCredentials
            required = ($env:STASIS_REQUIRE_SIGNED_EXECUTION -eq '1' -or $env:STASIS_SIGNING_MODE -eq 'required')
        } | ConvertTo-Json -Compress
    }
    'provision' {
        if (Test-ProductionMode) { throw 'production signing never provisions certificates' }
        $cert = New-SelfSignedCertificate -Type CodeSigningCert -Subject $developmentSubject -CertStoreLocation 'Cert:\CurrentUser\My' -KeyExportPolicy NonExportable -KeyLength 2048 -HashAlgorithm SHA256
        $record = Get-LocalRecordPath
        if (-not $record) { throw 'LOCALAPPDATA is not set; set STASIS_SIGNING_LOCAL_RECORD to persist the CurrentUser certificate selection' }
        $parent = Split-Path -Parent $record
        New-Item -ItemType Directory -Force -Path $parent | Out-Null
        $temporary = "$record.tmp-$PID"
        Set-Content -LiteralPath $temporary -Value $cert.Thumbprint -Encoding ascii
        Move-Item -LiteralPath $temporary -Destination $record -Force
        [ordered]@{ subject = $developmentSubject; store = 'CurrentUser\My'; thumbprint = $cert.Thumbprint } | ConvertTo-Json -Compress
    }
    'sign' {
        if (-not $Artifact) { throw 'sign requires at least one explicit artifact path' }
        foreach ($path in $Artifact) { Invoke-SignArtifact $path }
    }
    'verify' {
        if (-not $Artifact) { throw 'verify requires at least one explicit artifact path' }
        $signer = Resolve-SignTool -Verification
        if (-not $signer) { throw 'signtool.exe was not found; install the Windows SDK or set STASIS_AOT_SIGN_TOOL.' }
        if ([IO.Path]::GetFileNameWithoutExtension($signer.Path) -ne 'signtool') { throw "verification requires a real signtool.exe; configured legacy hook $($signer.Path) only supports signing" }
        foreach ($path in $Artifact) {
            if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw "verification input does not exist: $path" }
            & $signer.Path verify /pa /all $path
            if ($LASTEXITCODE -ne 0) { throw "signature verification failed for $path" }
        }
    }
}
