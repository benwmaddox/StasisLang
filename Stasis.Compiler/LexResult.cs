namespace Stasis.Compiler;

public sealed record LexResult(IReadOnlyList<Token> Tokens, IReadOnlyList<Diagnostic> Diagnostics);
