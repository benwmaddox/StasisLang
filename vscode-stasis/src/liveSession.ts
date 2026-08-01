import { ChildProcessWithoutNullStreams, spawn } from "node:child_process";
import * as vscode from "vscode";
import {
  isLiveResponse,
  JsonLineDecoder,
  LiveResponse,
  LiveValue,
} from "./protocol";

export type LiveSessionState = "starting" | "running" | "paused" | "stopped";

interface PendingRequest {
  resolve: (response: LiveResponse) => void;
  reject: (error: Error) => void;
  timeout: NodeJS.Timeout;
}

function record(value: unknown): Record<string, unknown> | undefined {
  return typeof value === "object" && value !== null
    ? (value as Record<string, unknown>)
    : undefined;
}

export class LiveSession implements vscode.Disposable {
  private readonly decoder = new JsonLineDecoder();
  private readonly pending = new Map<number, PendingRequest>();
  private readonly valuesByPath = new Map<string, LiveValue>();
  private readonly stateEmitter = new vscode.EventEmitter<LiveSessionState>();
  private readonly valuesEmitter = new vscode.EventEmitter<readonly LiveValue[]>();
  private process: ChildProcessWithoutNullStreams | undefined;
  private nextRequestId = 1;
  private disposed = false;
  private _state: LiveSessionState = "stopped";

  readonly onDidChangeState = this.stateEmitter.event;
  readonly onDidChangeValues = this.valuesEmitter.event;

  constructor(
    readonly root: string,
    private readonly executable: string,
    private readonly entry: string,
    private readonly output: vscode.OutputChannel,
  ) {}

  get state(): LiveSessionState {
    return this._state;
  }

  get values(): readonly LiveValue[] {
    return [...this.valuesByPath.values()].sort((left, right) =>
      left.path.localeCompare(right.path),
    );
  }

  async start(): Promise<void> {
    if (this.process) {
      return;
    }
    this.setState("starting");
    const args = ["--workspace", this.root, "tui"];
    if (this.entry.trim().length > 0) {
      args.push(this.entry.trim());
    }
    args.push("--live-stdio");
    this.output.appendLine(`Starting: ${this.executable} ${args.join(" ")}`);
    const child = spawn(this.executable, args, {
      cwd: this.root,
      stdio: "pipe",
      windowsHide: true,
    });
    this.process = child;
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk: string) => this.acceptStdout(chunk));
    child.stderr.on("data", (chunk: string) => this.output.append(chunk));
    child.once("error", (error) => this.end(error));
    child.once("exit", (code, signal) => {
      const detail = signal ? `signal ${signal}` : `exit code ${code ?? "unknown"}`;
      this.end(new Error(`Stasis play session ended with ${detail}.`));
    });

    const status = await this.request("status");
    this.applyStateFromResponse(status);
  }

  request(type: string, fields: Record<string, unknown> = {}): Promise<LiveResponse> {
    if (!this.process?.stdin.writable) {
      return Promise.reject(new Error("No Stasis play session is running."));
    }
    const requestId = this.nextRequestId++;
    const request = {
      schema_version: 1,
      request_id: requestId,
      type,
      ...fields,
    };
    return new Promise<LiveResponse>((resolve, reject) => {
      const timeout = setTimeout(() => {
        this.pending.delete(requestId);
        reject(new Error(`Stasis live request ${requestId} timed out.`));
      }, 300_000);
      this.pending.set(requestId, { resolve, reject, timeout });
      this.process?.stdin.write(`${JSON.stringify(request)}\n`, (error) => {
        if (!error) {
          return;
        }
        const pending = this.pending.get(requestId);
        if (pending) {
          clearTimeout(pending.timeout);
          this.pending.delete(requestId);
          pending.reject(error);
        }
      });
    });
  }

  async stop(): Promise<void> {
    const child = this.process;
    if (!child) {
      return;
    }
    try {
      await this.request("quit");
    } catch (error) {
      this.output.appendLine(`Graceful stop failed: ${String(error)}`);
      child.kill();
    }
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

  async refresh(): Promise<void> {
    const paths = this.values.filter((value) => value.watched).map((value) => value.path);
    for (const path of paths) {
      await this.request("inspect", { path });
    }
  }

  dispose(): void {
    this.disposed = true;
    this.process?.kill();
    this.end(new Error("Stasis extension disposed."));
    this.stateEmitter.dispose();
    this.valuesEmitter.dispose();
  }

  private acceptStdout(chunk: string): void {
    try {
      for (const decoded of this.decoder.push(chunk)) {
        if (!isLiveResponse(decoded)) {
          this.output.appendLine(`Ignored non-protocol stdout: ${JSON.stringify(decoded)}`);
          continue;
        }
        this.acceptResponse(decoded);
      }
    } catch (error) {
      this.output.appendLine(`Invalid live JSON response: ${String(error)}`);
    }
  }

  private acceptResponse(response: LiveResponse): void {
    this.applyStateFromResponse(response);
    this.applyValueFromResponse(response);
    if (response.request_id === 0 || ["completion_preparing", "edit_preparing"].includes(response.kind)) {
      return;
    }
    const pending = this.pending.get(response.request_id);
    if (!pending) {
      this.output.appendLine(`Unmatched live response #${response.request_id}: ${response.kind}`);
      return;
    }
    clearTimeout(pending.timeout);
    this.pending.delete(response.request_id);
    if (response.ok) {
      pending.resolve(response);
    } else {
      pending.reject(new Error(response.error ?? `Stasis live request failed: ${response.kind}`));
    }
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
      this.emitValues();
    }
  }

  private emitValues(): void {
    this.valuesEmitter.fire(this.values);
  }

  private setState(state: LiveSessionState): void {
    if (state === this._state) {
      return;
    }
    this._state = state;
    this.stateEmitter.fire(state);
  }

  private end(error: Error): void {
    if (!this.process && this._state === "stopped") {
      return;
    }
    this.process = undefined;
    this.setState("stopped");
    for (const pending of this.pending.values()) {
      clearTimeout(pending.timeout);
      pending.reject(error);
    }
    this.pending.clear();
    if (!this.disposed && !error.message.includes("exit code 0")) {
      this.output.appendLine(error.message);
    }
  }
}
