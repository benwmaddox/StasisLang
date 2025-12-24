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
            function f(): void {
                let x = 1;
            }
            """;

        var result = Parser.Parse(source);

        Assert.Empty(result.Diagnostics);
        var sema = new SemanticAnalyzer().Analyze(result.CompilationUnit);
        Assert.NotEmpty(sema.Diagnostics);
    }

    [Fact]
    public void Parses_typed_let_without_initializer_still_parses()
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
        var sema = new SemanticAnalyzer().Analyze(result.CompilationUnit);
        Assert.Contains(sema.Diagnostics, d => d.Message.Contains("must be initialized"));
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
    public void Parses_infix_extended_comparisons()
    {
        var source = """
            function ok(): void {
                if (1 <= 2) { }
                if (2 >= 1) { }
                if (1 != 2) { }
            }
            """;

        var result = Parser.Parse(source);

        Assert.Empty(result.Diagnostics);
        var func = Assert.IsType<FunctionDeclarationSyntax>(Assert.Single(result.CompilationUnit.Declarations));
        Assert.Equal(3, func.Body.Statements.Count);
        var if1 = Assert.IsType<IfStatementSyntax>(func.Body.Statements[0]);
        var if2 = Assert.IsType<IfStatementSyntax>(func.Body.Statements[1]);
        var if3 = Assert.IsType<IfStatementSyntax>(func.Body.Statements[2]);
        Assert.Equal(TokenKind.LessEqual, Assert.IsType<BinaryExpressionSyntax>(if1.Condition).OperatorToken.Kind);
        Assert.Equal(TokenKind.GreaterEqual, Assert.IsType<BinaryExpressionSyntax>(if2.Condition).OperatorToken.Kind);
        Assert.Equal(TokenKind.BangEqual, Assert.IsType<BinaryExpressionSyntax>(if3.Condition).OperatorToken.Kind);
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
                for (i = 0; i.<(10); i = i.+(1)) {
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

    [Fact]
    public void Parses_foreach_without_index()
    {
        var source = """
            global values: i32[4];
            function f(): void {
                foreach (let v in values) {
                }
            }
            """;

        var result = Parser.Parse(source);

        Assert.Empty(result.Diagnostics);
        var func = Assert.IsType<FunctionDeclarationSyntax>(result.CompilationUnit.Declarations[1]);
        var foreachStmt = Assert.IsType<ForeachStatementSyntax>(Assert.Single(func.Body.Statements));
        Assert.Equal("v", foreachStmt.Iterator.Text);
        Assert.Null(foreachStmt.IndexVariable);
        Assert.True(foreachStmt.BindByElement);
    }

    [Fact]
    public void Parses_foreach_with_index()
    {
        var source = """
            global values: i32[4];
            function f(): void {
                foreach (let v, i in values) {
                }
            }
            """;

        var result = Parser.Parse(source);

        Assert.Empty(result.Diagnostics);
        var func = Assert.IsType<FunctionDeclarationSyntax>(result.CompilationUnit.Declarations[1]);
        var foreachStmt = Assert.IsType<ForeachStatementSyntax>(Assert.Single(func.Body.Statements));
        Assert.Equal("v", foreachStmt.Iterator.Text);
        Assert.NotNull(foreachStmt.IndexVariable);
        Assert.Equal("i", foreachStmt.IndexVariable.Text);
        Assert.True(foreachStmt.BindByElement);
    }

    [Fact]
    public void Parses_foreach_index_only()
    {
        var source = """
            global values: i32[4];
            function f(): void {
                foreach (i in values) {
                }
            }
            """;

        var result = Parser.Parse(source);

        Assert.Empty(result.Diagnostics);
        var func = Assert.IsType<FunctionDeclarationSyntax>(result.CompilationUnit.Declarations[1]);
        var foreachStmt = Assert.IsType<ForeachStatementSyntax>(Assert.Single(func.Body.Statements));
        Assert.Equal("i", foreachStmt.Iterator.Text);
        Assert.Null(foreachStmt.IndexVariable);
        Assert.False(foreachStmt.BindByElement);
    }

    [Fact]
    public void Parses_enum_declaration()
    {
        var source = """
            enum State { Idle, Jump, Run, Fall }
            """;

        var result = Parser.Parse(source);

        Assert.Empty(result.Diagnostics);
        var enumDecl = Assert.IsType<EnumDeclarationSyntax>(Assert.Single(result.CompilationUnit.Declarations));
        Assert.Equal("State", enumDecl.Name.Text);
        Assert.Equal(4, enumDecl.Members.Count);
        Assert.Equal("Idle", enumDecl.Members[0].Identifier.Text);
        Assert.Equal("Jump", enumDecl.Members[1].Identifier.Text);
        Assert.Equal("Run", enumDecl.Members[2].Identifier.Text);
        Assert.Equal("Fall", enumDecl.Members[3].Identifier.Text);
    }

    [Fact]
    public void Parses_enum_with_trailing_comma()
    {
        var source = """
            enum Direction { North, South, East, West, }
            """;

        var result = Parser.Parse(source);

        Assert.Empty(result.Diagnostics);
        var enumDecl = Assert.IsType<EnumDeclarationSyntax>(Assert.Single(result.CompilationUnit.Declarations));
        Assert.Equal(4, enumDecl.Members.Count);
    }

    [Fact]
    public void Parses_enum_with_explicit_values()
    {
        var source = """
            enum Key { Escape = 41, Space = 44, Left = 80, Right = 79, }
            """;

        var result = Parser.Parse(source);

        Assert.Empty(result.Diagnostics);
        var enumDecl = Assert.IsType<EnumDeclarationSyntax>(Assert.Single(result.CompilationUnit.Declarations));
        Assert.Equal(4, enumDecl.Members.Count);
        Assert.Equal("Escape", enumDecl.Members[0].Identifier.Text);
        Assert.Equal("41", enumDecl.Members[0].ValueToken?.Text);
        Assert.Equal("Space", enumDecl.Members[1].Identifier.Text);
        Assert.Equal("44", enumDecl.Members[1].ValueToken?.Text);
    }

    [Fact]
    public void Parses_enum_member_access()
    {
        var source = """
            enum State { Idle, Jump }
            function get_state(): i32 {
                return State.Idle;
            }
            """;

        var result = Parser.Parse(source);

        Assert.Empty(result.Diagnostics);
        var func = Assert.IsType<FunctionDeclarationSyntax>(result.CompilationUnit.Declarations[1]);
        var ret = Assert.IsType<ReturnStatementSyntax>(Assert.Single(func.Body.Statements));
        var memberAccess = Assert.IsType<MemberAccessExpressionSyntax>(ret.Expression);
        var receiver = Assert.IsType<IdentifierExpressionSyntax>(memberAccess.Receiver);
        Assert.Equal("State", receiver.Identifier.Text);
        Assert.Equal("Idle", memberAccess.Member.Text);
    }
}
