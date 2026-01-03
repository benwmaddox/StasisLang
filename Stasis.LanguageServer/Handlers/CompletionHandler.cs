namespace Stasis.LanguageServer.Handlers;

using OmniSharp.Extensions.LanguageServer.Protocol.Client.Capabilities;
using OmniSharp.Extensions.LanguageServer.Protocol.Document;
using OmniSharp.Extensions.LanguageServer.Protocol.Models;
using Stasis.Compiler.Semantic;
using Stasis.LanguageServer.Models;
using Stasis.LanguageServer.Services;
using Stasis.Compiler.Syntax;

public class CompletionHandler : CompletionHandlerBase
{
    private readonly DocumentManager _documentManager;

    public CompletionHandler(DocumentManager documentManager)
    {
        _documentManager = documentManager;
    }

    public override Task<CompletionList> Handle(CompletionParams request, CancellationToken cancellationToken)
    {
        var uri = request.TextDocument.Uri.ToString();
        var doc = _documentManager.GetDocument(uri);

        if (doc?.ParseResult == null || doc.SymbolIndex == null)
            return Task.FromResult(new CompletionList());

        var offset = TextPositionConverter.PositionToOffset(doc.Content, request.Position);
        if (!TryGetMemberAccessReceiverChain(doc.Content, offset, out var receiverChain))
        {
            return Task.FromResult(new CompletionList());
        }

        var receiverTypeName = ResolveReceiverTypeName(receiverChain, doc, offset);
        if (string.IsNullOrEmpty(receiverTypeName))
        {
            return Task.FromResult(new CompletionList());
        }

        var items = GetMemberCompletions(receiverTypeName, doc.SymbolIndex);
        return Task.FromResult(new CompletionList(items));
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
            TriggerCharacters = new Container<string>(".")
        };
    }

    private static IReadOnlyList<CompletionItem> GetMemberCompletions(string receiverTypeName, SymbolIndex index)
    {
        if (index.GetEnum(receiverTypeName) is { } enumSymbol)
        {
            return enumSymbol.Members
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

    private static string? ResolveReceiverTypeName(IReadOnlyList<string> receiverChain, DocumentState doc, int cursorOffset)
    {
        if (receiverChain.Count == 0)
        {
            return null;
        }

        var baseName = receiverChain[0];

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

        if (doc.ParseResult?.CompilationUnit is not { } compilationUnit)
        {
            return null;
        }

        // Parse-tree fallback for globals/consts when semantic info is unavailable.
        foreach (var decl in compilationUnit.Declarations)
        {
            switch (decl)
            {
                case GlobalDeclarationSyntax global when string.Equals(global.Name.Text, baseName, StringComparison.Ordinal):
                    return ResolveMemberChain(GetNamedTypeName(global.Type), receiverChain, doc.SymbolIndex);
                case ConstDeclarationSyntax constant when string.Equals(constant.Name.Text, baseName, StringComparison.Ordinal):
                    return ResolveMemberChain(GetNamedTypeName(constant.Type), receiverChain, doc.SymbolIndex);
            }
        }

        // Local lookup inside the enclosing function/test (parameters + let bindings).
        if (!TryGetEnclosingCallable(compilationUnit, cursorOffset, out var parameters, out var body))
        {
            return null;
        }

        foreach (var parameter in parameters)
        {
            if (string.Equals(parameter.Name.Text, baseName, StringComparison.Ordinal))
            {
                return ResolveMemberChain(GetNamedTypeName(parameter.Type), receiverChain, doc.SymbolIndex);
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

        var localTypeName = best?.Type is null ? null : GetNamedTypeName(best.Type);
        return ResolveMemberChain(localTypeName, receiverChain, doc.SymbolIndex);
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
        var dotIndex = cursorOffset - 1;
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
            return null;
        }

        return typeText;
    }
}
