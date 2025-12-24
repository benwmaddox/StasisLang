using Microsoft.Extensions.Logging;
using OmniSharp.Extensions.LanguageServer.Protocol.Models;
using OmniSharp.Extensions.LanguageServer.Protocol.Server;
using OmniSharp.Extensions.LanguageServer.Server;
using Stasis.LanguageServer.Handlers;
using Stasis.LanguageServer.Services;

var documentManager = new DocumentManager();
var diagnosticsPublisher = new DiagnosticsPublisher();

var server = await LanguageServer.From(options =>
    options
        .WithInput(Console.OpenStandardInput())
        .WithOutput(Console.OpenStandardOutput())
        .ConfigureLogging(x => x
            .SetMinimumLevel(LogLevel.Debug))
        .AddHandler(new DidOpenTextDocumentDiagnosticsHandler(documentManager, diagnosticsPublisher))
        .AddHandler(new DidChangeTextDocumentDiagnosticsHandler(documentManager, diagnosticsPublisher))
        .AddHandler(new DidCloseTextDocumentDiagnosticsHandler(documentManager, diagnosticsPublisher))
        .AddHandler(new HoverHandler(documentManager))
        .AddHandler(new CompletionHandler(documentManager))
);

// Set the server reference on DiagnosticsPublisher now that the server is initialized
diagnosticsPublisher.SetLanguageServer(server);

await server.WaitForExit;
