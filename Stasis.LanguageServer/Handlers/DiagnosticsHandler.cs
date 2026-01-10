namespace Stasis.LanguageServer.Handlers;

using MediatR;
using OmniSharp.Extensions.LanguageServer.Protocol.Client.Capabilities;
using OmniSharp.Extensions.LanguageServer.Protocol.Document;
using OmniSharp.Extensions.LanguageServer.Protocol.Models;
using Stasis.LanguageServer.Services;

/// <summary>
/// Handles document open event. Creates a new document in the manager and publishes initial diagnostics.
/// </summary>
public class DidOpenTextDocumentDiagnosticsHandler : DidOpenTextDocumentHandlerBase
{
    private readonly DocumentManager _documentManager;
    private readonly DiagnosticsPublisher _diagnosticsPublisher;

    public DidOpenTextDocumentDiagnosticsHandler(DocumentManager documentManager, DiagnosticsPublisher diagnosticsPublisher)
    {
        _documentManager = documentManager;
        _diagnosticsPublisher = diagnosticsPublisher;
    }

    public override Task<Unit> Handle(DidOpenTextDocumentParams request, CancellationToken cancellationToken)
    {
        var uri = request.TextDocument.Uri.ToString();
        var content = request.TextDocument.Text;

        // Create or update document
        var doc = _documentManager.GetOrCreateDocument(uri, content);

        // Parse document and capture diagnostics
        _documentManager.UpdateDocument(uri, content, 1);

        // Publish diagnostics
        var updatedDoc = _documentManager.GetDocument(uri);
        if (updatedDoc != null)
        {
            _diagnosticsPublisher.PublishDiagnostics(uri, updatedDoc);
        }

        return Unit.Task;
    }

    protected override TextDocumentOpenRegistrationOptions CreateRegistrationOptions(TextSynchronizationCapability capability, ClientCapabilities clientCapabilities)
    {
        return new TextDocumentOpenRegistrationOptions();
    }
}

/// <summary>
/// Handles document change event. Updates the document and publishes updated diagnostics.
/// </summary>
public class DidChangeTextDocumentDiagnosticsHandler : DidChangeTextDocumentHandlerBase
{
    private readonly DocumentManager _documentManager;
    private readonly DiagnosticsPublisher _diagnosticsPublisher;

    public DidChangeTextDocumentDiagnosticsHandler(DocumentManager documentManager, DiagnosticsPublisher diagnosticsPublisher)
    {
        _documentManager = documentManager;
        _diagnosticsPublisher = diagnosticsPublisher;
    }

    public override Task<Unit> Handle(DidChangeTextDocumentParams request, CancellationToken cancellationToken)
    {
        var uri = request.TextDocument.Uri.ToString();
        var doc = _documentManager.GetDocument(uri);

        if (doc == null)
            return Unit.Task;

        // Apply changes to document content
        var updatedContent = ApplyChanges(doc.Content, request.ContentChanges);
        var version = request.TextDocument.Version ?? doc.Version + 1;

        // Update document (full reparse)
        _documentManager.UpdateDocument(uri, updatedContent, version);

        // Publish updated diagnostics
        var updatedDoc = _documentManager.GetDocument(uri);
        if (updatedDoc != null)
        {
            _diagnosticsPublisher.PublishDiagnostics(uri, updatedDoc);
        }

        return Unit.Task;
    }

    protected override TextDocumentChangeRegistrationOptions CreateRegistrationOptions(TextSynchronizationCapability capability, ClientCapabilities clientCapabilities)
    {
        return new TextDocumentChangeRegistrationOptions();
    }

    /// <summary>
    /// Applies text document changes to the current content.
    /// LSP Positions are UTF-16 code units; apply changes against the .NET string (also UTF-16).
    /// </summary>
    private static string ApplyChanges(string currentContent, IEnumerable<TextDocumentContentChangeEvent> changes)
    {
        var content = currentContent;

        foreach (var change in changes)
        {
            // If Range is null, replace entire document
            if (change.Range == null)
            {
                content = change.Text;
            }
            else
            {
                // Apply range-based change (convert to offsets and apply)
                var range = change.Range;
                var start = TextPositionConverter.PositionToOffset(content, range.Start);
                var end = TextPositionConverter.PositionToOffset(content, range.End);
                content = content.Substring(0, start) + change.Text + content.Substring(end);
            }
        }

        return content;
    }
}

/// <summary>
/// Handles document close event. Removes the document from the manager and clears diagnostics.
/// </summary>
public class DidCloseTextDocumentDiagnosticsHandler : DidCloseTextDocumentHandlerBase
{
    private readonly DocumentManager _documentManager;
    private readonly DiagnosticsPublisher _diagnosticsPublisher;

    public DidCloseTextDocumentDiagnosticsHandler(DocumentManager documentManager, DiagnosticsPublisher diagnosticsPublisher)
    {
        _documentManager = documentManager;
        _diagnosticsPublisher = diagnosticsPublisher;
    }

    public override Task<Unit> Handle(DidCloseTextDocumentParams request, CancellationToken cancellationToken)
    {
        var uri = request.TextDocument.Uri.ToString();

        // Clear diagnostics
        _diagnosticsPublisher.ClearDiagnostics(uri);

        // Remove document from manager
        _documentManager.CloseDocument(uri);

        return Unit.Task;
    }

    protected override TextDocumentCloseRegistrationOptions CreateRegistrationOptions(TextSynchronizationCapability capability, ClientCapabilities clientCapabilities)
    {
        return new TextDocumentCloseRegistrationOptions();
    }
}
