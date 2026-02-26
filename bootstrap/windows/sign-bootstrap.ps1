param(
    [string] $TargetDir = "",
    [string] $CertSubject = "CN=Stasis Local Dev",
    [switch] $CreateCert,
    [switch] $TrustLocalCert
)

$ErrorActionPreference = "Stop"

if ([string]::IsNullOrWhiteSpace($TargetDir)) {
    $TargetDir = Join-Path (Split-Path -Parent $PSScriptRoot) "windows\stasis-cli"
}

function Resolve-SignToolPath {
    $fromPath = Get-Command signtool.exe -ErrorAction SilentlyContinue
    if ($null -ne $fromPath) {
        return $fromPath.Source
    }

    $candidates = @(
        "$env:ProgramFiles(x86)\Windows Kits\10\bin\x64\signtool.exe",
        "$env:ProgramFiles(x86)\Windows Kits\10\App Certification Kit\signtool.exe",
        "$env:ProgramFiles\Windows Kits\10\bin\x64\signtool.exe"
    )

    foreach ($candidate in $candidates) {
        if (Test-Path $candidate) {
            return $candidate
        }
    }

    throw "signtool.exe not found. Install Windows SDK signing tools."
}

function Get-OrCreateCodeSigningCert([string] $subject, [bool] $allowCreate) {
    $cert = Get-ChildItem Cert:\CurrentUser\My |
        Where-Object { $_.Subject -eq $subject } |
        Sort-Object NotAfter -Descending |
        Select-Object -First 1

    if ($null -eq $cert -and $allowCreate) {
        $cert = New-SelfSignedCertificate `
            -Type CodeSigningCert `
            -Subject $subject `
            -CertStoreLocation "Cert:\CurrentUser\My" `
            -NotAfter (Get-Date).AddYears(5)
    }

    if ($null -eq $cert) {
        throw "Code-signing certificate not found for subject '$subject'. Re-run with -CreateCert."
    }

    return $cert
}

function Trust-CertificateLocally($cert) {
    $tempCer = Join-Path $env:TEMP ("stasis_cert_" + [guid]::NewGuid().ToString("N") + ".cer")
    try {
        Export-Certificate -Cert $cert -FilePath $tempCer -Force | Out-Null
        Import-Certificate -FilePath $tempCer -CertStoreLocation "Cert:\CurrentUser\TrustedPublisher" | Out-Null
        Import-Certificate -FilePath $tempCer -CertStoreLocation "Cert:\CurrentUser\Root" | Out-Null
    } finally {
        Remove-Item $tempCer -ErrorAction SilentlyContinue
    }
}

if (-not (Test-Path $TargetDir)) {
    throw "Target directory does not exist: $TargetDir"
}

$signTool = Resolve-SignToolPath
$cert = Get-OrCreateCodeSigningCert -subject $CertSubject -allowCreate:$CreateCert.IsPresent

if ($TrustLocalCert) {
    Trust-CertificateLocally $cert
}

$files = Get-ChildItem -Path $TargetDir -File -Recurse |
    Where-Object { $_.Extension -in @(".exe", ".dll") }

if ($files.Count -eq 0) {
    Write-Host "No .exe/.dll files found under $TargetDir"
    exit 0
}

foreach ($file in $files) {
    & $signTool sign /fd SHA256 /sha1 $cert.Thumbprint /nologo $file.FullName
    if ($LASTEXITCODE -ne 0) {
        throw "Signing failed for $($file.FullName)"
    }
}

Write-Host ("Signed {0} file(s) in {1}" -f $files.Count, $TargetDir)
