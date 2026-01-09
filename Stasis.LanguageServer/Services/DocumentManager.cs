namespace Stasis.LanguageServer.Services;

using System.Collections.Concurrent;
using System.Text;
using Stasis.Compiler;
using Stasis.Compiler.Semantic;
using Stasis.Compiler.Syntax;
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
            ParseDocument(uri, doc);
        }
    }

    public void CloseDocument(string uri)
    {
        _documents.TryRemove(uri, out _);
    }

    private static void ParseDocument(string uri, DocumentState doc)
    {
        var parseContent = StripImportLines(doc.Content);

        // Lexing
        var lexResult = Lexer.Lex(parseContent);

        // Parsing
        var parseResult = Parser.Parse(parseContent);
        doc.ParseResult = parseResult;

        var compilerDiagnostics = new List<Diagnostic>();
        var expanded = TryExpandImports(uri, doc.Content, compilerDiagnostics, out var importSegments);

        var expandedLex = Lexer.Lex(expanded);
        var expandedParse = Parser.Parse(expanded);
        doc.ExpandedParseResult = expandedParse;
        doc.SymbolIndex = SymbolIndex.Build(expandedParse.CompilationUnit);

        // Semantic Analysis
        if (!expandedParse.Diagnostics.Any())
        {
            var semanticAnalyzer = new SemanticAnalyzer();
            var semanticResult = semanticAnalyzer.Analyze(expandedParse.CompilationUnit);
            doc.SemanticResult = semanticResult;

            // Combine all diagnostics
            var allDiags = new List<Diagnostic>();
            allDiags.AddRange(lexResult.Diagnostics);
            allDiags.AddRange(parseResult.Diagnostics);
            allDiags.AddRange(FilterAndRemapDiagnostics(uri, doc.Content, compilerDiagnostics, importSegments, expanded));
            allDiags.AddRange(FilterAndRemapDiagnostics(uri, doc.Content, semanticResult.Diagnostics, importSegments, expanded));
            doc.AllDiagnostics = allDiags;
        }
        else
        {
            // Only include lex and parse diagnostics if parsing failed
            var allDiags = new List<Diagnostic>();
            allDiags.AddRange(lexResult.Diagnostics);
            allDiags.AddRange(parseResult.Diagnostics);
            allDiags.AddRange(FilterAndRemapDiagnostics(uri, doc.Content, compilerDiagnostics, importSegments, expanded));
            doc.AllDiagnostics = allDiags;
            doc.SemanticResult = null;
        }
    }

    private static string? TryGetFilePathFromUri(string uri)
    {
        if (!Uri.TryCreate(uri, UriKind.Absolute, out var parsed) || !parsed.IsFile)
        {
            return null;
        }

        try
        {
            return parsed.LocalPath;
        }
        catch
        {
            return null;
        }
    }

    private static string TryExpandImports(
        string uri,
        string source,
        List<Diagnostic> diagnostics,
        out IReadOnlyList<SourceImportSegment> segments)
    {
        segments = Array.Empty<SourceImportSegment>();
        var entryPath = TryGetFilePathFromUri(uri);
        if (string.IsNullOrWhiteSpace(entryPath))
        {
            return source;
        }

        try
        {
            var result = SourceImporter.ExpandImportsWithMap(entryPath!, source, diagnostics);
            segments = result.Segments;
            return result.ExpandedSource;
        }
        catch
        {
            return source;
        }
    }

    private static IReadOnlyList<Diagnostic> FilterAndRemapDiagnostics(
        string uri,
        string rootContent,
        IReadOnlyList<Diagnostic> diagnostics,
        IReadOnlyList<SourceImportSegment> segments,
        string expandedContent)
    {
        var rootPath = TryGetFilePathFromUri(uri);
        if (string.IsNullOrWhiteSpace(rootPath))
        {
            return diagnostics;
        }

        var fullRootPath = Path.GetFullPath(rootPath!);
        var mapped = new List<Diagnostic>(diagnostics.Count);

        foreach (var diag in diagnostics)
        {
            if (!string.IsNullOrWhiteSpace(diag.FilePath))
            {
                if (!Path.GetFullPath(diag.FilePath!).Equals(fullRootPath, StringComparison.OrdinalIgnoreCase))
                {
                    continue;
                }

                mapped.Add(diag with { FilePath = fullRootPath });
                continue;
            }

            if (!TryMapExpandedSpanToFile(diag.Span, segments, out var filePath, out var fileSpan))
            {
                continue;
            }

            if (!Path.GetFullPath(filePath).Equals(fullRootPath, StringComparison.OrdinalIgnoreCase))
            {
                continue;
            }

            mapped.Add(new Diagnostic(diag.Message, fileSpan, fullRootPath));
        }

        return mapped;
    }

    private static bool TryMapExpandedSpanToFile(
        SourceSpan expandedSpan,
        IReadOnlyList<SourceImportSegment> segments,
        out string filePath,
        out SourceSpan fileSpan)
    {
        filePath = string.Empty;
        fileSpan = new SourceSpan(0, 0);

        if (segments.Count == 0)
        {
            return false;
        }

        // Find segment containing start.
        SourceImportSegment? seg = null;
        for (var i = 0; i < segments.Count; i++)
        {
            var s = segments[i];
            var start = s.ExpandedSpan.Start;
            var end = s.ExpandedSpan.Start + s.ExpandedSpan.Length;
            if (expandedSpan.Start >= start && expandedSpan.Start < end)
            {
                seg = s;
                break;
            }
        }

        if (seg is null)
        {
            return false;
        }

        var offsetIntoSeg = expandedSpan.Start - seg.ExpandedSpan.Start;
        var sourceStart = seg.SourceSpan.Start + offsetIntoSeg;
        var sourceLen = Math.Min(expandedSpan.Length, Math.Max(0, seg.SourceSpan.Length - offsetIntoSeg));

        filePath = seg.FilePath;
        fileSpan = new SourceSpan(sourceStart, sourceLen);
        return true;
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

                if (SourceImporter.TryParseImportLine(trimmedLine, out _))
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

    public IReadOnlyList<DocumentState> GetAllDocuments()
    {
        return _documents.Values.ToList();
    }
}
