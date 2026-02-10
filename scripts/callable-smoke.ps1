param(
    [string]$Configuration = "Debug"
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
Push-Location $repoRoot
try {
    $project = "Stasis.Compiler.Tests/Stasis.Compiler.Tests.csproj"
    $filter = @(
        "FullyQualifiedName~CallableResolutionParityTests",
        "FullyQualifiedName~SemanticTests.Flags_extern_overloads_that_share_link_symbol",
        "FullyQualifiedName~LoweringTests.Extern_receiver_callable_falls_back_when_link_name_collides_with_receiverless_callable",
        "FullyQualifiedName~CraneliftBackendConfirmationTests.ExternReceiverCallable_FallsBackWhenNameCollidesWithReceiverlessCallable",
        "FullyQualifiedName~LoweringTests.Test_to_test_function_form_call_resolves_in_lowering",
        "FullyQualifiedName~CraneliftBackendConfirmationTests.Test_to_test_function_form_call_resolves_in_cranelift"
    ) -join "|"

    Write-Host "== Callable Smoke Suite =="
    dotnet test $project -c $Configuration -v minimal --filter $filter
    if ($LASTEXITCODE -ne 0) {
        throw "Callable smoke suite failed with exit code $LASTEXITCODE."
    }
}
finally {
    Pop-Location
}
