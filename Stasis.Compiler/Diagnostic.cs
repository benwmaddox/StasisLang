namespace Stasis.Compiler;

public sealed record Diagnostic(string Message, SourceSpan Span, string? FilePath = null);
