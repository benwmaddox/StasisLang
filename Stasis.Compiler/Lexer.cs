namespace Stasis.Compiler;

public sealed class Lexer
{
    private static readonly Dictionary<string, TokenKind> Keywords = new(StringComparer.Ordinal)
    {
        ["struct"] = TokenKind.StructKeyword,
        ["enum"] = TokenKind.EnumKeyword,
        ["global"] = TokenKind.GlobalKeyword,
        ["const"] = TokenKind.ConstKeyword,
        ["extern"] = TokenKind.ExternKeyword,
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
            if (_diagnostics.Count >= DiagnosticPolicy.MaxErrors)
            {
                AddToken(TokenKind.EndOfFile, string.Empty, 0);
                break;
            }

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
                case '@':
                    Advance();
                    AddToken(TokenKind.At, "@", 1);
                    break;
                case '&':
                    Advance();
                    if (!IsAtEnd() && Current == '&')
                    {
                        Advance();
                        AddToken(TokenKind.AmpAmp, "&&", 2);
                    }
                    else
                    {
                        AddUnknown(start);
                    }

                    break;
                case '|':
                    Advance();
                    if (!IsAtEnd() && Current == '|')
                    {
                        Advance();
                        AddToken(TokenKind.PipePipe, "||", 2);
                    }
                    else
                    {
                        AddUnknown(start);
                    }

                    break;
                case '!':
                    if (Peek == '=')
                    {
                        Advance();
                        Advance();
                        AddToken(TokenKind.BangEqual, "!=", 2);
                    }
                    else
                    {
                        Advance();
                        AddToken(TokenKind.Bang, "!", 1);
                    }
                    break;
                case '+':
                    LexPlus(start);
                    break;
                case '-':
                    LexMinus(start);
                    break;
                case '*':
                    LexStar(start);
                    break;
                case '/':
                    LexSlash(start);
                    break;
                case '%':
                    LexPercent(start);
                    break;
                case '<':
                    if (Peek == '=')
                    {
                        Advance();
                        Advance();
                        AddToken(TokenKind.LessEqual, "<=", 2);
                    }
                    else
                    {
                        Advance();
                        AddToken(TokenKind.Less, "<", 1);
                    }
                    break;
                case '>':
                    if (Peek == '=')
                    {
                        Advance();
                        Advance();
                        AddToken(TokenKind.GreaterEqual, ">=", 2);
                    }
                    else
                    {
                        Advance();
                        AddToken(TokenKind.Greater, ">", 1);
                    }
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
        bool hasU8Suffix = false;
        var digitsEnd = start;
        while (!IsAtEnd())
        {
            if (char.IsDigit(Current))
            {
                Advance();
                digitsEnd = _position;
                continue;
            }

            if (Current == '.' && !hasDot && PeekIsDigit())
            {
                hasDot = true;
                Advance(); // consume '.'
                Advance(); // consume digit after '.'
                digitsEnd = _position;
                continue;
            }

            break;
        }

        if (!hasDot && !IsAtEnd() && Current == 'u' && Peek == '8')
        {
            var afterIndex = _position + 2;
            var after = afterIndex < _text.Length ? _text[afterIndex] : '\0';
            if (after == '\0' || !(char.IsLetterOrDigit(after) || after == '_'))
            {
                hasU8Suffix = true;
                Advance(); // 'u'
                Advance(); // '8'
            }
        }

        var rawLen = _position - start;
        var digitsText = _text[start..digitsEnd];
        var kind = hasDot ? TokenKind.FloatLiteral : hasU8Suffix ? TokenKind.U8Literal : TokenKind.IntegerLiteral;
        AddToken(kind, digitsText, rawLen);
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

    private void LexPlus(int start)
    {
        Advance();
        if (!IsAtEnd() && Current == '=')
        {
            Advance();
            AddToken(TokenKind.PlusEqual, "+=", 2);
            return;
        }

        AddToken(TokenKind.Plus, "+", 1);
    }

    private void LexMinus(int start)
    {
        Advance();
        if (!IsAtEnd() && Current == '=')
        {
            Advance();
            AddToken(TokenKind.MinusEqual, "-=", 2);
            return;
        }

        AddToken(TokenKind.Minus, "-", 1);
    }

    private void LexStar(int start)
    {
        Advance();
        if (!IsAtEnd() && Current == '=')
        {
            Advance();
            AddToken(TokenKind.StarEqual, "*=", 2);
            return;
        }

        AddToken(TokenKind.Star, "*", 1);
    }

    private void LexSlash(int start)
    {
        Advance();
        if (!IsAtEnd() && Current == '=')
        {
            Advance();
            AddToken(TokenKind.SlashEqual, "/=", 2);
            return;
        }

        AddToken(TokenKind.Slash, "/", 1);
    }

    private void LexPercent(int start)
    {
        Advance();
        if (!IsAtEnd() && Current == '=')
        {
            Advance();
            AddToken(TokenKind.PercentEqual, "%=", 2);
            return;
        }

        AddToken(TokenKind.Percent, "%", 1);
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
        if (_diagnostics.Count >= DiagnosticPolicy.MaxErrors)
        {
            return;
        }

        _diagnostics.Add(new Diagnostic(message, new SourceSpan(start, length)));
    }

    private void AddUnknown(int start)
    {
        AddDiagnostic($"Unrecognized character '{_text[start]}'", start, 1);
        AddToken(TokenKind.Unknown, _text[start].ToString(), 1);
    }

    private void SkipWhitespace()
    {
        while (!IsAtEnd())
        {
            if (char.IsWhiteSpace(Current))
            {
                Advance();
                continue;
            }

            if (Current == '/' && Peek == '/')
            {
                // Line comment
                while (!IsAtEnd() && Current is not '\n' and not '\r')
                {
                    Advance();
                }

                continue;
            }

            if (Current == '/' && Peek == '*')
            {
                // Block comment
                Advance(); // /
                Advance(); // *
                while (!IsAtEnd() && !(Current == '*' && Peek == '/'))
                {
                    Advance();
                }

                if (!IsAtEnd())
                {
                    Advance(); // *
                    Advance(); // /
                }

                continue;
            }

            break;
        }
    }

    private void Advance() => _position++;

    private bool IsAtEnd() => _position >= _text.Length;

    private char Current => _text[_position];

    private char Peek => _position + 1 < _text.Length ? _text[_position + 1] : '\0';

    private bool PeekIsDigit()
    {
        var next = _position + 1;
        return next < _text.Length && char.IsDigit(_text[next]);
    }
}
