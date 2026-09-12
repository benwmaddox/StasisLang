[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string] $Certificate,
    [Parameter(Mandatory = $true)]
    [ValidatePattern('^[0-9A-Fa-f]{40}$')]
    [string] $ExpectedThumbprint
)

$ErrorActionPreference = 'Stop'

function Invoke-BoundedNativeCommand([string] $Label, [string] $Executable, [string[]] $Arguments) {
    $startInfo = [Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $Executable
    $startInfo.UseShellExecute = $false
    if ($null -eq $startInfo.ArgumentList) {
        throw "$Label requires PowerShell 7 or newer"
    }
    foreach ($argument in $Arguments) { $startInfo.ArgumentList.Add($argument) }
    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    try {
        if (-not $process.Start()) { throw "$Label did not start" }
        if (-not $process.WaitForExit(30000)) {
            $process.Kill()
            if (-not $process.WaitForExit(5000)) {
                throw "$Label timed out and did not terminate within 5 seconds"
            }
            throw "$Label timed out after 30 seconds"
        }
        if ($process.ExitCode -ne 0) {
            throw "$Label failed with exit code $($process.ExitCode)"
        }
    } finally {
        $process.Dispose()
    }
}

if (-not (Test-Path -LiteralPath $Certificate -PathType Leaf)) {
    throw "signing certificate does not exist: $Certificate"
}
if (-not $env:STASIS_SIGNING_PFX_PASSWORD) {
    throw 'STASIS_SIGNING_PFX_PASSWORD is required to validate the signing identity'
}

$openssl = (Get-Command openssl.exe -ErrorAction Stop).Source
$temporaryRoot = Join-Path ([IO.Path]::GetTempPath()) "stasis-signing-identity-$PID-$([Guid]::NewGuid().ToString('N'))"
$publicPem = "$temporaryRoot.pem"
$publicDer = "$temporaryRoot.der"
try {
    Invoke-BoundedNativeCommand 'public signing certificate extraction' $openssl @(
        'pkcs12', '-in', $Certificate, '-clcerts', '-nokeys', '-out', $publicPem,
        '-passin', 'env:STASIS_SIGNING_PFX_PASSWORD'
    )
    Invoke-BoundedNativeCommand 'public signing certificate conversion' $openssl @(
        'x509', '-in', $publicPem, '-outform', 'DER', '-out', $publicDer
    )

    $publicCertificate = [Security.Cryptography.X509Certificates.X509Certificate2]::new($publicDer)
    try {
        if ($publicCertificate.Thumbprint -ne $ExpectedThumbprint) {
            throw "signing certificate thumbprint $($publicCertificate.Thumbprint) does not match pinned thumbprint $ExpectedThumbprint"
        }
        if ($publicCertificate.Subject -ne $publicCertificate.Issuer) {
            throw 'pinned signing certificate is not self-signed'
        }
        [ordered]@{
            thumbprint = $publicCertificate.Thumbprint
            subject = $publicCertificate.Subject
            self_signed = $true
        } | ConvertTo-Json -Compress
    } finally {
        $publicCertificate.Dispose()
    }
} finally {
    Remove-Item -LiteralPath $publicPem, $publicDer -Force -ErrorAction SilentlyContinue
}
