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
                if (TryParseImport(line, out var importPath))
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

    private static bool TryParseImport(string line, out string path)
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
