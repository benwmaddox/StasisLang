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
        IReadOnlyDictionary<string, string> externFallbackSymbolNames)
    {
        var callableKey = CallableIdentity.GetCallableKey(function);
        if (externFallbackSymbolNames.TryGetValue(callableKey, out var fallbackSymbol))
        {
            return fallbackSymbol;
        }

        return GetExternLinkName(function) ?? function.Name.Text;
    }

    public static string GetCallableSymbolName(
        FunctionDeclarationSyntax function,
        IReadOnlySet<string> namesWithCollisions,
        IReadOnlyDictionary<string, string> externFallbackSymbolNames)
    {
        if (IsExternFunction(function))
        {
            return GetExternSymbolName(function, externFallbackSymbolNames);
        }

        return CallableIdentity.GetEmittedFunctionName(function, namesWithCollisions);
    }

    public static IReadOnlyDictionary<string, string> CollectExternFallbackSymbolNames(
        IEnumerable<FunctionDeclarationSyntax> functions,
        IReadOnlySet<string> namesWithCollisions)
    {
        var functionList = functions.ToList();
        var reservedSymbolNames = functionList
            .Where(fn => !IsExternFunction(fn))
            .Select(fn => CallableIdentity.GetEmittedFunctionName(fn, namesWithCollisions))
            .ToHashSet(StringComparer.Ordinal);
        var fallbackByCallableKey = new Dictionary<string, string>(StringComparer.Ordinal);

        var externGroups = functionList
            .Where(IsExternFunction)
            .GroupBy(fn => GetExternLinkName(fn) ?? fn.Name.Text, StringComparer.Ordinal);
        foreach (var group in externGroups)
        {
            var hasCollision = group.Count() > 1 || reservedSymbolNames.Contains(group.Key);
            if (!hasCollision)
            {
                reservedSymbolNames.Add(group.Key);
                continue;
            }

            foreach (var fn in group)
            {
                var fallbackBase = CallableIdentity.GetEmittedFunctionName(fn);
                if (!CallableIdentity.HasReceiver(fn))
                {
                    fallbackBase = $"{fn.Name.Text}__extern";
                }

                var fallback = fallbackBase;
                var suffix = 2;
                while (reservedSymbolNames.Contains(fallback))
                {
                    fallback = $"{fallbackBase}_{suffix}";
                    suffix++;
                }

                var callableKey = CallableIdentity.GetCallableKey(fn);
                fallbackByCallableKey[callableKey] = fallback;
                reservedSymbolNames.Add(fallback);
            }
        }

        return fallbackByCallableKey;
    }

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
