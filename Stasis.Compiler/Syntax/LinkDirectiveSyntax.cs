namespace Stasis.Compiler.Syntax;

public sealed record LinkDirectiveSyntax(
    Token AtToken,
    Token Name,
    Token Value,
    Token CloseParen,
    Token Semicolon)
    : DeclarationSyntax(new SourceSpan(AtToken.Span.Start, Semicolon.Span.End - AtToken.Span.Start));
