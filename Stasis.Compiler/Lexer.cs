namespace Stasis.Compiler;

public sealed class Lexer
{
    private static readonly Dictionary<string, TokenKind> Keywords = new(StringComparer.Ordinal)
    {
        ["struct"] = TokenKind.StructKeyword,
        ["enum"] = TokenKind.EnumKeyword,
        ["global"] = TokenKind.GlobalKeyword,
        ["function"] = TokenKind.FunctionKeyword,
        ["export"] = TokenKind.ExportKeyword,
        ["test"] = TokenKind.TestKeyword,
        ["return"] = TokenKind.ReturnKeyword,
        ["let"] = TokenKind.LetKeyword,
        ["if"] = TokenKind.IfKeyword,
        ["else"] = TokenKind.ElseKeyword,
        ["for"] = TokenKind.ForKeyword,
        ["foreach"] = TokenKind.ForeachKeyword,
        ["in"] = TokenKind.InKeyword,
        ["true"] = TokenKind.TrueKeyword,
        ["false"] = TokenKind.FalseKeyword
    };

    private readonly string _text;
    private readonly List<Token> _tokens = new();
    private readonly List<Diagnostic> _diagnostics = new();
    private int _position;

    private Lexer(string text)
    {
        _text = text;
    }

    public static LexResult Lex(string text)
    {
        var lexer = new Lexer(text);
        lexer.LexTokens();
        return new LexResult(lexer._tokens, lexer._diagnostics);
    }

    private void LexTokens()
    {
        while (true)
        {
            SkipWhitespace();
            if (IsAtEnd())
            {
                AddToken(TokenKind.EndOfFile, string.Empty, 0);
                break;
            }

            var start = _position;
            var ch = Current;
            if (char.IsLetter(ch))
            {
                LexIdentifier(start);
                continue;
            }

            if (char.IsDigit(ch))
            {
                LexNumber(start);
                continue;
            }

            switch (ch)
            {
                case '"':
                    LexString(start);
                    break;
                case '`':
                    LexBacktick(start);
                    break;
                case '.':
                    Advance();
                    AddToken(TokenKind.Dot, ".", 1);
                    break;
                case '+':
                    Advance();
                    AddToken(TokenKind.Plus, "+", 1);
                    break;
                case '-':
                    Advance();
                    AddToken(TokenKind.Minus, "-", 1);
                    break;
                case '*':
                    Advance();
                    AddToken(TokenKind.Star, "*", 1);
                    break;
                case '/':
                    Advance();
                    AddToken(TokenKind.Slash, "/", 1);
                    break;
                case '%':
                    Advance();
                    AddToken(TokenKind.Percent, "%", 1);
                    break;
                case '!':
                    Advance();
                    AddToken(TokenKind.Bang, "!", 1);
                    break;
                case '<':
                    Advance();
                    AddToken(TokenKind.Less, "<", 1);
                    break;
                case '>':
                    Advance();
                    AddToken(TokenKind.Greater, ">", 1);
                    break;
                case ':':
                    Advance();
                    AddToken(TokenKind.Colon, ":", 1);
                    break;
                case '=':
                    LexEquals(start);
                    break;
                case '(':
                    Advance();
                    AddToken(TokenKind.LParen, "(", 1);
                    break;
                case ')':
                    Advance();
                    AddToken(TokenKind.RParen, ")", 1);
                    break;
                case '{':
                    Advance();
                    AddToken(TokenKind.LBrace, "{", 1);
                    break;
                case '}':
                    Advance();
                    AddToken(TokenKind.RBrace, "}", 1);
                    break;
                case '[':
                    Advance();
                    AddToken(TokenKind.LBracket, "[", 1);
                    break;
                case ']':
                    Advance();
                    AddToken(TokenKind.RBracket, "]", 1);
                    break;
                case ',':
                    Advance();
                    AddToken(TokenKind.Comma, ",", 1);
                    break;
                case ';':
                    Advance();
                    AddToken(TokenKind.Semicolon, ";", 1);
                    break;
                default:
                    Advance();
                    AddUnknown(start);
                    break;
            }
        }
    }

    private void LexIdentifier(int start)
    {
        while (!IsAtEnd() && (char.IsLetterOrDigit(Current) || Current == '_'))
        {
            Advance();
        }

        var text = _text[start.._position];
        if (Keywords.TryGetValue(text, out var keywordKind))
        {
            AddToken(keywordKind, text, _position - start);
        }
        else
        {
            AddToken(TokenKind.Identifier, text, _position - start);
        }
    }

    private void LexNumber(int start)
    {
        bool hasDot = false;
        while (!IsAtEnd())
        {
            if (char.IsDigit(Current))
            {
                Advance();
                continue;
            }

            if (Current == '.' && !hasDot && PeekIsDigit())
            {
                hasDot = true;
                Advance(); // consume '.'
                Advance(); // consume digit after '.'
                continue;
            }

            break;
        }

        var text = _text[start.._position];
        var kind = hasDot ? TokenKind.FloatLiteral : TokenKind.IntegerLiteral;
        AddToken(kind, text, _position - start);
    }

    private void LexString(int start)
    {
        Advance(); // opening quote
        while (!IsAtEnd() && Current != '"')
        {
            Advance();
        }

        if (IsAtEnd())
        {
            AddDiagnostic("Unterminated string literal.", start, _position - start);
            AddToken(TokenKind.StringLiteral, _text[start.._position], _position - start);
            return;
        }

        Advance(); // closing quote
        AddToken(TokenKind.StringLiteral, _text[start.._position], _position - start);
    }

    private void LexBacktick(int start)
    {
        Advance(); // opening backtick
        while (!IsAtEnd() && Current != '`')
        {
            Advance();
        }

        if (IsAtEnd())
        {
            AddDiagnostic("Unterminated backtick literal.", start, _position - start);
            AddToken(TokenKind.BacktickLiteral, _text[start.._position], _position - start);
            return;
        }

        Advance(); // closing backtick
        AddToken(TokenKind.BacktickLiteral, _text[start.._position], _position - start);
    }

    private void LexEquals(int start)
    {
        Advance(); // consume '='
        if (!IsAtEnd() && Current == '=')
        {
            Advance();
            AddToken(TokenKind.EqualEqual, "==", 2);
            return;
        }

        AddToken(TokenKind.Equal, "=", 1);
    }

    private void AddToken(TokenKind kind, string text, int length)
    {
        _tokens.Add(new Token(kind, text, new SourceSpan(_position - length, length)));
    }

    private void AddDiagnostic(string message, int start, int length)
    {
        _diagnostics.Add(new Diagnostic(message, new SourceSpan(start, length)));
    }

    private void AddUnknown(int start)
    {
        AddDiagnostic($"Unrecognized character '{_text[start]}'", start, 1);
        AddToken(TokenKind.Unknown, _text[start].ToString(), 1);
    }

    private void SkipWhitespace()
    {
        while (!IsAtEnd() && char.IsWhiteSpace(Current))
        {
            Advance();
        }
    }

    private void Advance() => _position++;

    private bool IsAtEnd() => _position >= _text.Length;

    private char Current => _text[_position];

    private bool PeekIsDigit()
    {
        var next = _position + 1;
        return next < _text.Length && char.IsDigit(_text[next]);
    }
}
