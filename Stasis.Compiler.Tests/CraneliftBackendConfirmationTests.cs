using Stasis.Compiler.IR;
using Stasis.Compiler.Layout;
using Stasis.Compiler.Semantic;
using Xunit;

namespace Stasis.Compiler.Tests;

public class CraneliftBackendConfirmationTests
{
    [Fact]
    public void ComparisonExpressions_ConvertB1ToI32()
    {
        var ir = CompileCraneliftIr("""
            function lt(a: i32, b: i32): bool {
                return a < b;
            }
            """);

        Assert.Contains("icmp slt", ir);
        Assert.Contains("bint.i32", ir);
        Assert.DoesNotContain("TODO:", ir);
    }

    [Fact]
    public void UnaryNot_ConvertsB1ToI32()
    {
        var ir = CompileCraneliftIr("""
            function inv(a: bool): bool {
                return !a;
            }
            """);

        Assert.Contains("icmp eq", ir);
        Assert.Contains("bint.i32", ir);
        Assert.DoesNotContain("TODO:", ir);
    }

    [Fact]
    public void IfConditions_AreLoweredToB1ForBrif()
    {
        var ir = CompileCraneliftIr("""
            function choose(flag: bool): i32 {
                if (flag) {
                    return 1;
                } else {
                    return 0;
                }
            }
            """);

        Assert.Contains("brif", ir);
        Assert.Contains("icmp ne", ir);
        Assert.DoesNotContain("TODO:", ir);
    }

    [Fact]
    public void ForLoopConditions_AreLoweredToB1ForBrif()
    {
        var ir = CompileCraneliftIr("""
            function sum_to_n(n: i32): i32 {
                let total: i32 = 0;
                let i: i32 = 0;
                for (i = 0; i < n; i = i + 1) {
                    total = total + i;
                }
                return total;
            }
            """);

        Assert.Contains("brif", ir);
        Assert.Contains("icmp ne", ir);
        Assert.Contains("icmp slt", ir);
        Assert.Contains("bint.i32", ir);
        Assert.DoesNotContain("TODO:", ir);
    }

    private static string CompileCraneliftIr(string source, bool includeTests = false)
    {
        var parse = Parser.Parse(source);
        Assert.Empty(parse.Diagnostics);

        var semantic = new SemanticAnalyzer().Analyze(parse.CompilationUnit);
        Assert.Empty(semantic.Diagnostics);

        var layout = new LayoutPlanner(parse.CompilationUnit, semantic.Symbols).Plan();

        var options = new CodeGenerationOptions(
            ModuleName: "cranelift_confirm",
            IncludeTests: includeTests,
            EmitTestHarness: includeTests);

        using var generator = CodeGeneratorFactory.Create(BackendType.Cranelift, "cranelift_confirm");
        var result = generator.Generate(parse.CompilationUnit, semantic, layout, options);
        Assert.True(result.Success, string.Join("\n", result.Diagnostics.Select(d => d.Message)));
        Assert.NotEmpty(result.Ir);
        return result.Ir;
    }
}

