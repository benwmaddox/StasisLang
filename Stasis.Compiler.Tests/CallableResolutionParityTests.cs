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
            new[] { "damage__Enemy", "damage__Hero" }
        };

        yield return new object[]
        {
            "receiver-overloads-on-primitives",
            """
            function ping(value: i32): i32 { return value; }
            function ping(value: u8): i32 { return 7; }

            function main(): i32 {
                let a: i32 = ping(1);
                let b: i32 = ping(2u8);
                return a + b;
            }
            """,
            false,
            new[] { "ping__i32", "ping__u8" }
        };

        yield return new object[]
        {
            "extern-overloads-with-distinct-link-names",
            """
            function @extern("host_damage_enemy_i32") damage(enemy: i32, amount: i32): i32;
            function @extern("host_damage_enemy_f32") damage(hero: f32, amount: i32): i32;

            function main(): i32 {
                let a: i32 = damage(1, 2);
                let b: i32 = damage(1.0, 2);
                return a + b;
            }
            """,
            false,
            new[] { "host_damage_enemy_i32", "host_damage_enemy_f32" }
        };

        yield return new object[]
        {
            "function-form-binary-first-argument-dispatch",
            """
            function tag(value: i32): i32 { return 1; }
            function tag(value: f32): i32 { return 2; }

            function main(): i32 {
                return tag(1.+(2));
            }
            """,
            false,
            new[] { "tag__i32" }
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
            new[] { "host_damage_enemy", "damage__f32" }
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
