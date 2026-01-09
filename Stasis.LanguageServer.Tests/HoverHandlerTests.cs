namespace Stasis.LanguageServer.Tests;

using OmniSharp.Extensions.LanguageServer.Protocol.Models;
using Stasis.LanguageServer.Handlers;
using Stasis.LanguageServer.Services;
using Stasis.LanguageServer.Models;
using Xunit;

public class HoverHandlerTests
{
    [Fact]
    public async Task HandleAsync_ReturnsNullForNonExistentDocument()
    {
        // Arrange
        var manager = new DocumentManager();
        var handler = new HoverHandler(manager);
        var request = new HoverParams
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
        Assert.Null(result);
    }

    [Fact]
    public async Task HandleAsync_ReturnsNullWhenDocumentNotParsed()
    {
        // Arrange
        var manager = new DocumentManager();
        var handler = new HoverHandler(manager);
        var uri = "file:///test.stasis";

        manager.GetOrCreateDocument(uri, "");

        var request = new HoverParams
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
        Assert.Null(result);
    }

    [Fact]
    public async Task HandleAsync_ReturnsHoverForLocalSymbol()
    {
        // Arrange
        var manager = new DocumentManager();
        var handler = new HoverHandler(manager);
        var uri = "file:///test.stasis";
        var content = """
                      function main(): void {
                          let x: i32;
                          x = 5;
                      }
                      """;

        manager.GetOrCreateDocument(uri, content);
        manager.UpdateDocument(uri, content, 1);

        var xOffset = content.IndexOf("x = 5", StringComparison.Ordinal);
        Assert.True(xOffset >= 0, "Test content should contain x assignment.");
        var xPos = TextPositionConverter.OffsetToPosition(content, xOffset);

        var request = new HoverParams
        {
            TextDocument = new TextDocumentIdentifier
            {
                Uri = new Uri(uri)
            },
            Position = xPos
        };

        // Act
        var result = await handler.Handle(request, CancellationToken.None);

        // Assert
        Assert.NotNull(result);
        var markdown = GetHoverMarkdown(result!);
        Assert.NotNull(markdown);
        Assert.Contains("x", markdown!, StringComparison.Ordinal);
        Assert.Contains("i32", markdown!, StringComparison.Ordinal);
    }

    [Fact]
    public async Task HandleAsync_ReturnsHoverForNestedStructField()
    {
        var manager = new DocumentManager();
        var handler = new HoverHandler(manager);
        var uri = "file:///test.stasis";
        var content = """
                      struct Inner {
                          a: i32;
                      }

                      struct Outer {
                          inner: Inner;
                      }

                      function main(): void {
                          let s: Outer;
                          s.inner.a = 1;
                      }
                      """;

        manager.GetOrCreateDocument(uri, content);
        manager.UpdateDocument(uri, content, 1);

        var aOffset = content.IndexOf("a = 1", StringComparison.Ordinal);
        Assert.True(aOffset >= 0, "Test content should contain a assignment.");
        var aPos = TextPositionConverter.OffsetToPosition(content, aOffset);

        var request = new HoverParams
        {
            TextDocument = new TextDocumentIdentifier { Uri = new Uri(uri) },
            Position = aPos
        };

        var result = await handler.Handle(request, CancellationToken.None);

        Assert.NotNull(result);
        var markdown = GetHoverMarkdown(result!);
        Assert.NotNull(markdown);
        Assert.Contains("(field)", markdown!, StringComparison.Ordinal);
        Assert.Contains("a: i32", markdown!, StringComparison.Ordinal);
    }

    [Fact]
    public async Task HandleAsync_ReturnsHoverForImportedStructField()
    {
        var manager = new DocumentManager();
        var handler = new HoverHandler(manager);
        var tempRoot = Path.Combine(Path.GetTempPath(), $"stasis-hover-{Guid.NewGuid():N}");
        Directory.CreateDirectory(tempRoot);
        try
        {
            var importedPath = Path.Combine(tempRoot, "defs.stasis");
            var entryPath = Path.Combine(tempRoot, "entry.stasis");

            File.WriteAllText(importedPath, """
                                            struct Inner {
                                                a: i32;
                                            }

                                            struct Outer {
                                                inner: Inner;
                                            }
                                            """);

            File.WriteAllText(entryPath, """
                                         import "./defs.stasis";

                                         function main(): void {
                                             let s: Outer;
                                             s.inner.a = 1;
                                         }
                                         """);

            var docId = new TextDocumentIdentifier { Uri = new Uri(entryPath) };
            var uri = docId.Uri.ToString();
            var content = File.ReadAllText(entryPath);

            manager.GetOrCreateDocument(uri, content);
            manager.UpdateDocument(uri, content, 1);

            var aOffset = content.IndexOf("a = 1", StringComparison.Ordinal);
            Assert.True(aOffset >= 0, "Test content should contain a assignment.");
            var aPos = TextPositionConverter.OffsetToPosition(content, aOffset);

            var request = new HoverParams
            {
                TextDocument = docId,
                Position = aPos
            };

            var result = await handler.Handle(request, CancellationToken.None);
            Assert.NotNull(result);
            var markdown = GetHoverMarkdown(result!);
            Assert.NotNull(markdown);
            Assert.Contains("(field)", markdown!, StringComparison.Ordinal);
            Assert.Contains("a: i32", markdown!, StringComparison.Ordinal);
        }
        finally
        {
            try
            {
                Directory.Delete(tempRoot, recursive: true);
            }
            catch
            {
                // best effort
            }
        }
    }

    [Fact]
    public void CreateRegistrationOptions_ReturnsValidOptions()
    {
        // Arrange
        var manager = new DocumentManager();
        var handler = new HoverHandler(manager);

        // Act
        var options = handler.GetType()
            .GetMethod("CreateRegistrationOptions",
                System.Reflection.BindingFlags.NonPublic | System.Reflection.BindingFlags.Instance)
            ?.Invoke(handler, new object?[] { null, null });

        // Assert
        Assert.NotNull(options);
    }

    private static string? GetHoverMarkdown(Hover hover)
    {
        // OmniSharp exposes hover contents as a discriminated union. Use reflection so tests stay stable
        // across minor protocol type changes.
        var contents = hover.Contents;
        var contentsType = contents.GetType();

        var valueProp = contentsType.GetProperty("Value");
        if (valueProp?.GetValue(contents) is MarkupContent markup)
        {
            return markup.Value;
        }

        return contents.ToString();
    }
}
