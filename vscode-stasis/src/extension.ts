import { spawn } from "node:child_process";
import * as fs from "node:fs";
import * as path from "node:path";
import * as vscode from "vscode";
import { LiveSession, LiveSessionState } from "./liveSession";
import {
  byteOffsetToStringOffset,
  CompilerCompletion,
  displayRuntimeValue,
  LiveResponse,
  LiveValue,
  stringOffsetToByteOffset,
} from "./protocol";

const LANGUAGE_SELECTOR: vscode.DocumentSelector = [
  { language: "stasis", scheme: "file" },
  { language: "stasis", scheme: "untitled" },
];

interface CommandOutput {
  stdout: string;
  stderr: string;
}

interface SourceReference {
  file: string;
  kind: string;
  source_span: {
    start: number;
    end: number;
  };
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

function symbolAtPosition(document: vscode.TextDocument, position: vscode.Position): string | undefined {
  const source = document.getText();
  const offset = document.offsetAt(position);
  const symbols = /[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*/g;
  for (const match of source.matchAll(symbols)) {
    const start = match.index;
    const end = start + match[0].length;
    if (start <= offset && offset <= end) {
      return match[0];
    }
  }
  return undefined;
}

async function compilerReferences(
  document: vscode.TextDocument,
  position: vscode.Position,
  token: vscode.CancellationToken,
): Promise<SourceReference[]> {
  const root = findWorkspaceRoot(document);
  const symbol = symbolAtPosition(document, position);
  if (!root || !symbol || token.isCancellationRequested) {
    return [];
  }
  const output = await runStasis(
    ["--json", "--workspace", root, "symbol", "references", symbol, "--limit", "256"],
    root,
    undefined,
    token,
  );
  const envelope = asRecord(JSON.parse(output.stdout) as unknown);
  const result = asRecord(envelope?.result);
  const references = Array.isArray(result?.references) ? result.references : [];
  return references.flatMap((value) => {
    const reference = asRecord(value);
    const span = asRecord(reference?.source_span);
    if (
      typeof reference?.file !== "string" ||
      typeof reference.kind !== "string" ||
      typeof span?.start !== "number" ||
      typeof span.end !== "number"
    ) {
      return [];
    }
    return [{
      file: reference.file,
      kind: reference.kind,
      source_span: { start: span.start, end: span.end },
    }];
  });
}

async function referenceLocation(root: string, reference: SourceReference): Promise<vscode.Location> {
  const uri = vscode.Uri.file(path.join(root, reference.file));
  const document = await vscode.workspace.openTextDocument(uri);
  const source = document.getText();
  return new vscode.Location(
    uri,
    new vscode.Range(
      document.positionAt(byteOffsetToStringOffset(source, reference.source_span.start)),
      document.positionAt(byteOffsetToStringOffset(source, reference.source_span.end)),
    ),
  );
}

class StasisDefinitionProvider implements vscode.DefinitionProvider {
  async provideDefinition(
    document: vscode.TextDocument,
    position: vscode.Position,
    token: vscode.CancellationToken,
  ): Promise<vscode.Location[]> {
    const root = findWorkspaceRoot(document);
    if (!root) {
      return [];
    }
    const references = await compilerReferences(document, position, token);
    return Promise.all(
      references
        .filter((reference) => reference.kind === "definition")
        .map((reference) => referenceLocation(root, reference)),
    );
  }
}

class StasisReferenceProvider implements vscode.ReferenceProvider {
  async provideReferences(
    document: vscode.TextDocument,
    position: vscode.Position,
    context: vscode.ReferenceContext,
    token: vscode.CancellationToken,
  ): Promise<vscode.Location[]> {
    const root = findWorkspaceRoot(document);
    if (!root) {
      return [];
    }
    const references = await compilerReferences(document, position, token);
    return Promise.all(
      references
        .filter((reference) => context.includeDeclaration || reference.kind !== "definition")
        .map((reference) => referenceLocation(root, reference)),
    );
  }
}

class StasisTests implements vscode.Disposable {
  private readonly controller = vscode.tests.createTestController("stasisTests", "Stasis Tests");
  private readonly watcher = vscode.workspace.createFileSystemWatcher("**/*.test.stasis");
  private readonly runProfile: vscode.TestRunProfile;

  constructor(private readonly output: vscode.OutputChannel) {
    this.controller.resolveHandler = async () => this.refresh();
    this.runProfile = this.controller.createRunProfile(
      "Run Stasis Tests",
      vscode.TestRunProfileKind.Run,
      async (request, token) => this.run(request, token),
      true,
    );
    this.watcher.onDidCreate(() => void this.refresh());
    this.watcher.onDidChange(() => void this.refresh());
    this.watcher.onDidDelete(() => void this.refresh());
  }

  async refresh(): Promise<void> {
    const items: vscode.TestItem[] = [];
    for (const folder of vscode.workspace.workspaceFolders ?? []) {
      const manifestPath = path.join(folder.uri.fsPath, "stasis.json");
      if (!fs.existsSync(manifestPath)) {
        continue;
      }
      const manifest = asRecord(JSON.parse(fs.readFileSync(manifestPath, "utf8")) as unknown);
      const testsDirectory = typeof manifest?.tests === "string" ? manifest.tests : "tests";
      const pattern = new vscode.RelativePattern(folder, `${testsDirectory.replaceAll("\\", "/")}/**/*.test.stasis`);
      const files = await vscode.workspace.findFiles(
        pattern,
        "**/{.git,.stasis-cache,node_modules,target,build,dist}/**",
      );
      for (const uri of files.sort((left, right) => left.fsPath.localeCompare(right.fsPath))) {
        const label = path.relative(folder.uri.fsPath, uri.fsPath).replaceAll("\\", "/");
        items.push(this.controller.createTestItem(uri.toString(), label, uri));
      }
    }
    this.controller.items.replace(items);
  }

  fileUris(): string[] {
    const uris: string[] = [];
    this.controller.items.forEach((item) => {
      if (item.uri) {
        uris.push(item.uri.toString());
      }
    });
    return uris.sort();
  }

  async runFile(uri: vscode.Uri, token?: vscode.CancellationToken): Promise<CommandOutput> {
    const document = await vscode.workspace.openTextDocument(uri);
    const root = findWorkspaceRoot(document);
    if (!root) {
      throw new Error(`No Stasis workspace contains ${uri.fsPath}.`);
    }
    const relative = path.relative(root, uri.fsPath).replaceAll("\\", "/");
    return runStasis(["--json", "--workspace", root, "test", relative], root, undefined, token);
  }

  dispose(): void {
    this.runProfile.dispose();
    this.watcher.dispose();
    this.controller.dispose();
  }

  private async run(request: vscode.TestRunRequest, token: vscode.CancellationToken): Promise<void> {
    const run = this.controller.createTestRun(request);
    const items: vscode.TestItem[] = [];
    if (request.include) {
      items.push(...request.include);
    } else {
      this.controller.items.forEach((item) => items.push(item));
    }
    for (const item of items) {
      if (token.isCancellationRequested || !item.uri) {
        run.skipped(item);
        continue;
      }
      run.started(item);
      try {
        const result = await this.runFile(item.uri, token);
        run.appendOutput(result.stdout.replaceAll("\n", "\r\n"), undefined, item);
        run.passed(item);
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        this.output.appendLine(`Test failed: ${item.label}: ${message}`);
        run.failed(item, new vscode.TestMessage(message));
      }
    }
    run.end();
  }
}

function asRecord(value: unknown): Record<string, unknown> | undefined {
  return typeof value === "object" && value !== null
    ? (value as Record<string, unknown>)
    : undefined;
}

function completionKind(kind: string): vscode.CompletionItemKind {
  switch (kind.toLowerCase()) {
    case "function":
    case "method":
      return vscode.CompletionItemKind.Function;
    case "struct":
    case "enum":
    case "type":
      return vscode.CompletionItemKind.Struct;
    case "field":
      return vscode.CompletionItemKind.Field;
    case "local":
    case "parameter":
    case "global":
      return vscode.CompletionItemKind.Variable;
    case "keyword":
    case "command":
      return vscode.CompletionItemKind.Keyword;
    case "constant":
      return vscode.CompletionItemKind.Constant;
    default:
      return vscode.CompletionItemKind.Text;
  }
}

function compilerCompletionItem(item: CompilerCompletion, rank: number): vscode.CompletionItem {
  const result = new vscode.CompletionItem(item.text, completionKind(item.kind));
  result.insertText = item.text;
  result.detail = item.detail ?? item.type_name ?? item.kind;
  result.filterText = item.text;
  result.sortText = rank.toString().padStart(6, "0");
  return result;
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

class StasisCompletionProvider implements vscode.CompletionItemProvider {
  constructor(private readonly controller: LiveController) {}

  async provideCompletionItems(
    document: vscode.TextDocument,
    position: vscode.Position,
    token: vscode.CancellationToken,
  ): Promise<vscode.CompletionList> {
    const root = findWorkspaceRoot(document);
    if (!root || token.isCancellationRequested) {
      return new vscode.CompletionList();
    }
    const limit = Math.max(1, Math.min(200, configuration().get<number>("completion.limit", 64)));
    const session = this.controller.sessionFor(root);
    if (session && session.state !== "stopped") {
      const source = document.getText();
      const stringOffset = document.offsetAt(position);
      const response = await session.request("complete", {
        buffer: source,
        cursor: stringOffsetToByteOffset(source, stringOffset),
        limit,
        context: {
          file: path.relative(root, document.uri.fsPath).replaceAll("\\", "/"),
          source_offset: stringOffsetToByteOffset(source, stringOffset),
        },
      });
      if (token.isCancellationRequested) {
        return new vscode.CompletionList();
      }
      return this.liveCompletionList(document, source, response);
    }

    const word = document.getWordRangeAtPosition(position, /[A-Za-z_][A-Za-z0-9_.]*/);
    const query = word ? document.getText(word) : "";
    const args = [
      "--json",
      "--workspace",
      root,
      "symbol",
      "list",
      "--limit",
      String(limit),
    ];
    if (query.length > 0) {
      args.push("--query", query);
    }
    const output = await runStasis(args, root, undefined, token);
    const envelope = asRecord(JSON.parse(output.stdout) as unknown);
    const result = asRecord(envelope?.result);
    const items = Array.isArray(result?.items) ? result.items : [];
    return new vscode.CompletionList(
      items.flatMap((value, rank) => {
        const item = asRecord(value);
        if (!item || typeof item.name !== "string" || typeof item.kind !== "string") {
          return [];
        }
        return [
          compilerCompletionItem(
            {
              text: item.name,
              kind: item.kind,
              detail: typeof item.signature === "string" ? item.signature : undefined,
            },
            rank,
          ),
        ];
      }),
      Number(result?.total ?? items.length) > items.length,
    );
  }

  private liveCompletionList(
    document: vscode.TextDocument,
    source: string,
    response: LiveResponse,
  ): vscode.CompletionList {
    const data = asRecord(response.data);
    const values = Array.isArray(data?.items) ? data.items : [];
    const start = typeof data?.replacement_start === "number" ? data.replacement_start : 0;
    const end = typeof data?.replacement_end === "number" ? data.replacement_end : start;
    const range = new vscode.Range(
      document.positionAt(byteOffsetToStringOffset(source, start)),
      document.positionAt(byteOffsetToStringOffset(source, end)),
    );
    const items = values.flatMap((value, rank) => {
      const item = asRecord(value);
      if (!item || typeof item.text !== "string" || typeof item.kind !== "string") {
        return [];
      }
      const completion = compilerCompletionItem(
        {
          text: item.text,
          kind: item.kind,
          detail: typeof item.detail === "string" ? item.detail : undefined,
          type_name: typeof item.type_name === "string" ? item.type_name : undefined,
        },
        rank,
      );
      completion.range = range;
      return [completion];
    });
    return new vscode.CompletionList(items, response.truncated === true || data?.truncated === true);
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

  sessionFor(root: string): LiveSession | undefined {
    return this.current?.root === root ? this.current : undefined;
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

export function activate(context: vscode.ExtensionContext): StasisExtensionApi {
  const output = vscode.window.createOutputChannel("Stasis");
  const values = new LiveValuesProvider();
  const controller = new LiveController(values, output);
  const tests = new StasisTests(output);
  const command = (name: string, action: (...args: unknown[]) => Promise<void>) =>
    vscode.commands.registerCommand(name, (...args: unknown[]) => showCommandError(() => action(...args)));

  context.subscriptions.push(
    output,
    controller,
    tests,
    vscode.window.registerTreeDataProvider("stasis.liveValues", values),
    vscode.languages.registerDocumentFormattingEditProvider(LANGUAGE_SELECTOR, new StasisFormatter()),
    vscode.languages.registerDefinitionProvider(LANGUAGE_SELECTOR, new StasisDefinitionProvider()),
    vscode.languages.registerReferenceProvider(LANGUAGE_SELECTOR, new StasisReferenceProvider()),
    vscode.languages.registerCompletionItemProvider(
      LANGUAGE_SELECTOR,
      new StasisCompletionProvider(controller),
      ".",
    ),
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
  void tests.refresh();

  return {
    state: () => controller.state,
    values: () => controller.liveValues,
    start: () => controller.start(),
    stop: () => controller.stop(),
    request: (type, fields = {}) => controller.requireSession().request(type, fields),
    testFiles: () => tests.fileUris(),
    runTestFile: (uri) => tests.runFile(vscode.Uri.parse(uri)),
  } satisfies StasisExtensionApi;
}

export interface StasisExtensionApi {
  state(): LiveSessionState;
  values(): readonly LiveValue[];
  start(): Promise<void>;
  stop(): Promise<void>;
  request(type: string, fields?: Record<string, unknown>): Promise<LiveResponse>;
  testFiles(): readonly string[];
  runTestFile(uri: string): Promise<CommandOutput>;
}

export function deactivate(): void {}
