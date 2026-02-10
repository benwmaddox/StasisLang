using Stasis.Compiler.Layout;

namespace Stasis.Compiler.Tests;

public class SemanticFingerprintTests
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

        var hashA = Compute(sourceA);
        var hashB = Compute(sourceB);

        Assert.Equal(hashA, hashB);
    }

    [Fact]
    public void Changes_when_behavior_changes()
    {
        var sourceA = """
            function main(): i32 {
                return 0;
            }

            function tick(): i32 {
                return 1;
            }
            """;

        var sourceB = """
            function main(): i32 {
                return 0;
            }

            function tick(): i32 {
                return 2;
            }
            """;

        var hashA = Compute(sourceA);
        var hashB = Compute(sourceB);

        Assert.NotEqual(hashA, hashB);
    }

    [Fact]
    public void Changes_when_layout_changes()
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

        var hashA = Compute(sourceA);
        var hashB = Compute(sourceB);

        Assert.NotEqual(hashA, hashB);
    }

    private static string Compute(string source)
    {
        var parse = Parser.Parse(source);
        Assert.Empty(parse.Diagnostics);

        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);
        DiagnosticAsserts.AssertNoErrors(sema.Diagnostics);

        var layout = new LayoutPlanner(parse.CompilationUnit, sema.Symbols).Plan();
        return SemanticFingerprint.ComputeFileFingerprint(source, layout);
    }
}
