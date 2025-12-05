using Stasis.Compiler;

namespace Stasis.Compiler.Syntax;

public abstract record StatementSyntax(SourceSpan Span) : SyntaxNode(Span);

public sealed record BlockStatementSyntax(Token OpenBrace, IReadOnlyList<StatementSyntax> Statements, Token CloseBrace)
    : StatementSyntax(new SourceSpan(OpenBrace.Span.Start, CloseBrace.Span.End - OpenBrace.Span.Start));

public sealed record VariableDeclarationSyntax(Token LetKeyword, Token Name, TypeSyntax? Type, Token Semicolon)
    : StatementSyntax(new SourceSpan(LetKeyword.Span.Start, Semicolon.Span.End - LetKeyword.Span.Start));

public sealed record IfStatementSyntax(Token IfKeyword, ExpressionSyntax Condition, BlockStatementSyntax ThenBlock, BlockStatementSyntax? ElseBlock)
    : StatementSyntax(new SourceSpan(IfKeyword.Span.Start, (ElseBlock ?? ThenBlock).Span.End - IfKeyword.Span.Start));

public sealed record ForStatementSyntax(
    Token ForKeyword,
    ExpressionSyntax? Initializer,
    ExpressionSyntax? Condition,
    ExpressionSyntax? Step,
    BlockStatementSyntax Body)
    : StatementSyntax(new SourceSpan(ForKeyword.Span.Start, Body.Span.End - ForKeyword.Span.Start));

public sealed record ForeachStatementSyntax(
    Token ForeachKeyword,
    Token Iterator,
    ExpressionSyntax Iterable,
    BlockStatementSyntax Body)
    : StatementSyntax(new SourceSpan(ForeachKeyword.Span.Start, Body.Span.End - ForeachKeyword.Span.Start));

public sealed record ReturnStatementSyntax(Token ReturnKeyword, ExpressionSyntax? Expression, Token Semicolon)
    : StatementSyntax(new SourceSpan(ReturnKeyword.Span.Start, Semicolon.Span.End - ReturnKeyword.Span.Start));

public sealed record ExpressionStatementSyntax(ExpressionSyntax Expression, Token Semicolon)
    : StatementSyntax(new SourceSpan(Expression.Span.Start, Semicolon.Span.End - Expression.Span.Start));
