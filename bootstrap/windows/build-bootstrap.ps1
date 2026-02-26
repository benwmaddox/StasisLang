param(
    [string] $Configuration = "Release",
    [switch] $Sign,
    [string] $CertSubject = "CN=Stasis Local Dev",
    [switch] $CreateCert,
    [switch] $TrustLocalCert
)

$ErrorActionPreference = "Stop"

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$cliProject = Join-Path $repoRoot "Stasis.Cli\Stasis.Cli.csproj"
$buildOutDir = Join-Path $repoRoot ("Stasis.Cli\bin\" + $Configuration + "\net9.0")
$bootstrapOutDir = Join-Path $repoRoot "bootstrap\windows\stasis-cli"

if (-not (Test-Path $cliProject)) {
    throw "Missing project file: $cliProject"
}

dotnet build $cliProject -c $Configuration
if ($LASTEXITCODE -ne 0) {
    throw "dotnet build failed."
}

New-Item -ItemType Directory -Path $bootstrapOutDir -Force | Out-Null
Copy-Item -Path (Join-Path $buildOutDir "*") -Destination $bootstrapOutDir -Force

if ($Sign) {
    $signScript = Join-Path $PSScriptRoot "sign-bootstrap.ps1"
    & $signScript -TargetDir $bootstrapOutDir -CertSubject $CertSubject -CreateCert:$CreateCert.IsPresent -TrustLocalCert:$TrustLocalCert.IsPresent
    if ($LASTEXITCODE -ne 0) {
        throw "Signing failed."
    }
}

Write-Host "Bootstrap compiler updated at $bootstrapOutDir"
