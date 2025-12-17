namespace Stasis.LanguageServer.Handlers;

using OmniSharp.Extensions.LanguageServer.Protocol.Client.Capabilities;
using OmniSharp.Extensions.LanguageServer.Protocol.Document;
using OmniSharp.Extensions.LanguageServer.Protocol.Models;
using Stasis.LanguageServer.Services;

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

        if (doc?.SemanticResult == null)
            return Task.FromResult(new CompletionList());

        // TODO: Implement completion logic
        var items = new List<CompletionItem>();

        return Task.FromResult(new CompletionList(items));
    }

    public override Task<CompletionItem> Handle(CompletionItem request, CancellationToken cancellationToken)
    {
        // Return the completion item as-is (no additional resolution needed for now)
        return Task.FromResult(request);
    }

    protected override CompletionRegistrationOptions CreateRegistrationOptions(CompletionCapability? capability, ClientCapabilities clientCapabilities)
    {
        return new CompletionRegistrationOptions();
    }
}
