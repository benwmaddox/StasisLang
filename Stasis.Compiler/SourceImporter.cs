using System.IO;
using System.Text;

namespace Stasis.Compiler;

public sealed record SourceImportResult(string OriginalSource, string ExpandedSource);

public static class SourceImporter
{
    private const string ImportKeyword = "import";

    public static SourceImportResult ExpandImports(string entryPath, string source, List<Diagnostic> diagnostics)
    {
        var visited = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        var expanded = ExpandImportsInner(entryPath, source, diagnostics, visited);
        return new SourceImportResult(source, expanded);
    }

    private static string ExpandImportsInner(string currentPath, string source, List<Diagnostic> diagnostics, HashSet<string> visited)
    {
        var fullPath = Path.GetFullPath(currentPath);
        if (!visited.Add(fullPath))
        {
            return string.Empty;
        }

        if (IsStdlibPath(fullPath))
        {
            EnsureStdlibHasNoGlobals(fullPath, source, diagnostics);
        }

        var sb = new StringBuilder(source.Length);
        var lineStart = 0;
        var index = 0;
        while (index <= source.Length)
        {
            var isEnd = index == source.Length;
            var ch = isEnd ? '\n' : source[index];
            if (ch == '\n' || isEnd)
            {
                var lineLength = index - lineStart;
                var line = source.Substring(lineStart, lineLength).TrimEnd('\r');
                if (TryParseImportLine(line, out var importPath))
                {
                    var baseDir = Path.GetDirectoryName(fullPath) ?? string.Empty;
                    var resolvedPath = Path.GetFullPath(Path.Combine(baseDir, importPath));
                    if (!File.Exists(resolvedPath))
                    {
                        diagnostics.Add(new Diagnostic($"Import not found: {importPath}", new SourceSpan(lineStart, lineLength)));
                    }
                    else
                    {
                        var importedSource = File.ReadAllText(resolvedPath);
                        var expanded = ExpandImportsInner(resolvedPath, importedSource, diagnostics, visited);
                        sb.AppendLine(expanded);
                    }
                }
                else
                {
                    sb.AppendLine(line);
                }

                lineStart = index + 1;
            }

            index++;
        }

        return sb.ToString().TrimEnd();
    }

    private static bool IsStdlibPath(string fullPath)
    {
        var normalized = fullPath.Replace(Path.AltDirectorySeparatorChar, Path.DirectorySeparatorChar);
        var marker = $"{Path.DirectorySeparatorChar}src{Path.DirectorySeparatorChar}stdlib{Path.DirectorySeparatorChar}";
        return normalized.Contains(marker, StringComparison.OrdinalIgnoreCase);
    }

    private static void EnsureStdlibHasNoGlobals(string fullPath, string source, List<Diagnostic> diagnostics)
    {
        var lex = Lexer.Lex(source);
        foreach (var token in lex.Tokens)
        {
            if (token.Kind != TokenKind.GlobalKeyword)
            {
                continue;
            }

            diagnostics.Add(new Diagnostic($"stdlib files may not declare globals: {fullPath}", new SourceSpan(0, 0)));
            return;
        }
    }

    public static bool TryParseImportLine(string line, out string path)
    {
        path = string.Empty;
        var trimmed = line.Trim();
        if (!trimmed.StartsWith(ImportKeyword, StringComparison.Ordinal))
        {
            return false;
        }

        var remainder = trimmed.Substring(ImportKeyword.Length).TrimStart();
        if (remainder.Length < 2 || remainder[0] != '\"')
        {
            return false;
        }

        var endQuote = remainder.IndexOf('\"', 1);
        if (endQuote < 0)
        {
            return false;
        }

        path = remainder.Substring(1, endQuote - 1);
        var tail = remainder.Substring(endQuote + 1).Trim();
        return tail.Length == 0 || tail == ";";
    }
}
