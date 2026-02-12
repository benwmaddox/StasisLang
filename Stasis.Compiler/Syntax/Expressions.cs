using Stasis.Compiler;

namespace Stasis.Compiler.Syntax;

public abstract record ExpressionSyntax(SourceSpan Span) : SyntaxNode(Span);

public sealed record IdentifierExpressionSyntax(Token Identifier)
    : ExpressionSyntax(Identifier.Span);

public sealed record LiteralExpressionSyntax(Token Literal)
    : ExpressionSyntax(Literal.Span);

public sealed record ParenthesizedExpressionSyntax(Token OpenParen, ExpressionSyntax Expression, Token CloseParen)
    : ExpressionSyntax(new SourceSpan(OpenParen.Span.Start, CloseParen.Span.End - OpenParen.Span.Start));

public sealed record UnaryExpressionSyntax(Token OperatorToken, ExpressionSyntax Operand)
    : ExpressionSyntax(new SourceSpan(OperatorToken.Span.Start, Operand.Span.End - OperatorToken.Span.Start));

public sealed record MemberAccessExpressionSyntax(ExpressionSyntax Receiver, Token DotToken, Token Member)
    : ExpressionSyntax(new SourceSpan(Receiver.Span.Start, Member.Span.End - Receiver.Span.Start));

public sealed record ArrayAccessExpressionSyntax(ExpressionSyntax Receiver, Token LBracket, ExpressionSyntax Index, Token RBracket)
    : ExpressionSyntax(new SourceSpan(Receiver.Span.Start, RBracket.Span.End - Receiver.Span.Start));

public sealed record CallExpressionSyntax(ExpressionSyntax Callee, Token LParen, IReadOnlyList<ExpressionSyntax> Arguments, Token RParen)
    : ExpressionSyntax(new SourceSpan(Callee.Span.Start, RParen.Span.End - Callee.Span.Start));

public sealed record OperatorCallExpressionSyntax(ExpressionSyntax Receiver, Token DotToken, Token OperatorToken, Token LParen, IReadOnlyList<ExpressionSyntax> Arguments, Token RParen)
    : ExpressionSyntax(new SourceSpan(Receiver.Span.Start, RParen.Span.End - Receiver.Span.Start));

public sealed record AssignmentExpressionSyntax(ExpressionSyntax Left, Token OperatorToken, ExpressionSyntax Right)
    : ExpressionSyntax(new SourceSpan(Left.Span.Start, Right.Span.End - Left.Span.Start));

public sealed record BinaryExpressionSyntax(ExpressionSyntax Left, Token OperatorToken, ExpressionSyntax Right)
    : ExpressionSyntax(new SourceSpan(Left.Span.Start, Right.Span.End - Left.Span.Start));

public sealed record StructInitializerFieldSyntax(Token Name, Token EqualsToken, ExpressionSyntax Value, Token? TrailingComma)
    : SyntaxNode(new SourceSpan(Name.Span.Start, (TrailingComma is not null ? TrailingComma.Span.End : Value.Span.End) - Name.Span.Start));

public sealed record StructInitializerExpressionSyntax(Token OpenBrace, IReadOnlyList<StructInitializerFieldSyntax> Fields, Token CloseBrace)
    : ExpressionSyntax(new SourceSpan(OpenBrace.Span.Start, CloseBrace.Span.End - OpenBrace.Span.Start));
