using System.Collections.Generic;
using System.Linq;
using System.Text;
using Stasis.Compiler.Semantic;
using Stasis.Compiler.Syntax;

namespace Stasis.Compiler;

internal static class CallableIdentity
{
    private const string NoReceiver = "<none>";

    public static bool HasReceiver(FunctionDeclarationSyntax function) => function.Parameters.Count > 0;

    public static string GetCallableKey(FunctionDeclarationSyntax function)
    {
        var receiver = HasReceiver(function) ? TypeKey(function.Parameters[0].Type) : NoReceiver;
        return GetCallableKey(function.Name.Text, receiver);
    }

    public static string GetCallableKey(string name, string receiverTypeKey) => $"{name}|{receiverTypeKey}";

    public static string GetCallableKey(string name, TypeSymbol receiverType) => GetCallableKey(name, TypeKey(receiverType));

    public static string GetReceiverTypeKeyOrNone(FunctionDeclarationSyntax function) =>
        HasReceiver(function) ? TypeKey(function.Parameters[0].Type) : NoReceiver;

    public static string GetEmittedFunctionName(FunctionDeclarationSyntax function)
    {
        if (!HasReceiver(function))
        {
            return function.Name.Text;
        }

        var receiver = TypeKey(function.Parameters[0].Type);
        return $"{function.Name.Text}__{SanitizeForSymbol(receiver)}";
    }

    public static string GetEmittedFunctionName(FunctionDeclarationSyntax function, IReadOnlySet<string> namesWithCollisions)
        => GetEmittedFunctionName(function);

    public static HashSet<string> CollectNamesWithCollisions(IEnumerable<FunctionDeclarationSyntax> functions) =>
        functions
            .GroupBy(fn => fn.Name.Text, StringComparer.Ordinal)
            .Where(group => group.Count() > 1)
            .Select(group => group.Key)
            .ToHashSet(StringComparer.Ordinal);

    public static string TypeKey(TypeSyntax type) =>
        type switch
        {
            NamedTypeSyntax named => named.Name,
            ArrayTypeSyntax arr => $"{TypeKey(arr.ElementType)}[]",
            _ => "unknown"
        };

    public static string TypeKey(TypeSymbol type) =>
        type switch
        {
            PrimitiveTypeSymbol prim => NormalizeDispatchPrimitiveName(prim.PrimitiveName),
            NamedTypeSymbol named => named.TypeName,
            ArrayTypeSymbol arr => $"{TypeKey(arr.ElementType)}[]",
            VoidTypeSymbol => "void",
            _ => "unknown"
        };

    private static string SanitizeForSymbol(string input)
    {
        var sb = new StringBuilder(input.Length);
        foreach (var ch in input)
        {
            if (char.IsLetterOrDigit(ch) || ch == '_')
            {
                sb.Append(ch);
            }
            else
            {
                sb.Append('_');
            }
        }

        return sb.ToString();
    }

    private static string NormalizeDispatchPrimitiveName(string primitiveName) =>
        primitiveName switch
        {
            "string_literal" => "string",
            _ => primitiveName
        };
}
