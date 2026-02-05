import * as path from "path";
import * as vscode from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
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
          { exe: "Stasis.LanguageServer.dll", args: [] },
          { exe: "stasis-lsp.exe", args: [] },
          { exe: "Stasis.LanguageServer.exe", args: [] },
        ]
      : [
          { exe: "Stasis.LanguageServer.dll", args: [] },
          { exe: "stasis-lsp", args: [] },
          { exe: "Stasis.LanguageServer", args: [] },
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
    void vscode.window.showWarningMessage(
      "Stasis LSP server binary not found; autocomplete/hover/diagnostics are disabled (syntax highlighting still works). See vscode-stasis/README.md for install steps."
    );
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
  context.subscriptions.push({
    dispose: () => {
      void client?.stop();
    },
  });
}

export async function deactivate(): Promise<void> {
  await client?.stop();
}
