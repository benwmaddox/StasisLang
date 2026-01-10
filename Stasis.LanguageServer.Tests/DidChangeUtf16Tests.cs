namespace Stasis.LanguageServer.Tests;

using OmniSharp.Extensions.LanguageServer.Protocol.Models;
using Stasis.LanguageServer.Handlers;
using Stasis.LanguageServer.Services;
using Xunit;

public class DidChangeUtf16Tests
{
    [Fact]
    public void ApplyChanges_UsesUtf16Positions_ForEmojiSurrogatePairs()
    {
        var manager = new DocumentManager();
        var publisher = new DiagnosticsPublisher();

        var open = new DidOpenTextDocumentDiagnosticsHandler(manager, publisher);
        var change = new DidChangeTextDocumentDiagnosticsHandler(manager, publisher);

        var uri = "file:///test.stasis";
        var initial = "a\U0001F642b\n"; // a🙂b (🙂 is a surrogate pair in UTF-16)

        _ = open.Handle(new DidOpenTextDocumentParams
        {
            TextDocument = new TextDocumentItem
            {
                Uri = new Uri(uri),
                LanguageId = "stasis",
                Version = 1,
                Text = initial
            }
        }, CancellationToken.None);

        // Replace 'b' with 'c' using UTF-16 positions:
        // "a"=0, 🙂=1..2, "b"=3
        _ = change.Handle(new DidChangeTextDocumentParams
        {
            TextDocument = new OptionalVersionedTextDocumentIdentifier
            {
                Uri = new Uri(uri),
                Version = 2
            },
            ContentChanges = new Container<TextDocumentContentChangeEvent>(new[]
            {
                new TextDocumentContentChangeEvent
                {
                    Range = new Range(new Position(0, 3), new Position(0, 4)),
                    Text = "c"
                }
            })
        }, CancellationToken.None);

        var doc = manager.GetDocument(uri);
        Assert.NotNull(doc);
        Assert.Equal("a\U0001F642c\n", doc!.Content);
    }

    [Fact]
    public void ApplyChanges_UsesUtf16Positions_ForReplacingEmoji()
    {
        var manager = new DocumentManager();
        var publisher = new DiagnosticsPublisher();

        var open = new DidOpenTextDocumentDiagnosticsHandler(manager, publisher);
        var change = new DidChangeTextDocumentDiagnosticsHandler(manager, publisher);

        var uri = "file:///test.stasis";
        var initial = "a\U0001F642b\n"; // a🙂b

        _ = open.Handle(new DidOpenTextDocumentParams
        {
            TextDocument = new TextDocumentItem
            {
                Uri = new Uri(uri),
                LanguageId = "stasis",
                Version = 1,
                Text = initial
            }
        }, CancellationToken.None);

        // Replace 🙂 (UTF-16 positions 1..3) with "X"
        _ = change.Handle(new DidChangeTextDocumentParams
        {
            TextDocument = new OptionalVersionedTextDocumentIdentifier
            {
                Uri = new Uri(uri),
                Version = 2
            },
            ContentChanges = new Container<TextDocumentContentChangeEvent>(new[]
            {
                new TextDocumentContentChangeEvent
                {
                    Range = new Range(new Position(0, 1), new Position(0, 3)),
                    Text = "X"
                }
            })
        }, CancellationToken.None);

        var doc = manager.GetDocument(uri);
        Assert.NotNull(doc);
        Assert.Equal("aXb\n", doc!.Content);
    }
}

