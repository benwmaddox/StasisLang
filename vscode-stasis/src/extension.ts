import * as path from "path";
import * as vscode from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  CompletionTriggerKind,
  ServerOptions,
  TransportKind,
  Trace,
} from "vscode-languageclient/node";

let client: LanguageClient | undefined;
let clientStart: Promise<void> | undefined;
let sawLspCompletionRequest = false;

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function withTimeout<T>(promise: Promise<T>, ms: number): Promise<T> {
  const timeout = new Promise<never>((_, reject) => {
    const id = setTimeout(() => {
      clearTimeout(id);
      reject(new Error(`timeout after ${ms}ms`));
    }, ms);
  });
  return Promise.race([promise, timeout]);
}

function tryServerCommand(extensionPath: string): { command: string; args: string[] } | undefined {
  const serverDir = path.join(extensionPath, "server");

  const candidates: Array<{ exe: string; args: string[] }> =
    process.platform === "win32"
      ? [
          // Prefer `dotnet <dll>` on Windows to avoid policies that block running arbitrary EXEs from the
          // VS Code extensions folder (AppLocker/WDAC can manifest as "spawn UNKNOWN").
          { exe: "Stasis.LanguageServer.dll", args: [] },
          { exe: "stasis-lsp.exe", args: [] },
          { exe: "Stasis.LanguageServer.exe", args: [] },
        ]
      : [
          { exe: "stasis-lsp", args: [] },
          { exe: "Stasis.LanguageServer", args: [] },
          { exe: "Stasis.LanguageServer.dll", args: [] },
        ];

  for (const c of candidates) {
    const fullPath = path.join(serverDir, c.exe);
    if (!require("fs").existsSync(fullPath)) continue;

    if (fullPath.endsWith(".dll")) {
      return { command: "dotnet", args: [fullPath, ...c.args] };
    }

    return { command: fullPath, args: c.args };
  }

  return undefined;
}

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  const output = vscode.window.createOutputChannel("Stasis");
  const server = tryServerCommand(context.extensionPath);
  if (!server) {
    output.appendLine(
      "Stasis Language Server binary not found. LSP features are disabled; syntax highlighting still works."
    );
    output.appendLine("To enable LSP, publish the server into vscode-stasis/server/ (see vscode-stasis/README.md).");
    context.subscriptions.push(output);
    return;
  }

  const serverOptions: ServerOptions = {
    command: server.command,
    args: server.args,
    transport: TransportKind.stdio,
    options: {
      cwd: path.join(context.extensionPath, "server"),
    },
  };

  const clientOptions: LanguageClientOptions = {
    documentSelector: [{ scheme: "file", language: "stasis" }],
    outputChannel: output,
    traceOutputChannel: output,
    middleware: {
      provideCompletionItem: async (document, position, context, token, next) => {
        sawLspCompletionRequest = true;
        output.appendLine(
          `[completion:req] ${document.uri.toString()} ${position.line}:${position.character} trigger=${context.triggerCharacter ?? ""} kind=${context.triggerKind}`
        );
        const result = await next(document, position, context, token);
        const count = Array.isArray(result)
          ? result.length
          : result && typeof result === "object" && "items" in result
          ? // eslint-disable-next-line @typescript-eslint/no-explicit-any
            ((result as any).items?.length ?? 0)
          : 0;
        output.appendLine(`[completion:res] count=${count}`);
        return result;
      },
    },
  };

  output.appendLine(`Starting Stasis LSP: ${server.command} ${server.args.join(" ")}`.trim());

  client = new LanguageClient("stasisLanguageServer", "Stasis Language Server", serverOptions, clientOptions);
  void client.setTrace(Trace.Verbose);
  clientStart = client.start();
  client.onDidChangeState((e) => {
    output.appendLine(`[client:state] ${e.oldState} -> ${e.newState}`);
  });

  let fallbackCompletionProvider: vscode.Disposable | undefined;
  void (async () => {
    try {
      await withTimeout(clientStart!, 10_000);
      output.appendLine("[client:start] ok");
      if (client!.initializeResult) {
        output.appendLine("[client:init] ok");
      } else {
        output.appendLine("[client:init] missing initializeResult");
      }

      // Some servers (including OmniSharp.Extensions-based) may register completion dynamically
      // after initialize, which means initializeResult.capabilities.completionProvider can be null
      // even though completions work. Delay fallback registration until we're confident the client
      // didn't register an LSP-backed completion provider.
      output.appendLine("[completion:provider] checking server completion support...");
      await delay(2000);
      const hasLspCompletion = sawLspCompletionRequest || !!client!.initializeResult?.capabilities?.completionProvider;
      if (hasLspCompletion) {
        output.appendLine("[completion:provider] using LSP completion");
        return;
      }

      output.appendLine("[completion:provider] enabling fallback completion provider");

      // As a pragmatic fallback, register a VS Code completion provider that forwards to the LSP request directly.
      fallbackCompletionProvider = vscode.languages.registerCompletionItemProvider(
        { scheme: "file", language: "stasis" },
        {
          provideCompletionItems: async (document, position, token, context) => {
            if (!client) return [];
            try {
              if (!client.initializeResult && clientStart) {
                await withTimeout(clientStart, 2_000);
              }
            } catch (err) {
              output.appendLine(`[completion:provider] client not started: ${(err as Error)?.message ?? String(err)}`);
              await delay(250);
            }

            const params = {
              textDocument: client.code2ProtocolConverter.asTextDocumentIdentifier(document),
              position: client.code2ProtocolConverter.asPosition(position),
              context: {
                triggerKind:
                  context.triggerKind === vscode.CompletionTriggerKind.TriggerCharacter
                    ? CompletionTriggerKind.TriggerCharacter
                    : context.triggerKind === vscode.CompletionTriggerKind.TriggerForIncompleteCompletions
                    ? CompletionTriggerKind.TriggerForIncompleteCompletions
                    : CompletionTriggerKind.Invoked,
                triggerCharacter: context.triggerCharacter ?? undefined,
              },
            };

            output.appendLine(
              `[completion:provider] ${document.uri.toString()} ${position.line}:${position.character} trigger=${context.triggerCharacter ?? ""}`
            );

            try {
              // eslint-disable-next-line @typescript-eslint/no-explicit-any
              const result: any = await client.sendRequest("textDocument/completion", params, token);
              const items = Array.isArray(result) ? result : result?.items ?? [];
              output.appendLine(`[completion:provider] items=${items.length}`);

              return items.map((item: any) => {
                const ci = new vscode.CompletionItem(item.label);
                if (item.insertText) {
                  ci.insertText = item.insertText;
                }
                if (item.detail) {
                  ci.detail = item.detail;
                }
                if (item.documentation) {
                  ci.documentation =
                    typeof item.documentation === "string"
                      ? item.documentation
                      : item.documentation.value ?? item.documentation;
                }
                return ci;
              });
            } catch (err) {
              output.appendLine(`[completion:error] ${(err as Error)?.message ?? String(err)}`);
              return [];
            }
          },
        },
        "."
      );
      context.subscriptions.push(fallbackCompletionProvider);
    } catch (err) {
      output.appendLine(`[client:start] failed: ${(err as Error)?.message ?? String(err)}`);
    }
  })();

  context.subscriptions.push({
    dispose: () => {
      void client?.stop();
      fallbackCompletionProvider?.dispose();
    },
  });
}

export async function deactivate(): Promise<void> {
  await client?.stop();
}
