namespace Stasis.LanguageServer.Handlers;

using OmniSharp.Extensions.LanguageServer.Protocol.Client.Capabilities;
using OmniSharp.Extensions.LanguageServer.Protocol.Document;
using OmniSharp.Extensions.LanguageServer.Protocol.Models;
using Stasis.Compiler.Semantic;
using Stasis.LanguageServer.Models;
using Stasis.LanguageServer.Services;
using Stasis.Compiler.Syntax;
using CompilerSymbolKind = Stasis.Compiler.Semantic.SymbolKind;

public class CompletionHandler : CompletionHandlerBase
{
    private readonly DocumentManager _documentManager;

    public CompletionHandler(DocumentManager documentManager)
    {
        _documentManager = documentManager;
    }

    public override async Task<CompletionList> Handle(CompletionParams request, CancellationToken cancellationToken)
    {
        cancellationToken.ThrowIfCancellationRequested();
        var uri = request.TextDocument.Uri.ToString();
        var doc = _documentManager.GetDocument(uri);

        if (doc?.ParseResult == null)
        {
            await Console.Error.WriteLineAsync($"[completion] {uri} missing parse/symbol data");
            return new CompletionList();
        }

        var lineSpan = GetLineSpan(doc.Content, request.Position.Line);
        var lineOffset = lineSpan.Start + Math.Min(request.Position.Character, lineSpan.Text.Length);
        var offset = lineOffset;
        var triggerChar = request.Context?.TriggerCharacter;

        if (!TryGetMemberAccessReceiverChainFromLinePrefix(lineSpan.Text, request.Position.Character, out var receiverChain) &&
            !TryGetMemberAccessReceiverChain(doc.Content, offset, out receiverChain))
        {
            if (triggerChar == "." && TryGetMemberAccessReceiverChainFromPosition(doc.Content, offset, out receiverChain))
            {
                await Console.Error.WriteLineAsync($"[completion] {uri} fallback chain {string.Join(".", receiverChain)}");
            }
            else
            {
                // Not a member access completion; fall back to global identifier completions.
                var fallbackItems = GetGlobalCompletions(doc, offset, request.Context);
                return new CompletionList(fallbackItems);
            }
        }

        if (doc.SymbolIndex == null)
        {
            await Console.Error.WriteLineAsync($"[completion] {uri} missing symbol index");
            return new CompletionList();
        }

        var memberPrefix = string.Empty;
        var (receiverChainForType, effectiveOffset) =
            GetMemberPrefixAndReceiverChain(lineSpan.Text, lineSpan.Start, request.Position.Character, receiverChain, out memberPrefix);

        var receiverTypeName = ResolveReceiverTypeName(receiverChainForType, doc, effectiveOffset);
        if (string.IsNullOrEmpty(receiverTypeName))
        {
            var posInfoFail = $"{request.Position.Line}:{request.Position.Character}";
            await Console.Error.WriteLineAsync(
                $"[completion] {uri} unresolved chain {string.Join(".", receiverChainForType)} pos {posInfoFail} line[{request.Position.Line}]={EscapeLine(lineSpan.Text)}");
            return new CompletionList();
        }

        cancellationToken.ThrowIfCancellationRequested();
        var items = GetMemberCompletions(receiverTypeName, doc.SymbolIndex, memberPrefix);
        var preview = string.Join(", ", items.Take(10).Select(i => i.Label));
        var posInfoOk = $"{request.Position.Line}:{request.Position.Character}";
        await Console.Error.WriteLineAsync($"[completion] {uri} {string.Join(".", receiverChainForType)} -> {receiverTypeName} ({items.Count}) [{preview}] pos {posInfoOk} line[{request.Position.Line}]={EscapeLine(lineSpan.Text)}");
        return new CompletionList(items);
    }

    public override Task<CompletionItem> Handle(CompletionItem request, CancellationToken cancellationToken)
    {
        // Return the completion item as-is (no additional resolution needed for now)
        return Task.FromResult(request);
    }

    protected override CompletionRegistrationOptions CreateRegistrationOptions(CompletionCapability? capability, ClientCapabilities clientCapabilities)
    {
        return new CompletionRegistrationOptions
        {
            DocumentSelector = new TextDocumentSelector(new TextDocumentFilter { Language = "stasis" }),
            TriggerCharacters = new Container<string>(".")
        };
    }

    private static IReadOnlyList<CompletionItem> GetMemberCompletions(string receiverTypeName, SymbolIndex index, string memberPrefix)
    {
        if (index.GetEnum(receiverTypeName) is { } enumSymbol)
        {
            return enumSymbol.Members
                .Where(m => string.IsNullOrEmpty(memberPrefix) || m.StartsWith(memberPrefix, StringComparison.Ordinal))
                .Select(m => new CompletionItem
                {
                    Label = m,
                    Kind = CompletionItemKind.EnumMember,
                    Detail = $"{enumSymbol.Name} member",
                    InsertText = m
                })
                .ToArray();
        }

        if (index.GetStruct(receiverTypeName) is { } structSymbol)
        {
            return structSymbol.Fields
                .Where(f => string.IsNullOrEmpty(memberPrefix) || f.Name.StartsWith(memberPrefix, StringComparison.Ordinal))
                .Select(f => new CompletionItem
                {
                    Label = f.Name,
                    Kind = CompletionItemKind.Field,
                    Detail = $"{f.Name}: {f.TypeText}",
                    InsertText = f.Name
                })
                .ToArray();
        }

        return Array.Empty<CompletionItem>();
    }

    private static (IReadOnlyList<string> ReceiverChain, int EffectiveOffset) GetMemberPrefixAndReceiverChain(
        string lineText,
        int lineStartOffset,
        int character,
        IReadOnlyList<string> parsedChain,
        out string memberPrefix)
    {
        memberPrefix = string.Empty;

        if (parsedChain.Count == 0)
        {
            return (parsedChain, lineStartOffset);
        }

        // If the cursor is after a partial identifier (e.g. `state.config.ba`), treat the last segment as the prefix
        // and resolve the receiver type from the chain excluding that last segment.
        var endIndex = Math.Min(character, lineText.Length) - 1;
        while (endIndex >= 0 && char.IsWhiteSpace(lineText[endIndex]))
        {
            endIndex--;
        }

        if (endIndex >= 0 && lineText[endIndex] != '.')
        {
            if (parsedChain.Count >= 2)
            {
                memberPrefix = parsedChain[^1];
                return (parsedChain.Take(parsedChain.Count - 1).ToArray(), lineStartOffset + endIndex);
            }
        }

        return (parsedChain, lineStartOffset + Math.Max(0, endIndex));
    }

    private static IReadOnlyList<CompletionItem> GetGlobalCompletions(DocumentState doc, int cursorOffset, CompletionContext? context)
    {
        var prefix = GetIdentifierPrefix(doc.Content, cursorOffset);
        var items = new List<CompletionItem>(128);

        var isTopLevel = doc.ParseResult?.CompilationUnit is { } unit &&
            !TryGetEnclosingCallable(unit, cursorOffset, out _, out _);

        var triggerKind = context?.TriggerKind ?? default;
        var allowKeywords = isTopLevel && (triggerKind == CompletionTriggerKind.Invoked || !string.IsNullOrEmpty(prefix));
        if (allowKeywords)
        {
            AddKeywordCompletions(items, prefix);
        }

        if (string.IsNullOrEmpty(prefix) && items.Count == 0)
        {
            return Array.Empty<CompletionItem>();
        }

        if (doc.SemanticResult?.Symbols is { Count: > 0 } symbols)
        {
            foreach (var (name, sym) in symbols)
            {
                if (!IsGoodGlobalCompletionName(name, prefix))
                {
                    continue;
                }

                if (IsBuiltinTypeName(name))
                {
                    continue;
                }

                var kind = sym.Kind switch
                {
                    CompilerSymbolKind.Function => CompletionItemKind.Function,
                    CompilerSymbolKind.Global => CompletionItemKind.Variable,
                    CompilerSymbolKind.Const => CompletionItemKind.Constant,
                    CompilerSymbolKind.Struct => CompletionItemKind.Struct,
                    CompilerSymbolKind.Enum => CompletionItemKind.Enum,
                    _ => CompletionItemKind.Text
                };

                items.Add(new CompletionItem
                {
                    Label = name,
                    Kind = kind,
                    InsertText = name
                });
            }
        }

        // Parse-tree fallback: if semantic isn't available, still offer types and globals defined in this file.
        var fallbackUnit = doc.ExpandedParseResult?.CompilationUnit ?? doc.ParseResult?.CompilationUnit;
        if (items.Count == 0 && fallbackUnit is { } compilationUnit)
        {
            foreach (var decl in compilationUnit.Declarations)
            {
                var name = decl switch
                {
                    StructDeclarationSyntax s => s.Name.Text,
                    EnumDeclarationSyntax e => e.Name.Text,
                    GlobalDeclarationSyntax g => g.Name.Text,
                    ConstDeclarationSyntax c => c.Name.Text,
                    FunctionDeclarationSyntax f => f.Name.Text,
                    _ => null
                };

                if (name == null || !IsGoodGlobalCompletionName(name, prefix) || IsBuiltinTypeName(name))
                {
                    continue;
                }

                items.Add(new CompletionItem
                {
                    Label = name,
                    Kind = decl switch
                    {
                        StructDeclarationSyntax => CompletionItemKind.Struct,
                        EnumDeclarationSyntax => CompletionItemKind.Enum,
                        GlobalDeclarationSyntax => CompletionItemKind.Variable,
                        ConstDeclarationSyntax => CompletionItemKind.Constant,
                        FunctionDeclarationSyntax => CompletionItemKind.Function,
                        _ => CompletionItemKind.Text
                    },
                    InsertText = name
                });
            }
        }

        return items
            .OrderBy(i => i.Label, StringComparer.Ordinal)
            .Take(200)
            .ToArray();
    }

    private static void AddKeywordCompletions(List<CompletionItem> items, string prefix)
    {
        var keywords = new[]
        {
            "import",
            "const",
            "global",
            "struct",
            "enum",
            "function",
            "test",
        };

        foreach (var keyword in keywords)
        {
            if (!string.IsNullOrEmpty(prefix) && !keyword.StartsWith(prefix, StringComparison.Ordinal))
            {
                continue;
            }

            items.Add(new CompletionItem
            {
                Label = keyword,
                Kind = CompletionItemKind.Keyword,
                InsertText = keyword
            });
        }
    }

    private static bool IsGoodGlobalCompletionName(string name, string prefix)
    {
        if (name.Length < prefix.Length)
        {
            return false;
        }

        if (name.Contains('.', StringComparison.Ordinal))
        {
            // Enum members are resolved via member access completion (State.Idle).
            return false;
        }

        return name.StartsWith(prefix, StringComparison.Ordinal);
    }

    private static bool IsBuiltinTypeName(string name) =>
        name is "u8" or "u16" or "u32" or "i32" or "f32" or "f64" or "bool" or "string" or "utf8" or "ascii" or "void";

    private static string GetIdentifierPrefix(string content, int cursorOffset)
    {
        if (cursorOffset < 0)
        {
            cursorOffset = 0;
        }
        if (cursorOffset > content.Length)
        {
            cursorOffset = content.Length;
        }

        var i = cursorOffset - 1;
        while (i >= 0 && IsIdentifierChar(content[i]))
        {
            i--;
        }

        var start = i + 1;
        if (start < 0 || start > cursorOffset)
        {
            return string.Empty;
        }

        return content.Substring(start, cursorOffset - start);
    }

    private static string? ResolveReceiverTypeName(IReadOnlyList<string> receiverChain, DocumentState doc, int cursorOffset)
    {
        if (receiverChain.Count == 0)
        {
            return null;
        }

        var baseName = receiverChain[0];

        // Prefer in-scope locals/parameters over globals (including imported globals) when semantic info is unavailable.
        if (TryResolveLocalTypeName(doc.ParseResult?.CompilationUnit, cursorOffset, baseName, out var localTypeName))
        {
            return ResolveMemberChain(localTypeName, receiverChain, doc.SymbolIndex);
        }

        // Enum member completion (State.Idle). Keep after locals so a local named "State" can still win.
        if (doc.SymbolIndex?.IsEnum(baseName) == true)
        {
            return baseName;
        }

        // Global symbol table lookup (types, globals, consts, functions, built-ins).
        if (doc.SemanticResult?.Symbols is not null &&
            doc.SemanticResult.Symbols.TryGetValue(baseName, out var receiverSymbol))
        {
            return ResolveMemberChain(receiverSymbol.Type switch
            {
                NamedTypeSymbol named => named.TypeName,
                _ => null
            }, receiverChain, doc.SymbolIndex);
        }

        // Parse-tree fallback for globals/consts when semantic info is unavailable.
        // Prefer the import-expanded parse (it has the full declaration set) and fall back to the local parse.
        var globalsUnit = doc.ExpandedParseResult?.CompilationUnit ?? doc.ParseResult?.CompilationUnit;
        var localsUnit = doc.ParseResult?.CompilationUnit;

        if (globalsUnit is null)
        {
            return null;
        }

        // Prefer globals/consts declared in the current file over imported declarations when names collide.
        if (localsUnit is not null)
        {
            foreach (var decl in localsUnit.Declarations)
            {
                switch (decl)
                {
                    case GlobalDeclarationSyntax global when string.Equals(global.Name.Text, baseName, StringComparison.Ordinal):
                        return ResolveMemberChain(GetNamedTypeName(global.Type), receiverChain, doc.SymbolIndex);
                    case ConstDeclarationSyntax constant when string.Equals(constant.Name.Text, baseName, StringComparison.Ordinal):
                        return ResolveMemberChain(GetNamedTypeName(constant.Type), receiverChain, doc.SymbolIndex);
                }
            }
        }

        foreach (var decl in globalsUnit.Declarations)
        {
            switch (decl)
            {
                case GlobalDeclarationSyntax global when string.Equals(global.Name.Text, baseName, StringComparison.Ordinal):
                    return ResolveMemberChain(GetNamedTypeName(global.Type), receiverChain, doc.SymbolIndex);
                case ConstDeclarationSyntax constant when string.Equals(constant.Name.Text, baseName, StringComparison.Ordinal):
                    return ResolveMemberChain(GetNamedTypeName(constant.Type), receiverChain, doc.SymbolIndex);
            }
        }

        return null;
    }

    private static bool TryResolveLocalTypeName(
        CompilationUnitSyntax? compilationUnit,
        int cursorOffset,
        string baseName,
        out string? localTypeName)
    {
        localTypeName = null;

        if (compilationUnit is null)
        {
            return false;
        }

        if (!TryGetEnclosingCallable(compilationUnit, cursorOffset, out var parameters, out var body))
        {
            return false;
        }

        foreach (var parameter in parameters)
        {
            if (string.Equals(parameter.Name.Text, baseName, StringComparison.Ordinal))
            {
                localTypeName = GetNamedTypeName(parameter.Type);
                return !string.IsNullOrEmpty(localTypeName);
            }
        }

        VariableDeclarationSyntax? best = null;
        foreach (var decl in EnumerateVariableDeclarations(body))
        {
            if (!string.Equals(decl.Name.Text, baseName, StringComparison.Ordinal))
            {
                continue;
            }

            if (decl.Span.Start > cursorOffset)
            {
                continue;
            }

            if (best == null || decl.Span.Start >= best.Span.Start)
            {
                best = decl;
            }
        }

        localTypeName = best?.Type is null ? null : GetNamedTypeName(best.Type);
        return !string.IsNullOrEmpty(localTypeName);
    }

    private static bool TryGetEnclosingCallable(
        CompilationUnitSyntax compilationUnit,
        int cursorOffset,
        out IReadOnlyList<ParameterSyntax> parameters,
        out BlockStatementSyntax body)
    {
        foreach (var decl in compilationUnit.Declarations)
        {
            switch (decl)
            {
                case FunctionDeclarationSyntax fn when ContainsOffset(fn.Span, cursorOffset):
                    if (fn.Body == null)
                    {
                        break;
                    }
                    parameters = fn.Parameters;
                    body = fn.Body;
                    return true;
                case TestDeclarationSyntax test when ContainsOffset(test.Span, cursorOffset):
                    parameters = test.Parameters;
                    body = test.Body;
                    return true;
            }
        }

        parameters = Array.Empty<ParameterSyntax>();
        body = null!;
        return false;
    }

    private static IEnumerable<VariableDeclarationSyntax> EnumerateVariableDeclarations(StatementSyntax stmt)
    {
        switch (stmt)
        {
            case VariableDeclarationSyntax v:
                yield return v;
                yield break;
            case BlockStatementSyntax block:
                foreach (var s in block.Statements)
                {
                    foreach (var v in EnumerateVariableDeclarations(s))
                    {
                        yield return v;
                    }
                }
                yield break;
            case IfStatementSyntax ifStmt:
                foreach (var v in EnumerateVariableDeclarations(ifStmt.ThenBlock))
                {
                    yield return v;
                }
                if (ifStmt.ElseBlock != null)
                {
                    foreach (var v in EnumerateVariableDeclarations(ifStmt.ElseBlock))
                    {
                        yield return v;
                    }
                }
                yield break;
            case ForStatementSyntax forStmt:
                if (forStmt.Initializer != null)
                {
                    // no variable decls in expressions yet
                }
                foreach (var v in EnumerateVariableDeclarations(forStmt.Body))
                {
                    yield return v;
                }
                yield break;
            case ForeachStatementSyntax foreachStmt:
                foreach (var v in EnumerateVariableDeclarations(foreachStmt.Body))
                {
                    yield return v;
                }
                yield break;
            default:
                yield break;
        }
    }

    private static bool ContainsOffset(Stasis.Compiler.SourceSpan span, int offset) =>
        span.Start <= offset && offset < span.Start + span.Length;

    private static string? GetNamedTypeName(TypeSyntax type) =>
        type switch
        {
            NamedTypeSyntax named => named.Name,
            ArrayTypeSyntax arr when arr.ElementType is NamedTypeSyntax element => element.Name,
            _ => null
        };

    private static bool TryGetMemberAccessReceiverChain(string content, int cursorOffset, out IReadOnlyList<string> receiverChain)
    {
        receiverChain = Array.Empty<string>();
        if (cursorOffset < 0 || cursorOffset > content.Length)
        {
            return false;
        }

        // Find the dot token for the member access immediately before the cursor.
        var dotIndex = cursorOffset;
        if (dotIndex < content.Length && content[dotIndex] == '.')
        {
            // dotIndex already points at the dot
        }
        else
        {
            dotIndex = cursorOffset - 1;
            if (!TrySkipInlineWhitespaceLeft(content, ref dotIndex))
            {
                return false;
            }

            // If we're in the middle of typing an identifier, walk backwards to find the dot.
            var identStart = dotIndex;
            while (identStart >= 0 && IsIdentifierChar(content[identStart]))
            {
                identStart--;
            }

            dotIndex = identStart;
            if (!TrySkipInlineWhitespaceLeft(content, ref dotIndex))
            {
                return false;
            }

            if (dotIndex < 0 || content[dotIndex] != '.')
            {
                return false;
            }
        }

        // Extract receiver chain to the left of '.'.
        var names = new Stack<string>();
        var i = dotIndex - 1;
        while (true)
        {
            if (!TrySkipInlineWhitespaceLeft(content, ref i))
            {
                break;
            }

            if (i < 0)
            {
                break;
            }

            var end = i;
            while (i >= 0 && IsIdentifierChar(content[i]))
            {
                i--;
            }

            var start = i + 1;
            if (start > end)
            {
                break;
            }

            names.Push(content.Substring(start, end - start + 1));

            if (!TrySkipInlineWhitespaceLeft(content, ref i))
            {
                break;
            }

            if (i >= 0 && content[i] == '.')
            {
                i--;
                continue;
            }

            break;
        }

        if (names.Count == 0)
        {
            return false;
        }

        receiverChain = names.ToArray();
        return true;
    }

    private static bool IsIdentifierChar(char c) => char.IsLetterOrDigit(c) || c == '_';

    private static bool TryGetMemberAccessReceiverChainFromPosition(string content, int cursorOffset, out IReadOnlyList<string> receiverChain)
    {
        receiverChain = Array.Empty<string>();
        if (cursorOffset <= 0 || cursorOffset > content.Length)
        {
            return false;
        }

        var names = new Stack<string>();
        var i = cursorOffset - 1;

        if (!TrySkipInlineWhitespaceLeft(content, ref i) || i < 0)
        {
            return false;
        }

        while (true)
        {
            var end = i;
            while (i >= 0 && IsIdentifierChar(content[i]))
            {
                i--;
            }

            var start = i + 1;
            if (start > end)
            {
                break;
            }

            names.Push(content.Substring(start, end - start + 1));

            if (!TrySkipInlineWhitespaceLeft(content, ref i))
            {
                break;
            }

            if (i >= 0 && content[i] == '.')
            {
                i--;
                if (!TrySkipInlineWhitespaceLeft(content, ref i))
                {
                    break;
                }
                continue;
            }

            break;
        }

        if (names.Count == 0)
        {
            return false;
        }

        receiverChain = names.ToArray();
        return true;
    }

    private static bool TryGetMemberAccessReceiverChainFromLinePrefix(string lineText, int character, out IReadOnlyList<string> receiverChain)
    {
        receiverChain = Array.Empty<string>();
        if (character < 0)
        {
            return false;
        }

        var names = new Stack<string>();
        var endIndex = Math.Min(character, lineText.Length) - 1;
        while (endIndex >= 0 && char.IsWhiteSpace(lineText[endIndex]))
        {
            endIndex--;
        }

        if (endIndex < 0)
        {
            return false;
        }

        if (lineText[endIndex] == '.')
        {
            endIndex--;
            while (endIndex >= 0 && char.IsWhiteSpace(lineText[endIndex]))
            {
                endIndex--;
            }
        }

        if (endIndex < 0 || lineText.LastIndexOf('.', endIndex) < 0)
        {
            return false;
        }

        var i = endIndex;
        while (true)
        {
            if (!TrySkipInlineWhitespaceLeft(lineText, ref i))
            {
                break;
            }

            if (i < 0)
            {
                break;
            }

            var end = i;
            while (i >= 0 && IsIdentifierChar(lineText[i]))
            {
                i--;
            }

            var start = i + 1;
            if (start > end)
            {
                break;
            }

            names.Push(lineText.Substring(start, end - start + 1));

            if (!TrySkipInlineWhitespaceLeft(lineText, ref i))
            {
                break;
            }

            if (i >= 0 && lineText[i] == '.')
            {
                i--;
                continue;
            }

            break;
        }

        if (names.Count == 0)
        {
            return false;
        }

        receiverChain = names.ToArray();
        return true;
    }

    private static bool TrySkipInlineWhitespaceLeft(string content, ref int index)
    {
        while (index >= 0 && char.IsWhiteSpace(content[index]))
        {
            if (content[index] == '\n' || content[index] == '\r')
            {
                return false;
            }
            index--;
        }

        return true;
    }

    private static string? ResolveMemberChain(string? baseTypeName, IReadOnlyList<string> receiverChain, SymbolIndex? index)
    {
        if (string.IsNullOrEmpty(baseTypeName))
        {
            return null;
        }

        if (receiverChain.Count <= 1)
        {
            return baseTypeName;
        }

        if (index == null)
        {
            return null;
        }

        var currentType = baseTypeName;
        for (var i = 1; i < receiverChain.Count; i++)
        {
            var memberName = receiverChain[i];
            if (index.GetStruct(currentType) is not { } structSymbol)
            {
                return null;
            }

            var field = structSymbol.Fields.FirstOrDefault(f => string.Equals(f.Name, memberName, StringComparison.Ordinal));
            if (field == null)
            {
                return null;
            }

            currentType = ExtractNamedTypeName(field.TypeText);
            if (string.IsNullOrEmpty(currentType))
            {
                return null;
            }
        }

        return currentType;
    }

    private static string? ExtractNamedTypeName(string typeText)
    {
        if (string.IsNullOrEmpty(typeText))
        {
            return null;
        }

        typeText = typeText.Trim();

        var bracketIndex = typeText.IndexOf('[');
        if (bracketIndex >= 0)
        {
            var element = typeText[..bracketIndex].Trim();
            return string.IsNullOrEmpty(element) ? null : element;
        }

        return typeText;
    }

    private static string GetSnippet(string content, int offset, int radius)
    {
        if (offset < 0)
        {
            offset = 0;
        }
        if (offset > content.Length)
        {
            offset = content.Length;
        }

        var start = Math.Max(0, offset - radius);
        var length = Math.Min(content.Length - start, radius * 2);
        var snippet = content.Substring(start, length);
        return snippet.Replace("\r", "\\r").Replace("\n", "\\n");
    }

    private static (string Text, int Start, int LineCount) GetLineSpan(string content, int lineIndex)
    {
        var line = 0;
        var lineStart = 0;
        var offset = 0;

        while (offset < content.Length)
        {
            var ch = content[offset];
            if (ch == '\r' || ch == '\n')
            {
                if (line == lineIndex)
                {
                    var lineText = content.Substring(lineStart, offset - lineStart);
                    return (lineText, lineStart, line + 1);
                }

                if (ch == '\r' && offset + 1 < content.Length && content[offset + 1] == '\n')
                {
                    offset += 2;
                }
                else
                {
                    offset += 1;
                }

                line++;
                lineStart = offset;
                continue;
            }

            offset += 1;
        }

        if (line == lineIndex)
        {
            var lineText = content.Substring(lineStart, offset - lineStart);
            return (lineText, lineStart, line + 1);
        }

        return (string.Empty, lineStart, line + 1);
    }

    private static string EscapeLine(string lineText) =>
        lineText.Replace("\r", "\\r").Replace("\n", "\\n");
}
