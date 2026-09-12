[CmdletBinding()]
param(
    [ValidateSet('add', 'remove')]
    [string] $Mode = 'add',
    [string] $Certificate,
    [Parameter(Mandatory = $true)]
    [string] $ExpectedThumbprint
)

$ErrorActionPreference = 'Stop'

function Invoke-BoundedNativeCommand([string] $Label, [string] $Executable, [string[]] $Arguments) {
    $startInfo = [Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $Executable
    $startInfo.UseShellExecute = $false
    foreach ($argument in $Arguments) { $startInfo.ArgumentList.Add($argument) }
    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    try {
        Write-Host "Starting $Label"
        if (-not $process.Start()) { throw "$Label process did not start" }
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

$certutil = (Get-Command certutil.exe -ErrorAction Stop).Source
if ($Mode -eq 'remove') {
    Invoke-BoundedNativeCommand 'temporary signer root trust removal' $certutil @(
        '-user', '-f', '-silent', '-delstore', 'Root', $ExpectedThumbprint
    )
    exit 0
}

if (-not (Test-Path -LiteralPath $Certificate -PathType Leaf)) {
    throw "signing certificate does not exist: $Certificate"
}

$openssl = (Get-Command openssl.exe -ErrorAction Stop).Source
$publicPem = Join-Path $env:RUNNER_TEMP "stasis-signing-public.pem"
$publicDer = Join-Path $env:RUNNER_TEMP "stasis-signing-public.cer"
try {
    Invoke-BoundedNativeCommand 'public signing certificate extraction' $openssl @(
        'pkcs12', '-in', $Certificate, '-clcerts', '-nokeys',
        '-out', $publicPem, '-passin', 'env:STASIS_SIGNING_PFX_PASSWORD'
    )
    Invoke-BoundedNativeCommand 'public signing certificate conversion' $openssl @(
        'x509', '-in', $publicPem, '-outform', 'DER', '-out', $publicDer
    )

    $publicCertificate = [Security.Cryptography.X509Certificates.X509Certificate2]::new($publicDer)
    try {
        if ($publicCertificate.Thumbprint -ne $ExpectedThumbprint) {
            throw "Windows signing certificate does not match the pinned release identity"
        }
        if ($publicCertificate.Subject -ne $publicCertificate.Issuer) {
            throw "Pinned private signing identity must remain self-signed"
        }
    } finally {
        $publicCertificate.Dispose()
    }

    Invoke-BoundedNativeCommand 'temporary signer root trust' $certutil @(
        '-user', '-f', '-silent', '-addstore', 'Root', $publicDer
    )
} finally {
    Remove-Item -LiteralPath $publicPem, $publicDer -Force -ErrorAction SilentlyContinue
}
