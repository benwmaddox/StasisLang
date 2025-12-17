using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging;
using OmniSharp.Extensions.LanguageServer.Protocol.Models;
using OmniSharp.Extensions.LanguageServer.Protocol.Server;
using OmniSharp.Extensions.LanguageServer.Server;
using Stasis.LanguageServer.Handlers;
using Stasis.LanguageServer.Services;

var documentManager = new DocumentManager();

var server = await LanguageServer.From(options =>
    options
        .WithInput(Console.OpenStandardInput())
        .WithOutput(Console.OpenStandardOutput())
        .ConfigureLogging(x => x
            .SetMinimumLevel(LogLevel.Debug))
        .AddHandler(new HoverHandler(documentManager))
        .AddHandler(new CompletionHandler(documentManager))
);

await server.WaitForExit;
