import * as vscode from "vscode";
import { LanguageClient } from "vscode-languageclient/node";
import {
  isLiveResponse,
  LiveCollection,
  LiveResponse,
  LiveRuntimeIdentity,
  LiveValue,
} from "./protocol";

export type LiveSessionState = "starting" | "running" | "paused" | "stopped";

const LIVE_GLOBAL_VALUE_LIMIT = 4096;

function record(value: unknown): Record<string, unknown> | undefined {
  return typeof value === "object" && value !== null
    ? (value as Record<string, unknown>)
    : undefined;
}

interface LiveStateNotification {
  state?: unknown;
  detail?: unknown;
}

export class LiveSession implements vscode.Disposable {
  private readonly valuesByPath = new Map<string, LiveValue>();
  private _collections: readonly LiveCollection[] = [];
  private readonly stateEmitter = new vscode.EventEmitter<LiveSessionState>();
  private readonly valuesEmitter = new vscode.EventEmitter<readonly LiveValue[]>();
  private readonly subscriptions: vscode.Disposable[];
  private disposed = false;
  private _state: LiveSessionState = "stopped";
  private _runtimeIdentity: LiveRuntimeIdentity | undefined;

  readonly onDidChangeState = this.stateEmitter.event;
  readonly onDidChangeValues = this.valuesEmitter.event;

  constructor(
    readonly root: string,
    private readonly client: LanguageClient,
    private readonly entry: string,
    private readonly output: vscode.OutputChannel,
  ) {
    this.subscriptions = [
      client.onNotification("stasis/liveEvent", (value: unknown) => {
        if (isLiveResponse(value)) {
          this.acceptResponse(value);
        } else {
          this.output.appendLine(`Ignored invalid LSP live event: ${JSON.stringify(value)}`);
        }
      }),
      client.onNotification("stasis/liveState", (notification: LiveStateNotification) => {
        if (notification.state === "starting") {
          this.setState("starting");
          return;
        }
        if (notification.state === "stopped") {
          this._runtimeIdentity = undefined;
          this.valuesByPath.clear();
          this._collections = [];
          this.emitValues();
          this.setState("stopped");
          if (typeof notification.detail === "string" && notification.detail !== "live Workshop stopped") {
            this.output.appendLine(notification.detail);
          }
        }
      }),
      client.onNotification("stasis/liveLog", (notification: unknown) => {
        const fields = record(notification);
        if (typeof fields?.message === "string") {
          this.output.appendLine(fields.message);
        }
      }),
    ];
  }

  get state(): LiveSessionState {
    return this._state;
  }

  get values(): readonly LiveValue[] {
    return [...this.valuesByPath.values()].sort((left, right) =>
      left.path.localeCompare(right.path),
    );
  }

  get runtimeIdentity(): LiveRuntimeIdentity | undefined {
    return this._runtimeIdentity;
  }

  get collections(): readonly LiveCollection[] {
    return this._collections;
  }

  async start(): Promise<void> {
    if (this._state !== "stopped") {
      return;
    }
    this.setState("starting");
    this.output.appendLine(`Starting LSP-owned play session: ${this.root}`);
    try {
      const response = await this.client.sendRequest<unknown>("stasis/live/start", {
        entry: this.entry.trim() || undefined,
      });
      this.acceptCommandResponse(response);
    } catch (error) {
      this.setState("stopped");
      throw error;
    }
  }

  async request(type: string, fields: Record<string, unknown> = {}): Promise<LiveResponse> {
    if (this._state === "stopped") {
      throw new Error("No Stasis play session is running.");
    }
    const response = await this.client.sendRequest<unknown>("stasis/live/request", {
      type,
      ...fields,
    });
    return this.acceptCommandResponse(response);
  }

  async stop(): Promise<void> {
    if (this._state === "stopped") {
      return;
    }
    const response = await this.client.sendRequest<unknown>("stasis/live/stop", {});
    if (isLiveResponse(response)) {
      this.acceptResponse(response);
    }
    this._runtimeIdentity = undefined;
    this.valuesByPath.clear();
    this._collections = [];
    this.emitValues();
    this.setState("stopped");
  }

  async addWatch(path: string): Promise<void> {
    await this.request("watch", { path });
    await this.request("inspect", { path });
  }

  async removeWatch(path: string): Promise<void> {
    await this.request("unwatch", { path });
    this.valuesByPath.delete(path);
    this.emitValues();
  }

  async refresh(everyTicks?: number): Promise<void> {
    const paths = this.values.filter((value) => value.watched).map((value) => value.path);
    const snapshot = await this.request("inspect_all", {
      limit: LIVE_GLOBAL_VALUE_LIMIT,
      concise: false,
      ...(everyTicks === undefined ? {} : { every_ticks: everyTicks }),
    });
    this.output.appendLine(`Live Values refresh response: ${snapshot.kind} at tick ${snapshot.tick}`);
    for (const path of paths) {
      await this.request("watch", { path });
    }
  }

  async stopRefreshing(): Promise<void> {
    await this.request("inspect_all", { every_ticks: 0 });
    await this.request("unwatch");
    this.output.appendLine("Live Values polling stopped because the view is hidden.");
  }

  dispose(): void {
    this.disposed = true;
    for (const subscription of this.subscriptions) {
      subscription.dispose();
    }
    this.subscriptions.length = 0;
    this.stateEmitter.dispose();
    this.valuesEmitter.dispose();
  }

  private acceptCommandResponse(value: unknown): LiveResponse {
    if (!isLiveResponse(value)) {
      throw new Error("The Stasis LSP returned an invalid live response.");
    }
    this.acceptResponse(value);
    if (!value.ok) {
      throw new Error(value.error ?? `Stasis live request failed: ${value.kind}`);
    }
    return value;
  }

  private acceptResponse(response: LiveResponse): void {
    if (this.disposed) {
      return;
    }
    if (response.runtime_identity) {
      this._runtimeIdentity = response.runtime_identity;
    }
    this.applyStateFromResponse(response);
    this.applyValueFromResponse(response);
  }

  private applyStateFromResponse(response: LiveResponse): void {
    if (response.kind === "paused" || response.kind === "step_scheduled") {
      this.setState("paused");
      return;
    }
    if (response.kind === "resumed") {
      this.setState("running");
      return;
    }
    if (response.kind === "status") {
      this.setState(record(response.data)?.paused === true ? "paused" : "running");
    }
  }

  private applyValueFromResponse(response: LiveResponse): void {
    const data = record(response.data);
    if (!data) {
      return;
    }
    if (["inspection", "watch", "watch_added", "watch_error"].includes(response.kind)) {
      const path = typeof data.path === "string" ? data.path : undefined;
      if (!path) {
        return;
      }
      const existing = this.valuesByPath.get(path);
      this.valuesByPath.set(path, {
        path,
        staticType: typeof data.static_type === "string" ? data.static_type : existing?.staticType,
        value: data.value,
        error: typeof data.error === "string" ? data.error : undefined,
        tick: response.tick,
        watched: response.kind.startsWith("watch") || existing?.watched === true,
      });
      this.emitValues();
      return;
    }
    if (response.kind === "state_inspection" && Array.isArray(data.items)) {
      for (const [path, value] of this.valuesByPath) {
        if (!value.watched) {
          this.valuesByPath.delete(path);
        }
      }
      for (const item of data.items) {
        const fields = record(item);
        if (!fields || typeof fields.path !== "string") {
          continue;
        }
        const existing = this.valuesByPath.get(fields.path);
        this.valuesByPath.set(fields.path, {
          path: fields.path,
          staticType: typeof fields.static_type === "string" ? fields.static_type : undefined,
          value: fields.value,
          tick: response.tick,
          watched: existing?.watched === true,
        });
      }
      this._collections = Array.isArray(data.collections)
        ? data.collections.flatMap((collection): LiveCollection[] => {
            const fields = record(collection);
            if (!fields || typeof fields.path !== "string") {
              return [];
            }
            const collectionFields = Array.isArray(fields.fields)
              ? fields.fields.flatMap((field) => {
                  const value = record(field);
                  return value && typeof value.field === "string" && typeof value.type_name === "string"
                    ? [{ field: value.field, staticType: value.type_name }]
                    : [];
                })
              : [];
            const rows = Array.isArray(fields.rows)
              ? fields.rows.flatMap((row) => {
                  const value = record(row);
                  return value && typeof value.index === "number" && record(value.values)
                    ? [{ index: value.index, values: record(value.values)! }]
                    : [];
                })
              : Array.isArray(fields.row_values)
                ? fields.row_values.flatMap((row, rowOffset) => {
                    if (!Array.isArray(row)) {
                      return [];
                    }
                    const rowStart = typeof fields.row_start === "number" ? fields.row_start : 0;
                    return [{
                      index: rowStart + rowOffset,
                      values: Object.fromEntries(
                        collectionFields.map((field, fieldIndex) => [field.field, row[fieldIndex]]),
                      ),
                    }];
                  })
                : [];
            return [{
              path: fields.path,
              elementShape: typeof fields.element_shape === "string" ? fields.element_shape : "unknown",
              capacity: typeof fields.capacity === "number" ? fields.capacity : rows.length,
              activeCount: typeof fields.active_count === "number" ? fields.active_count : rows.length,
              fields: collectionFields,
              rows,
              rowsTruncated: fields.rows_truncated === true,
              tick: response.tick,
            }];
          })
        : [];
      this.output.appendLine(
        `Live Values snapshot: ${data.items.length} globals, ${this._collections.length} collections at tick ${response.tick}`,
      );
      this.emitValues();
    }
  }

  private emitValues(): void {
    if (!this.disposed) {
      this.valuesEmitter.fire(this.values);
    }
  }

  private setState(state: LiveSessionState): void {
    if (state === this._state) {
      return;
    }
    this._state = state;
    if (!this.disposed) {
      this.stateEmitter.fire(state);
    }
  }
}
