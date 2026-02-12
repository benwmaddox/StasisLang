using Stasis.Compiler;
using Stasis.Compiler.IR;
using Stasis.Compiler.IR.Cranelift;
using Stasis.Compiler.Layout;
using Xunit;

namespace Stasis.Compiler.Tests;

public sealed class CraneliftArrayFieldLoweringTests
{
    [Fact]
    public void ChainedArrayFieldStoreAndLoad_CompilesWithoutFallbackDiagnostics()
    {
        const string source = """
struct Enemy {
    health: i32;
    damage: i32[4];
}

global state: Enemy[2];

function main(): i32 {
    state[0].damage[1] = 5;
    state[1].damage[2] = state[0].damage[1];
    state[1].health = state[1].damage[2];
    return state[1].health;
}
""";

        var result = CompileToCranelift(source);

        AssertNoErrors(result.ParseDiagnostics);
        AssertNoErrors(result.SemanticDiagnostics);
        AssertNoErrors(result.CodegenDiagnostics);
        Assert.DoesNotContain(result.CodegenDiagnostics, d => d.Message.Contains("array field", StringComparison.OrdinalIgnoreCase));
        Assert.Contains("Enemy__damage", result.Ir, StringComparison.Ordinal);
    }

    [Fact]
    public void ChainedArrayFieldWithDynamicIndex_CompilesWithoutStoreAccessErrors()
    {
        const string source = """
struct Cell {
    values: i32[3];
}

global cells: Cell[4];

function write_and_read(index: i32): i32 {
    cells[index].values[0] = index;
    cells[index].values[1] = cells[index].values[0];
    return cells[index].values[1];
}

function main(): i32 {
    return write_and_read(2);
}
""";

        var result = CompileToCranelift(source);

        AssertNoErrors(result.ParseDiagnostics);
        AssertNoErrors(result.SemanticDiagnostics);
        AssertNoErrors(result.CodegenDiagnostics);
        Assert.DoesNotContain(result.CodegenDiagnostics, d => d.Message.Contains("Array element field", StringComparison.Ordinal));
        Assert.Contains("Cell__values", result.Ir, StringComparison.Ordinal);
    }

    private static void AssertNoErrors(IReadOnlyList<Diagnostic> diagnostics)
    {
        var errors = diagnostics.Where(d => d.Severity == DiagnosticSeverity.Error).ToList();
        Assert.True(
            errors.Count == 0,
            $"Expected no error diagnostics, got:{Environment.NewLine}{string.Join(Environment.NewLine, errors.Select(d => d.Message))}");
    }

    private static CompileSnapshot CompileToCranelift(string source)
    {
        var parse = Parser.Parse(source);
        var semantic = new SemanticAnalyzer(new SemanticAnalyzerOptions(EnableGraphicsBuiltins: false, EnableAudioBuiltins: false))
            .Analyze(parse.CompilationUnit);
        var layout = new LayoutPlanner(parse.CompilationUnit, semantic.Symbols).Plan();

        using var codegen = new CraneliftCodeGenerator("test_module");
        var codegenResult = codegen.Generate(
            parse.CompilationUnit,
            semantic,
            layout,
            CodeGenerationOptions.Debug with
            {
                IncludeTests = false,
                EmitTestHarness = false,
                HeadlessGraphics = true
            });

        return new CompileSnapshot(parse.Diagnostics, semantic.Diagnostics, codegenResult.Diagnostics, codegenResult.Ir);
    }

    private sealed record CompileSnapshot(
        IReadOnlyList<Diagnostic> ParseDiagnostics,
        IReadOnlyList<Diagnostic> SemanticDiagnostics,
        IReadOnlyList<Diagnostic> CodegenDiagnostics,
        string Ir);
}
