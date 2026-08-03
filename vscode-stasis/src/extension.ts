import { spawn } from "node:child_process";
import * as fs from "node:fs";
import * as path from "node:path";
import * as vscode from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
  Trace,
} from "vscode-languageclient/node";
import { LiveSession, LiveSessionState } from "./liveSession";
import {
  displayRuntimeValue,
  LiveResponse,
  LiveValue,
} from "./protocol";

const LANGUAGE_SELECTOR: vscode.DocumentSelector = [
  { language: "stasis", scheme: "file" },
  { language: "stasis", scheme: "untitled" },
];

interface CommandOutput {
  stdout: string;
  stderr: string;
}

function configuration(): vscode.WorkspaceConfiguration {
  return vscode.workspace.getConfiguration("stasis");
}

function executablePath(): string {
  return configuration().get<string>("executablePath", "stasis").trim() || "stasis";
}

function findWorkspaceRoot(document?: vscode.TextDocument): string | undefined {
  if (document?.uri.scheme === "file") {
    const folder = vscode.workspace.getWorkspaceFolder(document.uri);
    let candidate = path.dirname(document.uri.fsPath);
    const boundary = folder?.uri.fsPath;
    while (true) {
      if (fs.existsSync(path.join(candidate, "stasis.json"))) {
        return candidate;
      }
      if (candidate === boundary) {
        break;
      }
      const parent = path.dirname(candidate);
      if (parent === candidate || (boundary && !parent.startsWith(boundary))) {
        break;
      }
      candidate = parent;
    }
    if (folder && fs.existsSync(path.join(folder.uri.fsPath, "stasis.json"))) {
      return folder.uri.fsPath;
    }
  }
  return vscode.workspace.workspaceFolders?.find((folder) =>
    fs.existsSync(path.join(folder.uri.fsPath, "stasis.json")),
  )?.uri.fsPath;
}

function runStasis(
  args: readonly string[],
  cwd: string,
  input: string | undefined,
  token?: vscode.CancellationToken,
): Promise<CommandOutput> {
  return new Promise((resolve, reject) => {
    const child = spawn(executablePath(), args, {
      cwd,
      stdio: "pipe",
      windowsHide: true,
    });
    let stdout = "";
    let stderr = "";
    const maximumBytes = 16 * 1024 * 1024;
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk: string) => {
      stdout += chunk;
      if (stdout.length > maximumBytes) {
        child.kill();
        reject(new Error("Stasis returned more than 16 MiB on stdout."));
      }
    });
    child.stderr.on("data", (chunk: string) => {
      stderr += chunk;
      if (stderr.length > maximumBytes) {
        child.kill();
        reject(new Error("Stasis returned more than 16 MiB on stderr."));
      }
    });
    child.once("error", (error) => reject(error));
    child.once("exit", (code) => {
      if (code === 0) {
        resolve({ stdout, stderr });
      } else {
        reject(new Error(stderr.trim() || `Stasis exited with code ${code ?? "unknown"}.`));
      }
    });
    const cancellation = token?.onCancellationRequested(() => child.kill());
    child.once("close", () => cancellation?.dispose());
    child.stdin.end(input);
  });
}

class StasisFormatter implements vscode.DocumentFormattingEditProvider {
  async provideDocumentFormattingEdits(
    document: vscode.TextDocument,
    _options: vscode.FormattingOptions,
    token: vscode.CancellationToken,
  ): Promise<vscode.TextEdit[]> {
    const cwd = findWorkspaceRoot(document) ?? path.dirname(document.uri.fsPath);
    const output = await runStasis(["format", "--stdin"], cwd, document.getText(), token);
    const end = document.lineAt(document.lineCount - 1).rangeIncludingLineBreak.end;
    if (output.stdout === document.getText()) {
      return [];
    }
    return [vscode.TextEdit.replace(new vscode.Range(new vscode.Position(0, 0), end), output.stdout)];
  }
}

class LiveValueItem extends vscode.TreeItem {
  constructor(readonly liveValue: LiveValue) {
    super(liveValue.path, vscode.TreeItemCollapsibleState.None);
    this.contextValue = liveValue.watched ? "stasisLiveWatch" : "stasisLiveValue";
    this.description = liveValue.error
      ? `error: ${liveValue.error}`
      : `${liveValue.staticType ? `${liveValue.staticType} = ` : ""}${displayRuntimeValue(liveValue.value)}`;
    this.tooltip = `${liveValue.path}\n${this.description}\ntick ${liveValue.tick}`;
    this.iconPath = new vscode.ThemeIcon(liveValue.error ? "error" : liveValue.watched ? "eye" : "symbol-variable");
  }
}

class LiveValuesProvider implements vscode.TreeDataProvider<vscode.TreeItem> {
  private readonly emitter = new vscode.EventEmitter<vscode.TreeItem | undefined>();
  private state: LiveSessionState = "stopped";
  private values: readonly LiveValue[] = [];
  readonly onDidChangeTreeData = this.emitter.event;

  update(state: LiveSessionState, values: readonly LiveValue[]): void {
    this.state = state;
    this.values = values;
    this.emitter.fire(undefined);
  }

  getTreeItem(element: vscode.TreeItem): vscode.TreeItem {
    return element;
  }

  getChildren(): vscode.TreeItem[] {
    const status = new vscode.TreeItem(`Play session: ${this.state}`);
    status.iconPath = new vscode.ThemeIcon(this.state === "stopped" ? "debug-stop" : "pulse");
    status.contextValue = "stasisLiveStatus";
    if (this.state === "stopped") {
      status.command = { command: "stasis.startPlaySession", title: "Start Play Session" };
    }
    return [status, ...this.values.map((value) => new LiveValueItem(value))];
  }
}

class LiveController implements vscode.Disposable {
  private current: LiveSession | undefined;
  private sessionSubscriptions: vscode.Disposable[] = [];
  readonly status: vscode.StatusBarItem;

  constructor(
    private readonly values: LiveValuesProvider,
    private readonly output: vscode.OutputChannel,
  ) {
    this.status = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 20);
    this.status.command = "stasis.startPlaySession";
    this.updateState("stopped");
    this.status.show();
  }

  get state(): LiveSessionState {
    return this.current?.state ?? "stopped";
  }

  get liveValues(): readonly LiveValue[] {
    return this.current?.values ?? [];
  }

  requireSession(): LiveSession {
    if (!this.current || this.current.state === "stopped") {
      throw new Error("Start a Stasis play session first.");
    }
    return this.current;
  }

  async start(): Promise<void> {
    const document = vscode.window.activeTextEditor?.document;
    const root = findWorkspaceRoot(document);
    if (!root) {
      throw new Error("Open a folder containing stasis.json before starting a play session.");
    }
    if (this.current && this.current.state !== "stopped") {
      if (this.current.root === root) {
        return;
      }
      await this.stop();
    }
    const saved = await vscode.workspace.saveAll(false);
    if (!saved) {
      throw new Error("Save the workspace before starting the Stasis play session.");
    }
    this.current?.dispose();
    this.disposeSessionSubscriptions();
    const session = new LiveSession(
      root,
      executablePath(),
      configuration().get<string>("live.entry", ""),
      this.output,
    );
    this.current = session;
    this.sessionSubscriptions = [
      session.onDidChangeState((state) => this.updateState(state)),
      session.onDidChangeValues((values) => this.values.update(session.state, values)),
    ];
    this.updateState("starting");
    try {
      await session.start();
      this.output.appendLine(`Play session ready: ${root}`);
    } catch (error) {
      session.dispose();
      this.current = undefined;
      this.updateState("stopped");
      throw error;
    }
  }

  async stop(): Promise<void> {
    await this.current?.stop();
  }

  dispose(): void {
    this.disposeSessionSubscriptions();
    this.current?.dispose();
    this.status.dispose();
  }

  private updateState(state: LiveSessionState): void {
    this.values.update(state, state === "stopped" ? [] : (this.current?.values ?? []));
    this.status.text = `$(pulse) Stasis: ${state}`;
    this.status.tooltip = state === "stopped" ? "Start a Stasis play session" : "Stasis live play session";
    this.status.command = state === "stopped" ? "stasis.startPlaySession" : "stasis.showOutput";
    void vscode.commands.executeCommand("setContext", "stasis.liveSessionActive", state !== "stopped");
    void vscode.commands.executeCommand("setContext", "stasis.liveSessionRunning", state === "running");
    void vscode.commands.executeCommand("setContext", "stasis.liveSessionPaused", state === "paused");
  }

  private disposeSessionSubscriptions(): void {
    for (const disposable of this.sessionSubscriptions) {
      disposable.dispose();
    }
    this.sessionSubscriptions = [];
  }
}

class StasisLanguageClients implements vscode.Disposable {
  private readonly clients = new Map<string, LanguageClient>();
  private readonly subscriptions: vscode.Disposable[];

  constructor(private readonly output: vscode.LogOutputChannel) {
    this.subscriptions = [
      vscode.workspace.onDidChangeWorkspaceFolders((event) => {
        for (const folder of event.removed) {
          void this.stopFolder(folder);
        }
        for (const folder of event.added) {
          if (fs.existsSync(path.join(folder.uri.fsPath, "stasis.json"))) {
            void this.startFolder(folder);
          }
        }
      }),
      vscode.workspace.onDidChangeConfiguration((event) => {
        if (
          event.affectsConfiguration("stasis.executablePath") ||
          event.affectsConfiguration("stasis.completion.limit")
        ) {
          void this.restart();
        }
      }),
    ];
  }

  async start(): Promise<void> {
    const folders = (vscode.workspace.workspaceFolders ?? []).filter((folder) =>
      fs.existsSync(path.join(folder.uri.fsPath, "stasis.json")),
    );
    await Promise.all(folders.map((folder) => this.startFolder(folder)));
  }

  dispose(): void {
    for (const subscription of this.subscriptions) {
      subscription.dispose();
    }
    for (const client of this.clients.values()) {
      void client.stop();
    }
    this.clients.clear();
  }

  private async restart(): Promise<void> {
    const clients = [...this.clients.values()];
    this.clients.clear();
    await Promise.all(clients.map((client) => client.stop()));
    await this.start();
  }

  private async startFolder(folder: vscode.WorkspaceFolder): Promise<void> {
    const root = folder.uri.fsPath;
    if (this.clients.has(root)) {
      return;
    }
    const serverOptions: ServerOptions = {
      command: executablePath(),
      args: ["--workspace", root, "lsp", "--stdio"],
      options: {
        cwd: root,
      },
    };
    const clientOptions: LanguageClientOptions = {
      documentSelector: [
        {
          language: "stasis",
          scheme: "file",
        },
      ],
      workspaceFolder: folder,
      initializationOptions: {
        completionLimit: Math.max(
          1,
          Math.min(256, vscode.workspace.getConfiguration("stasis", folder.uri).get<number>("completion.limit", 64)),
        ),
      },
      outputChannel: this.output,
      traceOutputChannel: this.output,
      middleware: {
        didOpen: (document, next) => {
          const owned = vscode.workspace.getWorkspaceFolder(document.uri)?.index === folder.index;
          return owned ? next(document) : Promise.resolve();
        },
        didChange: (event, next) => {
          const owned = vscode.workspace.getWorkspaceFolder(event.document.uri)?.index === folder.index;
          return owned ? next(event) : Promise.resolve();
        },
        didClose: (document, next) =>
          vscode.workspace.getWorkspaceFolder(document.uri)?.index === folder.index
            ? next(document)
            : Promise.resolve(),
      },
    };
    const client = new LanguageClient(
      `stasis-${folder.index}`,
      `Stasis (${folder.name})`,
      serverOptions,
      clientOptions,
    );
    this.clients.set(root, client);
    try {
      await client.start();
      await client.setTrace(Trace.Verbose);
      this.output.appendLine(`Language server ready: ${root}`);
    } catch (error) {
      this.clients.delete(root);
      throw error;
    }
  }

  private async stopFolder(folder: vscode.WorkspaceFolder): Promise<void> {
    const client = this.clients.get(folder.uri.fsPath);
    if (!client) {
      return;
    }
    this.clients.delete(folder.uri.fsPath);
    await client.stop();
  }
}

async function askForPath(prompt: string): Promise<string | undefined> {
  return vscode.window.showInputBox({
    prompt,
    placeHolder: "state.player.health or enemies[0].hp",
    validateInput: (value) => (value.trim().length === 0 ? "Enter a state path or query." : undefined),
  });
}

async function showCommandError(action: () => Promise<void>): Promise<void> {
  try {
    await action();
  } catch (error) {
    void vscode.window.showErrorMessage(`Stasis: ${error instanceof Error ? error.message : String(error)}`);
  }
}

export async function activate(context: vscode.ExtensionContext): Promise<StasisExtensionApi> {
  const output = vscode.window.createOutputChannel("Stasis", { log: true });
  const languageClients = new StasisLanguageClients(output);
  const values = new LiveValuesProvider();
  const controller = new LiveController(values, output);
  const command = (name: string, action: (...args: unknown[]) => Promise<void>) =>
    vscode.commands.registerCommand(name, (...args: unknown[]) => showCommandError(() => action(...args)));

  context.subscriptions.push(
    output,
    languageClients,
    controller,
    vscode.window.registerTreeDataProvider("stasis.liveValues", values),
    vscode.languages.registerDocumentFormattingEditProvider(LANGUAGE_SELECTOR, new StasisFormatter()),
    command("stasis.startPlaySession", async () => controller.start()),
    command("stasis.stopPlaySession", async () => controller.stop()),
    command("stasis.pausePlaySession", async () => {
      await controller.requireSession().request("pause");
    }),
    command("stasis.resumePlaySession", async () => {
      await controller.requireSession().request("resume");
    }),
    command("stasis.stepPlaySession", async () => {
      await controller.requireSession().request("step", { ticks: 1 });
    }),
    command("stasis.inspectValue", async () => {
      const session = controller.requireSession();
      const livePath = await askForPath("Inspect a value in the running Stasis game");
      if (livePath) {
        await session.request("inspect", { path: livePath.trim() });
      }
    }),
    command("stasis.addWatch", async () => {
      const session = controller.requireSession();
      const livePath = await askForPath("Watch a value while the Stasis game runs");
      if (livePath) {
        await session.addWatch(livePath.trim());
      }
    }),
    command("stasis.removeWatch", async (item) => {
      if (item instanceof LiveValueItem) {
        await controller.requireSession().removeWatch(item.liveValue.path);
      }
    }),
    command("stasis.refreshLiveValues", async () => controller.requireSession().refresh()),
    command("stasis.showOutput", async () => output.show(true)),
  );

  await languageClients.start();

  return {
    state: () => controller.state,
    values: () => controller.liveValues,
    start: () => controller.start(),
    stop: () => controller.stop(),
    request: (type, fields = {}) => controller.requireSession().request(type, fields),
  } satisfies StasisExtensionApi;
}

export interface StasisExtensionApi {
  state(): LiveSessionState;
  values(): readonly LiveValue[];
  start(): Promise<void>;
  stop(): Promise<void>;
  request(type: string, fields?: Record<string, unknown>): Promise<LiveResponse>;
}

export function deactivate(): void {}
