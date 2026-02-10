using Stasis.Compiler.Syntax;

namespace Stasis.Compiler;

internal static class CallableSymbolNameResolver
{
    public static bool IsExternFunction(FunctionDeclarationSyntax function) =>
        function.IsExtern || HasExternAttribute(function);

    public static string? GetExternLinkName(FunctionDeclarationSyntax function)
    {
        var raw = function.Attributes
            .FirstOrDefault(a => string.Equals(a.Text, "extern", StringComparison.Ordinal))?
            .StringValue;

        if (string.IsNullOrWhiteSpace(raw))
        {
            return null;
        }

        return UnquoteStringLiteral(raw);
    }

    public static string GetExternSymbolName(
        FunctionDeclarationSyntax function,
        IReadOnlyDictionary<string, string> _externFallbackSymbolNames)
    {
        return GetExternLinkName(function) ?? function.Name.Text;
    }

    public static string GetCallableSymbolName(
        FunctionDeclarationSyntax function,
        IReadOnlySet<string> _namesWithCollisions,
        IReadOnlyDictionary<string, string> externFallbackSymbolNames)
    {
        if (IsExternFunction(function))
        {
            return GetExternSymbolName(function, externFallbackSymbolNames);
        }

        return CallableIdentity.GetEmittedFunctionName(function);
    }

    public static IReadOnlyDictionary<string, string> CollectExternFallbackSymbolNames(
        IEnumerable<FunctionDeclarationSyntax> _functions,
        IReadOnlySet<string> _namesWithCollisions) =>
        new Dictionary<string, string>(StringComparer.Ordinal);

    private static bool HasExternAttribute(FunctionDeclarationSyntax function) =>
        function.Attributes.Any(attr => string.Equals(attr.Text, "extern", StringComparison.Ordinal));

    private static string UnquoteStringLiteral(string text)
    {
        if (text.Length >= 2 && text[0] == '"' && text[^1] == '"')
        {
            return text.Substring(1, text.Length - 2);
        }

        return text;
    }
}
