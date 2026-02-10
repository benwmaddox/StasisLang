using System.Text.RegularExpressions;
using Stasis.Compiler.IR;
using Stasis.Compiler.Layout;
using Stasis.Compiler.Semantic;
using Stasis.Compiler.Syntax;

namespace Stasis.Compiler.Tests;

public class CallableResolutionParityTests
{
    private const string ModuleName = "parity";

    static CallableResolutionParityTests()
    {
        Stasis.Compiler.LlvmNativeLoader.EnsureLoaded();
    }

    public static IEnumerable<object[]> CallableCases()
    {
        yield return new object[]
        {
            "receiver-overloads",
            """
            struct Enemy { hp: i32; }
            struct Hero { hp: i32; }

            function damage(enemy: Enemy, amount: i32): i32 { return amount; }
            function damage(hero: Hero, amount: i32): i32 { return amount + 1; }

            function main(): i32 {
                let enemy: Enemy = 0;
                let hero: Hero = 0;
                let a: i32 = damage(enemy, 5);
                let b: i32 = damage(hero, 6);
                return a + b;
            }
            """,
            false,
            new[] { "damage__recv__Enemy", "damage__recv__Hero" }
        };

        yield return new object[]
        {
            "receiverless-and-receiver-overload",
            """
            function ping(): i32 { return 7; }
            function ping(value: i32): i32 { return value; }

            function main(): i32 {
                let a: i32 = ping();
                let b: i32 = ping(1);
                return a + b;
            }
            """,
            false,
            new[] { "ping", "ping__recv__i32" }
        };

        yield return new object[]
        {
            "extern-overload-collision-fallback",
            """
            extern function damage(enemy: i32, amount: i32): i32;
            extern function damage(hero: f32, amount: i32): i32;

            function main(): i32 {
                let a: i32 = damage(1, 2);
                let b: i32 = damage(1.0, 2);
                return a + b;
            }
            """,
            true,
            new[] { "damage__recv__i32", "damage__recv__f32" }
        };

        yield return new object[]
        {
            "extern-vs-receiverless-collision-fallback",
            """
            function foo(): i32 { return 7; }
            extern function foo(value: i32): i32;

            function main(): i32 {
                let a: i32 = foo();
                let b: i32 = foo(1);
                return a + b;
            }
            """,
            false,
            new[] { "foo", "foo__recv__i32" }
        };

        yield return new object[]
        {
            "extern-linkname-and-overload",
            """
            function @extern("host_damage_enemy") damage(enemy: i32, amount: i32): i32;
            function damage(enemy: f32, amount: i32): i32 { return amount; }

            function main(): i32 {
                let a: i32 = damage(1, 2);
                let b: i32 = damage(1.0, 2);
                return a + b;
            }
            """,
            false,
            new[] { "host_damage_enemy", "damage__recv__f32" }
        };
    }

    [Theory]
    [MemberData(nameof(CallableCases))]
    public void Differential_CallTargetSelection_MatchesAcrossBackends(
        string _,
        string source,
        bool allowSemanticDiagnostics,
        string[] expectedTargets)
    {
        var (compilationUnit, semantic, layout) = Analyze(source, allowSemanticDiagnostics);

        var llvm = new ModuleLowerer().LowerToIr(compilationUnit, semantic, layout, ModuleName, LowerOptions.Production);
        Assert.Empty(llvm.Diagnostics);
        var llvmTargets = ExtractLlvmCallTargets(llvm.Ir);

        using var generator = CodeGeneratorFactory.Create(BackendType.Cranelift, ModuleName);
        var cranelift = generator.Generate(
            compilationUnit,
            semantic,
            layout,
            new CodeGenerationOptions(ModuleName, IncludeTests: false, EmitTestHarness: false));
        Assert.True(cranelift.Success, string.Join("\n", cranelift.Diagnostics.Select(d => d.Message)));
        var craneliftTargets = ExtractCraneliftCallTargets(cranelift.Ir);

        var expected = expectedTargets.ToHashSet(StringComparer.Ordinal);
        Assert.Equal(expected, llvmTargets);
        Assert.Equal(expected, craneliftTargets);
    }

    private static (CompilationUnitSyntax CompilationUnit, SemanticResult Semantic, LayoutPlan Layout) Analyze(string source, bool allowSemanticDiagnostics)
    {
        var parse = Parser.Parse(source);
        Assert.Empty(parse.Diagnostics);

        var semantic = new SemanticAnalyzer().Analyze(parse.CompilationUnit);
        if (!allowSemanticDiagnostics)
        {
            DiagnosticAsserts.AssertNoErrors(semantic.Diagnostics);
        }

        var layout = new LayoutPlanner(parse.CompilationUnit, semantic.Symbols).Plan();
        return (parse.CompilationUnit, semantic, layout);
    }

    private static HashSet<string> ExtractLlvmCallTargets(string ir)
    {
        var result = new HashSet<string>(StringComparer.Ordinal);
        foreach (Match m in Regex.Matches(ir, @"\bcall\b[^\n@]*@([A-Za-z0-9_\.]+)\(", RegexOptions.CultureInvariant))
        {
            var symbol = m.Groups[1].Value;
            if (symbol.StartsWith("llvm.", StringComparison.Ordinal))
            {
                continue;
            }

            result.Add(symbol);
        }

        return result;
    }

    private static HashSet<string> ExtractCraneliftCallTargets(string ir)
    {
        var result = new HashSet<string>(StringComparer.Ordinal);
        var modulePrefix = $"{ModuleName}__";
        foreach (Match m in Regex.Matches(ir, @"\bcall\s+%([A-Za-z0-9_\.]+)\(", RegexOptions.CultureInvariant))
        {
            var symbol = m.Groups[1].Value;
            if (symbol.StartsWith(modulePrefix, StringComparison.Ordinal))
            {
                symbol = symbol.Substring(modulePrefix.Length);
            }

            result.Add(symbol);
        }

        return result;
    }
}
