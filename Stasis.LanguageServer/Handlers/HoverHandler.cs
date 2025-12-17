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

        if (doc?.SemanticResult == null || doc.ParseResult == null)
            return Task.FromResult<Hover?>(null);

        var offset = TextPositionConverter.PositionToOffset(doc.Content, request.Position);
        var node = FindNodeAtPosition(doc.ParseResult.CompilationUnit, offset);

        if (node == null)
            return Task.FromResult<Hover?>(null);

        var hover = GetHoverInfo(node, doc.SemanticResult, offset);
        return Task.FromResult(hover);
    }

    protected override HoverRegistrationOptions CreateRegistrationOptions(HoverCapability? capability, ClientCapabilities clientCapabilities)
    {
        return new HoverRegistrationOptions();
    }

    /// <summary>
    /// Recursively finds the most specific syntax node at the given offset.
    /// </summary>
    private static SyntaxNode? FindNodeAtPosition(SyntaxNode node, int offset)
    {
        // Check if offset is within this node's span
        if (node.Span.Start > offset || offset >= node.Span.Start + node.Span.Length)
            return null;

        return node switch
        {
            CompilationUnitSyntax cu => FindInCompilationUnit(cu, offset),
            DeclarationSyntax decl => FindInDeclaration(decl, offset),
            StatementSyntax stmt => FindInStatement(stmt, offset),
            ExpressionSyntax expr => FindInExpression(expr, offset),
            _ => node
        };
    }

    private static SyntaxNode? FindInCompilationUnit(CompilationUnitSyntax cu, int offset)
    {
        foreach (var decl in cu.Declarations)
        {
            var found = FindNodeAtPosition(decl, offset);
            if (found != null)
                return found;
        }
        return null;
    }

    private static SyntaxNode? FindInDeclaration(DeclarationSyntax decl, int offset)
    {
        return decl switch
        {
            FunctionDeclarationSyntax func => FindNodeAtPosition(func.Body, offset) ?? decl,
            _ => decl
        };
    }

    private static SyntaxNode? FindInStatement(StatementSyntax stmt, int offset)
    {
        return stmt switch
        {
            BlockStatementSyntax block => FindInBlock(block, offset) ?? stmt,
            VariableDeclarationSyntax varDecl when varDecl.Initializer != null =>
                FindNodeAtPosition(varDecl.Initializer, offset) ?? stmt,
            ExpressionStatementSyntax exprStmt =>
                FindNodeAtPosition(exprStmt.Expression, offset) ?? stmt,
            IfStatementSyntax ifStmt =>
                FindNodeAtPosition(ifStmt.Condition, offset) ??
                FindNodeAtPosition(ifStmt.ThenBlock, offset) ??
                (ifStmt.ElseBlock != null ? FindNodeAtPosition(ifStmt.ElseBlock, offset) : null) ??
                stmt,
            ForStatementSyntax forStmt =>
                (forStmt.Condition != null ? FindNodeAtPosition(forStmt.Condition, offset) : null) ??
                FindNodeAtPosition(forStmt.Body, offset) ??
                stmt,
            ForeachStatementSyntax foreachStmt =>
                FindNodeAtPosition(foreachStmt.Iterable, offset) ??
                FindNodeAtPosition(foreachStmt.Body, offset) ??
                stmt,
            ReturnStatementSyntax retStmt when retStmt.Expression != null =>
                FindNodeAtPosition(retStmt.Expression, offset) ?? stmt,
            _ => stmt
        };
    }

    private static SyntaxNode? FindInBlock(BlockStatementSyntax block, int offset)
    {
        foreach (var statement in block.Statements)
        {
            var found = FindNodeAtPosition(statement, offset);
            if (found != null)
                return found;
        }
        return null;
    }

    private static SyntaxNode? FindInExpression(ExpressionSyntax expr, int offset)
    {
        return expr switch
        {
            IdentifierExpressionSyntax => expr, // Found an identifier - this is what we want!
            BinaryExpressionSyntax bin =>
                FindNodeAtPosition(bin.Left, offset) ??
                FindNodeAtPosition(bin.Right, offset) ??
                expr,
            UnaryExpressionSyntax unary =>
                FindNodeAtPosition(unary.Operand, offset) ?? expr,
            CallExpressionSyntax call =>
                FindNodeAtPosition(call.Callee, offset) ??
                FindInArguments(call.Arguments, offset) ??
                expr,
            MemberAccessExpressionSyntax member =>
                FindNodeAtPosition(member.Receiver, offset) ?? expr,
            ArrayAccessExpressionSyntax arrayAccess =>
                FindNodeAtPosition(arrayAccess.Receiver, offset) ??
                FindNodeAtPosition(arrayAccess.Index, offset) ??
                expr,
            ParenthesizedExpressionSyntax paren =>
                FindNodeAtPosition(paren.Expression, offset) ?? expr,
            AssignmentExpressionSyntax assign =>
                FindNodeAtPosition(assign.Left, offset) ??
                FindNodeAtPosition(assign.Right, offset) ??
                expr,
            _ => expr
        };
    }

    private static SyntaxNode? FindInArguments(IReadOnlyList<ExpressionSyntax> arguments, int offset)
    {
        foreach (var arg in arguments)
        {
            var found = FindNodeAtPosition(arg, offset);
            if (found != null)
                return found;
        }
        return null;
    }

    /// <summary>
    /// Extracts hover information from a syntax node using semantic analysis.
    /// </summary>
    private static Hover? GetHoverInfo(SyntaxNode node, SemanticResult semanticResult, int offset)
    {
        // Extract identifier name from the node
        var identifierName = node switch
        {
            IdentifierExpressionSyntax ident => ident.Identifier.Text,
            FunctionDeclarationSyntax func => func.Name.Text,
            _ => null
        };

        if (string.IsNullOrEmpty(identifierName))
            return null;

        // Look up symbol in semantic result
        if (!semanticResult.Symbols.TryGetValue(identifierName, out var symbol))
            return null;

        // Format hover content as markdown
        var markdown = FormatSymbolInfo(symbol);

        return new Hover
        {
            Contents = new MarkedStringsOrMarkupContent(new MarkupContent
            {
                Kind = MarkupKind.Markdown,
                Value = markdown
            })
        };
    }

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
}
