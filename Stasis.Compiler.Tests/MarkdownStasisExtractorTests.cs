using Stasis.Compiler;

namespace Stasis.Compiler.Tests;

public class MarkdownStasisExtractorTests
{
    [Fact]
    public void Preserves_line_numbers_and_extracts_only_stasis_blocks()
    {
        var md = string.Join('\n',
            "# Title",
            "",
            "Not code.",
            "",
            "```stasis",
            "function a(): i32 { return 1; }",
            "```",
            "",
            "```notstasis",
            "function b(): i32 { return 2; }",
            "```",
            "",
            "More text.",
            "",
            "```stasis",
            "function c(): i32 { return 3; }",
            "```",
            "");

        var extracted = MarkdownStasisExtractor.Extract(md);

        // Same number of lines so diagnostics can reference the markdown line numbers.
        Assert.Equal(CountLines(md), CountLines(extracted));

        Assert.Contains("function a()", extracted);
        Assert.Contains("function c()", extracted);
        Assert.DoesNotContain("function b()", extracted);

        // Ensure function a() appears on the same line number in both strings.
        var mdLineA = LineNumberOf(md, "function a(): i32 { return 1; }");
        var exLineA = LineNumberOf(extracted, "function a(): i32 { return 1; }");
        Assert.Equal(mdLineA, exLineA);
    }

    private static int CountLines(string s)
    {
        var count = 1;
        for (var i = 0; i < s.Length; i++)
        {
            if (s[i] == '\n')
            {
                count++;
            }
        }
        return count;
    }

    private static int LineNumberOf(string s, string needle)
    {
        var idx = s.IndexOf(needle, StringComparison.Ordinal);
        Assert.True(idx >= 0, $"needle not found: {needle}");

        var line = 1;
        for (var i = 0; i < idx; i++)
        {
            if (s[i] == '\n')
            {
                line++;
            }
        }
        return line;
    }
}
