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
        if (!TryGetMemberAccessReceiver(doc.Content, offset, out var receiverName))
        {
            return Task.FromResult(new CompletionList());
        }

        var receiverTypeName = ResolveReceiverTypeName(receiverName, doc, offset);
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

    private static string? ResolveReceiverTypeName(string receiverName, DocumentState doc, int cursorOffset)
    {
        if (doc.SymbolIndex?.IsEnum(receiverName) == true)
        {
            return receiverName;
        }

        // Global symbol table lookup (types, globals, consts, functions, built-ins).
        if (doc.SemanticResult?.Symbols is not null &&
            doc.SemanticResult.Symbols.TryGetValue(receiverName, out var receiverSymbol))
        {
            return receiverSymbol.Type switch
            {
                NamedTypeSymbol named => named.TypeName,
                _ => null
            };
        }

        // Local lookup inside the enclosing function/test (parameters + let bindings).
        if (doc.ParseResult?.CompilationUnit is not { } compilationUnit)
        {
            return null;
        }

        if (!TryGetEnclosingCallable(compilationUnit, cursorOffset, out var parameters, out var body))
        {
            return null;
        }

        foreach (var parameter in parameters)
        {
            if (string.Equals(parameter.Name.Text, receiverName, StringComparison.Ordinal))
            {
                return GetNamedTypeName(parameter.Type);
            }
        }

        VariableDeclarationSyntax? best = null;
        foreach (var decl in EnumerateVariableDeclarations(body))
        {
            if (!string.Equals(decl.Name.Text, receiverName, StringComparison.Ordinal))
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

        return best?.Type is null ? null : GetNamedTypeName(best.Type);
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

    private static bool TryGetMemberAccessReceiver(string content, int cursorOffset, out string receiverName)
    {
        receiverName = string.Empty;
        if (cursorOffset < 0 || cursorOffset > content.Length)
        {
            return false;
        }

        // Find the dot token for the member access immediately before the cursor.
        var dotIndex = cursorOffset - 1;
        while (dotIndex >= 0 && char.IsWhiteSpace(content[dotIndex]))
        {
            dotIndex--;
        }

        // If we're in the middle of typing an identifier, walk backwards to find the dot.
        var identStart = dotIndex;
        while (identStart >= 0 && IsIdentifierChar(content[identStart]))
        {
            identStart--;
        }

        dotIndex = identStart;
        while (dotIndex >= 0 && char.IsWhiteSpace(content[dotIndex]))
        {
            dotIndex--;
        }

        if (dotIndex < 0 || content[dotIndex] != '.')
        {
            return false;
        }

        // Extract receiver identifier to the left of '.'.
        var i = dotIndex - 1;
        while (i >= 0 && char.IsWhiteSpace(content[i]))
        {
            i--;
        }

        var end = i;
        while (i >= 0 && IsIdentifierChar(content[i]))
        {
            i--;
        }

        var start = i + 1;
        if (start > end)
        {
            return false;
        }

        receiverName = content.Substring(start, end - start + 1);
        return receiverName.Length > 0;
    }

    private static bool IsIdentifierChar(char c) => char.IsLetterOrDigit(c) || c == '_';
}
