using Stasis.Compiler.Syntax;

namespace Stasis.Compiler;

public sealed class Parser
{
    private readonly IReadOnlyList<Token> _tokens;
    private readonly List<Diagnostic> _diagnostics = new();
    private int _position;

    private Parser(IReadOnlyList<Token> tokens)
    {
        _tokens = tokens;
    }

    public static ParseResult Parse(string text)
    {
        var lex = Lexer.Lex(text);
        var parser = new Parser(lex.Tokens);
        var compilation = parser.ParseCompilationUnit();
        var diagnostics = lex.Diagnostics.Concat(parser._diagnostics).ToArray();
        return new ParseResult(compilation, diagnostics);
    }

    private CompilationUnitSyntax ParseCompilationUnit()
    {
        var declarations = new List<DeclarationSyntax>();
        while (!IsAtEnd() && Current.Kind != TokenKind.EndOfFile)
        {
            var decl = ParseTopLevel();
            if (decl is not null)
            {
                declarations.Add(decl);
            }
        }

        var eof = Consume(TokenKind.EndOfFile, "Expected end of file.");
        return new CompilationUnitSyntax(declarations, eof);
    }

    private DeclarationSyntax? ParseTopLevel()
    {
        return Current.Kind switch
        {
            TokenKind.StructKeyword => ParseStruct(),
            TokenKind.EnumKeyword => ParseEnum(),
            TokenKind.GlobalKeyword => ParseGlobal(),
            TokenKind.ConstKeyword => ParseConst(),
            TokenKind.ExportKeyword or TokenKind.FunctionKeyword => ParseFunction(),
            TokenKind.TestKeyword => ParseTest(),
            _ => UnexpectedTopLevel()
        };
    }

    private DeclarationSyntax? UnexpectedTopLevel()
    {
        AddDiagnostic("Unexpected token at top-level.", Current.Span);
        Advance();
        return null;
    }

    private StructDeclarationSyntax ParseStruct()
    {
        var structKeyword = Consume(TokenKind.StructKeyword, "Expected 'struct'.");
        var name = Consume(TokenKind.Identifier, "Expected struct name.");
        Consume(TokenKind.LBrace, "Expected '{' to start struct body.");

        var fields = new List<StructFieldSyntax>();
        while (Current.Kind != TokenKind.RBrace && Current.Kind != TokenKind.EndOfFile)
        {
            var fieldName = Consume(TokenKind.Identifier, "Expected field name.");
            Consume(TokenKind.Colon, "Expected ':' before type.");
            var type = ParseType();
            var semicolon = Consume(TokenKind.Semicolon, "Expected ';' after field.");
            fields.Add(new StructFieldSyntax(fieldName, type, semicolon));
        }

        var closeBrace = Consume(TokenKind.RBrace, "Expected '}' to close struct.");
        return new StructDeclarationSyntax(structKeyword, name, fields, closeBrace);
    }

    private EnumDeclarationSyntax ParseEnum()
    {
        var enumKeyword = Consume(TokenKind.EnumKeyword, "Expected 'enum'.");
        var name = Consume(TokenKind.Identifier, "Expected enum name.");
        Consume(TokenKind.LBrace, "Expected '{' to start enum body.");

        var members = new List<EnumMemberSyntax>();
        if (Current.Kind != TokenKind.RBrace && Current.Kind != TokenKind.EndOfFile)
        {
            while (true)
            {
                var memberName = Consume(TokenKind.Identifier, "Expected enum member name.");
                Token? trailingComma = null;
                if (Match(TokenKind.Comma))
                {
                    trailingComma = Previous;
                    if (Current.Kind == TokenKind.RBrace)
                    {
                        // allow trailing comma
                    }
                    else
                    {
                        // continue parsing additional members
                    }
                }

                members.Add(new EnumMemberSyntax(memberName, trailingComma));

                if (Current.Kind == TokenKind.RBrace || IsAtEnd())
                {
                    break;
                }

                if (trailingComma is null && Current.Kind != TokenKind.Comma)
                {
                    break;
                }
            }
        }

        var closeBrace = Consume(TokenKind.RBrace, "Expected '}' to close enum.");
        return new EnumDeclarationSyntax(enumKeyword, name, members, closeBrace);
    }

    private GlobalDeclarationSyntax ParseGlobal()
    {
        var globalKeyword = Consume(TokenKind.GlobalKeyword, "Expected 'global'.");
        var name = Consume(TokenKind.Identifier, "Expected global name.");
        Consume(TokenKind.Colon, "Expected ':' before type.");
        var type = ParseType();
        var semicolon = Consume(TokenKind.Semicolon, "Expected ';' after global declaration.");
        return new GlobalDeclarationSyntax(globalKeyword, name, type, semicolon);
    }

    private ConstDeclarationSyntax ParseConst()
    {
        var constKeyword = Consume(TokenKind.ConstKeyword, "Expected 'const'.");
        var name = Consume(TokenKind.Identifier, "Expected constant name.");
        Consume(TokenKind.Colon, "Expected ':' before type.");
        var type = ParseType();
        Consume(TokenKind.Equal, "Expected '=' before constant initializer.");
        var initializer = ParseExpression();
        var semicolon = Consume(TokenKind.Semicolon, "Expected ';' after constant declaration.");
        return new ConstDeclarationSyntax(constKeyword, name, type, initializer, semicolon);
    }

    private FunctionDeclarationSyntax ParseFunction()
    {
        Token? exportKeyword = null;
        if (Match(TokenKind.ExportKeyword))
        {
            exportKeyword = Previous;
        }

        var functionKeyword = Consume(TokenKind.FunctionKeyword, "Expected 'function'.");
        var name = Consume(TokenKind.Identifier, "Expected function name.");
        Consume(TokenKind.LParen, "Expected '(' after function name.");
        var parameters = ParseParameterList();
        Consume(TokenKind.RParen, "Expected ')' after parameters.");
        TypeSyntax? returnType = null;
        if (Match(TokenKind.Colon))
        {
            returnType = ParseType();
        }

        var body = ParseBlock();
        return new FunctionDeclarationSyntax(functionKeyword, name, parameters, returnType, body, exportKeyword);
    }

    private TestDeclarationSyntax ParseTest()
    {
        var testKeyword = Consume(TokenKind.TestKeyword, "Expected 'test'.");
        var name = ConsumeOneOf("Expected test name.", TokenKind.Identifier, TokenKind.BacktickLiteral);
        Consume(TokenKind.LParen, "Expected '(' after test name.");
        var parameters = ParseParameterList();
        Consume(TokenKind.RParen, "Expected ')' after parameters.");
        TypeSyntax? returnType = null;
        if (Match(TokenKind.Colon))
        {
            returnType = ParseType();
        }

        var body = ParseBlock();
        return new TestDeclarationSyntax(testKeyword, name, parameters, returnType, body);
    }

    private IReadOnlyList<ParameterSyntax> ParseParameterList()
    {
        var parameters = new List<ParameterSyntax>();
        if (Current.Kind == TokenKind.RParen)
        {
            return parameters;
        }

        while (true)
        {
            var name = Consume(TokenKind.Identifier, "Expected parameter name.");
            Consume(TokenKind.Colon, "Expected ':' after parameter name.");
            var type = ParseType();
            parameters.Add(new ParameterSyntax(name, type));

            if (!Match(TokenKind.Comma))
            {
                break;
            }

            if (Current.Kind == TokenKind.RParen)
            {
                break;
            }
        }

        return parameters;
    }

    private BlockStatementSyntax ParseBlock()
    {
        var openBrace = Consume(TokenKind.LBrace, "Expected '{'.");
        var statements = new List<StatementSyntax>();
        while (Current.Kind != TokenKind.RBrace && Current.Kind != TokenKind.EndOfFile)
        {
            statements.Add(ParseStatement());
        }

        var closeBrace = Consume(TokenKind.RBrace, "Expected '}'.");
        return new BlockStatementSyntax(openBrace, statements, closeBrace);
    }

    private StatementSyntax ParseStatement()
    {
        return Current.Kind switch
        {
            TokenKind.LBrace => ParseBlock(),
            TokenKind.LetKeyword => ParseVariableDeclaration(),
            TokenKind.IfKeyword => ParseIf(),
            TokenKind.ForKeyword => ParseFor(),
            TokenKind.ForeachKeyword => ParseForeach(),
            TokenKind.ReturnKeyword => ParseReturn(),
            _ => ParseExpressionStatement()
        };
    }

    private VariableDeclarationSyntax ParseVariableDeclaration()
    {
        var letKeyword = Consume(TokenKind.LetKeyword, "Expected 'let'.");
        var name = Consume(TokenKind.Identifier, "Expected variable name.");
        TypeSyntax? type = null;
        if (Match(TokenKind.Colon))
        {
            type = ParseType();
        }

        var semicolon = Consume(TokenKind.Semicolon, "Expected ';' after variable declaration.");
        return new VariableDeclarationSyntax(letKeyword, name, type, semicolon);
    }

    private IfStatementSyntax ParseIf()
    {
        var ifKeyword = Consume(TokenKind.IfKeyword, "Expected 'if'.");
        Consume(TokenKind.LParen, "Expected '(' after 'if'.");
        var condition = ParseExpression();
        Consume(TokenKind.RParen, "Expected ')' after condition.");
        var thenBlock = ParseBlock();
        BlockStatementSyntax? elseBlock = null;
        if (Match(TokenKind.ElseKeyword))
        {
            elseBlock = ParseBlock();
        }

        return new IfStatementSyntax(ifKeyword, condition, thenBlock, elseBlock);
    }

    private ForStatementSyntax ParseFor()
    {
        var forKeyword = Consume(TokenKind.ForKeyword, "Expected 'for'.");
        ExpressionSyntax? initializer = null;
        if (!Match(TokenKind.Semicolon))
        {
            initializer = ParseExpression();
            Consume(TokenKind.Semicolon, "Expected ';' after initializer.");
        }

        ExpressionSyntax? condition = null;
        if (!Match(TokenKind.Semicolon))
        {
            condition = ParseExpression();
            Consume(TokenKind.Semicolon, "Expected ';' after condition.");
        }

        ExpressionSyntax? step = null;
        if (Current.Kind != TokenKind.LBrace)
        {
            step = ParseExpression();
        }

        var body = ParseBlock();
        return new ForStatementSyntax(forKeyword, initializer, condition, step, body);
    }

    private ForeachStatementSyntax ParseForeach()
    {
        var foreachKeyword = Consume(TokenKind.ForeachKeyword, "Expected 'foreach'.");
        Consume(TokenKind.LParen, "Expected '(' after 'foreach'.");
        var iterator = Consume(TokenKind.Identifier, "Expected iterator name.");
        Consume(TokenKind.InKeyword, "Expected 'in'.");
        var iterable = ParseExpression();
        Consume(TokenKind.RParen, "Expected ')' after iterable.");
        var body = ParseBlock();
        return new ForeachStatementSyntax(foreachKeyword, iterator, iterable, body);
    }

    private ReturnStatementSyntax ParseReturn()
    {
        var returnKeyword = Consume(TokenKind.ReturnKeyword, "Expected 'return'.");
        ExpressionSyntax? expression = null;
        if (Current.Kind != TokenKind.Semicolon)
        {
            expression = ParseExpression();
        }

        var semicolon = Consume(TokenKind.Semicolon, "Expected ';' after return.");
        return new ReturnStatementSyntax(returnKeyword, expression, semicolon);
    }

    private ExpressionStatementSyntax ParseExpressionStatement()
    {
        var expr = ParseExpression();
        var semicolon = Consume(TokenKind.Semicolon, "Expected ';' after expression.");
        return new ExpressionStatementSyntax(expr, semicolon);
    }

    private ExpressionSyntax ParseExpression(int minPrecedence = 0)
    {
        var expr = ParsePrefix();

        while (true)
        {
            if (Current.Kind == TokenKind.Dot && Peek(1).Kind == TokenKind.Equal)
            {
                var span = new SourceSpan(Current.Span.Start, Peek(1).Span.End - Current.Span.Start);
                AddDiagnostic("Use infix '=' for assignments instead of '.=()'.", span);
                Advance(); // '.'
                Advance(); // '='

                if (Match(TokenKind.LParen))
                {
                    var depth = 1;
                    while (!IsAtEnd() && depth > 0)
                    {
                        if (Match(TokenKind.LParen))
                        {
                            depth++;
                        }
                        else if (Match(TokenKind.RParen))
                        {
                            depth--;
                        }
                        else
                        {
                            Advance();
                        }
                    }
                }

                continue;
            }

            var precedence = GetBinaryPrecedence(Current.Kind);
            if (precedence < minPrecedence)
            {
                break;
            }

            var op = NextToken();
            var right = ParseExpression(IsRightAssociative(op.Kind) ? precedence : precedence + 1);
            if (IsAssignmentOperator(op.Kind))
            {
                expr = new AssignmentExpressionSyntax(expr, op, right);
            }
            else
            {
                expr = new BinaryExpressionSyntax(expr, op, right);
            }
        }

        return expr;
    }

    private ExpressionSyntax ParsePrefix()
    {
        if (Current.Kind is TokenKind.Bang or TokenKind.Minus)
        {
            var op = NextToken();
            var operand = ParseExpression(PrefixPrecedence);
            return new UnaryExpressionSyntax(op, operand);
        }

        return ParsePostfix();
    }

    private const int PrefixPrecedence = 8;

    private ExpressionSyntax ParsePostfix()
    {
        var expr = ParsePrimary();
        while (true)
        {
            if (Current.Kind == TokenKind.Dot && IsOperatorToken(Peek(1).Kind))
            {
                var dot = Consume(TokenKind.Dot, "Expected '.'.");
                var op = NextToken();
                var lparen = Consume(TokenKind.LParen, "Expected '(' after operator.");
                var args = ParseArgumentList();
                var rparen = Consume(TokenKind.RParen, "Expected ')' after arguments.");
                expr = new OperatorCallExpressionSyntax(expr, dot, op, lparen, args, rparen);
                continue;
            }

            if (Current.Kind == TokenKind.Dot && Peek(1).Kind == TokenKind.Identifier)
            {
                var dot = Consume(TokenKind.Dot, "Expected '.'.");
                var member = Consume(TokenKind.Identifier, "Expected member name.");
                expr = new MemberAccessExpressionSyntax(expr, dot, member);
                continue;
            }

            if (Match(TokenKind.LBracket))
            {
                var lbracket = Previous;
                var index = ParseExpression();
                var rbracket = Consume(TokenKind.RBracket, "Expected ']'.");
                expr = new ArrayAccessExpressionSyntax(expr, lbracket, index, rbracket);
                continue;
            }

            if (Match(TokenKind.LParen))
            {
                var lparen = Previous;
                var args = ParseArgumentList();
                var rparen = Consume(TokenKind.RParen, "Expected ')' after arguments.");
                expr = new CallExpressionSyntax(expr, lparen, args, rparen);
                continue;
            }

            break;
        }

        return expr;
    }

    private int GetBinaryPrecedence(TokenKind kind) =>
        kind switch
        {
            TokenKind.Equal or TokenKind.PlusEqual or TokenKind.MinusEqual or TokenKind.StarEqual or TokenKind.SlashEqual or TokenKind.PercentEqual => 1,
            TokenKind.PipePipe => 2,
            TokenKind.AmpAmp => 3,
            TokenKind.EqualEqual => 4,
            TokenKind.Less or TokenKind.Greater => 5,
            TokenKind.Plus or TokenKind.Minus => 6,
            TokenKind.Star or TokenKind.Slash or TokenKind.Percent => 7,
            _ => -1
        };

    private bool IsRightAssociative(TokenKind kind) => IsAssignmentOperator(kind);

    private bool IsAssignmentOperator(TokenKind kind) =>
        kind is TokenKind.Equal or TokenKind.PlusEqual or TokenKind.MinusEqual or TokenKind.StarEqual or TokenKind.SlashEqual or TokenKind.PercentEqual;

    private IReadOnlyList<ExpressionSyntax> ParseArgumentList()
    {
        var args = new List<ExpressionSyntax>();
        if (Current.Kind == TokenKind.RParen)
        {
            return args;
        }

        while (true)
        {
            args.Add(ParseExpression());
            if (!Match(TokenKind.Comma))
            {
                break;
            }

            if (Current.Kind == TokenKind.RParen)
            {
                break;
            }
        }

        return args;
    }

    private ExpressionSyntax ParsePrimary()
    {
        return Current.Kind switch
        {
            TokenKind.Identifier => new IdentifierExpressionSyntax(NextToken()),
            TokenKind.IntegerLiteral or TokenKind.FloatLiteral or TokenKind.StringLiteral or TokenKind.BacktickLiteral => new LiteralExpressionSyntax(NextToken()),
            TokenKind.TrueKeyword or TokenKind.FalseKeyword => new LiteralExpressionSyntax(NextToken()),
            TokenKind.LParen => ParseParenthesized(),
            _ => UnexpectedPrimary()
        };
    }

    private ExpressionSyntax ParseParenthesized()
    {
        var lparen = Consume(TokenKind.LParen, "Expected '('.");
        var expr = ParseExpression();
        var rparen = Consume(TokenKind.RParen, "Expected ')'.");
        return new ParenthesizedExpressionSyntax(lparen, expr, rparen);
    }

    private ExpressionSyntax UnexpectedPrimary()
    {
        var span = Current.Span;
        if (IsOperatorToken(Current.Kind))
        {
            AddDiagnostic($"Operator tokens must follow a '.' as method calls; found '{Current.Text}'.", span);
        }
        else
        {
            AddDiagnostic("Unexpected token in expression.", span);
        }

        var token = NextToken();
        return new IdentifierExpressionSyntax(token);
    }

    private TypeSyntax ParseType()
    {
        var nameToken = Consume(TokenKind.Identifier, "Expected type name.");
        var type = new NamedTypeSyntax(nameToken);

        if (Match(TokenKind.LBracket))
        {
            var lbracket = Previous;
            var sizeToken = Consume(TokenKind.IntegerLiteral, "Expected array size.");
            var rbracket = Consume(TokenKind.RBracket, "Expected ']'.");
            return new ArrayTypeSyntax(type, lbracket, sizeToken, rbracket);
        }

        return type;
    }

    private bool IsOperatorToken(TokenKind kind) =>
        kind is TokenKind.Plus
        or TokenKind.Minus
        or TokenKind.Star
        or TokenKind.Slash
        or TokenKind.Percent
        or TokenKind.Less
        or TokenKind.Greater
        or TokenKind.EqualEqual;

    private Token Consume(TokenKind kind, string message)
    {
        if (Current.Kind == kind)
        {
            return NextToken();
        }

        var span = Current.Span;
        AddDiagnostic(message, span);
        Advance();
        return new Token(kind, string.Empty, span);
    }

    private Token ConsumeOneOf(string message, params TokenKind[] kinds)
    {
        if (kinds.Contains(Current.Kind))
        {
            return NextToken();
        }

        var span = Current.Span;
        AddDiagnostic(message, span);
        Advance();
        return new Token(kinds.First(), string.Empty, span);
    }

    private bool Match(TokenKind kind)
    {
        if (Current.Kind == kind)
        {
            Advance();
            return true;
        }

        return false;
    }

    private Token NextToken()
    {
        var token = Current;
        Advance();
        return token;
    }

    private void Advance()
    {
        if (!IsAtEnd())
        {
            _position++;
        }
    }

    private bool IsAtEnd() => Current.Kind == TokenKind.EndOfFile || _position >= _tokens.Count;

    private Token Current => _tokens[Math.Min(_position, _tokens.Count - 1)];

    private Token Previous => _tokens[Math.Max(_position - 1, 0)];

    private Token Peek(int offset)
    {
        var index = _position + offset;
        if (index >= _tokens.Count)
        {
            return _tokens[^1];
        }

        return _tokens[index];
    }

    private void AddDiagnostic(string message, SourceSpan span) => _diagnostics.Add(new Diagnostic(message, span));
}
