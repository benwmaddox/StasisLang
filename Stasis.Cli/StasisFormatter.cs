using System.Text;

namespace Stasis.Cli;

internal static class StasisFormatter
{
    public static string Format(string source)
    {
        // Minimal formatter: normalize struct initializer literals to multi-line form:
        //   target = {
        //       field = expr,
        //   };
        //
        // This is intentionally narrow (does not attempt full formatting).
        return FormatStructInitializers(source);
    }

    private static string FormatStructInitializers(string source)
    {
        if (string.IsNullOrEmpty(source))
        {
            return source;
        }

        var sb = new StringBuilder(source.Length);
        var i = 0;

        while (i < source.Length)
        {
            // Find '=' then optional whitespace then '{'.
            var eq = source.IndexOf('=', i);
            if (eq < 0)
            {
                sb.Append(source, i, source.Length - i);
                break;
            }

            // Copy up to '='.
            sb.Append(source, i, eq - i);
            sb.Append('=');

            var j = eq + 1;
            while (j < source.Length && IsWhitespace(source[j]))
            {
                sb.Append(source[j]);
                j++;
            }

            if (j >= source.Length || source[j] != '{')
            {
                i = j;
                continue;
            }

            var openBrace = j;
            if (!TryFindMatchingBrace(source, openBrace, out var closeBrace))
            {
                // Unbalanced; leave as-is.
                sb.Append(source, j, source.Length - j);
                break;
            }

            var baseIndent = GetLineIndent(source, openBrace);
            var formatted = FormatStructInitializerBlock(source, openBrace, closeBrace, baseIndent);
            if (formatted is null)
            {
                // If we can't confidently parse fields, keep original.
                sb.Append(source, openBrace, closeBrace - openBrace + 1);
            }
            else
            {
                sb.Append(formatted);
            }

            i = closeBrace + 1;
        }

        return sb.ToString();
    }

    private static bool TryFindMatchingBrace(string source, int openBrace, out int closeBrace)
    {
        closeBrace = -1;
        var depth = 0;
        var inString = false;
        var i = openBrace;

        while (i < source.Length)
        {
            var c = source[i];

            if (!inString && c == '/' && i + 1 < source.Length && source[i + 1] == '/')
            {
                // Line comment.
                i += 2;
                while (i < source.Length && source[i] != '\n')
                {
                    i++;
                }
                continue;
            }

            if (c == '"' && (i == 0 || source[i - 1] != '\\'))
            {
                inString = !inString;
                i++;
                continue;
            }

            if (inString)
            {
                i++;
                continue;
            }

            if (c == '{')
            {
                depth++;
            }
            else if (c == '}')
            {
                depth--;
                if (depth == 0)
                {
                    closeBrace = i;
                    return true;
                }
            }

            i++;
        }

        return false;
    }

    private static string? FormatStructInitializerBlock(string source, int openBrace, int closeBrace, string baseIndent)
    {
        var inner = source.Substring(openBrace + 1, closeBrace - openBrace - 1);
        var fields = SplitTopLevelCommaSeparated(inner);
        if (fields is null)
        {
            return null;
        }

        // Empty initializer: keep as a compact multi-line block.
        if (fields.Count == 0)
        {
            return "{\n" + baseIndent + "}";
        }

        var innerIndent = baseIndent + "    ";
        var sb = new StringBuilder();
        sb.Append("{\n");

        foreach (var fieldText in fields)
        {
            if (!TryParseField(fieldText, out var name, out var expr))
            {
                return null;
            }

            sb.Append(innerIndent);
            sb.Append(name);
            sb.Append(" = ");
            sb.Append(expr);
            sb.Append(",\n");
        }

        sb.Append(baseIndent);
        sb.Append('}');
        return sb.ToString();
    }

    private static List<string>? SplitTopLevelCommaSeparated(string text)
    {
        var fields = new List<string>();

        var depthBrace = 0;
        var depthParen = 0;
        var depthBracket = 0;
        var inString = false;

        var start = 0;
        var i = 0;
        while (i < text.Length)
        {
            var c = text[i];

            if (!inString && c == '/' && i + 1 < text.Length && text[i + 1] == '/')
            {
                i += 2;
                while (i < text.Length && text[i] != '\n')
                {
                    i++;
                }
                continue;
            }

            if (c == '"' && (i == 0 || text[i - 1] != '\\'))
            {
                inString = !inString;
                i++;
                continue;
            }

            if (inString)
            {
                i++;
                continue;
            }

            switch (c)
            {
                case '{': depthBrace++; break;
                case '}': depthBrace--; break;
                case '(': depthParen++; break;
                case ')': depthParen--; break;
                case '[': depthBracket++; break;
                case ']': depthBracket--; break;
            }

            if (c == ',' && depthBrace == 0 && depthParen == 0 && depthBracket == 0)
            {
                var part = text.Substring(start, i - start).Trim();
                if (part.Length > 0)
                {
                    fields.Add(part);
                }
                start = i + 1;
            }

            i++;
        }

        if (inString || depthBrace != 0 || depthParen != 0 || depthBracket != 0)
        {
            return null;
        }

        var last = text.Substring(start).Trim();
        if (last.Length > 0)
        {
            fields.Add(last);
        }

        return fields;
    }

    private static bool TryParseField(string fieldText, out string name, out string expr)
    {
        name = string.Empty;
        expr = string.Empty;

        var s = fieldText.Trim();
        if (s.Length == 0)
        {
            return false;
        }

        var i = 0;
        if (!IsIdentStart(s[i]))
        {
            return false;
        }
        i++;
        while (i < s.Length && IsIdentPart(s[i]))
        {
            i++;
        }

        name = s.Substring(0, i);
        while (i < s.Length && IsWhitespace(s[i]))
        {
            i++;
        }

        if (i >= s.Length || s[i] != '=')
        {
            return false;
        }

        i++; // '='
        while (i < s.Length && IsWhitespace(s[i]))
        {
            i++;
        }

        expr = s.Substring(i).TrimEnd();
        if (expr.Length == 0)
        {
            return false;
        }

        return true;
    }

    private static string GetLineIndent(string source, int index)
    {
        var lineStart = source.LastIndexOf('\n', Math.Max(0, index - 1));
        lineStart = lineStart < 0 ? 0 : lineStart + 1;
        var i = lineStart;
        while (i < source.Length && (source[i] == ' ' || source[i] == '\t'))
        {
            i++;
        }
        return source.Substring(lineStart, i - lineStart);
    }

    private static bool IsWhitespace(char c) => c is ' ' or '\t' or '\r' or '\n';
    private static bool IsIdentStart(char c) => c is '_' or >= 'A' and <= 'Z' or >= 'a' and <= 'z';
    private static bool IsIdentPart(char c) => IsIdentStart(c) || (c >= '0' && c <= '9');
}
