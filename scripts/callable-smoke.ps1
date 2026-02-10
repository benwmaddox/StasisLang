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
        "FullyQualifiedName~SemanticTests.Flags_arity_overloading_for_same_callable_name",
        "FullyQualifiedName~LoweringTests.Receiver_form_zero_arg_dispatch_uses_receiver_type_symbol",
        "FullyQualifiedName~CraneliftBackendConfirmationTests.ReceiverFormZeroArgDispatch_UsesReceiverTypeSymbolsInCranelift",
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
