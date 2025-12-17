namespace Stasis.LanguageServer.Tests;

using OmniSharp.Extensions.LanguageServer.Protocol.Models;
using Stasis.LanguageServer.Handlers;
using Stasis.LanguageServer.Services;
using Xunit;

public class CompletionHandlerTests
{
    private static Position PositionAt(string content, int offset)
    {
        int line = 0;
        int character = 0;
        for (int i = 0; i < offset && i < content.Length; i++)
        {
            if (content[i] == '\n')
            {
                line++;
                character = 0;
            }
            else
            {
                character++;
            }
        }

        return new Position(line, character);
    }

    [Fact]
    public async Task HandleAsync_ReturnsEmptyListForNonExistentDocument()
    {
        // Arrange
        var manager = new DocumentManager();
        var handler = new CompletionHandler(manager);
        var request = new CompletionParams
        {
            TextDocument = new TextDocumentIdentifier
            {
                Uri = new Uri("file:///nonexistent.stasis")
            },
            Position = new Position(0, 0)
        };

        // Act
        var result = await handler.Handle(request, CancellationToken.None);

        // Assert
        Assert.NotNull(result);
        Assert.Empty(result.Items ?? new Container<CompletionItem>());
    }

    [Fact]
    public async Task HandleAsync_ReturnsEmptyListWhenDocumentNotParsed()
    {
        // Arrange
        var manager = new DocumentManager();
        var handler = new CompletionHandler(manager);
        var uri = "file:///test.stasis";

        manager.GetOrCreateDocument(uri, "");

        var request = new CompletionParams
        {
            TextDocument = new TextDocumentIdentifier
            {
                Uri = new Uri(uri)
            },
            Position = new Position(0, 0)
        };

        // Act
        var result = await handler.Handle(request, CancellationToken.None);

        // Assert
        Assert.NotNull(result);
        Assert.Empty(result.Items ?? new Container<CompletionItem>());
    }

    [Fact]
    public async Task HandleAsync_ReturnsCompletionListWhenDocumentIsValid()
    {
        // Arrange
        var manager = new DocumentManager();
        var handler = new CompletionHandler(manager);
        var uri = "file:///test.stasis";
        var content = "enum State { Idle, Jump }";

        manager.GetOrCreateDocument(uri, content);
        manager.UpdateDocument(uri, content, 1);

        var request = new CompletionParams
        {
            TextDocument = new TextDocumentIdentifier
            {
                Uri = new Uri(uri)
            },
            Position = new Position(0, 0)
        };

        // Act
        var result = await handler.Handle(request, CancellationToken.None);

        // Assert
        Assert.NotNull(result);
        Assert.Empty(result.Items ?? new Container<CompletionItem>());
    }

    [Fact]
    public async Task HandleAsync_OffersEnumMembersAfterDot()
    {
        // Arrange
        var manager = new DocumentManager();
        var handler = new CompletionHandler(manager);
        var uri = "file:///test.stasis";
        var content = """
enum State { Idle, Jump }
function main() {
  let x: State = State.Idle;
}
""";

        manager.GetOrCreateDocument(uri, content);
        manager.UpdateDocument(uri, content, 1);

        var dotOffset = content.IndexOf("State.Idle", StringComparison.Ordinal) + "State.".Length;

        var request = new CompletionParams
        {
            TextDocument = new TextDocumentIdentifier { Uri = new Uri(uri) },
            Position = PositionAt(content, dotOffset)
        };

        // Act
        var result = await handler.Handle(request, CancellationToken.None);

        // Assert
        var labels = (result.Items ?? new Container<CompletionItem>()).Select(i => i.Label).ToHashSet(StringComparer.Ordinal);
        Assert.Contains("Idle", labels);
        Assert.Contains("Jump", labels);
    }

    [Fact]
    public async Task HandleAsync_OffersStructFieldsAfterDot()
    {
        // Arrange
        var manager = new DocumentManager();
        var handler = new CompletionHandler(manager);
        var uri = "file:///test.stasis";
        var content = """
struct Player {
  hp: i32;
  name: string;
}

function main() {
  let p: Player;
  p.hp;
}
""";

        manager.GetOrCreateDocument(uri, content);
        manager.UpdateDocument(uri, content, 1);

        var dotOffset = content.IndexOf("p.hp", StringComparison.Ordinal) + "p.".Length;

        var request = new CompletionParams
        {
            TextDocument = new TextDocumentIdentifier { Uri = new Uri(uri) },
            Position = PositionAt(content, dotOffset)
        };

        // Act
        var result = await handler.Handle(request, CancellationToken.None);

        // Assert
        var labels = (result.Items ?? new Container<CompletionItem>()).Select(i => i.Label).ToHashSet(StringComparer.Ordinal);
        Assert.Contains("hp", labels);
        Assert.Contains("name", labels);
    }

    [Fact]
    public async Task HandleCompletionItemAsync_ReturnsItemUnmodified()
    {
        // Arrange
        var manager = new DocumentManager();
        var handler = new CompletionHandler(manager);
        var item = new CompletionItem
        {
            Label = "test",
            Kind = CompletionItemKind.EnumMember,
            Detail = "Test enum member"
        };

        // Act
        var result = await handler.Handle(item, CancellationToken.None);

        // Assert
        Assert.NotNull(result);
        Assert.Equal(item.Label, result.Label);
        Assert.Equal(item.Detail, result.Detail);
    }

    [Fact]
    public void CreateRegistrationOptions_ReturnsValidOptions()
    {
        // Arrange
        var manager = new DocumentManager();
        var handler = new CompletionHandler(manager);

        // Act
        var options = handler.GetType()
            .GetMethod("CreateRegistrationOptions",
                System.Reflection.BindingFlags.NonPublic | System.Reflection.BindingFlags.Instance)
            ?.Invoke(handler, new object?[] { null, null });

        // Assert
        Assert.NotNull(options);
    }
}
