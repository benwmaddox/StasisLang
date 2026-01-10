namespace Stasis.LanguageServer.Tests;

using OmniSharp.Extensions.LanguageServer.Protocol.Models;
using Stasis.LanguageServer.Handlers;
using Stasis.LanguageServer.Services;
using Stasis.Compiler;
using CompilerDiagnostic = Stasis.Compiler.Diagnostic;
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
    public async Task HandleAsync_OffersStructFieldsAfterDot_WhenMemberNameIsMissing()
    {
        var manager = new DocumentManager();
        var handler = new CompletionHandler(manager);
        var uri = "file:///test.stasis";
        var content = """
struct GameState {
  score: i32;
  phase: i32;
}

global state: GameState;

function main() {
  state.
}
""";

        manager.GetOrCreateDocument(uri, content);
        manager.UpdateDocument(uri, content, 1);

        var dotOffset = content.IndexOf("state.", StringComparison.Ordinal) + "state.".Length;

        var request = new CompletionParams
        {
            TextDocument = new TextDocumentIdentifier { Uri = new Uri(uri) },
            Position = PositionAt(content, dotOffset)
        };

        var result = await handler.Handle(request, CancellationToken.None);

        var labels = (result.Items ?? new Container<CompletionItem>()).Select(i => i.Label).ToHashSet(StringComparer.Ordinal);
        Assert.Contains("score", labels);
        Assert.Contains("phase", labels);
    }

    [Fact]
    public async Task HandleAsync_OffersStructFieldsAfterDot_WhenSemanticUnavailableAndParseHasErrors()
    {
        var manager = new DocumentManager();
        var handler = new CompletionHandler(manager);
        var uri = "file:///test.stasis";
        var content = """
struct GameState {
  score: i32;
  phase: i32;
}

global state: GameState;

function main() {
  state.
  let broken: = 1;
}
""";

        manager.GetOrCreateDocument(uri, content);
        manager.UpdateDocument(uri, content, 1);

        var doc = manager.GetDocument(uri);
        Assert.NotNull(doc);
        doc!.SemanticResult = null;

        var dotOffset = content.IndexOf("state.", StringComparison.Ordinal) + "state.".Length;

        var request = new CompletionParams
        {
            TextDocument = new TextDocumentIdentifier { Uri = new Uri(uri) },
            Position = PositionAt(content, dotOffset)
        };

        var result = await handler.Handle(request, CancellationToken.None);

        var labels = (result.Items ?? new Container<CompletionItem>()).Select(i => i.Label).ToHashSet(StringComparer.Ordinal);
        Assert.Contains("score", labels);
        Assert.Contains("phase", labels);
    }

    [Fact]
    public async Task HandleAsync_OffersStructFieldsAfterNestedDot_WhenMemberNameIsMissing()
    {
        var manager = new DocumentManager();
        var handler = new CompletionHandler(manager);
        var uri = "file:///test.stasis";
        var content = """
struct Layout {
  play_w: f32;
  play_h: f32;
}

struct GameState {
  layout: Layout;
}

global state: GameState;

function main() {
  state.layout.
}
""";

        manager.GetOrCreateDocument(uri, content);
        manager.UpdateDocument(uri, content, 1);

        var dotOffset = content.IndexOf("state.layout.", StringComparison.Ordinal) + "state.layout.".Length;

        var request = new CompletionParams
        {
            TextDocument = new TextDocumentIdentifier { Uri = new Uri(uri) },
            Position = PositionAt(content, dotOffset)
        };

        var result = await handler.Handle(request, CancellationToken.None);

        var labels = (result.Items ?? new Container<CompletionItem>()).Select(i => i.Label).ToHashSet(StringComparer.Ordinal);
        Assert.Contains("play_w", labels);
        Assert.Contains("play_h", labels);
    }

    [Fact]
    public async Task HandleAsync_OffersEnumMembersAfterDot_WhenMemberNameIsMissing()
    {
        var manager = new DocumentManager();
        var handler = new CompletionHandler(manager);
        var uri = "file:///test.stasis";
        var content = """
enum Phase { Boot, Play }

function main() {
  Phase.
}
""";

        manager.GetOrCreateDocument(uri, content);
        manager.UpdateDocument(uri, content, 1);

        var dotOffset = content.IndexOf("Phase.", StringComparison.Ordinal) + "Phase.".Length;

        var request = new CompletionParams
        {
            TextDocument = new TextDocumentIdentifier { Uri = new Uri(uri) },
            Position = PositionAt(content, dotOffset)
        };

        var result = await handler.Handle(request, CancellationToken.None);

        var labels = (result.Items ?? new Container<CompletionItem>()).Select(i => i.Label).ToHashSet(StringComparer.Ordinal);
        Assert.Contains("Boot", labels);
        Assert.Contains("Play", labels);
    }

    [Fact]
    public async Task HandleAsync_OffersImportedStructFieldsAfterDot_WhenMemberNameIsMissing()
    {
        var manager = new DocumentManager();
        var handler = new CompletionHandler(manager);

        var tempDir = Directory.CreateTempSubdirectory("stasis_completion_dot_missing_import_");
        try
        {
            var importedPath = Path.Combine(tempDir.FullName, "types.stasis");
            File.WriteAllText(importedPath, """
struct Player {
  hp: i32;
  name: string;
}
""");

            var entryPath = Path.Combine(tempDir.FullName, "main.stasis");
            var content = """
import "types.stasis";

global p: Player;

function main() {
  p.
}
""";

            var docId = new TextDocumentIdentifier { Uri = new Uri(entryPath) };
            var uri = docId.Uri.ToString();

            var importDiags = new List<CompilerDiagnostic>();
            var expanded = SourceImporter.ExpandImportsWithMap(entryPath, content, importDiags);
            Assert.Empty(importDiags);
            Assert.Contains("struct Player", expanded.ExpandedSource, StringComparison.Ordinal);

            manager.GetOrCreateDocument(uri, content);
            manager.UpdateDocument(uri, content, 1);

            var doc = manager.GetDocument(uri);
            Assert.NotNull(doc);
            Assert.NotNull(doc!.SymbolIndex);
            Assert.NotNull(doc.SymbolIndex!.GetStruct("Player"));

            var dotOffset = content.IndexOf("p.", StringComparison.Ordinal) + "p.".Length;

            var request = new CompletionParams
            {
                TextDocument = docId,
                Position = PositionAt(content, dotOffset)
            };

            var result = await handler.Handle(request, CancellationToken.None);

            var labels = (result.Items ?? new Container<CompletionItem>()).Select(i => i.Label).ToHashSet(StringComparer.Ordinal);
            Assert.Contains("hp", labels);
            Assert.Contains("name", labels);
        }
        finally
        {
            tempDir.Delete(recursive: true);
        }
    }

    [Fact]
    public async Task HandleAsync_PrefersLocalOverImportedGlobal_WhenSemanticUnavailable()
    {
        var manager = new DocumentManager();
        var handler = new CompletionHandler(manager);

        var tempDir = Directory.CreateTempSubdirectory("stasis_completion_shadow_local_import_global_");
        try
        {
            var importedPath = Path.Combine(tempDir.FullName, "globals.stasis");
            File.WriteAllText(importedPath, """
struct ImportedState {
  imported_only: i32;
}

global state: ImportedState;
""");

            var entryPath = Path.Combine(tempDir.FullName, "main.stasis");
            var content = """
import "globals.stasis";

struct LocalState {
  local_only: i32;
}

function main() {
  let state: LocalState;
  state.
}
""";

            var docId = new TextDocumentIdentifier { Uri = new Uri(entryPath) };
            var uri = docId.Uri.ToString();

            manager.GetOrCreateDocument(uri, content);
            manager.UpdateDocument(uri, content, 1);

            // Force fallback path (semantic analysis unavailable) but keep parse/index data.
            var doc = manager.GetDocument(uri);
            Assert.NotNull(doc);
            doc!.SemanticResult = null;

            var dotOffset = content.IndexOf("state.", StringComparison.Ordinal) + "state.".Length;

            var request = new CompletionParams
            {
                TextDocument = docId,
                Position = PositionAt(content, dotOffset)
            };

            var result = await handler.Handle(request, CancellationToken.None);
            var labels = (result.Items ?? new Container<CompletionItem>()).Select(i => i.Label).ToHashSet(StringComparer.Ordinal);
            Assert.Contains("local_only", labels);
            Assert.DoesNotContain("imported_only", labels);
        }
        finally
        {
            tempDir.Delete(recursive: true);
        }
    }

    [Fact]
    public async Task HandleAsync_PrefersCurrentFileGlobalOverImportedGlobal_WhenSemanticUnavailable()
    {
        var manager = new DocumentManager();
        var handler = new CompletionHandler(manager);

        var tempDir = Directory.CreateTempSubdirectory("stasis_completion_shadow_global_import_global_");
        try
        {
            var importedPath = Path.Combine(tempDir.FullName, "globals.stasis");
            File.WriteAllText(importedPath, """
struct ImportedState {
  imported_only: i32;
}

global state: ImportedState;
""");

            var entryPath = Path.Combine(tempDir.FullName, "main.stasis");
            var content = """
import "globals.stasis";

struct LocalState {
  local_only: i32;
}

global state: LocalState;

function main() {
  state.
}
""";

            var docId = new TextDocumentIdentifier { Uri = new Uri(entryPath) };
            var uri = docId.Uri.ToString();

            manager.GetOrCreateDocument(uri, content);
            manager.UpdateDocument(uri, content, 1);

            var doc = manager.GetDocument(uri);
            Assert.NotNull(doc);
            doc!.SemanticResult = null;

            var dotOffset = content.IndexOf("state.", StringComparison.Ordinal) + "state.".Length;

            var request = new CompletionParams
            {
                TextDocument = docId,
                Position = PositionAt(content, dotOffset)
            };

            var result = await handler.Handle(request, CancellationToken.None);
            var labels = (result.Items ?? new Container<CompletionItem>()).Select(i => i.Label).ToHashSet(StringComparer.Ordinal);
            Assert.Contains("local_only", labels);
            Assert.DoesNotContain("imported_only", labels);
        }
        finally
        {
            tempDir.Delete(recursive: true);
        }
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
