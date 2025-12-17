namespace Stasis.LanguageServer.Services;

using OmniSharp.Extensions.LanguageServer.Protocol.Models;
using OmniSharp.Extensions.LanguageServer.Protocol.Server;
using OmniSharp.Extensions.LanguageServer.Protocol.Workspace;
using Stasis.LanguageServer.Models;
using CompilerDiagnostic = Stasis.Compiler.Diagnostic;
using CompilerSourceSpan = Stasis.Compiler.SourceSpan;

/// <summary>
/// Service responsible for converting Stasis diagnostics to LSP format and publishing them to the client.
/// </summary>
public class DiagnosticsPublisher
{
    private ILanguageServerFacade? _languageServer;

    public DiagnosticsPublisher(ILanguageServerFacade? languageServer = null)
    {
        _languageServer = languageServer;
    }

    /// <summary>
    /// Sets the language server facade after it's initialized.
    /// </summary>
    public void SetLanguageServer(ILanguageServerFacade languageServer)
    {
        _languageServer = languageServer;
    }

    /// <summary>
    /// Publishes diagnostics for a document to the client.
    /// </summary>
    public void PublishDiagnostics(string uri, DocumentState document)
    {
        if (_languageServer == null)
            return;

        var diagnostics = new List<Diagnostic>();

        foreach (var diag in document.AllDiagnostics)
        {
            var range = SourceSpanToRange(document.Content, diag.Span);
            diagnostics.Add(new Diagnostic
            {
                Range = range,
                Message = diag.Message,
                Severity = DiagnosticSeverity.Error,
                Source = "Stasis"
            });
        }

        _languageServer.SendNotification(new PublishDiagnosticsParams
        {
            Uri = new Uri(uri),
            Diagnostics = new Container<Diagnostic>(diagnostics)
        });
    }

    /// <summary>
    /// Clears all diagnostics for a document.
    /// </summary>
    public void ClearDiagnostics(string uri)
    {
        if (_languageServer == null)
            return;

        _languageServer.SendNotification(new PublishDiagnosticsParams
        {
            Uri = new Uri(uri),
            Diagnostics = new Container<Diagnostic>()
        });
    }

    /// <summary>
    /// Converts a Stasis SourceSpan to an LSP Range using line/column coordinates.
    /// </summary>
    private static Range SourceSpanToRange(string content, CompilerSourceSpan span)
    {
        var start = TextPositionConverter.OffsetToPosition(content, span.Start);
        var end = TextPositionConverter.OffsetToPosition(content, span.Start + span.Length);

        return new Range
        {
            Start = start,
            End = end
        };
    }
}
