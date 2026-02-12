namespace Stasis.Compiler;

public sealed record Token(TokenKind Kind, string Text, SourceSpan Span);
