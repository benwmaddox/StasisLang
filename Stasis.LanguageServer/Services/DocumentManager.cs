namespace Stasis.LanguageServer.Services;

using System.Collections.Concurrent;
using Stasis.Compiler;
using Stasis.LanguageServer.Models;

public class DocumentManager
{
    private readonly ConcurrentDictionary<string, DocumentState> _documents = new();

    public DocumentState GetOrCreateDocument(string uri, string initialContent = "")
    {
        return _documents.GetOrAdd(uri, _ => new DocumentState { Content = initialContent });
    }

    public DocumentState? GetDocument(string uri)
    {
        return _documents.TryGetValue(uri, out var doc) ? doc : null;
    }

    public void UpdateDocument(string uri, string newContent, int version)
    {
        if (_documents.TryGetValue(uri, out var doc))
        {
            doc.Content = newContent;
            doc.Version = version;
            ParseDocument(doc);
        }
    }

    public void CloseDocument(string uri)
    {
        _documents.TryRemove(uri, out _);
    }

    private static void ParseDocument(DocumentState doc)
    {
        // Lexing
        var lexResult = Lexer.Lex(doc.Content);

        // Parsing
        var parseResult = Parser.Parse(doc.Content);
        doc.ParseResult = parseResult;
        doc.SymbolIndex = SymbolIndex.Build(parseResult.CompilationUnit);

        // Semantic Analysis
        if (!parseResult.Diagnostics.Any())
        {
            var semanticAnalyzer = new SemanticAnalyzer();
            var semanticResult = semanticAnalyzer.Analyze(parseResult.CompilationUnit);
            doc.SemanticResult = semanticResult;

            // Combine all diagnostics
            var allDiags = new List<Diagnostic>();
            allDiags.AddRange(lexResult.Diagnostics);
            allDiags.AddRange(parseResult.Diagnostics);
            allDiags.AddRange(semanticResult.Diagnostics);
            doc.AllDiagnostics = allDiags;
        }
        else
        {
            // Only include lex and parse diagnostics if parsing failed
            var allDiags = new List<Diagnostic>();
            allDiags.AddRange(lexResult.Diagnostics);
            allDiags.AddRange(parseResult.Diagnostics);
            doc.AllDiagnostics = allDiags;
            doc.SemanticResult = null;
        }
    }

    public IReadOnlyList<DocumentState> GetAllDocuments()
    {
        return _documents.Values.ToList();
    }
}
