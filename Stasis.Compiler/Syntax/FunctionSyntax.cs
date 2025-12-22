using Stasis.Compiler;

namespace Stasis.Compiler.Syntax;

public sealed record ParameterSyntax(Token Name, TypeSyntax Type)
    : SyntaxNode(new SourceSpan(Name.Span.Start, Type.Span.End - Name.Span.Start));

public sealed record FunctionDeclarationSyntax(
    Token FunctionKeyword,
    IReadOnlyList<Token> Attributes,
    Token Name,
    IReadOnlyList<ParameterSyntax> Parameters,
    TypeSyntax? ReturnType,
    BlockStatementSyntax Body,
    Token? ExportKeyword)
    : DeclarationSyntax(new SourceSpan(
        (ExportKeyword ?? FunctionKeyword).Span.Start,
        Body.Span.End - (ExportKeyword ?? FunctionKeyword).Span.Start))
{
    public bool IsExported => ExportKeyword is not null;
}

public sealed record TestDeclarationSyntax(
    Token TestKeyword,
    Token Name,
    IReadOnlyList<ParameterSyntax> Parameters,
    TypeSyntax? ReturnType,
    BlockStatementSyntax Body)
    : DeclarationSyntax(new SourceSpan(TestKeyword.Span.Start, Body.Span.End - TestKeyword.Span.Start));
