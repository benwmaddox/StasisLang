namespace Stasis.LanguageServer.Services;

using System.Collections.Concurrent;
using System.Text;
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
        var parseContent = StripImportLines(doc.Content);

        // Lexing
        var lexResult = Lexer.Lex(parseContent);

        // Parsing
        var parseResult = Parser.Parse(parseContent);
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

    private static string StripImportLines(string content)
    {
        var sb = new StringBuilder(content.Length);
        var lineStart = 0;
        var index = 0;
        while (index <= content.Length)
        {
            var isEnd = index == content.Length;
            var ch = isEnd ? '\n' : content[index];
            if (ch == '\n' || isEnd)
            {
                var lineLength = index - lineStart;
                var lineText = content.Substring(lineStart, lineLength);
                var trimmedLine = lineText.TrimEnd('\r');
                var hasCarriageReturn = lineText.EndsWith('\r');

                if (IsImportLine(trimmedLine))
                {
                    sb.Append(' ', trimmedLine.Length);
                    if (hasCarriageReturn)
                    {
                        sb.Append('\r');
                    }
                }
                else
                {
                    sb.Append(lineText);
                }

                if (!isEnd)
                {
                    sb.Append('\n');
                }

                lineStart = index + 1;
            }

            index++;
        }

        return sb.ToString();
    }

    private static bool IsImportLine(string line)
    {
        var trimmed = line.Trim();
        if (!trimmed.StartsWith("import", StringComparison.Ordinal))
        {
            return false;
        }

        var remainder = trimmed.Substring("import".Length).TrimStart();
        if (remainder.Length < 2 || remainder[0] != '"')
        {
            return false;
        }

        var endQuote = remainder.IndexOf('"', 1);
        if (endQuote < 0)
        {
            return false;
        }

        var tail = remainder.Substring(endQuote + 1).Trim();
        return tail.Length == 0 || tail == ";";
    }

    public IReadOnlyList<DocumentState> GetAllDocuments()
    {
        return _documents.Values.ToList();
    }
}
