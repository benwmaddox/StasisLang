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
    string BodyHash,
    bool IsInline);

public sealed record FunctionSemanticProfile(
    ulong LayoutHash,
    string DeclarationHash,
    IReadOnlyDictionary<string, FunctionSemanticHashEntry> Functions);

public sealed record FunctionSemanticDiff(
    bool AnyChange,
    bool LayoutChanged,
    bool DeclarationChanged,
    bool SignatureChanged,
    bool InlineBodyChanged,
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
        var declarationHash = ComputeDeclarationHash(compilationUnit, lex.Tokens, includeTests);
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
            var isInline = fn.Attributes.Any(attr => string.Equals(attr.Text, "inline", StringComparison.Ordinal));

            functions[callableKey] = new FunctionSemanticHashEntry(callableKey, emittedName, signatureHash, bodyHash, isInline);
        }

        return new FunctionSemanticProfile(SemanticFingerprint.ComputeLayoutHash(layout), declarationHash, functions);
    }

    public static FunctionSemanticDiff Diff(FunctionSemanticProfile? previous, FunctionSemanticProfile current)
    {
        ArgumentNullException.ThrowIfNull(current);

        if (previous is null)
        {
            return new FunctionSemanticDiff(
                AnyChange: current.Functions.Count > 0,
                LayoutChanged: false,
                DeclarationChanged: false,
                SignatureChanged: false,
                InlineBodyChanged: false,
                FunctionSetChanged: false,
                RequiresConservativeRebuild: true,
                ChangedBodyCallableKeys: current.Functions.Keys.OrderBy(k => k, StringComparer.Ordinal).ToArray(),
                RecompiledFunctions: current.Functions.Count,
                ReusedFunctions: 0);
        }

        var layoutChanged = previous.LayoutHash != current.LayoutHash;
        var declarationChanged = !string.Equals(previous.DeclarationHash, current.DeclarationHash, StringComparison.Ordinal);
        var signatureChanged = false;
        var inlineBodyChanged = false;
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
                if (prevEntry.IsInline || currEntry.IsInline)
                {
                    inlineBodyChanged = true;
                }
            }
        }

        var requiresConservativeRebuild = layoutChanged || declarationChanged || signatureChanged || inlineBodyChanged || functionSetChanged;
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
            DeclarationChanged: declarationChanged,
            SignatureChanged: signatureChanged,
            InlineBodyChanged: inlineBodyChanged,
            FunctionSetChanged: functionSetChanged,
            RequiresConservativeRebuild: requiresConservativeRebuild,
            ChangedBodyCallableKeys: changedBodyKeys,
            RecompiledFunctions: recompiledFunctions,
            ReusedFunctions: reusedFunctions);
    }

    private static string ComputeDeclarationHash(
        CompilationUnitSyntax compilationUnit,
        IReadOnlyList<Token> tokens,
        bool includeTests)
    {
        var ranges = new List<(int Start, int End)>();
        foreach (var declaration in compilationUnit.Declarations)
        {
            switch (declaration)
            {
                case FunctionDeclarationSyntax fn when fn.IsExtern || fn.Body is null:
                    ranges.Add((fn.Span.Start, fn.Span.End));
                    break;
                case FunctionDeclarationSyntax fn:
                    {
                        var start = (fn.ExportKeyword ?? fn.ExternKeyword ?? fn.FunctionKeyword).Span.Start;
                        var end = fn.Body!.OpenBrace.Span.Start;
                        ranges.Add((start, end));
                        break;
                    }
                case TestDeclarationSyntax test when includeTests:
                    ranges.Add((test.Span.Start, test.Span.End));
                    break;
                case TestDeclarationSyntax:
                    break;
                default:
                    ranges.Add((declaration.Span.Start, declaration.Span.End));
                    break;
            }
        }

        return HashTokenRanges(tokens, ranges);
    }

    private static string HashTokenRange(IReadOnlyList<Token> tokens, int startInclusive, int endExclusive)
        => HashTokenRanges(tokens, new[] { (startInclusive, endExclusive) });

    private static string HashTokenRanges(IReadOnlyList<Token> tokens, IReadOnlyList<(int Start, int End)> ranges)
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

            var inRange = false;
            for (var r = 0; r < ranges.Count; r++)
            {
                var range = ranges[r];
                if (token.Span.Start >= range.Start && token.Span.End <= range.End)
                {
                    inRange = true;
                    break;
                }
            }

            if (!inRange)
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
