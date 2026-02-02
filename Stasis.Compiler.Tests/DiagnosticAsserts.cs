using Stasis.Compiler;

namespace Stasis.Compiler.Tests;

internal static class DiagnosticAsserts
{
    public static void AssertNoErrors(IEnumerable<Diagnostic> diagnostics)
    {
        Assert.DoesNotContain(diagnostics, d => d.Severity == DiagnosticSeverity.Error);
    }
}
