using System.Buffers.Binary;
using System.Security.Cryptography;
using System.Text;
using Stasis.Compiler.IR;
using Stasis.Compiler.Layout;
using Stasis.Compiler.Syntax;

namespace Stasis.Compiler;

public sealed record FunctionSemanticHashEntry(
    string CallableKey,
    string EmittedName,
    string SignatureHash,
    string BodyHash);

public sealed record FunctionSemanticProfile(
    ulong LayoutHash,
    IReadOnlyDictionary<string, FunctionSemanticHashEntry> Functions);

public sealed record FunctionSemanticDiff(
    bool AnyChange,
    bool LayoutChanged,
    bool SignatureChanged,
    bool FunctionSetChanged,
    bool RequiresConservativeRebuild,
    IReadOnlyList<string> ChangedBodyCallableKeys,
    int RecompiledFunctions,
    int ReusedFunctions);

public static class FunctionSemanticFingerprint
{
    public static FunctionSemanticProfile ComputeProfile(
        string source,
        CompilationUnitSyntax compilationUnit,
        LayoutPlan layout,
        bool includeTests,
        bool allowReachabilityFallback)
    {
        ArgumentNullException.ThrowIfNull(source);
        ArgumentNullException.ThrowIfNull(compilationUnit);
        ArgumentNullException.ThrowIfNull(layout);

        var lex = Lexer.Lex(source);
        var reachableFunctions = Reachability.CollectReachableFunctions(compilationUnit, includeTests, allowReachabilityFallback);
        var reachableDeclarations = compilationUnit.Declarations
            .OfType<FunctionDeclarationSyntax>()
            .Where(fn => !fn.IsExtern && fn.Body is not null && reachableFunctions.Contains(CallableIdentity.GetCallableKey(fn)))
            .ToArray();
        var namesWithCollisions = CallableIdentity.CollectNamesWithCollisions(reachableDeclarations);

        var functions = new Dictionary<string, FunctionSemanticHashEntry>(StringComparer.Ordinal);
        foreach (var fn in reachableDeclarations)
        {
            if (fn.Body is null)
            {
                continue;
            }

            var callableKey = CallableIdentity.GetCallableKey(fn);
            var emittedName = CallableIdentity.GetEmittedFunctionName(fn, namesWithCollisions);

            var signatureStart = (fn.ExportKeyword ?? fn.ExternKeyword ?? fn.FunctionKeyword).Span.Start;
            var signatureEnd = fn.Body.OpenBrace.Span.Start;
            var signatureHash = HashTokenRange(lex.Tokens, signatureStart, signatureEnd);
            var bodyHash = HashTokenRange(lex.Tokens, fn.Body.Span.Start, fn.Body.Span.End);

            functions[callableKey] = new FunctionSemanticHashEntry(callableKey, emittedName, signatureHash, bodyHash);
        }

        return new FunctionSemanticProfile(SemanticFingerprint.ComputeLayoutHash(layout), functions);
    }

    public static FunctionSemanticDiff Diff(FunctionSemanticProfile? previous, FunctionSemanticProfile current)
    {
        ArgumentNullException.ThrowIfNull(current);

        if (previous is null)
        {
            return new FunctionSemanticDiff(
                AnyChange: current.Functions.Count > 0,
                LayoutChanged: false,
                SignatureChanged: false,
                FunctionSetChanged: false,
                RequiresConservativeRebuild: true,
                ChangedBodyCallableKeys: current.Functions.Keys.OrderBy(k => k, StringComparer.Ordinal).ToArray(),
                RecompiledFunctions: current.Functions.Count,
                ReusedFunctions: 0);
        }

        var layoutChanged = previous.LayoutHash != current.LayoutHash;
        var signatureChanged = false;
        var functionSetChanged = false;
        var changedBodyKeys = new List<string>();

        var allKeys = new HashSet<string>(previous.Functions.Keys, StringComparer.Ordinal);
        allKeys.UnionWith(current.Functions.Keys);

        foreach (var key in allKeys.OrderBy(k => k, StringComparer.Ordinal))
        {
            var inPrevious = previous.Functions.TryGetValue(key, out var prevEntry);
            var inCurrent = current.Functions.TryGetValue(key, out var currEntry);
            if (!inPrevious || !inCurrent)
            {
                functionSetChanged = true;
                continue;
            }

            if (!string.Equals(prevEntry!.SignatureHash, currEntry!.SignatureHash, StringComparison.Ordinal))
            {
                signatureChanged = true;
                continue;
            }

            if (!string.Equals(prevEntry.BodyHash, currEntry.BodyHash, StringComparison.Ordinal))
            {
                changedBodyKeys.Add(key);
            }
        }

        var requiresConservativeRebuild = layoutChanged || signatureChanged || functionSetChanged;
        var anyChange = requiresConservativeRebuild || changedBodyKeys.Count > 0;
        var recompiledFunctions = anyChange
            ? (requiresConservativeRebuild ? current.Functions.Count : changedBodyKeys.Count)
            : 0;
        var reusedFunctions = anyChange
            ? (requiresConservativeRebuild ? 0 : current.Functions.Count - changedBodyKeys.Count)
            : current.Functions.Count;

        return new FunctionSemanticDiff(
            AnyChange: anyChange,
            LayoutChanged: layoutChanged,
            SignatureChanged: signatureChanged,
            FunctionSetChanged: functionSetChanged,
            RequiresConservativeRebuild: requiresConservativeRebuild,
            ChangedBodyCallableKeys: changedBodyKeys,
            RecompiledFunctions: recompiledFunctions,
            ReusedFunctions: reusedFunctions);
    }

    private static string HashTokenRange(IReadOnlyList<Token> tokens, int startInclusive, int endExclusive)
    {
        using var hasher = IncrementalHash.CreateHash(HashAlgorithmName.SHA256);
        Span<byte> u32 = stackalloc byte[4];

        for (var i = 0; i < tokens.Count; i++)
        {
            var token = tokens[i];
            if (token.Kind == TokenKind.EndOfFile)
            {
                continue;
            }

            if (token.Span.Start < startInclusive || token.Span.End > endExclusive)
            {
                continue;
            }

            BinaryPrimitives.WriteInt32LittleEndian(u32, (int)token.Kind);
            hasher.AppendData(u32);

            if (token.Text.Length > 0)
            {
                hasher.AppendData(Encoding.UTF8.GetBytes(token.Text));
            }

            hasher.AppendData([0]);
        }

        var digest = hasher.GetHashAndReset();
        return Convert.ToHexString(digest).ToLowerInvariant();
    }
}
