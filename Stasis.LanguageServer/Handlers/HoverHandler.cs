namespace Stasis.LanguageServer.Handlers;

using OmniSharp.Extensions.LanguageServer.Protocol.Client.Capabilities;
using OmniSharp.Extensions.LanguageServer.Protocol.Document;
using OmniSharp.Extensions.LanguageServer.Protocol.Models;
using Stasis.Compiler;
using Stasis.Compiler.Semantic;
using Stasis.Compiler.Syntax;
using Stasis.LanguageServer.Models;
using Stasis.LanguageServer.Services;
using CompilerSymbolKind = Stasis.Compiler.Semantic.SymbolKind;

public class HoverHandler : HoverHandlerBase
{
    private readonly DocumentManager _documentManager;

    public HoverHandler(DocumentManager documentManager)
    {
        _documentManager = documentManager;
    }

    public override Task<Hover?> Handle(HoverParams request, CancellationToken cancellationToken)
    {
        var uri = request.TextDocument.Uri.ToString();
        var doc = _documentManager.GetDocument(uri);

        if (doc?.ParseResult?.CompilationUnit == null)
            return Task.FromResult<Hover?>(null);

        var offset = TextPositionConverter.PositionToOffset(doc.Content, request.Position);
        if (!TryGetIdentifierChainAtOffset(doc.Content, offset, out var identifierChain))
        {
            return Task.FromResult<Hover?>(null);
        }

        var hover = GetHoverInfo(identifierChain, doc, offset);
        return Task.FromResult<Hover?>(hover);
    }

    protected override HoverRegistrationOptions CreateRegistrationOptions(HoverCapability? capability, ClientCapabilities clientCapabilities)
    {
        return new HoverRegistrationOptions();
    }

    /// <summary>
    /// Extracts hover information using semantic analysis + the import-expanded symbol index.
    /// </summary>
    private static Hover? GetHoverInfo(IReadOnlyList<string> identifierChain, DocumentState doc, int cursorOffset)
    {
        if (identifierChain.Count == 0)
        {
            return null;
        }

        if (identifierChain.Count == 1)
        {
            var name = identifierChain[0];

            // Prefer in-scope locals/parameters over globals/builtins when names collide.
            if (TryResolveLocalOrDeclarationSymbol(doc.ParseResult!.CompilationUnit!, cursorOffset, name, out var kindLabel, out var typeText))
            {
                return CreateHover(FormatLocalSymbolInfo(kindLabel, name, typeText));
            }

            if (doc.SemanticResult?.Symbols is not null &&
                doc.SemanticResult.Symbols.TryGetValue(name, out var symbol))
            {
                return CreateHover(FormatSymbolInfo(symbol));
            }

            return null;
        }

        if (doc.SymbolIndex == null)
        {
            return null;
        }

        var baseName = identifierChain[0];
        var memberName = identifierChain[^1];

        if (doc.SymbolIndex.GetEnum(baseName) is { } enumSymbol)
        {
            if (enumSymbol.Members.Any(m => string.Equals(m, memberName, StringComparison.Ordinal)))
            {
                return CreateHover(FormatEnumMemberInfo(baseName, memberName));
            }
        }

        // Prefer in-scope locals/parameters over globals/builtins when names collide.
        string? receiverTypeName = null;
        if (TryResolveLocalOrDeclarationSymbol(doc.ParseResult!.CompilationUnit!, cursorOffset, baseName, out _, out var receiverTypeText) &&
            !string.IsNullOrEmpty(receiverTypeText))
        {
            receiverTypeName = ExtractNamedTypeName(receiverTypeText);
        }

        if (string.IsNullOrEmpty(receiverTypeName) &&
            doc.SemanticResult?.Symbols is not null &&
            doc.SemanticResult.Symbols.TryGetValue(baseName, out var receiverSymbol))
        {
            receiverTypeName = receiverSymbol.Type is NamedTypeSymbol named ? named.TypeName : null;
        }

        if (string.IsNullOrEmpty(receiverTypeName))
        {
            return null;
        }

        if (!TryResolveMemberReceiverType(receiverTypeName, identifierChain, doc.SymbolIndex, out var memberReceiverType))
        {
            return null;
        }

        if (doc.SymbolIndex.GetStruct(memberReceiverType) is not { } structSymbol)
        {
            if (doc.SymbolIndex.GetEnum(memberReceiverType) is { } enumReceiver &&
                enumReceiver.Members.Any(m => string.Equals(m, memberName, StringComparison.Ordinal)))
            {
                return CreateHover(FormatEnumMemberInfo(memberReceiverType, memberName));
            }

            return null;
        }

        var field = structSymbol.Fields.FirstOrDefault(f => string.Equals(f.Name, memberName, StringComparison.Ordinal));
        if (field == null)
        {
            return null;
        }

        return CreateHover(FormatFieldInfo(field));
    }

    private static Hover CreateHover(string markdown) =>
        new()
        {
            Contents = new MarkedStringsOrMarkupContent(new MarkupContent
            {
                Kind = MarkupKind.Markdown,
                Value = markdown
            })
        };

    /// <summary>
    /// Formats symbol information as markdown for hover display.
    /// </summary>
    private static string FormatSymbolInfo(Symbol symbol)
    {
        var kindLabel = symbol.Kind switch
        {
            CompilerSymbolKind.Function => "function",
            CompilerSymbolKind.Parameter => "parameter",
            CompilerSymbolKind.Local => "local variable",
            CompilerSymbolKind.Global => "global variable",
            CompilerSymbolKind.Const => "constant",
            CompilerSymbolKind.Struct => "struct",
            CompilerSymbolKind.Enum => "enum",
            CompilerSymbolKind.Test => "test function",
            _ => "symbol"
        };

        var typeInfo = symbol.Type != null ? $": {symbol.Type.Name}" : "";

        return $"```stasis\n({kindLabel}) {symbol.Name}{typeInfo}\n```";
    }

    private static string FormatLocalSymbolInfo(string kindLabel, string name, string? typeText)
    {
        var typeInfo = string.IsNullOrWhiteSpace(typeText) ? "" : $": {typeText}";
        return $"```stasis\n({kindLabel}) {name}{typeInfo}\n```";
    }

    private static string FormatFieldInfo(StructFieldSymbol field) =>
        $"```stasis\n(field) {field.Name}: {field.TypeText}\n```";

    private static string FormatEnumMemberInfo(string enumName, string memberName) =>
        $"```stasis\n(enum member) {enumName}.{memberName}\n```";

    private static bool TryResolveMemberReceiverType(
        string baseTypeName,
        IReadOnlyList<string> identifierChain,
        SymbolIndex index,
        out string memberReceiverTypeName)
    {
        memberReceiverTypeName = baseTypeName;

        if (identifierChain.Count <= 1)
        {
            return true;
        }

        // Walk through chain excluding the last member name; we want the receiver type for the hovered member.
        for (var i = 1; i < identifierChain.Count - 1; i++)
        {
            var member = identifierChain[i];
            if (index.GetStruct(memberReceiverTypeName) is not { } structSymbol)
            {
                return false;
            }

            var field = structSymbol.Fields.FirstOrDefault(f => string.Equals(f.Name, member, StringComparison.Ordinal));
            if (field == null)
            {
                return false;
            }

            var nextType = ExtractNamedTypeName(field.TypeText);
            if (string.IsNullOrEmpty(nextType))
            {
                return false;
            }

            memberReceiverTypeName = nextType;
        }

        return true;
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
            return null;
        }

        return typeText;
    }

    private static bool TryResolveLocalOrDeclarationSymbol(
        CompilationUnitSyntax compilationUnit,
        int cursorOffset,
        string name,
        out string kindLabel,
        out string? typeText)
    {
        kindLabel = "symbol";
        typeText = null;

        foreach (var decl in compilationUnit.Declarations)
        {
            switch (decl)
            {
                case StructDeclarationSyntax s when string.Equals(s.Name.Text, name, StringComparison.Ordinal):
                    kindLabel = "struct";
                    typeText = null;
                    return true;
                case EnumDeclarationSyntax e when string.Equals(e.Name.Text, name, StringComparison.Ordinal):
                    kindLabel = "enum";
                    typeText = null;
                    return true;
                case GlobalDeclarationSyntax g when string.Equals(g.Name.Text, name, StringComparison.Ordinal):
                    kindLabel = "global variable";
                    typeText = TypeSyntaxToString(g.Type);
                    return true;
                case ConstDeclarationSyntax c when string.Equals(c.Name.Text, name, StringComparison.Ordinal):
                    kindLabel = "constant";
                    typeText = TypeSyntaxToString(c.Type);
                    return true;
            }
        }

        if (!TryGetEnclosingCallable(compilationUnit, cursorOffset, out var parameters, out var body))
        {
            return false;
        }

        foreach (var parameter in parameters)
        {
            if (string.Equals(parameter.Name.Text, name, StringComparison.Ordinal))
            {
                kindLabel = "parameter";
                typeText = TypeSyntaxToString(parameter.Type);
                return true;
            }
        }

        VariableDeclarationSyntax? best = null;
        foreach (var decl in EnumerateVariableDeclarations(body))
        {
            if (!string.Equals(decl.Name.Text, name, StringComparison.Ordinal))
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

        if (best == null)
        {
            return false;
        }

        kindLabel = "local variable";
        typeText = best.Type == null ? null : TypeSyntaxToString(best.Type);
        return true;
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

    private static bool ContainsOffset(SourceSpan span, int cursorOffset) =>
        span.Start <= cursorOffset && cursorOffset < span.Start + span.Length;

    private static string TypeSyntaxToString(TypeSyntax type) =>
        type switch
        {
            NamedTypeSyntax named => named.Name,
            ArrayTypeSyntax arr when string.IsNullOrEmpty(arr.SizeText) => $"{TypeSyntaxToString(arr.ElementType)}[]",
            ArrayTypeSyntax arr => $"{TypeSyntaxToString(arr.ElementType)}[{arr.SizeText}]",
            _ => "unknown"
        };

    private static bool TryGetIdentifierChainAtOffset(string content, int offset, out IReadOnlyList<string> chain)
    {
        chain = Array.Empty<string>();

        if (offset < 0)
        {
            offset = 0;
        }
        if (offset > content.Length)
        {
            offset = content.Length;
        }

        // Prefer the character under the cursor; if it's not an identifier, fall back to the previous character.
        var scanIndex = offset;
        if (scanIndex == content.Length || (scanIndex < content.Length && !IsIdentifierChar(content[scanIndex])))
        {
            scanIndex = Math.Max(0, offset - 1);
        }

        if (scanIndex < 0 || scanIndex >= content.Length || !IsIdentifierChar(content[scanIndex]))
        {
            return false;
        }

        var end = scanIndex + 1;
        while (end < content.Length && IsIdentifierChar(content[end]))
        {
            end++;
        }

        var start = scanIndex;
        while (start - 1 >= 0 && IsIdentifierChar(content[start - 1]))
        {
            start--;
        }

        var names = new List<string>(8)
        {
            content.Substring(start, end - start)
        };

        var i = start - 1;
        while (TrySkipInlineWhitespaceLeft(content, ref i) && i >= 0 && content[i] == '.')
        {
            i--; // skip '.'
            if (!TrySkipInlineWhitespaceLeft(content, ref i))
            {
                break;
            }

            var prevEnd = i + 1;
            while (i >= 0 && IsIdentifierChar(content[i]))
            {
                i--;
            }

            var prevStart = i + 1;
            if (prevStart >= prevEnd)
            {
                break;
            }

            names.Add(content.Substring(prevStart, prevEnd - prevStart));
        }

        names.Reverse();
        chain = names;
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

    private static bool IsIdentifierChar(char ch) =>
        (ch >= 'a' && ch <= 'z') ||
        (ch >= 'A' && ch <= 'Z') ||
        (ch >= '0' && ch <= '9') ||
        ch == '_';
}
