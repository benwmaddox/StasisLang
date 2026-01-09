namespace Stasis.LanguageServer.Services;

using System.Collections.Concurrent;
using System.Text;
using Stasis.Compiler;
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
        var importDiagnostics = new List<Diagnostic>();
        var importedUnits = new List<CompilationUnitSyntax>();
        CollectImportedCompilationUnits(uri, doc.Content, importDiagnostics, importedUnits);

        var parseContent = StripImportLines(doc.Content);

        // Lexing
        var lexResult = Lexer.Lex(parseContent);

        // Parsing
        var parseResult = Parser.Parse(parseContent);
        doc.ParseResult = parseResult;
        doc.SymbolIndex = BuildSymbolIndex(parseResult.CompilationUnit, importedUnits);

        // Semantic Analysis
        if (!parseResult.Diagnostics.Any())
        {
            var semanticAnalyzer = new SemanticAnalyzer();
            var semanticResult = semanticAnalyzer.Analyze(BuildSemanticCompilationUnit(parseResult.CompilationUnit, importedUnits));
            doc.SemanticResult = semanticResult;

            // Combine all diagnostics
            var allDiags = new List<Diagnostic>();
            allDiags.AddRange(lexResult.Diagnostics);
            allDiags.AddRange(parseResult.Diagnostics);
            allDiags.AddRange(importDiagnostics);
            allDiags.AddRange(semanticResult.Diagnostics);
            doc.AllDiagnostics = allDiags;
        }
        else
        {
            // Only include lex and parse diagnostics if parsing failed
            var allDiags = new List<Diagnostic>();
            allDiags.AddRange(lexResult.Diagnostics);
            allDiags.AddRange(parseResult.Diagnostics);
            allDiags.AddRange(importDiagnostics);
            doc.AllDiagnostics = allDiags;
            doc.SemanticResult = null;
        }
    }

    private static SymbolIndex BuildSymbolIndex(CompilationUnitSyntax compilationUnit, IReadOnlyList<CompilationUnitSyntax> importedUnits)
    {
        var index = SymbolIndex.Build(compilationUnit);
        foreach (var imported in importedUnits)
        {
            index.AddFrom(imported);
        }
        return index;
    }

    private static CompilationUnitSyntax BuildSemanticCompilationUnit(CompilationUnitSyntax compilationUnit, IReadOnlyList<CompilationUnitSyntax> importedUnits)
    {
        if (importedUnits.Count == 0)
        {
            return compilationUnit;
        }

        var declarations = new List<DeclarationSyntax>(compilationUnit.Declarations.Count + importedUnits.Sum(u => u.Declarations.Count));
        declarations.AddRange(compilationUnit.Declarations);
        foreach (var imported in importedUnits)
        {
            declarations.AddRange(imported.Declarations);
        }

        return new CompilationUnitSyntax(declarations, compilationUnit.EndOfFileToken);
    }

    private static void CollectImportedCompilationUnits(
        string uri,
        string source,
        List<Diagnostic> importDiagnostics,
        List<CompilationUnitSyntax> importedUnits)
    {
        var entryPath = TryGetFilePathFromUri(uri);
        if (string.IsNullOrWhiteSpace(entryPath))
        {
            return;
        }

        var visited = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        CollectImportedCompilationUnitsInner(entryPath!, source, importDiagnostics, importedUnits, visited, isRoot: true, depth: 0);
    }

    private static void CollectImportedCompilationUnitsInner(
        string currentPath,
        string source,
        List<Diagnostic> importDiagnostics,
        List<CompilationUnitSyntax> importedUnits,
        HashSet<string> visited,
        bool isRoot,
        int depth)
    {
        if (depth > 128 || importedUnits.Count > 512)
        {
            return;
        }

        var fullPath = Path.GetFullPath(currentPath);
        if (!visited.Add(fullPath))
        {
            return;
        }

        var baseDir = Path.GetDirectoryName(fullPath) ?? string.Empty;

        var lineStart = 0;
        var index = 0;
        while (index <= source.Length)
        {
            var isEnd = index == source.Length;
            var ch = isEnd ? '\n' : source[index];
            if (ch == '\n' || isEnd)
            {
                var lineLength = index - lineStart;
                var lineText = source.Substring(lineStart, lineLength);
                var trimmedLine = lineText.TrimEnd('\r');

                if (SourceImporter.TryParseImportLine(trimmedLine, out var importPath))
                {
                    var resolvedPath = Path.GetFullPath(Path.Combine(baseDir, importPath));

                    if (!File.Exists(resolvedPath))
                    {
                        if (isRoot)
                        {
                            importDiagnostics.Add(new Diagnostic($"Import not found: {importPath}", new SourceSpan(lineStart, lineLength)));
                        }
                    }
                    else
                    {
                        var importedSource = File.ReadAllText(resolvedPath);
                        var parseContent = StripImportLines(importedSource);
                        var parseResult = Parser.Parse(parseContent);
                        if (!parseResult.Diagnostics.Any())
                        {
                            importedUnits.Add(StubImportedCompilationUnit(parseResult.CompilationUnit));
                        }

                        CollectImportedCompilationUnitsInner(resolvedPath, importedSource, importDiagnostics, importedUnits, visited, isRoot: false, depth: depth + 1);
                    }
                }

                if (!isEnd)
                {
                    lineStart = index + 1;
                }
            }

            index++;
        }
    }

    private static CompilationUnitSyntax StubImportedCompilationUnit(CompilationUnitSyntax compilationUnit)
    {
        var declarations = new List<DeclarationSyntax>(compilationUnit.Declarations.Count);
        foreach (var decl in compilationUnit.Declarations)
        {
            switch (decl)
            {
                case FunctionDeclarationSyntax fn:
                    declarations.Add(fn with { Body = null });
                    break;
                case TestDeclarationSyntax:
                    // Skip imported tests in LSP context.
                    break;
                case ConstDeclarationSyntax constant when constant.Initializer is not LiteralExpressionSyntax:
                    declarations.Add(constant with { Initializer = new LiteralExpressionSyntax(new Token(TokenKind.IntegerLiteral, "0", new SourceSpan(0, 1))) });
                    break;
                default:
                    declarations.Add(decl);
                    break;
            }
        }

        return new CompilationUnitSyntax(declarations, compilationUnit.EndOfFileToken);
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
