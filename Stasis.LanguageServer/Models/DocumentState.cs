namespace Stasis.LanguageServer.Models;

using Stasis.Compiler;
using Stasis.Compiler.Semantic;
using Stasis.Compiler.Syntax;
using Stasis.LanguageServer.Services;

public class DocumentState
{
    public string Content { get; set; } = string.Empty;
    public int Version { get; set; }
    public ParseResult? ParseResult { get; set; }
    public ParseResult? ExpandedParseResult { get; set; }
    public SemanticResult? SemanticResult { get; set; }
    public SymbolIndex? SymbolIndex { get; set; }
    public IReadOnlyList<Diagnostic> AllDiagnostics { get; set; } = Array.Empty<Diagnostic>();
}
