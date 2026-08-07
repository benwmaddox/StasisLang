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
import { buildLiveCollectionTableModel } from "./liveCollectionTable";
import { buildLiveValueTree, LiveValueTreeNode } from "./liveValueTree";
import { displayRuntimeValue, LiveCollection, LiveResponse, LiveValue } from "./protocol";
import { resolveEditorToolchain } from "./toolchain";

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

let activeToolchainExecutable: string | undefined;

function executablePath(): string {
  if (!activeToolchainExecutable) {
    throw new Error("the Stasis editor toolchain has not been verified");
  }
  return activeToolchainExecutable;
}

function workspaceRootKey(root: string): string {
  let resolved: string;
  try {
    resolved = fs.realpathSync.native(root);
  } catch {
    resolved = path.resolve(root);
  }
  return process.platform === "win32" ? resolved.toLowerCase() : resolved;
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

function pathIsWithin(root: string, candidate: string): boolean {
  const relative = path.relative(root, candidate);
  return relative === "" || (!relative.startsWith("..") && !path.isAbsolute(relative));
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
      const manifestUris = await vscode.workspace.findFiles(
        new vscode.RelativePattern(folder, "**/stasis.json"),
        "**/{.git,.stasis-cache,node_modules,target,build,dist}/**",
      );
      for (const manifestUri of manifestUris.sort((left, right) => left.fsPath.localeCompare(right.fsPath))) {
        const projectRoot = path.dirname(manifestUri.fsPath);
        const manifest = asRecord(JSON.parse(fs.readFileSync(manifestUri.fsPath, "utf8")) as unknown);
        const testsDirectory = typeof manifest?.tests === "string" ? manifest.tests : "tests";
        const files = await vscode.workspace.findFiles(
          new vscode.RelativePattern(projectRoot, `${testsDirectory.replaceAll("\\", "/")}/**/*.test.stasis`),
          "**/{.git,.stasis-cache,node_modules,target,build,dist}/**",
        );
        for (const uri of files.sort((left, right) => left.fsPath.localeCompare(right.fsPath))) {
          const label = path.relative(folder.uri.fsPath, uri.fsPath).replaceAll("\\", "/");
          items.push(this.controller.createTestItem(uri.toString(), label, uri));
        }
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

class LiveValueItem extends vscode.TreeItem {
  constructor(
    readonly node: LiveValueTreeNode,
    tableCollections: ReadonlySet<string>,
    filterInactiveRows: boolean,
  ) {
    super(
      node.label,
      node.children.length > 0 || node.kind === "collection"
        ? vscode.TreeItemCollapsibleState.Collapsed
        : vscode.TreeItemCollapsibleState.None,
    );
    const value = node.value;
    if (value) {
      this.contextValue = value.watched ? "stasisLiveWatch" : "stasisLiveValue";
      this.description = value.error
        ? `error: ${value.error}`
        : `${value.staticType ? `${value.staticType} = ` : ""}${displayRuntimeValue(value.value)}`;
      this.tooltip = `${value.path}\n${this.description}\ntick ${value.tick}`;
      this.iconPath = new vscode.ThemeIcon(value.error ? "error" : value.watched ? "eye" : "symbol-variable");
      return;
    }
    if (node.kind === "collection" && node.collection) {
      const table = tableCollections.has(node.path);
      const collection = node.collection;
      const truncated = collection.rowsTruncated ? ", partial" : "";
      const activeField = collection.fields.find((field) =>
        field.field.toLowerCase() === "active" && field.staticType === "bool",
      );
      const shownRows = filterInactiveRows && activeField
        ? collection.rows.filter((row) => row.values[activeField.field] === true).length
        : collection.rows.length;
      const filtered = filterInactiveRows && activeField ? `, ${shownRows} shown` : "";
      this.description = `[${collection.activeCount}/${collection.capacity}${truncated}${filtered}] | ${table ? "table" : "tree"}`;
      this.tooltip = `${collection.path}\n${collection.elementShape}\n${collection.fields.map((field) => `${field.field || "value"}: ${field.staticType}`).join("\n")}`;
      this.iconPath = new vscode.ThemeIcon(table ? "table" : "symbol-array");
      if (collection.fields.some((field) => field.field.length > 0)) {
        this.contextValue = table ? "stasisLiveCollectionTable" : "stasisLiveCollectionTree";
      }
      return;
    }
    if (node.kind === "collection-row" && node.collection && node.rowIndex !== undefined) {
      const row = node.collection.rows.find((candidate) => candidate.index === node.rowIndex);
      if (tableCollections.has(node.collection.path) && row) {
        this.description = node.collection.fields
          .map((field) => `${field.field || "value"}: ${displayRuntimeValue(row.values[field.field])}`)
          .join("  |  ");
        this.tooltip = `${node.path}\n${this.description}\ntick ${node.collection.tick}`;
      }
      this.iconPath = new vscode.ThemeIcon("record");
      return;
    }
    this.contextValue = "stasisLiveGroup";
    this.iconPath = new vscode.ThemeIcon("symbol-object");
  }

  get liveValue(): LiveValue | undefined {
    return this.node.value;
  }
}

class LiveValuesProvider implements vscode.TreeDataProvider<vscode.TreeItem> {
  private readonly emitter = new vscode.EventEmitter<vscode.TreeItem | undefined>();
  private state: LiveSessionState = "stopped";
  private values: readonly LiveValue[] = [];
  private collections: readonly LiveCollection[] = [];
  private readonly tableCollections = new Set<string>();
  private filterInactiveRows: boolean;
  readonly onDidChangeTreeData = this.emitter.event;

  constructor(filterInactiveRows: boolean) {
    this.filterInactiveRows = filterInactiveRows;
  }

  update(
    state: LiveSessionState,
    values: readonly LiveValue[],
    collections: readonly LiveCollection[] = [],
  ): void {
    this.state = state;
    this.values = values;
    this.collections = collections;
    const currentPaths = new Set(collections.map((collection) => collection.path));
    for (const path of this.tableCollections) {
      if (!currentPaths.has(path)) {
        this.tableCollections.delete(path);
      }
    }
    this.emitter.fire(undefined);
  }

  setCollectionTable(path: string, table: boolean): void {
    if (table) {
      this.tableCollections.add(path);
    } else {
      this.tableCollections.delete(path);
    }
    this.emitter.fire(undefined);
  }

  setFilterInactiveRows(enabled: boolean): void {
    this.filterInactiveRows = enabled;
    this.emitter.fire(undefined);
  }

  collection(path: string): LiveCollection | undefined {
    return this.collections.find((collection) => collection.path === path);
  }

  get filtersInactiveRows(): boolean {
    return this.filterInactiveRows;
  }

  getTreeItem(element: vscode.TreeItem): vscode.TreeItem {
    return element;
  }

  getChildren(element?: vscode.TreeItem): vscode.TreeItem[] {
    if (element instanceof LiveValueItem) {
      return element.node.children.map((node) =>
        new LiveValueItem(node, this.tableCollections, this.filterInactiveRows),
      );
    }
    const status = new vscode.TreeItem(`Play session: ${this.state}`);
    status.description = this.state === "paused"
      ? "Resume or step one tick from the toolbar"
      : this.state === "running"
        ? "Polling while Live Values is open"
        : this.state === "starting"
          ? "Preparing compiler and runtime"
          : "Select to start";
    status.tooltip = this.state === "paused"
      ? "The game is paused. Use Resume or Step One Tick in the Live Values toolbar."
      : `Stasis play session: ${this.state}`;
    status.iconPath = new vscode.ThemeIcon(
      this.state === "stopped" ? "debug-stop" : this.state === "paused" ? "debug-pause" : "pulse",
    );
    status.contextValue = "stasisLiveStatus";
    if (this.state === "stopped") {
      status.command = { command: "stasis.startPlaySession", title: "Start Play Session" };
    }
    const roots = buildLiveValueTree(
      this.values,
      this.collections,
      this.tableCollections,
      this.filterInactiveRows,
    );
    return [
      status,
      ...roots.map((node) =>
        new LiveValueItem(node, this.tableCollections, this.filterInactiveRows),
      ),
    ];
  }
}

class LiveCollectionTablePanel implements vscode.Disposable {
  private panel: vscode.WebviewPanel | undefined;
  private path: string | undefined;
  private readonly subscription: vscode.Disposable;

  constructor(
    private readonly values: LiveValuesProvider,
    private readonly onDidChangeVisibility: (visible: boolean) => void,
  ) {
    this.subscription = values.onDidChangeTreeData(() => this.render());
  }

  show(path: string): void {
    if (this.path && this.path !== path) {
      this.values.setCollectionTable(this.path, false);
    }
    this.path = path;
    this.values.setCollectionTable(path, true);

    if (!this.panel) {
      const panel = vscode.window.createWebviewPanel(
        "stasis.liveCollectionTable",
        `Live Table: ${path}`,
        vscode.ViewColumn.Beside,
        { enableScripts: true, retainContextWhenHidden: true },
      );
      panel.webview.html = liveCollectionTableHtml(panel.webview);
      panel.webview.onDidReceiveMessage((message: unknown) => {
        if (asRecord(message)?.type === "ready") {
          this.render();
        }
      });
      panel.onDidChangeViewState((event) => {
        this.onDidChangeVisibility(event.webviewPanel.visible);
        if (event.webviewPanel.visible) {
          this.render();
        }
      });
      panel.onDidDispose(() => {
        const closedPath = this.path;
        this.panel = undefined;
        this.path = undefined;
        if (closedPath) {
          this.values.setCollectionTable(closedPath, false);
        }
        this.onDidChangeVisibility(false);
      });
      this.panel = panel;
    } else {
      this.panel.title = `Live Table: ${path}`;
      this.panel.reveal(vscode.ViewColumn.Beside, true);
    }
    this.onDidChangeVisibility(true);
    this.render();
  }

  close(path: string): void {
    if (this.path === path && this.panel) {
      this.panel.dispose();
      return;
    }
    this.values.setCollectionTable(path, false);
  }

  dispose(): void {
    this.subscription.dispose();
    this.panel?.dispose();
  }

  private render(): void {
    if (!this.panel || !this.path) {
      return;
    }
    const collection = this.values.collection(this.path);
    if (!collection) {
      void this.panel.webview.postMessage({ type: "unavailable", path: this.path });
      return;
    }
    void this.panel.webview.postMessage({
      type: "update",
      model: buildLiveCollectionTableModel(collection, this.values.filtersInactiveRows),
    });
  }
}

function liveCollectionTableHtml(webview: vscode.Webview): string {
  const nonce = Array.from({ length: 32 }, () => Math.floor(Math.random() * 36).toString(36)).join("");
  return `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'nonce-${nonce}'; script-src 'nonce-${nonce}';">
  <style nonce="${nonce}">
    :root { color-scheme: light dark; }
    body { color: var(--vscode-foreground); background: var(--vscode-editor-background); font-family: var(--vscode-font-family); margin: 0; padding: 20px; }
    header { align-items: end; display: flex; flex-wrap: wrap; gap: 8px 20px; justify-content: space-between; margin-bottom: 16px; }
    h1 { font-size: 18px; font-weight: 600; margin: 0 0 4px; }
    #shape { color: var(--vscode-descriptionForeground); font-family: var(--vscode-editor-font-family); }
    #metadata { color: var(--vscode-descriptionForeground); font-size: 12px; }
    .table-frame { border: 1px solid var(--vscode-panel-border); max-height: calc(100vh - 105px); overflow: auto; }
    table { border-collapse: separate; border-spacing: 0; font-family: var(--vscode-editor-font-family); font-size: var(--vscode-editor-font-size); min-width: 100%; width: max-content; }
    th, td { border-bottom: 1px solid var(--vscode-panel-border); border-right: 1px solid var(--vscode-panel-border); padding: 7px 12px; text-align: left; white-space: nowrap; }
    th:last-child, td:last-child { border-right: 0; }
    thead th { background: var(--vscode-sideBar-background); font-family: var(--vscode-font-family); font-weight: 600; position: sticky; top: 0; z-index: 1; }
    thead small { color: var(--vscode-descriptionForeground); display: block; font-family: var(--vscode-editor-font-family); font-size: 10px; font-weight: 400; margin-top: 2px; }
    tbody tr:nth-child(even) { background: var(--vscode-list-inactiveSelectionBackground); }
    tbody tr:hover { background: var(--vscode-list-hoverBackground); }
    tbody tr:last-child td { border-bottom: 0; }
    .index { color: var(--vscode-descriptionForeground); text-align: right; }
    .empty { color: var(--vscode-descriptionForeground); font-family: var(--vscode-font-family); padding: 20px; text-align: center; }
  </style>
</head>
<body>
  <header><div><h1 id="path">Live collection</h1><div id="shape"></div></div><div id="metadata"></div></header>
  <div class="table-frame"><table aria-label="Live collection values"><thead><tr id="columns"></tr></thead><tbody id="rows"></tbody></table></div>
  <script nonce="${nonce}">
    const path = document.getElementById('path');
    const shape = document.getElementById('shape');
    const metadata = document.getElementById('metadata');
    const columns = document.getElementById('columns');
    const rows = document.getElementById('rows');
    const vscode = acquireVsCodeApi();
    const cell = (tag, text, className) => { const node = document.createElement(tag); node.textContent = text; if (className) node.className = className; return node; };
    window.addEventListener('message', ({ data }) => {
      if (data.type === 'unavailable') { path.textContent = data.path; metadata.textContent = 'Collection unavailable'; columns.replaceChildren(); rows.replaceChildren(); return; }
      if (data.type !== 'update') return;
      const model = data.model;
      path.textContent = model.path;
      shape.textContent = model.elementShape;
      const details = [model.activeCount + '/' + model.capacity + ' rows', model.rows.length + ' shown', 'tick ' + model.tick];
      if (model.filtered) details.splice(2, 0, 'inactive filtered');
      if (model.rowsTruncated) details.push('partial snapshot');
      metadata.textContent = details.join('  ·  ');
      const indexHeader = cell('th', 'index'); indexHeader.appendChild(cell('small', 'i32')); columns.replaceChildren(indexHeader);
      for (const column of model.columns) { const header = cell('th', column.label); header.appendChild(cell('small', column.staticType)); columns.appendChild(header); }
      rows.replaceChildren();
      for (const row of model.rows) { const tr = document.createElement('tr'); tr.appendChild(cell('td', String(row.index), 'index')); row.cells.forEach(value => tr.appendChild(cell('td', value))); rows.appendChild(tr); }
      if (model.rows.length === 0) { const td = cell('td', 'No rows match the current filter.', 'empty'); td.colSpan = model.columns.length + 1; const tr = document.createElement('tr'); tr.appendChild(td); rows.appendChild(tr); }
    });
    vscode.postMessage({ type: 'ready' });
  </script>
</body>
</html>`;
}

class LiveController implements vscode.Disposable {
  private current: LiveSession | undefined;
  private sessionSubscriptions: vscode.Disposable[] = [];
  private valuesViewVisible = false;
  private tableViewVisible = false;
  private visibilityUpdate: Promise<void> = Promise.resolve();
  readonly status: vscode.StatusBarItem;

  constructor(
    private readonly values: LiveValuesProvider,
    private readonly output: vscode.OutputChannel,
    private readonly clientForRoot: (root: string) => LanguageClient | undefined,
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
    const client = this.clientForRoot(root);
    if (!client) {
      throw new Error(`The Stasis language server is not ready for ${root}.`);
    }
    const session = new LiveSession(
      root,
      client,
      configuration().get<string>("live.entry", ""),
      this.output,
    );
    this.current = session;
    this.sessionSubscriptions = [
      session.onDidChangeState((state) => {
        this.updateState(state);
      }),
      session.onDidChangeValues((values) => {
        this.values.update(session.state, values, session.collections);
      }),
    ];
    this.updateState("starting");
    try {
      await session.start();
      await this.queueVisibilityUpdate();
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

  async updateRefreshCadence(): Promise<void> {
    if (this.valuesViewVisible && this.current && this.current.state !== "stopped") {
      await this.queueVisibilityUpdate();
    }
  }

  async setValuesViewVisible(visible: boolean): Promise<void> {
    this.valuesViewVisible = visible;
    await this.queueVisibilityUpdate();
  }

  async setTableViewVisible(visible: boolean): Promise<void> {
    this.tableViewVisible = visible;
    await this.queueVisibilityUpdate();
  }

  async refreshLiveValues(): Promise<void> {
    if (!this.valuesViewVisible && !this.tableViewVisible) {
      throw new Error("Open Live Values or a live collection table before refreshing live data.");
    }
    await this.requireSession().refresh(this.refreshEveryTicks());
  }

  async addWatch(path: string): Promise<void> {
    if (!this.valuesViewVisible) {
      throw new Error("Open the Live Values view before adding a live watch.");
    }
    await this.requireSession().addWatch(path);
  }

  dispose(): void {
    this.disposeSessionSubscriptions();
    this.current?.dispose();
    this.status.dispose();
  }

  private updateState(state: LiveSessionState): void {
    this.values.update(
      state,
      state === "stopped" ? [] : (this.current?.values ?? []),
      state === "stopped" ? [] : (this.current?.collections ?? []),
    );
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

  private refreshEveryTicks(): number {
    return Math.max(1, configuration().get<number>("live.refreshEveryTicks", 30));
  }

  private queueVisibilityUpdate(): Promise<void> {
    const update = this.visibilityUpdate
      .catch(() => undefined)
      .then(async () => {
        const session = this.current;
        if (!session || session.state === "stopped" || session.state === "starting") {
          return;
        }
        if (this.valuesViewVisible || this.tableViewVisible) {
          await session.refresh(this.refreshEveryTicks());
        } else {
          await session.stopRefreshing();
        }
      });
    this.visibilityUpdate = update;
    return update;
  }
}

class StasisLanguageClients implements vscode.Disposable {
  private readonly clients = new Map<string, LanguageClient>();
  private readonly starts = new Map<string, Promise<void>>();
  private readonly subscriptions: vscode.Disposable[];

  constructor(private readonly output: vscode.LogOutputChannel) {
    this.subscriptions = [
      vscode.workspace.onDidChangeWorkspaceFolders((event) => {
        for (const folder of event.removed) {
          void this.stopFolder(folder);
        }
        for (const folder of event.added) {
          if (fs.existsSync(path.join(folder.uri.fsPath, "stasis.json"))) {
            this.ensureProjectWithReporting(folder.uri.fsPath, folder);
          }
        }
      }),
      vscode.workspace.onDidOpenTextDocument((document) => {
        if (document.languageId === "stasis") {
          this.ensureDocumentWithReporting(document);
        }
      }),
      vscode.workspace.onDidChangeConfiguration((event) => {
        if (
          event.affectsConfiguration("stasis.completion.limit")
        ) {
          void this.restart();
        }
      }),
    ];
  }

  async start(): Promise<void> {
    const starts: Promise<void>[] = [];
    for (const folder of vscode.workspace.workspaceFolders ?? []) {
      if (fs.existsSync(path.join(folder.uri.fsPath, "stasis.json"))) {
        starts.push(this.ensureProject(folder.uri.fsPath, folder));
      }
    }
    for (const document of vscode.workspace.textDocuments) {
      if (document.languageId === "stasis") {
        starts.push(this.ensureDocument(document));
      }
    }
    const results = await Promise.allSettled(starts);
    const failure = results.find((result): result is PromiseRejectedResult => result.status === "rejected");
    if (failure) {
      await this.reportStartError(failure.reason);
    }
  }

  clientForRoot(root: string): LanguageClient | undefined {
    return this.clients.get(workspaceRootKey(root));
  }

  dispose(): void {
    for (const subscription of this.subscriptions) {
      subscription.dispose();
    }
    for (const client of this.clients.values()) {
      void client.stop();
    }
    this.clients.clear();
    this.starts.clear();
  }

  private async restart(): Promise<void> {
    const clients = [...this.clients.values()];
    this.clients.clear();
    this.starts.clear();
    await Promise.all(clients.map((client) => client.stop()));
    await this.start();
  }

  private ensureDocument(document: vscode.TextDocument): Promise<void> {
    const root = findWorkspaceRoot(document);
    const folder = vscode.workspace.getWorkspaceFolder(document.uri);
    if (!root || !folder) {
      return Promise.resolve();
    }
    return this.ensureProject(root, folder);
  }

  private ensureDocumentWithReporting(document: vscode.TextDocument): void {
    void this.ensureDocument(document).catch((error) => this.reportStartError(error));
  }

  private ensureProjectWithReporting(root: string, folder: vscode.WorkspaceFolder): void {
    void this.ensureProject(root, folder).catch((error) => this.reportStartError(error));
  }

  private async reportStartError(error: unknown): Promise<void> {
    const message = error instanceof Error ? error.message : String(error);
    this.output.error(message);
    await vscode.window.showErrorMessage(`Stasis language tooling is unavailable: ${message}`);
  }

  private ensureProject(root: string, folder: vscode.WorkspaceFolder): Promise<void> {
    const key = workspaceRootKey(root);
    if (this.clients.has(key)) {
      return Promise.resolve();
    }
    const existing = this.starts.get(key);
    if (existing) {
      return existing;
    }
    const start = this.startProject(root, key, folder).finally(() => this.starts.delete(key));
    this.starts.set(key, start);
    return start;
  }

  private async startProject(root: string, key: string, folder: vscode.WorkspaceFolder): Promise<void> {
    if (this.clients.has(key)) {
      return;
    }
    const toolchain = executablePath();
    const serverOptions: ServerOptions = {
      command: toolchain,
      args: ["--workspace", root, "lsp", "--stdio"],
      options: {
        cwd: root,
      },
    };
    const relativeRoot = path.relative(folder.uri.fsPath, root).replaceAll("\\", "/");
    const projectPattern = relativeRoot ? `${relativeRoot}/**/*.stasis` : "**/*.stasis";
    const clientOptions: LanguageClientOptions = {
      documentSelector: [
        {
          language: "stasis",
          scheme: "file",
          pattern: {
            baseUri: folder.uri.toString(),
            pattern: projectPattern,
          },
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
          const owner = findWorkspaceRoot(document);
          const owned = owner !== undefined && workspaceRootKey(owner) === key;
          return owned ? next(document) : Promise.resolve();
        },
        didChange: (event, next) => {
          const owner = findWorkspaceRoot(event.document);
          const owned = owner !== undefined && workspaceRootKey(owner) === key;
          return owned ? next(event) : Promise.resolve();
        },
        didClose: (document, next) => {
          const owner = findWorkspaceRoot(document);
          return owner !== undefined && workspaceRootKey(owner) === key ? next(document) : Promise.resolve();
        },
      },
    };
    const client = new LanguageClient(
      `stasis-${folder.index}-${this.clients.size}`,
      `Stasis (${path.basename(root)})`,
      serverOptions,
      clientOptions,
    );
    this.clients.set(key, client);
    try {
      await client.start();
      await client.setTrace(Trace.Verbose);
      this.output.appendLine(`Language server ready: ${root}`);
    } catch (error) {
      this.clients.delete(key);
      throw error;
    }
  }

  private async stopFolder(folder: vscode.WorkspaceFolder): Promise<void> {
    const stopped: Promise<void>[] = [];
    for (const [root, client] of this.clients) {
      if (pathIsWithin(folder.uri.fsPath, root)) {
        this.clients.delete(root);
        stopped.push(client.stop());
      }
    }
    await Promise.all(stopped);
  }
}

class StasisDebugAdapterFactory implements vscode.DebugAdapterDescriptorFactory {
  createDebugAdapterDescriptor(
    session: vscode.DebugSession,
    executable: vscode.DebugAdapterExecutable | undefined,
  ): vscode.ProviderResult<vscode.DebugAdapterDescriptor> {
    if (executable) {
      return executable;
    }
    const configuredRoot = session.configuration.__stasisProjectRoot;
    const root = typeof configuredRoot === "string" ? configuredRoot : debugProjectRoot(session.workspaceFolder);
    if (!root) {
      throw new Error("Open a folder containing stasis.json before debugging Stasis.");
    }
    return new vscode.DebugAdapterExecutable(
      executablePath(),
      ["--workspace", root, "dap", "--stdio"],
      { cwd: root },
    );
  }
}

class StasisDebugConfigurationProvider implements vscode.DebugConfigurationProvider {
  resolveDebugConfiguration(
    folder: vscode.WorkspaceFolder | undefined,
    config: vscode.DebugConfiguration,
  ): vscode.ProviderResult<vscode.DebugConfiguration> {
    const root = debugProjectRoot(folder);
    if (!root) {
      void vscode.window.showErrorMessage("Stasis: open a folder containing stasis.json before debugging.");
      return undefined;
    }
    return {
      ...config,
      type: "stasis",
      request: config.request ?? "launch",
      name: config.name ?? "Debug Stasis",
      stopOnEntry: config.stopOnEntry ?? false,
      __stasisProjectRoot: root,
    };
  }
}

function debugProjectRoot(folder?: vscode.WorkspaceFolder): string | undefined {
  const activeRoot = findWorkspaceRoot(vscode.window.activeTextEditor?.document);
  if (activeRoot && (!folder || pathIsWithin(folder.uri.fsPath, activeRoot))) {
    return activeRoot;
  }
  if (folder && fs.existsSync(path.join(folder.uri.fsPath, "stasis.json"))) {
    return folder.uri.fsPath;
  }
  return findWorkspaceRoot();
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
  try {
    activeToolchainExecutable = await resolveEditorToolchain(
      context.extensionPath,
      configuration().get<string>("developer.executablePath", ""),
    );
    output.appendLine(`Verified editor toolchain: ${activeToolchainExecutable}`);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    output.appendLine(`Toolchain verification failed: ${message}`);
    void vscode.window.showErrorMessage(`Stasis: ${message}`);
    throw error;
  }
  const languageClients = new StasisLanguageClients(output);
  const values = new LiveValuesProvider(
    configuration().get<boolean>("live.filterInactiveCollectionRows", true),
  );
  const controller = new LiveController(
    values,
    output,
    (root) => languageClients.clientForRoot(root),
  );
  const collectionTable = new LiveCollectionTablePanel(values, (visible) => {
    void showCommandError(() => controller.setTableViewVisible(visible));
  });
  const liveValuesView = vscode.window.createTreeView("stasis.liveValues", {
    treeDataProvider: values,
  });
  const tests = new StasisTests(output);
  const debugFactory = new StasisDebugAdapterFactory();
  const debugConfiguration = new StasisDebugConfigurationProvider();
  const command = (name: string, action: (...args: unknown[]) => Promise<void>) =>
    vscode.commands.registerCommand(name, (...args: unknown[]) => showCommandError(() => action(...args)));

  context.subscriptions.push(
    output,
    languageClients,
    controller,
    collectionTable,
    liveValuesView,
    tests,
    vscode.debug.registerDebugAdapterDescriptorFactory("stasis", debugFactory),
    vscode.debug.registerDebugConfigurationProvider("stasis", debugConfiguration),
    liveValuesView.onDidChangeVisibility((event) => {
      void showCommandError(() => controller.setValuesViewVisible(event.visible));
    }),
    vscode.workspace.onDidChangeConfiguration((event) => {
      if (event.affectsConfiguration("stasis.live.refreshEveryTicks")) {
        void showCommandError(() => controller.updateRefreshCadence());
      }
      if (event.affectsConfiguration("stasis.live.filterInactiveCollectionRows")) {
        values.setFilterInactiveRows(
          configuration().get<boolean>("live.filterInactiveCollectionRows", true),
        );
      }
    }),
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
      const livePath = await askForPath("Watch a value while the Stasis game runs");
      if (livePath) {
        await controller.addWatch(livePath.trim());
      }
    }),
    command("stasis.removeWatch", async (item) => {
      if (item instanceof LiveValueItem && item.liveValue) {
        await controller.requireSession().removeWatch(item.liveValue.path);
      }
    }),
    command("stasis.showCollectionAsTable", async (item) => {
      if (item instanceof LiveValueItem && item.node.kind === "collection") {
        collectionTable.show(item.node.path);
      }
    }),
    command("stasis.showCollectionAsTree", async (item) => {
      if (item instanceof LiveValueItem && item.node.kind === "collection") {
        collectionTable.close(item.node.path);
      }
    }),
    command("stasis.refreshLiveValues", async () => controller.refreshLiveValues()),
    command("stasis.showOutput", async () => output.show(true)),
  );
  void controller.setValuesViewVisible(liveValuesView.visible);
  void tests.refresh();

  await languageClients.start();

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
