using Stasis.Compiler;

namespace Stasis.Compiler.Syntax;

public sealed record CompilationUnitSyntax(IReadOnlyList<DeclarationSyntax> Declarations, Token EndOfFileToken)
    : SyntaxNode(new SourceSpan(
        Declarations.FirstOrDefault()?.Span.Start ?? EndOfFileToken.Span.Start,
        EndOfFileToken.Span.End - (Declarations.FirstOrDefault()?.Span.Start ?? EndOfFileToken.Span.Start)));

public abstract record DeclarationSyntax(SourceSpan Span) : SyntaxNode(Span);

public sealed record StructFieldSyntax(Token Identifier, TypeSyntax Type, Token Semicolon)
    : SyntaxNode(new SourceSpan(Identifier.Span.Start, Semicolon.Span.End - Identifier.Span.Start));

public sealed record StructDeclarationSyntax(Token StructKeyword, Token Name, IReadOnlyList<StructFieldSyntax> Fields, Token CloseBrace)
    : DeclarationSyntax(new SourceSpan(StructKeyword.Span.Start, CloseBrace.Span.End - StructKeyword.Span.Start));

public sealed record EnumMemberSyntax(Token Identifier, Token? TrailingComma)
    : SyntaxNode(TrailingComma is null
        ? Identifier.Span
        : new SourceSpan(Identifier.Span.Start, TrailingComma.Span.End - Identifier.Span.Start));

public sealed record EnumDeclarationSyntax(Token EnumKeyword, Token Name, IReadOnlyList<EnumMemberSyntax> Members, Token CloseBrace)
    : DeclarationSyntax(new SourceSpan(EnumKeyword.Span.Start, CloseBrace.Span.End - EnumKeyword.Span.Start));

public sealed record GlobalDeclarationSyntax(Token GlobalKeyword, Token Name, TypeSyntax Type, Token Semicolon)
    : DeclarationSyntax(new SourceSpan(GlobalKeyword.Span.Start, Semicolon.Span.End - GlobalKeyword.Span.Start));
