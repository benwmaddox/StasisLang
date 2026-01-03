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
        var danglingOffset = FindDanglingMemberAccessOffset(document.Content);

        foreach (var diag in document.AllDiagnostics)
        {
            if (danglingOffset.HasValue && diag.Span.Start >= danglingOffset.Value)
            {
                continue;
            }

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

    private static int? FindDanglingMemberAccessOffset(string content)
    {
        for (var i = content.Length - 1; i >= 0; i--)
        {
            if (content[i] != '.')
            {
                continue;
            }

            var left = i - 1;
            while (left >= 0 && char.IsWhiteSpace(content[left]))
            {
                left--;
            }

            if (left < 0 || !IsIdentifierChar(content[left]))
            {
                continue;
            }

            var right = i + 1;
            while (right < content.Length && char.IsWhiteSpace(content[right]))
            {
                if (content[right] == '\n' || content[right] == '\r')
                {
                    return i;
                }
                right++;
            }

            if (right >= content.Length)
            {
                return i;
            }

            if (!IsIdentifierChar(content[right]))
            {
                continue;
            }
        }

        return null;
    }

    private static bool IsIdentifierChar(char c) => char.IsLetterOrDigit(c) || c == '_';
}
