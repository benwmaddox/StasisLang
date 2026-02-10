param(
    [string]$Configuration = "Debug",
    [switch]$Quick,
    [switch]$SkipStasisAll
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
Push-Location $repoRoot
try {
    Write-Host "== Step 1/3: Callable Smoke Suite =="
    & ".\scripts\callable-smoke.ps1" -Configuration $Configuration
    if ($LASTEXITCODE -ne 0) {
        throw "Callable smoke suite failed with exit code $LASTEXITCODE."
    }

    if (-not $Quick) {
        Write-Host "== Step 2/3: Full Compiler Tests =="
        dotnet test "Stasis.Compiler.Tests/Stasis.Compiler.Tests.csproj" -c $Configuration -v minimal
        if ($LASTEXITCODE -ne 0) {
            throw "Compiler test suite failed with exit code $LASTEXITCODE."
        }
    }
    else {
        Write-Host "== Step 2/3: Full Compiler Tests (skipped by -Quick) =="
    }

    if (-not $SkipStasisAll) {
        Write-Host "== Step 3/3: stasis test --all =="
        .\stasis.bat test --all
        if ($LASTEXITCODE -ne 0) {
            throw "stasis test --all failed with exit code $LASTEXITCODE."
        }
    }
    else {
        Write-Host "== Step 3/3: stasis test --all (skipped by -SkipStasisAll) =="
    }

    Write-Host "Pre-push gate passed."
}
finally {
    Pop-Location
}
