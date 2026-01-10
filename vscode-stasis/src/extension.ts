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
      didChange: async (event, next) => {
        // Force full-text sync to keep the server document in sync for large files.
        const doc = event.document;
        const end = doc.lineAt(doc.lineCount - 1).range.end;
        const fullText = doc.getText();
        output.appendLine(`[didChange] ${doc.uri.toString()} len=${fullText.length} lines=${doc.lineCount}`);
        const fullEvent = {
          document: doc,
          contentChanges: [
            {
              range: new vscode.Range(0, 0, end.line, end.character),
              rangeOffset: 0,
              rangeLength: fullText.length,
              text: fullText,
            },
          ],
          reason: event.reason,
        };
        return next(fullEvent);
      },
      provideCompletionItem: async (document, position, context, token, next) => {
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
  void client.start();

  // Some language server stacks rely on dynamic registration and may not advertise completionProvider
  // in initialize results. As a pragmatic fallback, register a VS Code completion provider that forwards
  // to the LSP request directly so completions still work.
  const completionProvider = vscode.languages.registerCompletionItemProvider(
    { scheme: "file", language: "stasis" },
    {
      provideCompletionItems: async (document, position, token, context) => {
        if (!client) return [];

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
  context.subscriptions.push(completionProvider);

  context.subscriptions.push({
    dispose: () => {
      void client?.stop();
    },
  });
}

export async function deactivate(): Promise<void> {
  await client?.stop();
}
