using Stasis.Compiler.Syntax;

namespace Stasis.Compiler.Tests;

public class ParserTests
{
    [Fact]
    public void Parses_struct_global_and_function()
    {
        var source = """
            struct Player { hp: u8; }
            global players: Player[10];
            function update(p: Player): void {
                p.hp = p.hp.-(1);
            }
            """;

        var result = Parser.Parse(source);

        Assert.Empty(result.Diagnostics);
        Assert.Equal(3, result.CompilationUnit.Declarations.Count);
        Assert.IsType<StructDeclarationSyntax>(result.CompilationUnit.Declarations[0]);
        Assert.IsType<GlobalDeclarationSyntax>(result.CompilationUnit.Declarations[1]);
        Assert.IsType<FunctionDeclarationSyntax>(result.CompilationUnit.Declarations[2]);
    }

    [Fact]
    public void Parses_test_with_backtick_name()
    {
        var source = """
            test `enemy takes damage`(): bool {
                return true;
            }
            """;

        var result = Parser.Parse(source);

        Assert.Empty(result.Diagnostics);
        var testDecl = Assert.IsType<TestDeclarationSyntax>(Assert.Single(result.CompilationUnit.Declarations));
        Assert.Equal("`enemy takes damage`", testDecl.Name.Text);
    }

    [Fact]
    public void Reports_missing_semicolon_in_let()
    {
        var source = """
            let x : i32
            """;

        var result = Parser.Parse(source);

        Assert.NotEmpty(result.Diagnostics);
    }

    [Fact]
    public void Reports_equal_in_let()
    {
        var source = """
            let x = 1;
            """;

        var result = Parser.Parse(source);

        Assert.NotEmpty(result.Diagnostics);
    }

    [Fact]
    public void Parses_typed_let_without_initializer()
    {
        var source = """
            function f(): void {
                let hp: i32;
            }
            """;

        var result = Parser.Parse(source);

        Assert.Empty(result.Diagnostics);
        var func = Assert.IsType<FunctionDeclarationSyntax>(Assert.Single(result.CompilationUnit.Declarations));
        Assert.IsType<VariableDeclarationSyntax>(Assert.Single(func.Body.Statements));
    }

    [Fact]
    public void Parses_infix_assignment()
    {
        var source = """
            function ok(): void {
                x = 5;
            }
            """;

        var result = Parser.Parse(source);

        Assert.Empty(result.Diagnostics);
        var func = Assert.IsType<FunctionDeclarationSyntax>(Assert.Single(result.CompilationUnit.Declarations));
        var exprStmt = Assert.IsType<ExpressionStatementSyntax>(Assert.Single(func.Body.Statements));
        Assert.IsType<AssignmentExpressionSyntax>(exprStmt.Expression);
    }

    [Fact]
    public void Parses_infix_comparison()
    {
        var source = """
            function ok(): void {
                if (1 < 2) { }
            }
            """;

        var result = Parser.Parse(source);

        Assert.Empty(result.Diagnostics);
        var func = Assert.IsType<FunctionDeclarationSyntax>(Assert.Single(result.CompilationUnit.Declarations));
        var ifStmt = Assert.IsType<IfStatementSyntax>(Assert.Single(func.Body.Statements));
        Assert.IsType<BinaryExpressionSyntax>(ifStmt.Condition);
    }

    [Fact]
    public void Parses_compound_assignment()
    {
        var source = """
            function ok(): void {
                x += 2;
            }
            """;

        var result = Parser.Parse(source);

        Assert.Empty(result.Diagnostics);
        var func = Assert.IsType<FunctionDeclarationSyntax>(Assert.Single(result.CompilationUnit.Declarations));
        var exprStmt = Assert.IsType<ExpressionStatementSyntax>(Assert.Single(func.Body.Statements));
        var assignment = Assert.IsType<AssignmentExpressionSyntax>(exprStmt.Expression);
        Assert.Equal(TokenKind.PlusEqual, assignment.OperatorToken.Kind);
    }

    [Fact]
    public void Respects_operator_precedence()
    {
        var source = """
            function ok(): i32 {
                return 1 + 2 * 3;
            }
            """;

        var result = Parser.Parse(source);

        Assert.Empty(result.Diagnostics);
        var func = Assert.IsType<FunctionDeclarationSyntax>(Assert.Single(result.CompilationUnit.Declarations));
        var ret = Assert.IsType<ReturnStatementSyntax>(Assert.Single(func.Body.Statements));
        var add = Assert.IsType<BinaryExpressionSyntax>(ret.Expression);
        Assert.Equal(TokenKind.Plus, add.OperatorToken.Kind);
        var rhs = Assert.IsType<BinaryExpressionSyntax>(add.Right);
        Assert.Equal(TokenKind.Star, rhs.OperatorToken.Kind);
    }

    [Fact]
    public void Parses_for_with_assignment()
    {
        var source = """
            function loop(): void {
                for i = 0; i.<(10); i = i.+(1) {
                }
            }
            """;

        var result = Parser.Parse(source);

        Assert.Empty(result.Diagnostics);
        var func = Assert.IsType<FunctionDeclarationSyntax>(Assert.Single(result.CompilationUnit.Declarations));
        var forStmt = Assert.IsType<ForStatementSyntax>(Assert.Single(func.Body.Statements));
        Assert.NotNull(forStmt.Initializer);
    }

    [Fact]
    public void Parses_spaced_assignment()
    {
        var source = """
            function demo(): void {
                hp = (5);
            }
            """;

        var result = Parser.Parse(source);

        Assert.Empty(result.Diagnostics);
        var func = Assert.IsType<FunctionDeclarationSyntax>(Assert.Single(result.CompilationUnit.Declarations));
        var exprStmt = Assert.IsType<ExpressionStatementSyntax>(Assert.Single(func.Body.Statements));
        Assert.IsType<AssignmentExpressionSyntax>(exprStmt.Expression);
    }
}
