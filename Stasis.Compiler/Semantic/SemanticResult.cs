namespace Stasis.Compiler.Semantic;

public sealed record SemanticResult(IReadOnlyList<Diagnostic> Diagnostics, IReadOnlyDictionary<string, Symbol> Symbols);
