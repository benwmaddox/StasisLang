using System.Text;

namespace Stasis.Compiler;

public static class MarkdownStasisExtractor
{
    // Extracts ```stasis fenced code blocks from a markdown document.
    // Preserves line numbers by replacing non-stasis lines with blank lines.
    public static string Extract(string markdown)
    {
        if (string.IsNullOrEmpty(markdown))
        {
            return string.Empty;
        }

        // Normalize newlines so line/column mapping is stable across platforms.
        var normalized = markdown.Replace("\r\n", "\n").Replace("\r", "\n");
        var lines = normalized.Split('\n', StringSplitOptions.None);

        var sb = new StringBuilder(normalized.Length);
        var inStasis = false;

        for (var i = 0; i < lines.Length; i++)
        {
            var line = lines[i];
            var trimmed = line.TrimStart();

            if (!inStasis)
            {
                if (IsFenceStart(trimmed))
                {
                    inStasis = true;
                }

                // Blank line to preserve line numbers.
                sb.Append(string.Empty);
            }
            else
            {
                if (IsFenceEnd(trimmed))
                {
                    inStasis = false;
                    sb.Append(string.Empty);
                }
                else
                {
                    sb.Append(line);
                }
            }

            if (i < lines.Length - 1)
            {
                sb.Append('\n');
            }
        }

        return sb.ToString();
    }

    private static bool IsFenceStart(string trimmedLine)
    {
        if (!trimmedLine.StartsWith("```", StringComparison.Ordinal))
        {
            return false;
        }

        var lang = trimmedLine.Substring(3).Trim();
        return string.Equals(lang, "stasis", StringComparison.OrdinalIgnoreCase);
    }

    private static bool IsFenceEnd(string trimmedLine) =>
        trimmedLine.StartsWith("```", StringComparison.Ordinal);
}
