[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string] $Artifact,
    [Parameter(Mandatory = $true)]
    [string] $ExpectedThumbprint
)

$ErrorActionPreference = 'Stop'

if (-not (Test-Path -LiteralPath $Artifact -PathType Leaf)) {
    throw "signed artifact does not exist: $Artifact"
}

$signature = Get-AuthenticodeSignature -LiteralPath $Artifact
$certificate = $signature.SignerCertificate
if (-not $certificate) {
    throw "signed artifact does not contain a signer certificate: $Artifact"
}
if ($certificate.Thumbprint -ne $ExpectedThumbprint) {
    throw "Windows signing certificate does not match the pinned release identity"
}
if ($certificate.Subject -ne $certificate.Issuer) {
    throw "Pinned private signing identity must remain self-signed"
}

$trustedRoots = [Security.Cryptography.X509Certificates.X509Store]::new(
    'Root',
    [Security.Cryptography.X509Certificates.StoreLocation]::CurrentUser
)
try {
    $trustedRoots.Open([Security.Cryptography.X509Certificates.OpenFlags]::ReadWrite)
    $trustedRoots.Add($certificate)
} finally {
    $trustedRoots.Close()
}
