import * as path from "path";
import * as vscode from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
  TransportKind,
} from "vscode-languageclient/node";

let client: LanguageClient | undefined;

function serverCommandForPlatform(extensionPath: string): { command: string; args: string[] } {
  const serverDir = path.join(extensionPath, "server");

  if (process.platform === "win32") {
    return { command: path.join(serverDir, "stasis-lsp.exe"), args: [] };
  }

  return { command: path.join(serverDir, "stasis-lsp"), args: [] };
}

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  const { command, args } = serverCommandForPlatform(context.extensionPath);

  const serverOptions: ServerOptions = {
    command,
    args,
    transport: TransportKind.stdio,
  };

  const clientOptions: LanguageClientOptions = {
    documentSelector: [{ scheme: "file", language: "stasis" }],
  };

  client = new LanguageClient("stasisLanguageServer", "Stasis Language Server", serverOptions, clientOptions);
  context.subscriptions.push(client.start());
}

export async function deactivate(): Promise<void> {
  await client?.stop();
}

