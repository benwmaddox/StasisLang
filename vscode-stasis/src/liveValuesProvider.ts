import * as vscode from "vscode";
import { LiveSessionState } from "./liveSession";
import { buildLiveValueTree, LiveValueTreeNode } from "./liveValueTree";
import { displayRuntimeValue, LiveCollection, LiveValue } from "./protocol";

export class LiveValueItem extends vscode.TreeItem {
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
      this.description = `${collection.elementShape} [${collection.activeCount}/${collection.capacity}${truncated}${filtered}] · ${table ? "table" : "tree"}`;
      this.tooltip = `${collection.path}\n${collection.fields.map((field) => `${field.field || "value"}: ${field.staticType}`).join("\n")}`;
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

export class LiveValuesProvider implements vscode.TreeDataProvider<vscode.TreeItem> {
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
    status.iconPath = new vscode.ThemeIcon(this.state === "stopped" ? "debug-stop" : "pulse");
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
