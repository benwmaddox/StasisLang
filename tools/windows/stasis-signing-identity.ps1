[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string] $Certificate,
    [Parameter(Mandatory = $true)]
    [ValidatePattern('^[0-9A-Fa-f]{40}$')]
    [string] $ExpectedThumbprint
)

$ErrorActionPreference = 'Stop'

if (-not (Test-Path -LiteralPath $Certificate -PathType Leaf)) {
    throw "signing certificate does not exist: $Certificate"
}
if (-not $env:STASIS_SIGNING_PFX_PASSWORD) {
    throw 'STASIS_SIGNING_PFX_PASSWORD is required to validate the signing identity'
}

$flags = [Security.Cryptography.X509Certificates.X509KeyStorageFlags]::EphemeralKeySet
$certificateIdentity = [Security.Cryptography.X509Certificates.X509Certificate2]::new(
    $Certificate,
    $env:STASIS_SIGNING_PFX_PASSWORD,
    $flags
)
try {
    if ($certificateIdentity.Thumbprint -ne $ExpectedThumbprint) {
        throw "signing certificate thumbprint $($certificateIdentity.Thumbprint) does not match pinned thumbprint $ExpectedThumbprint"
    }
    if ($certificateIdentity.Subject -ne $certificateIdentity.Issuer) {
        throw 'pinned signing certificate is not self-signed'
    }
    if (-not $certificateIdentity.HasPrivateKey) {
        throw 'pinned signing certificate has no private key'
    }
    $now = [DateTime]::UtcNow
    if ($now -lt $certificateIdentity.NotBefore.ToUniversalTime() -or $now -gt $certificateIdentity.NotAfter.ToUniversalTime()) {
        throw 'pinned signing certificate is outside its validity window'
    }
    $codeSigning = '1.3.6.1.5.5.7.3.3'
    $enhancedKeyUsage = @($certificateIdentity.Extensions | Where-Object {
        $_ -is [Security.Cryptography.X509Certificates.X509EnhancedKeyUsageExtension]
    })
    if ($enhancedKeyUsage.Count -eq 0 -or -not @($enhancedKeyUsage[0].EnhancedKeyUsages | Where-Object { $_.Value -eq $codeSigning })) {
        throw 'pinned signing certificate is not valid for code signing'
    }
    [ordered]@{
        thumbprint = $certificateIdentity.Thumbprint
        subject = $certificateIdentity.Subject
        self_signed = $true
    } | ConvertTo-Json -Compress
} finally {
    $certificateIdentity.Dispose()
}
