using Stasis.Compiler;

namespace Stasis.Compiler.Syntax;

public sealed record FunctionAttributeSyntax(
    Token Name,
    Token? OpenParen,
    Token? Value,
    Token? CloseParen)
    : SyntaxNode(new SourceSpan(
        Name.Span.Start,
        ((CloseParen ?? Value ?? OpenParen ?? Name).Span.End) - Name.Span.Start))
{
    public string Text => Name.Text;
    public string? StringValue => Value?.Text;
}

