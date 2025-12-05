using Stasis.Compiler.Semantic;
using Stasis.Compiler.Syntax;

namespace Stasis.Compiler.Tests;

public class SemanticTests
{
    [Fact]
    public void Flags_unknown_type_in_global()
    {
        var source = """
            global bad: MissingType;
            """;

        var parse = Parser.Parse(source);
        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);

        Assert.Contains(sema.Diagnostics, d => d.Message.Contains("Unknown type"));
    }

    [Fact]
    public void Flags_let_without_type()
    {
        var source = """
            function f(): void {
                let x;
            }
            """;

        var parse = Parser.Parse(source);
        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);

        Assert.Contains(sema.Diagnostics, d => d.Message.Contains("Local variables must declare a type"));
    }

    [Fact]
    public void Flags_local_struct_type()
    {
        var source = """
            struct Player { hp: u8; }
            function f(): void {
                let p: Player;
            }
            """;

        var parse = Parser.Parse(source);
        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);

        Assert.Contains(sema.Diagnostics, d => d.Message.Contains("Locals and parameters must be primitive types"));
    }

    [Fact]
    public void Flags_parameter_struct_type()
    {
        var source = """
            struct Player { hp: u8; }
            function f(p: Player): void {
            }
            """;

        var parse = Parser.Parse(source);
        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);

        Assert.Contains(sema.Diagnostics, d => d.Message.Contains("Locals and parameters must be primitive types"));
    }

    [Fact]
    public void Allows_global_struct()
    {
        var source = """
            struct Player { hp: u8; }
            global players: Player[10];
            """;

        var parse = Parser.Parse(source);
        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);

        Assert.Empty(sema.Diagnostics);
        Assert.Contains("players", sema.Symbols.Keys);
    }

    [Fact]
    public void Flags_assignment_to_literal()
    {
        var source = """
            function f(): void {
                5.=(3);
            }
            """;

        var parse = Parser.Parse(source);
        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);

        Assert.Contains(sema.Diagnostics, d => d.Message.Contains("assignable location"));
    }

    [Fact]
    public void Flags_operator_wrong_arity()
    {
        var source = """
            function f(): void {
                x.=(1, 2);
            }
            """;

        var parse = Parser.Parse(source);
        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);

        Assert.Contains(sema.Diagnostics, d => d.Message.Contains("requires exactly one argument"));
    }

    [Fact]
    public void Flags_undefined_identifier()
    {
        var source = """
            function f(): void {
                hp.=(1);
            }
            """;

        var parse = Parser.Parse(source);
        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);

        Assert.Contains(sema.Diagnostics, d => d.Message.Contains("Undefined identifier 'hp'"));
    }
}
