namespace Stasis.Compiler.Semantic;

public sealed record Symbol(string Name, SymbolKind Kind, TypeSymbol? Type);
