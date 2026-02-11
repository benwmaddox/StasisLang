using Stasis.Compiler.IR;
using Stasis.Compiler.IR.Cranelift;
using Stasis.Compiler.Layout;
using Stasis.Compiler.Semantic;

namespace Stasis.Compiler.Tests;

public class CraneliftIncrementalCodegenTests
{
    [Fact]
    public void Reuses_unchanged_function_bodies_for_body_only_edit()
    {
        var sourceA = """
            function main(): i32 {
                return 0;
            }

            function helper(value: i32): i32 {
                return value + 1;
            }

            function tick(): i32 {
                return helper(5);
            }
            """;

        var sourceB = """
            function main(): i32 {
                return 0;
            }

            function helper(value: i32): i32 {
                return value + 2;
            }

            function tick(): i32 {
                return helper(5);
            }
            """;

        var (parseA, semaA, layoutA) = Analyze(sourceA);
        var (parseB, semaB, layoutB) = Analyze(sourceB);

        var profileA = FunctionSemanticFingerprint.ComputeProfile(sourceA, parseA.CompilationUnit, layoutA, includeTests: false, allowReachabilityFallback: true);
        var profileB = FunctionSemanticFingerprint.ComputeProfile(sourceB, parseB.CompilationUnit, layoutB, includeTests: false, allowReachabilityFallback: true);
        var diff = FunctionSemanticFingerprint.Diff(profileA, profileB);

        Assert.False(diff.RequiresConservativeRebuild);
        Assert.Single(diff.ChangedBodyCallableKeys);

        using var baselineGenerator = new CraneliftCodeGenerator("inc");
        var baselineOptions = new CodeGenerationOptions(
            ModuleName: "inc",
            IncludeTests: false,
            EmitTestHarness: false,
            HeadlessGraphics: true,
            AllowReachabilityFallback: true);
        var baselineResultA = baselineGenerator.Generate(parseA.CompilationUnit, semaA, layoutA, baselineOptions);
        DiagnosticAsserts.AssertNoErrors(baselineResultA.Diagnostics);

        using var fullGenerator = new CraneliftCodeGenerator("inc");
        var fullResultB = fullGenerator.Generate(parseB.CompilationUnit, semaB, layoutB, baselineOptions);
        DiagnosticAsserts.AssertNoErrors(fullResultB.Diagnostics);

        using var incrementalGenerator = new CraneliftCodeGenerator("inc");
        var incrementalOptions = baselineOptions with
        {
            RebuildFunctionKeys = new HashSet<string>(diff.ChangedBodyCallableKeys, StringComparer.Ordinal),
            ReuseFunctionBodiesByCallableKey = baselineGenerator.LastFunctionBodiesByCallableKey
        };
        var incrementalResultB = incrementalGenerator.Generate(parseB.CompilationUnit, semaB, layoutB, incrementalOptions);
        DiagnosticAsserts.AssertNoErrors(incrementalResultB.Diagnostics);

        Assert.Equal(fullResultB.Ir, incrementalResultB.Ir);
        Assert.Equal(diff.RecompiledFunctions, incrementalGenerator.LastFunctionsBuilt);
        Assert.Equal(diff.ReusedFunctions, incrementalGenerator.LastFunctionsReused);
    }

    private static (ParseResult Parse, SemanticResult Semantic, LayoutPlan Layout) Analyze(string source)
    {
        var parse = Parser.Parse(source);
        Assert.Empty(parse.Diagnostics);

        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);
        DiagnosticAsserts.AssertNoErrors(sema.Diagnostics);

        var layout = new LayoutPlanner(parse.CompilationUnit, sema.Symbols).Plan();
        return (parse, sema, layout);
    }
}
