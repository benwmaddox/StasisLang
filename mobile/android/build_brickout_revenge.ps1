param(
    [switch]$Install,
    [string]$Serial = "",
    [switch]$ValidateAot
)

$ErrorActionPreference = "Stop"
$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path

& (Join-Path $scriptRoot "build_published.ps1") `
    -Game "brickout_revenge" `
    -PackageName "com.rootbeergames.brickoutrevenge" `
    -Serial $Serial `
    -Install:$Install `
    -ValidateAot:$ValidateAot
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}
