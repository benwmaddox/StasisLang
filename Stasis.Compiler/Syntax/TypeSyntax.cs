using Stasis.Compiler;

namespace Stasis.Compiler.Syntax;

public abstract record TypeSyntax(SourceSpan Span) : SyntaxNode(Span);

public sealed record NamedTypeSyntax(Token NameToken) : TypeSyntax(NameToken.Span)
{
    public string Name => NameToken.Text;
}

public sealed record ArrayTypeSyntax(TypeSyntax ElementType, Token LBracket, Token? SizeToken, Token RBracket)
    : TypeSyntax(new SourceSpan(ElementType.Span.Start, RBracket.Span.End - ElementType.Span.Start))
{
    public string SizeText => SizeToken?.Text ?? string.Empty;
}
