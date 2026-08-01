export interface LiveResponse {
  schema_version: number;
  request_id: number;
  tick: number;
  ok: boolean;
  kind: string;
  data?: unknown;
  error?: string;
  truncated?: boolean;
}

export interface LiveValue {
  path: string;
  staticType?: string;
  value: unknown;
  error?: string;
  tick: number;
  watched: boolean;
}

export interface CompilerCompletion {
  text: string;
  kind: string;
  detail?: string;
  type_name?: string;
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

export function byteOffsetToStringOffset(text: string, byteOffset: number): number {
  if (byteOffset <= 0) {
    return 0;
  }
  let bytes = 0;
  let stringOffset = 0;
  for (const character of text) {
    const width = Buffer.byteLength(character, "utf8");
    if (bytes + width > byteOffset) {
      break;
    }
    bytes += width;
    stringOffset += character.length;
  }
  return stringOffset;
}

export function stringOffsetToByteOffset(text: string, stringOffset: number): number {
  return Buffer.byteLength(text.slice(0, Math.max(0, stringOffset)), "utf8");
}
