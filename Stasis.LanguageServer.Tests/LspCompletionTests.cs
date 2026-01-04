using System.Collections.Concurrent;
using Nerdbank.Streams;
using System.Text;
using System.Text.Json;
using OmniSharp.Extensions.LanguageServer.Protocol.Models;
using LspServer = OmniSharp.Extensions.LanguageServer.Server.LanguageServer;
using Stasis.LanguageServer.Handlers;
using Stasis.LanguageServer.Services;
using Stasis.Compiler;
using Xunit;
using System.Reflection;
using System.IO;
using LspRange = OmniSharp.Extensions.LanguageServer.Protocol.Models.Range;

namespace Stasis.LanguageServer.Tests;

public sealed class LspCompletionTests
{
    [Fact]
    public async Task CompletesGlobalStructMembersAfterDot()
    {
        using var cts = new CancellationTokenSource(TimeSpan.FromSeconds(10));

        var document = string.Join("\n", new[]
        {
            "struct GameState {",
            "    foo: i32;",
            "    bar: i32;",
            "}",
            "",
            "global state: GameState;",
            "",
            "function main(): i32 {",
            "    state.",
            "    return 0;",
            "}"
        });

        var uri = "file:///test/test.stasis";
        var position = GetPositionAfter(document, "state.");

        await using var harness = await LspTestHarness.StartAsync(cts.Token);
        await harness.InitializeAsync(cts.Token);
        await harness.DidOpenAsync(uri, document, cts.Token);

        var labels = await harness.RequestCompletionLabelsAsync(uri, position.Line, position.Character, cts.Token);
        Assert.Contains("foo", labels);
        Assert.Contains("bar", labels);
    }

    [Fact]
    public async Task CompletesNestedStructMembersAfterDot()
    {
        using var cts = new CancellationTokenSource(TimeSpan.FromSeconds(10));

        var document = string.Join("\n", new[]
        {
            "struct GameState {",
            "    rng_state: i32;",
            "    score: i32;",
            "}",
            "",
            "struct GlobalState {",
            "    game: GameState;",
            "}",
            "",
            "global state: GlobalState;",
            "",
            "function main(): i32 {",
            "    state.game.",
            "    return 0;",
            "}"
        });

        var uri = "file:///test/test_nested.stasis";
        var position = GetPositionAfter(document, "state.game.");

        await using var harness = await LspTestHarness.StartAsync(cts.Token);
        await harness.InitializeAsync(cts.Token);
        await harness.DidOpenAsync(uri, document, cts.Token);

        var labels = await harness.RequestCompletionLabelsAsync(uri, position.Line, position.Character, cts.Token);
        Assert.Contains("rng_state", labels);
        Assert.Contains("score", labels);
    }

    [Fact]
    public async Task CompletesChainedStructMembersAfterDot()
    {
        using var cts = new CancellationTokenSource(TimeSpan.FromSeconds(10));

        var document = string.Join("\n", new[]
        {
            "struct ScreenConfig {",
            "    width: i32;",
            "    height: i32;",
            "}",
            "",
            "struct GlobalConfig {",
            "    screen: ScreenConfig;",
            "}",
            "",
            "struct Sprites {",
            "    paddle: i32;",
            "    ball: i32;",
            "}",
            "",
            "struct GlobalState {",
            "    config: GlobalConfig;",
            "    sprites: Sprites;",
            "}",
            "",
            "global state: GlobalState;",
            "",
            "function main(): i32 {",
            "    state.config.screen.",
            "    state.sprites.",
            "    return 0;",
            "}"
        });

        var uri = "file:///test/test_chain.stasis";

        await using var harness = await LspTestHarness.StartAsync(cts.Token);
        await harness.InitializeAsync(cts.Token);
        await harness.DidOpenAsync(uri, document, cts.Token);

        var screenPosition = GetPositionAfter(document, "state.config.screen.");
        var screenLabels = await harness.RequestCompletionLabelsAsync(uri, screenPosition.Line, screenPosition.Character, cts.Token);
        Assert.Contains("width", screenLabels);
        Assert.Contains("height", screenLabels);

        var spritesPosition = GetPositionAfter(document, "state.sprites.");
        var spritesLabels = await harness.RequestCompletionLabelsAsync(uri, spritesPosition.Line, spritesPosition.Character, cts.Token);
        Assert.Contains("paddle", spritesLabels);
        Assert.Contains("ball", spritesLabels);
    }

    [Fact]
    public async Task CompletesDeepNestedStructMembersAfterDot()
    {
        using var cts = new CancellationTokenSource(TimeSpan.FromSeconds(10));

        var document = string.Join("\n", new[]
        {
            "struct ScreenConfig {",
            "    width: i32;",
            "    height: i32;",
            "}",
            "",
            "struct BrickConfig {",
            "    width: f32;",
            "    height: f32;",
            "}",
            "",
            "struct Config {",
            "    screen: ScreenConfig;",
            "    brick: BrickConfig;",
            "}",
            "",
            "struct GameState {",
            "    config: Config;",
            "}",
            "",
            "global state: GameState;",
            "",
            "function main(): i32 {",
            "    state.config.brick.",
            "    return 0;",
            "}"
        });

        var uri = "file:///test/test_deep.stasis";
        var position = GetPositionAfter(document, "state.config.brick.");

        await using var harness = await LspTestHarness.StartAsync(cts.Token);
        await harness.InitializeAsync(cts.Token);
        await harness.DidOpenAsync(uri, document, cts.Token);

        var labels = await harness.RequestCompletionLabelsAsync(uri, position.Line, position.Character, cts.Token);
        Assert.Contains("width", labels);
        Assert.Contains("height", labels);
    }

    [Fact]
    public async Task CompletesParameterStructMembersAfterDot()
    {
        using var cts = new CancellationTokenSource(TimeSpan.FromSeconds(10));

        var document = string.Join("\n", new[]
        {
            "struct Vec2 {",
            "    x: f32;",
            "    y: f32;",
            "}",
            "",
            "function move(pos: Vec2): i32 {",
            "    pos.",
            "    return 0;",
            "}"
        });

        var uri = "file:///test/test_param.stasis";
        var position = GetPositionAfter(document, "pos.");

        await using var harness = await LspTestHarness.StartAsync(cts.Token);
        await harness.InitializeAsync(cts.Token);
        await harness.DidOpenAsync(uri, document, cts.Token);

        var labels = await harness.RequestCompletionLabelsAsync(uri, position.Line, position.Character, cts.Token);
        Assert.Contains("x", labels);
        Assert.Contains("y", labels);
    }

    [Fact]
    public async Task CompletesLocalStructMembersAfterDot()
    {
        using var cts = new CancellationTokenSource(TimeSpan.FromSeconds(10));

        var document = string.Join("\n", new[]
        {
            "struct Vec2 {",
            "    x: f32;",
            "    y: f32;",
            "}",
            "",
            "function main(): i32 {",
            "    let pos: Vec2 = 0;",
            "    pos.",
            "    return 0;",
            "}"
        });

        var uri = "file:///test/test_local.stasis";
        var position = GetPositionAfter(document, "pos.");

        await using var harness = await LspTestHarness.StartAsync(cts.Token);
        await harness.InitializeAsync(cts.Token);
        await harness.DidOpenAsync(uri, document, cts.Token);

        var labels = await harness.RequestCompletionLabelsAsync(uri, position.Line, position.Character, cts.Token);
        Assert.Contains("x", labels);
        Assert.Contains("y", labels);
    }

    [Fact]
    public async Task CompletesEnumMembersAfterDot()
    {
        using var cts = new CancellationTokenSource(TimeSpan.FromSeconds(10));

        var document = string.Join("\n", new[]
        {
            "enum Phase {",
            "    Idle,",
            "    Running,",
            "}",
            "",
            "function main(): i32 {",
            "    Phase.",
            "    return 0;",
            "}"
        });

        var uri = "file:///test/test_enum.stasis";
        var position = GetPositionAfter(document, "Phase.");

        await using var harness = await LspTestHarness.StartAsync(cts.Token);
        await harness.InitializeAsync(cts.Token);
        await harness.DidOpenAsync(uri, document, cts.Token);

        var labels = await harness.RequestCompletionLabelsAsync(uri, position.Line, position.Character, cts.Token);
        Assert.Contains("Idle", labels);
        Assert.Contains("Running", labels);
    }

    [Fact]
    public async Task CompletesNestedMembersWithOtherStateUsesInFile()
    {
        using var cts = new CancellationTokenSource(TimeSpan.FromSeconds(10));

        var document = string.Join("\n", new[]
        {
            "struct Config {",
            "    screen: ScreenConfig;",
            "}",
            "",
            "struct ScreenConfig {",
            "    w: i32;",
            "    h: i32;",
            "}",
            "",
            "struct GameState {",
            "    config: Config;",
            "    game: i32;",
            "}",
            "",
            "global state: GameState;",
            "",
            "function foo(): void {",
            "    state.game = 1;",
            "}",
            "",
            "function main(): i32 {",
            "    state.config.",
            "    return 0;",
            "}"
        });

        var uri = "file:///test/test_state_variants.stasis";
        var position = GetPositionAfter(document, "state.config.");

        await using var harness = await LspTestHarness.StartAsync(cts.Token);
        await harness.InitializeAsync(cts.Token);
        await harness.DidOpenAsync(uri, document, cts.Token);

        var labels = await harness.RequestCompletionLabelsAsync(uri, position.Line, position.Character, cts.Token);
        Assert.Contains("screen", labels);
    }

    [Fact]
    public void BuildsSymbolIndexForNestedStructs()
    {
        var document = string.Join("\n", new[]
        {
            "struct ScreenConfig {",
            "    width: i32;",
            "    height: i32;",
            "}",
            "",
            "struct GlobalConfig {",
            "    screen: ScreenConfig;",
            "}",
            "",
            "struct Sprites {",
            "    paddle: i32;",
            "    ball: i32;",
            "}",
            "",
            "struct GlobalState {",
            "    config: GlobalConfig;",
            "    sprites: Sprites;",
            "}",
            "",
            "global state: GlobalState;"
        });

        var parse = Parser.Parse(document);
        var index = SymbolIndex.Build(parse.CompilationUnit);

        var sprites = index.GetStruct("Sprites");
        Assert.NotNull(sprites);
        Assert.Contains(sprites!.Fields, f => f.Name == "paddle");
        Assert.Contains(sprites.Fields, f => f.Name == "ball");

        var global = index.GetStruct("GlobalState");
        Assert.NotNull(global);
        Assert.Contains(global!.Fields, f => f.Name == "sprites");
    }

    [Fact]
    public void ExtractsReceiverChainsForMultipleMemberAccess()
    {
        var document = string.Join("\n", new[]
        {
            "struct ScreenConfig { width: i32; }",
            "struct GlobalConfig { screen: ScreenConfig; }",
            "struct Sprites { paddle: i32; }",
            "struct GlobalState { config: GlobalConfig; sprites: Sprites; }",
            "global state: GlobalState;",
            "function main(): i32 {",
            "    state.config.screen.",
            "    state.sprites.",
            "    return 0;",
            "}"
        });

        var screenPos = GetPositionAfter(document, "state.config.screen.");
        var spritesPos = GetPositionAfter(document, "state.sprites.");

        var screenOffset = TextPositionConverter.PositionToOffset(document, new Position(screenPos.Line, screenPos.Character));
        var spritesOffset = TextPositionConverter.PositionToOffset(document, new Position(spritesPos.Line, spritesPos.Character));
        var spritesDotOffset = TextPositionConverter.PositionToOffset(document, new Position(spritesPos.Line, Math.Max(0, spritesPos.Character - 1)));

        var method = typeof(CompletionHandler).GetMethod(
            "TryGetMemberAccessReceiverChain",
            BindingFlags.NonPublic | BindingFlags.Static);
        Assert.NotNull(method);

        var screenArgs = new object?[] { document, screenOffset, null };
        var screenOk = (bool)method!.Invoke(null, screenArgs)!;
        Assert.True(screenOk);
        var screenChain = (IReadOnlyList<string>)screenArgs[2]!;
        Assert.Equal(new[] { "state", "config", "screen" }, screenChain);

        var spritesArgs = new object?[] { document, spritesOffset, null };
        var spritesOk = (bool)method!.Invoke(null, spritesArgs)!;
        Assert.True(spritesOk);
        var spritesChain = (IReadOnlyList<string>)spritesArgs[2]!;
        Assert.Equal(new[] { "state", "sprites" }, spritesChain);

        var spritesDotArgs = new object?[] { document, spritesDotOffset, null };
        var spritesDotOk = (bool)method!.Invoke(null, spritesDotArgs)!;
        Assert.True(spritesDotOk);
        var spritesDotChain = (IReadOnlyList<string>)spritesDotArgs[2]!;
        Assert.Equal(new[] { "state", "sprites" }, spritesDotChain);
    }

    [Fact]
    public void ExtractsReceiverChainsForDeepMemberAccess()
    {
        var document = string.Join("\n", new[]
        {
            "struct ScreenConfig { width: i32; }",
            "struct BrickConfig { width: f32; }",
            "struct Config { screen: ScreenConfig; brick: BrickConfig; }",
            "struct GameState { config: Config; }",
            "global state: GameState;",
            "function main(): i32 {",
            "    state.config.brick.",
            "    return 0;",
            "}"
        });

        var pos = GetPositionAfter(document, "state.config.brick.");
        var offset = TextPositionConverter.PositionToOffset(document, new Position(pos.Line, pos.Character));

        var method = typeof(CompletionHandler).GetMethod(
            "TryGetMemberAccessReceiverChain",
            BindingFlags.NonPublic | BindingFlags.Static);
        Assert.NotNull(method);

        var args = new object?[] { document, offset, null };
        var ok = (bool)method!.Invoke(null, args)!;
        Assert.True(ok);
        var chain = (IReadOnlyList<string>)args[2]!;
        Assert.Equal(new[] { "state", "config", "brick" }, chain);
    }

    [Fact]
    public async Task CompletesAfterIncrementalChange()
    {
        using var cts = new CancellationTokenSource(TimeSpan.FromSeconds(10));

        var document = string.Join("\n", new[]
        {
            "struct Config {",
            "    screen: ScreenConfig;",
            "}",
            "",
            "struct ScreenConfig {",
            "    w: i32;",
            "    h: i32;",
            "}",
            "",
            "struct GameState {",
            "    config: Config;",
            "}",
            "",
            "global state: GameState;",
            "",
            "function main(): i32 {",
            "    state.",
            "    return 0;",
            "}"
        });

        var uri = "file:///test/test_incremental.stasis";
        await using var harness = await LspTestHarness.StartAsync(cts.Token);
        await harness.InitializeAsync(cts.Token);
        await harness.DidOpenAsync(uri, document, cts.Token);

        var replacement = "    state.config.";
        var editOffset = document.IndexOf("    state.", StringComparison.Ordinal);
        Assert.True(editOffset >= 0, "Expected to find state line for incremental update.");
        var editStart = GetPositionAt(document, editOffset);
        var editEnd = new Position(editStart.Line, editStart.Character + "    state.".Length);
        await harness.DidChangeAsync(uri, new[]
        {
            new TextDocumentContentChangeEvent
            {
                Range = new LspRange(editStart, editEnd),
                Text = replacement
            }
        }, 2, cts.Token);

        var updatedDocument = document.Replace("    state.", replacement);
        var position = GetPositionAfter(updatedDocument, "state.config.");
        var labels = await harness.RequestCompletionLabelsAsync(uri, position.Line, position.Character, cts.Token);
        Assert.Contains("screen", labels);
    }

    [Fact]
    public async Task CompletesNestedMembersInBrickoutSample()
    {
        using var cts = new CancellationTokenSource(TimeSpan.FromSeconds(10));

        var repoRoot = Path.GetFullPath(Path.Combine(AppContext.BaseDirectory, "../../../../"));
        var samplePath = Path.Combine(repoRoot, "samples", "brickout_revenge", "brickout_revenge.stasis");
        Assert.True(File.Exists(samplePath), $"Missing sample file: {samplePath}");

        var document = await File.ReadAllTextAsync(samplePath, cts.Token);
        var uri = "file:///samples/brickout_revenge/brickout_revenge.stasis";

        await using var harness = await LspTestHarness.StartAsync(cts.Token);
        await harness.InitializeAsync(cts.Token);
        await harness.DidOpenAsync(uri, document, cts.Token);

        var configPos = GetPositionAfter(document, "state.config.");
        var configLabels = await harness.RequestCompletionLabelsAsync(uri, configPos.Line, configPos.Character, cts.Token);
        Assert.Contains("screen", configLabels);

        if (configLabels.Contains("brick"))
        {
            var brickPos = GetPositionAfter(document, "state.config.brick.");
            var brickLabels = await harness.RequestCompletionLabelsAsync(uri, brickPos.Line, brickPos.Character, cts.Token);
            Assert.Contains("width", brickLabels);
            Assert.Contains("height", brickLabels);
        }
    }

    private static (int Line, int Character) GetPositionAfter(string content, string marker)
    {
        var index = content.IndexOf(marker, StringComparison.Ordinal);
        if (index < 0)
        {
            throw new InvalidOperationException($"Marker not found: {marker}");
        }

        var offset = index + marker.Length;
        var position = GetPositionAt(content, offset);
        return (position.Line, position.Character);
    }

    private static Position GetPositionAt(string content, int offset)
    {
        var line = 0;
        var column = 0;
        for (var i = 0; i < offset && i < content.Length; i++)
        {
            if (content[i] == '\n')
            {
                line++;
                column = 0;
            }
            else
            {
                column++;
            }
        }

        return new Position(line, column);
    }
}

internal sealed class LspTestHarness : IAsyncDisposable
{
    private static readonly JsonSerializerOptions JsonOptions = new()
    {
        PropertyNameCaseInsensitive = true
    };

    private readonly CancellationTokenSource _shutdownCts = new();
    private Task _readerTask;
    private readonly Stream _clientIn;
    private readonly Stream _clientOut;
    private readonly Task<LspServer> _serverTask;
    private readonly DiagnosticsPublisher _diagnosticsPublisher;
    private LspServer? _server;
    private readonly ConcurrentDictionary<int, TaskCompletionSource<JsonElement>> _pending = new();
    private int _nextId = 1;

    private LspTestHarness(
        Stream clientIn,
        Stream clientOut,
        Task<LspServer> serverTask,
        DiagnosticsPublisher diagnosticsPublisher,
        Task readerTask)
    {
        _clientIn = clientIn;
        _clientOut = clientOut;
        _serverTask = serverTask;
        _diagnosticsPublisher = diagnosticsPublisher;
        _readerTask = readerTask;
    }

    public static Task<LspTestHarness> StartAsync(CancellationToken cancellationToken)
    {
        var (clientStream, serverStream) = FullDuplexStream.CreatePair();
        var clientOut = clientStream;
        var clientIn = clientStream;

        var documentManager = new DocumentManager();
        var diagnosticsPublisher = new DiagnosticsPublisher();
        var serverTask = LspServer.From(options =>
            options
                .WithInput(serverStream)
                .WithOutput(serverStream)
                .AddHandler(new DidOpenTextDocumentDiagnosticsHandler(documentManager, diagnosticsPublisher))
                .AddHandler(new DidChangeTextDocumentDiagnosticsHandler(documentManager, diagnosticsPublisher))
                .AddHandler(new DidCloseTextDocumentDiagnosticsHandler(documentManager, diagnosticsPublisher))
                .AddHandler(new HoverHandler(documentManager))
                .AddHandler(new CompletionHandler(documentManager))
        , CancellationToken.None);

        var harness = new LspTestHarness(clientIn, clientOut, serverTask, diagnosticsPublisher, Task.CompletedTask);
        harness._readerTask = Task.Run(() => harness.ReadLoopAsync(), CancellationToken.None);
        return Task.FromResult(harness);
    }

    public async Task InitializeAsync(CancellationToken cancellationToken)
    {
        var initParams = new
        {
            processId = (int?)null,
            rootUri = "file:///test",
            capabilities = new { }
        };
        await SendRequestAsync("initialize", initParams, cancellationToken);
        await SendNotificationAsync("initialized", new { }, cancellationToken);
#pragma warning disable VSTHRD003
        _server = await _serverTask;
#pragma warning restore VSTHRD003
        _diagnosticsPublisher.SetLanguageServer(_server);
    }

    public Task DidOpenAsync(string uri, string text, CancellationToken cancellationToken)
    {
        var @params = new
        {
            textDocument = new
            {
                uri,
                languageId = "stasis",
                version = 1,
                text
            }
        };
        return SendNotificationAsync("textDocument/didOpen", @params, cancellationToken);
    }

    public Task DidChangeAsync(
        string uri,
        IReadOnlyList<TextDocumentContentChangeEvent> changes,
        int version,
        CancellationToken cancellationToken)
    {
        var @params = new
        {
            textDocument = new
            {
                uri,
                version
            },
            contentChanges = changes.Select(c => new
            {
                range = c.Range,
                rangeLength = c.RangeLength,
                text = c.Text
            })
        };

        return SendNotificationAsync("textDocument/didChange", @params, cancellationToken);
    }

    public async Task<IReadOnlyList<string>> RequestCompletionLabelsAsync(
        string uri,
        int line,
        int character,
        CancellationToken cancellationToken)
    {
        var @params = new
        {
            textDocument = new { uri },
            position = new { line, character }
        };

        var result = await SendRequestAsync("textDocument/completion", @params, cancellationToken);

        if (result.ValueKind == JsonValueKind.Array)
        {
            return ExtractLabels(result);
        }

        if (result.ValueKind == JsonValueKind.Object && result.TryGetProperty("items", out var items))
        {
            return ExtractLabels(items);
        }

        return Array.Empty<string>();
    }

    public async ValueTask DisposeAsync()
    {
#pragma warning disable VSTHRD003, VSTHRD103
        try
        {
            await SendRequestAsync("shutdown", new { }, CancellationToken.None);
            await SendNotificationAsync("exit", new { }, CancellationToken.None);
        }
        catch
        {
            // Ignore shutdown failures in tests.
        }

        await _shutdownCts.CancelAsync();
        await _readerTask;
        _clientIn.Dispose();
        _clientOut.Dispose();
        if (_server == null)
        {
            _server = await _serverTask;
        }
        _server.Dispose();
#pragma warning restore VSTHRD003, VSTHRD103
    }

    private async Task<JsonElement> SendRequestAsync(string method, object @params, CancellationToken cancellationToken)
    {
        var id = Interlocked.Increment(ref _nextId);
        var tcs = new TaskCompletionSource<JsonElement>(TaskCreationOptions.RunContinuationsAsynchronously);
        _pending[id] = tcs;

        var payload = JsonSerializer.Serialize(new
        {
            jsonrpc = "2.0",
            id,
            method,
            @params
        });

        await WriteMessageAsync(payload, cancellationToken);

        using var linked = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken, _shutdownCts.Token);
        await using var _ = linked.Token.Register(() => tcs.TrySetCanceled(linked.Token));
        return await tcs.Task;
    }

    private Task SendNotificationAsync(string method, object @params, CancellationToken cancellationToken)
    {
        var payload = JsonSerializer.Serialize(new
        {
            jsonrpc = "2.0",
            method,
            @params
        });
        return WriteMessageAsync(payload, cancellationToken);
    }

    private async Task WriteMessageAsync(string json, CancellationToken cancellationToken)
    {
        var bytes = Encoding.UTF8.GetBytes(json);
        var header = Encoding.ASCII.GetBytes($"Content-Length: {bytes.Length}\r\n\r\n");
        await _clientOut.WriteAsync(header, cancellationToken);
        await _clientOut.WriteAsync(bytes, cancellationToken);
        await _clientOut.FlushAsync(cancellationToken);
    }

    private async Task ReadLoopAsync()
    {
        while (!_shutdownCts.IsCancellationRequested)
        {
            string? message;
            try
            {
                message = await ReadMessageAsync(_clientIn, _shutdownCts.Token);
            }
            catch (OperationCanceledException)
            {
                return;
            }
            catch (ObjectDisposedException)
            {
                return;
            }
            if (message == null)
            {
                break;
            }

            using var doc = JsonDocument.Parse(message);
            var root = doc.RootElement;
            if (root.TryGetProperty("id", out var idElement) && idElement.ValueKind == JsonValueKind.Number)
            {
                var id = idElement.GetInt32();
                if (_pending.TryRemove(id, out var tcs))
                {
                    if (root.TryGetProperty("result", out var result))
                    {
                        tcs.TrySetResult(result.Clone());
                    }
                    else if (root.TryGetProperty("error", out var error))
                    {
                        tcs.TrySetException(new InvalidOperationException(error.ToString()));
                    }
                }
            }
        }
    }

    private static async Task<string?> ReadMessageAsync(Stream input, CancellationToken cancellationToken)
    {
        var headerBytes = new List<byte>();
        var lastFour = new Queue<byte>(4);

        while (true)
        {
            var buffer = new byte[1];
            var read = await input.ReadAsync(buffer, cancellationToken);
            if (read == 0)
            {
                return null;
            }

            headerBytes.Add(buffer[0]);
            lastFour.Enqueue(buffer[0]);
            if (lastFour.Count > 4)
            {
                lastFour.Dequeue();
            }

            if (lastFour.Count == 4 &&
                lastFour.ElementAt(0) == '\r' &&
                lastFour.ElementAt(1) == '\n' &&
                lastFour.ElementAt(2) == '\r' &&
                lastFour.ElementAt(3) == '\n')
            {
                break;
            }
        }

        var headerText = Encoding.ASCII.GetString(headerBytes.ToArray());
        var contentLength = ParseContentLength(headerText);
        var content = new byte[contentLength];
        var offset = 0;
        while (offset < contentLength)
        {
            var read = await input.ReadAsync(content.AsMemory(offset, contentLength - offset), cancellationToken);
            if (read == 0)
            {
                throw new EndOfStreamException("Unexpected end of stream while reading LSP message.");
            }
            offset += read;
        }

        return Encoding.UTF8.GetString(content);
    }

    private static int ParseContentLength(string headerText)
    {
        foreach (var line in headerText.Split(new[] { "\r\n" }, StringSplitOptions.RemoveEmptyEntries))
        {
            if (line.StartsWith("Content-Length:", StringComparison.OrdinalIgnoreCase))
            {
                var value = line.Substring("Content-Length:".Length).Trim();
                if (int.TryParse(value, out var length))
                {
                    return length;
                }
            }
        }

        throw new InvalidOperationException($"Missing Content-Length header: {headerText}");
    }

    private static IReadOnlyList<string> ExtractLabels(JsonElement itemsElement)
    {
        if (itemsElement.ValueKind != JsonValueKind.Array)
        {
            return Array.Empty<string>();
        }

        var labels = new List<string>();
        foreach (var item in itemsElement.EnumerateArray())
        {
            if (item.ValueKind == JsonValueKind.Object &&
                item.TryGetProperty("label", out var labelElement) &&
                labelElement.ValueKind == JsonValueKind.String)
            {
                labels.Add(labelElement.GetString() ?? string.Empty);
            }
        }

        return labels;
    }
}
