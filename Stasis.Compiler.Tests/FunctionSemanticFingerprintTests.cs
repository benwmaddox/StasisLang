using Stasis.Compiler.Layout;

namespace Stasis.Compiler.Tests;

public class FunctionSemanticFingerprintTests
{
    [Fact]
    public void Ignores_whitespace_and_comments()
    {
        var sourceA = """
            global counter: i32;

            function main(): i32 {
                return 0;
            }

            function tick(): i32 {
                counter = counter + 1;
                return counter;
            }
            """;

        var sourceB = """
            // formatting-only edit
            global counter:i32; /* inline comment */

            function main(): i32 { return 0; }

            function tick(): i32 {
                counter=counter+1; // same semantics
                return counter;
            }
            """;

        var profileA = Compute(sourceA);
        var profileB = Compute(sourceB);
        var diff = FunctionSemanticFingerprint.Diff(profileA, profileB);

        Assert.Equal(profileA.LayoutHash, profileB.LayoutHash);
        Assert.Equal(profileA.Functions.Count, profileB.Functions.Count);
        Assert.False(diff.AnyChange);
        Assert.False(diff.RequiresConservativeRebuild);
        Assert.Empty(diff.ChangedBodyCallableKeys);
    }

    [Fact]
    public void Body_change_only_marks_affected_function()
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

        var profileA = Compute(sourceA);
        var profileB = Compute(sourceB);
        var diff = FunctionSemanticFingerprint.Diff(profileA, profileB);

        Assert.True(diff.AnyChange);
        Assert.False(diff.LayoutChanged);
        Assert.False(diff.DeclarationChanged);
        Assert.False(diff.SignatureChanged);
        Assert.False(diff.InlineBodyChanged);
        Assert.False(diff.FunctionSetChanged);
        Assert.False(diff.RequiresConservativeRebuild);
        Assert.Single(diff.ChangedBodyCallableKeys);
        Assert.Equal(1, diff.RecompiledFunctions);
        Assert.Equal(profileB.Functions.Count - 1, diff.ReusedFunctions);
    }

    [Fact]
    public void Signature_change_forces_conservative_rebuild()
    {
        var sourceA = """
            function main(): i32 {
                return apply(2, 3);
            }

            function apply(x: i32, y: i32): i32 {
                return x + y;
            }
            """;

        var sourceB = """
            function main(): i32 {
                return apply(2, 3, 4);
            }

            function apply(x: i32, y: i32, z: i32): i32 {
                return x + y + z;
            }
            """;

        var profileA = Compute(sourceA);
        var profileB = Compute(sourceB);
        var diff = FunctionSemanticFingerprint.Diff(profileA, profileB);

        Assert.True(diff.AnyChange);
        Assert.True(diff.DeclarationChanged);
        Assert.True(diff.SignatureChanged || diff.FunctionSetChanged);
        Assert.True(diff.RequiresConservativeRebuild);
        Assert.Equal(profileB.Functions.Count, diff.RecompiledFunctions);
        Assert.Equal(0, diff.ReusedFunctions);
    }

    [Fact]
    public void Layout_change_forces_conservative_rebuild()
    {
        var sourceA = """
            global values: i32[2];

            function main(): i32 {
                return 0;
            }

            function tick(): i32 {
                return values[0];
            }
            """;

        var sourceB = """
            global values: i32[3];

            function main(): i32 {
                return 0;
            }

            function tick(): i32 {
                return values[0];
            }
            """;

        var profileA = Compute(sourceA);
        var profileB = Compute(sourceB);
        var diff = FunctionSemanticFingerprint.Diff(profileA, profileB);

        Assert.True(diff.AnyChange);
        Assert.True(diff.LayoutChanged);
        Assert.True(diff.RequiresConservativeRebuild);
        Assert.Equal(profileB.Functions.Count, diff.RecompiledFunctions);
        Assert.Equal(0, diff.ReusedFunctions);
    }

    [Fact]
    public void Const_initializer_change_forces_conservative_rebuild()
    {
        var sourceA = """
            const bonus: i32 = 1;

            function main(): i32 {
                return tick();
            }

            function tick(): i32 {
                return bonus;
            }
            """;

        var sourceB = """
            const bonus: i32 = 2;

            function main(): i32 {
                return tick();
            }

            function tick(): i32 {
                return bonus;
            }
            """;

        var profileA = Compute(sourceA);
        var profileB = Compute(sourceB);
        var diff = FunctionSemanticFingerprint.Diff(profileA, profileB);

        Assert.True(diff.AnyChange);
        Assert.True(diff.DeclarationChanged);
        Assert.True(diff.RequiresConservativeRebuild);
        Assert.Equal(profileB.Functions.Count, diff.RecompiledFunctions);
        Assert.Equal(0, diff.ReusedFunctions);
    }

    [Fact]
    public void Inline_callee_body_change_forces_conservative_rebuild()
    {
        var sourceA = """
            function @inline inc(value: i32): i32 {
                return value + 1;
            }

            function main(): i32 {
                return tick();
            }

            function tick(): i32 {
                return inc(10);
            }
            """;

        var sourceB = """
            function @inline inc(value: i32): i32 {
                return value + 2;
            }

            function main(): i32 {
                return tick();
            }

            function tick(): i32 {
                return inc(10);
            }
            """;

        var profileA = Compute(sourceA);
        var profileB = Compute(sourceB);
        var diff = FunctionSemanticFingerprint.Diff(profileA, profileB);

        Assert.True(diff.AnyChange);
        Assert.True(diff.InlineBodyChanged);
        Assert.True(diff.RequiresConservativeRebuild);
        Assert.Equal(profileB.Functions.Count, diff.RecompiledFunctions);
        Assert.Equal(0, diff.ReusedFunctions);
    }

    private static FunctionSemanticProfile Compute(string source)
    {
        var parse = Parser.Parse(source);
        Assert.Empty(parse.Diagnostics);

        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);
        DiagnosticAsserts.AssertNoErrors(sema.Diagnostics);

        var layout = new LayoutPlanner(parse.CompilationUnit, sema.Symbols).Plan();
        return FunctionSemanticFingerprint.ComputeProfile(
            source,
            parse.CompilationUnit,
            layout,
            includeTests: false,
            allowReachabilityFallback: true);
    }
}
