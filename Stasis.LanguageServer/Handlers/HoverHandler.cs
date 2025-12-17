namespace Stasis.LanguageServer.Handlers;

using OmniSharp.Extensions.LanguageServer.Protocol.Client.Capabilities;
using OmniSharp.Extensions.LanguageServer.Protocol.Document;
using OmniSharp.Extensions.LanguageServer.Protocol.Models;
using Stasis.Compiler;
using Stasis.Compiler.Syntax;
using Stasis.LanguageServer.Models;
using Stasis.LanguageServer.Services;

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

        if (doc?.SemanticResult == null || doc.ParseResult == null)
            return Task.FromResult<Hover?>(null);

        var offset = PositionToOffset(doc.Content, request.Position);
        var node = FindNodeAtPosition(doc.ParseResult.CompilationUnit, offset);

        if (node == null)
            return Task.FromResult<Hover?>(null);

        var hover = GetHoverInfo(node, doc, offset);
        return Task.FromResult(hover);
    }

    protected override HoverRegistrationOptions CreateRegistrationOptions(HoverCapability? capability, ClientCapabilities clientCapabilities)
    {
        return new HoverRegistrationOptions();
    }

    private static int PositionToOffset(string content, Position position)
    {
        int offset = 0;
        int currentLine = 0;

        foreach (var ch in content)
        {
            if (currentLine == position.Line)
            {
                if (offset - GetLineStart(content, currentLine) == position.Character)
                    return offset;
            }

            offset++;
            if (ch == '\n')
                currentLine++;
        }

        return offset;
    }

    private static int GetLineStart(string content, int line)
    {
        int offset = 0;
        int currentLine = 0;

        foreach (var ch in content)
        {
            if (currentLine == line)
                return offset;

            if (ch == '\n')
                currentLine++;

            offset++;
        }

        return offset;
    }

    private static SyntaxNode? FindNodeAtPosition(SyntaxNode node, int offset)
    {
        // Simple recursive search for node containing offset
        if (node is not CompilationUnitSyntax cu)
            return null;

        foreach (var decl in cu.Declarations)
        {
            var found = SearchDeclaration(decl, offset);
            if (found != null)
                return found;
        }

        return null;
    }

    private static SyntaxNode? SearchDeclaration(DeclarationSyntax decl, int offset)
    {
        if (decl.Span.Start <= offset && offset < decl.Span.Start + decl.Span.Length)
        {
            if (decl is FunctionDeclarationSyntax func)
            {
                return SearchStatement(func.Body, offset) ?? decl;
            }
        }
        return null;
    }

    private static SyntaxNode? SearchStatement(BlockStatementSyntax block, int offset)
    {
        foreach (var stmt in block.Statements)
        {
            if (stmt.Span.Start <= offset && offset < stmt.Span.Start + stmt.Span.Length)
            {
                return stmt;
            }
        }
        return null;
    }

    private static Hover? GetHoverInfo(SyntaxNode node, DocumentState doc, int offset)
    {
        // TODO: Implement proper hover info extraction
        // For now, return generic hover
        var content = $"Position: {offset}";
        return new Hover
        {
            Contents = new MarkedStringsOrMarkupContent(new MarkupContent
            {
                Kind = MarkupKind.Markdown,
                Value = content
            })
        };
    }
}
