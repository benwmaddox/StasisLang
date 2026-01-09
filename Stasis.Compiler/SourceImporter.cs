using System.IO;
using System.Text;

namespace Stasis.Compiler;

public sealed record SourceImportResult(string OriginalSource, string ExpandedSource);

public sealed record SourceImportSegment(string FilePath, SourceSpan SourceSpan, SourceSpan ExpandedSpan);

public sealed record SourceImportResultWithMap(string OriginalSource, string ExpandedSource, IReadOnlyList<SourceImportSegment> Segments);

public static class SourceImporter
{
    private const string ImportKeyword = "import";

    public static SourceImportResult ExpandImports(string entryPath, string source, List<Diagnostic> diagnostics)
    {
        var visited = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        var expanded = ExpandImportsInner(entryPath, source, diagnostics, visited).ExpandedSource;
        return new SourceImportResult(source, expanded);
    }

    public static SourceImportResultWithMap ExpandImportsWithMap(string entryPath, string source, List<Diagnostic> diagnostics)
    {
        var visited = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        return ExpandImportsInner(entryPath, source, diagnostics, visited);
    }

    private static SourceImportResultWithMap ExpandImportsInner(string currentPath, string source, List<Diagnostic> diagnostics, HashSet<string> visited)
    {
        var fullPath = Path.GetFullPath(currentPath);
        if (!visited.Add(fullPath))
        {
            return new SourceImportResultWithMap(source, string.Empty, Array.Empty<SourceImportSegment>());
        }

        if (IsStdlibPath(fullPath))
        {
            EnsureStdlibHasNoGlobals(fullPath, source, diagnostics);
        }

        var sb = new StringBuilder(source.Length);
        var segments = new List<SourceImportSegment>(128);
        var lineStart = 0;
        var index = 0;
        while (index <= source.Length)
        {
            var isEnd = index == source.Length;
            var ch = isEnd ? '\n' : source[index];
            if (ch == '\n' || isEnd)
            {
                var lineLength = index - lineStart;
                var rawLine = source.Substring(lineStart, lineLength);
                var line = rawLine.TrimEnd('\r');
                if (TryParseImportLine(line, out var importPath))
                {
                    var baseDir = Path.GetDirectoryName(fullPath) ?? string.Empty;
                    var resolvedPath = Path.GetFullPath(Path.Combine(baseDir, importPath));
                    if (!File.Exists(resolvedPath))
                    {
                        diagnostics.Add(new Diagnostic($"Import not found: {importPath}", new SourceSpan(lineStart, lineLength), fullPath));
                    }
                    else
                    {
                        var importedSource = File.ReadAllText(resolvedPath);
                        var expanded = ExpandImportsInner(resolvedPath, importedSource, diagnostics, visited);

                        var importExpandedStart = sb.Length;
                        sb.AppendLine(expanded.ExpandedSource);
                        var importExpandedLength = sb.Length - importExpandedStart;

                        foreach (var seg in expanded.Segments)
                        {
                            segments.Add(new SourceImportSegment(
                                seg.FilePath,
                                seg.SourceSpan,
                                new SourceSpan(importExpandedStart + seg.ExpandedSpan.Start, seg.ExpandedSpan.Length)));
                        }

                        if (expanded.Segments.Count == 0 && expanded.ExpandedSource.Length > 0)
                        {
                            // Fallback: if an imported file produced output but no segments, attribute it to the imported file.
                            segments.Add(new SourceImportSegment(
                                Path.GetFullPath(resolvedPath),
                                new SourceSpan(0, expanded.ExpandedSource.Length),
                                new SourceSpan(importExpandedStart, importExpandedLength)));
                        }
                    }
                }
                else
                {
                    var expandedStart = sb.Length;
                    sb.AppendLine(line);
                    var expandedEnd = sb.Length;

                    // Map the line content (excluding newline inserted by AppendLine).
                    var appendedNewlineLength = Environment.NewLine.Length;
                    var appendedLineLength = Math.Max(0, expandedEnd - expandedStart - appendedNewlineLength);
                    if (appendedLineLength > 0)
                    {
                        segments.Add(new SourceImportSegment(
                            fullPath,
                            new SourceSpan(lineStart, appendedLineLength),
                            new SourceSpan(expandedStart, appendedLineLength)));
                    }

                    // Map the newline inserted by AppendLine (best-effort).
                    if (expandedEnd - expandedStart >= appendedNewlineLength)
                    {
                        var newlineExpandedStart = expandedEnd - appendedNewlineLength;
                        var newlineSourceStart = Math.Min(lineStart + line.Length, Math.Max(0, source.Length - 1));
                        var newlineSourceLen = Math.Min(appendedNewlineLength, Math.Max(0, source.Length - newlineSourceStart));
                        if (newlineSourceLen > 0)
                        {
                            segments.Add(new SourceImportSegment(
                                fullPath,
                                new SourceSpan(newlineSourceStart, newlineSourceLen),
                                new SourceSpan(newlineExpandedStart, appendedNewlineLength)));
                        }
                    }
                }

                lineStart = index + 1;
            }

            index++;
        }

        var expandedText = sb.ToString().TrimEnd();
        var expandedLen = expandedText.Length;
        var trimmedSegments = new List<SourceImportSegment>(segments.Count);
        foreach (var seg in segments)
        {
            if (seg.ExpandedSpan.Start >= expandedLen)
            {
                continue;
            }

            var maxLen = Math.Min(seg.ExpandedSpan.Length, expandedLen - seg.ExpandedSpan.Start);
            if (maxLen <= 0)
            {
                continue;
            }

            trimmedSegments.Add(seg with { ExpandedSpan = new SourceSpan(seg.ExpandedSpan.Start, maxLen) });
        }

        return new SourceImportResultWithMap(source, expandedText, trimmedSegments);
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

            diagnostics.Add(new Diagnostic($"stdlib files may not declare globals: {fullPath}", new SourceSpan(0, 0), fullPath));
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
