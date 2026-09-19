[CmdletBinding()]
param(
    [Parameter(Position = 0)]
    [ValidateSet('provision')]
    [string] $Command = 'provision'
)

$ErrorActionPreference = 'Stop'
$subject = "CN=StasisLang Ephemeral CI Signing $PID-$([Guid]::NewGuid().ToString('N'))"

function New-RandomPfxPassword {
    $bytes = [byte[]]::new(32)
    $generator = [Security.Cryptography.RandomNumberGenerator]::Create()
    try {
        $generator.GetBytes($bytes)
        return [Convert]::ToBase64String($bytes)
    } finally {
        $generator.Dispose()
    }
}

if (-not $env:STASIS_SIGNING_CERTIFICATE) {
    throw 'STASIS_SIGNING_CERTIFICATE must name the ephemeral CI PFX output path'
}

$outputPath = [IO.Path]::GetFullPath($env:STASIS_SIGNING_CERTIFICATE)
$outputParent = Split-Path -Parent $outputPath
if (-not $outputParent) { throw 'STASIS_SIGNING_CERTIFICATE must include a parent directory' }
$temporaryPath = "$outputPath.tmp-$PID"
$password = New-RandomPfxPassword
$certificate = $null
$moved = $false
try {
    New-Item -ItemType Directory -Force -Path $outputParent | Out-Null
    $pkiModule = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\Modules\PKI\PKI.psd1'
    Import-Module $pkiModule -ErrorAction Stop
    if (-not (Get-PSDrive -Name Cert -ErrorAction SilentlyContinue)) {
        New-PSDrive -Name Cert -PSProvider Certificate -Root '\' -Scope Script -ErrorAction Stop | Out-Null
    }
    # The Windows SDK SignTool rejects PFX files exported from CertificateRequest
    # even though .NET and certutil accept them. Create an exportable signature key
    # briefly in CurrentUser\My, export it, then remove the exact thumbprint below.
    # The workflow supervises this entire script with a hard timeout.
    $certificate = New-SelfSignedCertificate `
        -Type CodeSigningCert `
        -Subject $subject `
        -CertStoreLocation 'Cert:\CurrentUser\My' `
        -KeyExportPolicy Exportable `
        -KeySpec Signature `
        -KeyLength 2048 `
        -HashAlgorithm SHA256 `
        -NotAfter (Get-Date).AddHours(24)
    $securePassword = ConvertTo-SecureString -String $password -AsPlainText -Force
    Export-PfxCertificate -Cert $certificate -FilePath $temporaryPath -Password $securePassword -Force | Out-Null
    if (-not (Test-Path -LiteralPath $temporaryPath -PathType Leaf)) {
        throw 'ephemeral CI PFX export did not create its output file'
    }
    Move-Item -LiteralPath $temporaryPath -Destination $outputPath -Force
    $moved = $true
    [ordered]@{
        certificate = $outputPath
        password = $password
        thumbprint = $certificate.Thumbprint.Replace(' ', '').ToUpperInvariant()
        subject = $certificate.Subject
        not_after = $certificate.NotAfter.ToUniversalTime().ToString('o')
    } | ConvertTo-Json -Compress
} finally {
    $cleanupFailure = $null
    if ($certificate) {
        try {
            Remove-Item -LiteralPath "Cert:\CurrentUser\My\$($certificate.Thumbprint)" -Force -ErrorAction Stop
        } catch {
            $cleanupFailure = "failed to remove ephemeral CurrentUser signing certificate $($certificate.Thumbprint): $($_.Exception.Message)"
        } finally {
            $certificate.Dispose()
        }
    }
    if (Test-Path -LiteralPath $temporaryPath -PathType Leaf) {
        Remove-Item -LiteralPath $temporaryPath -Force -ErrorAction SilentlyContinue
    }
    if ($cleanupFailure) {
        if (Test-Path -LiteralPath $outputPath -PathType Leaf) {
            Remove-Item -LiteralPath $outputPath -Force -ErrorAction SilentlyContinue
        }
        throw $cleanupFailure
    }
    if (-not $moved -and (Test-Path -LiteralPath $outputPath -PathType Leaf)) {
        Remove-Item -LiteralPath $outputPath -Force -ErrorAction SilentlyContinue
    }
}
