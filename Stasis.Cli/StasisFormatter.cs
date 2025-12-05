using System.Text.RegularExpressions;

namespace Stasis.Cli;

internal static class StasisFormatter
{
    private static readonly Regex AssignSpacing = new(@"(\S)\s*\.\s*=\s*\(\s*", RegexOptions.Compiled);

    public static string Format(string source)
    {
        // Enforce a space before '.' and a space after '=' for .=( ) calls.
        // Example: x.=(y) => x .= (y)
        return AssignSpacing.Replace(source, "$1 .= (");
    }
}
