export interface LiveResponse {
  schema_version: number;
  request_id: number;
  tick: number;
  ok: boolean;
  kind: string;
  data?: unknown;
  error?: string;
  truncated?: boolean;
  runtime_identity?: LiveRuntimeIdentity;
}
export interface LiveRuntimeIdentity {
  session_id: string;
  generation: number;
  source_hashes: Record<string, string>;
  indexed_collections?: Array<{
    path: string;
    fields: Record<string, string>;
  }>;
  complete?: boolean;
}

export interface LiveValue {
  path: string;
  staticType?: string;
  value: unknown;
  error?: string;
  tick: number;
  watched: boolean;
}

export class JsonLineDecoder {
  private buffered = "";

  push(chunk: string): unknown[] {
    this.buffered += chunk;
    const lines = this.buffered.split(/\r?\n/);
    this.buffered = lines.pop() ?? "";
    return lines
      .filter((line) => line.trim().length > 0)
      .map((line) => JSON.parse(line) as unknown);
  }

  finish(): unknown[] {
    const tail = this.buffered.trim();
    this.buffered = "";
    return tail.length === 0 ? [] : [JSON.parse(tail) as unknown];
  }
}

export function isLiveResponse(value: unknown): value is LiveResponse {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const candidate = value as Partial<LiveResponse>;
  return (
    candidate.schema_version === 1 &&
    typeof candidate.request_id === "number" &&
    typeof candidate.tick === "number" &&
    typeof candidate.ok === "boolean" &&
    typeof candidate.kind === "string"
  );
}

export function displayRuntimeValue(value: unknown): string {
  if (
    typeof value === "object" &&
    value !== null &&
    "value" in value &&
    Object.keys(value).every((key) => ["type", "value"].includes(key))
  ) {
    return displayRuntimeValue((value as { value: unknown }).value);
  }
  if (typeof value === "string") {
    return value;
  }
  if (value === null) {
    return "null";
  }
  if (["number", "boolean", "bigint"].includes(typeof value)) {
    return String(value);
  }
  try {
    return JSON.stringify(value);
  } catch {
    return "<value>";
  }
}
