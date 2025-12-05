using Stasis.Compiler;

namespace Stasis.Compiler.Tests;

public class LexerTests
{
    [Fact]
    public void Lexes_keywords_and_identifiers()
    {
        var input = "struct enum global function export test return let if else for foreach in true false foo bar";

        var result = Lexer.Lex(input);

        Assert.Empty(result.Diagnostics);
        Assert.Equal(
            new[]
            {
                TokenKind.StructKeyword,
                TokenKind.EnumKeyword,
                TokenKind.GlobalKeyword,
                TokenKind.FunctionKeyword,
                TokenKind.ExportKeyword,
                TokenKind.TestKeyword,
                TokenKind.ReturnKeyword,
                TokenKind.LetKeyword,
                TokenKind.IfKeyword,
                TokenKind.ElseKeyword,
                TokenKind.ForKeyword,
                TokenKind.ForeachKeyword,
                TokenKind.InKeyword,
                TokenKind.TrueKeyword,
                TokenKind.FalseKeyword,
                TokenKind.Identifier,
                TokenKind.Identifier,
                TokenKind.EndOfFile
            },
            result.Tokens.Select(t => t.Kind).ToArray());
    }

    [Fact]
    public void Lexes_operator_method_chain()
    {
        var input = "value.=(other.+(1))";

        var result = Lexer.Lex(input);

        Assert.Empty(result.Diagnostics);
        Assert.Equal(
            new[]
            {
                TokenKind.Identifier,
                TokenKind.Dot,
                TokenKind.Equal,
                TokenKind.LParen,
                TokenKind.Identifier,
                TokenKind.Dot,
                TokenKind.Plus,
                TokenKind.LParen,
                TokenKind.IntegerLiteral,
                TokenKind.RParen,
                TokenKind.RParen,
                TokenKind.EndOfFile
            },
            result.Tokens.Select(t => t.Kind).ToArray());
    }

    [Fact]
    public void Lexes_literals()
    {
        var input = "\"text\" `case name` 123 4.56";

        var result = Lexer.Lex(input);

        Assert.Empty(result.Diagnostics);
        Assert.Equal(
            new[]
            {
                TokenKind.StringLiteral,
                TokenKind.BacktickLiteral,
                TokenKind.IntegerLiteral,
                TokenKind.FloatLiteral,
                TokenKind.EndOfFile
            },
            result.Tokens.Select(t => t.Kind).ToArray());
    }

    [Fact]
    public void Reports_unterminated_string()
    {
        var input = "\"unterminated";

        var result = Lexer.Lex(input);

        Assert.Single(result.Diagnostics);
        Assert.Contains("Unterminated string literal", result.Diagnostics[0].Message);
        Assert.Equal(TokenKind.EndOfFile, result.Tokens.Last().Kind);
    }
}
