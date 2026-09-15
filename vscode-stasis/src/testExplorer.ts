import * as fs from "node:fs";
import * as path from "node:path";

export interface TestSourceOffsets {
  start: number;
  end: number;
}

export type PassedTestMatch = "passed" | "ambiguous" | "not_found";

interface TestResultIdentity {
  file: string;
  label: string;
}

function asRecord(value: unknown): Record<string, unknown> | undefined {
  return typeof value === "object" && value !== null
    ? (value as Record<string, unknown>)
    : undefined;
}

function isAbsolutePath(value: string): boolean {
  return path.isAbsolute(value) || /^[A-Za-z]:[\\/]/u.test(value);
}

function normalizePath(value: string): string {
  const normalized = value.replaceAll("\\", "/").replace(/\/+$/u, "");
  return process.platform === "win32" ? normalized.toLowerCase() : normalized;
}

function resolvedPath(value: string, basePath?: string): string {
  if (isAbsolutePath(value)) {
    return canonicalPath(value);
  }
  return canonicalPath(basePath ? path.resolve(basePath, value) : path.resolve(value));
}

function canonicalPath(value: string): string {
  try {
    return fs.realpathSync.native(value);
  } catch {
    return value;
  }
}

function parseTestResultIdentity(value: unknown): TestResultIdentity | undefined {
  if (typeof value !== "string") {
    return undefined;
  }
  const separator = value.indexOf(" :: ");
  if (separator <= 0) {
    return undefined;
  }
  const file = value.slice(0, separator).trim();
  const label = value.slice(separator + " :: ".length);
  return file && label ? { file, label } : undefined;
}

export function hasDuplicateTestLabel(labels: readonly string[], label: string): boolean {
  let matches = 0;
  for (const candidate of labels) {
    if (candidate === label && ++matches > 1) {
      return true;
    }
  }
  return false;
}

/**
 * Match the CLI's full `path :: display name` identity, retaining ambiguity
 * rather than allowing one duplicate declaration to inherit another result.
 */
export function matchPassedTest(
  passedTests: readonly unknown[],
  filePath: string,
  label: string,
  resultBasePath?: string,
): PassedTestMatch {
  const expectedFile = normalizePath(resolvedPath(filePath));
  let matches = 0;
  for (const value of passedTests) {
    const identity = parseTestResultIdentity(value);
    if (!identity || identity.label !== label) {
      continue;
    }
    const resultFile = resolvedPath(identity.file, resultBasePath);
    if (normalizePath(resultFile) === expectedFile) {
      matches += 1;
    }
  }
  return matches === 1 ? "passed" : matches > 1 ? "ambiguous" : "not_found";
}

function isUtf8Boundary(bytes: Buffer, offset: number): boolean {
  return offset === 0 || offset === bytes.length || (bytes[offset]! & 0xc0) !== 0x80;
}

/**
 * Convert a compiler byte span to JavaScript string offsets only when the
 * metadata is a finite, ordered, in-bounds UTF-8 range.
 */
export function validTestSourceOffsets(source: string, span: unknown): TestSourceOffsets | undefined {
  const record = asRecord(span);
  const start = record?.start;
  const end = record?.end;
  if (
    typeof start !== "number" ||
    typeof end !== "number" ||
    !Number.isFinite(start) ||
    !Number.isFinite(end) ||
    !Number.isInteger(start) ||
    !Number.isInteger(end) ||
    start < 0 ||
    end < start
  ) {
    return undefined;
  }

  const bytes = Buffer.from(source, "utf8");
  if (end > bytes.length || !isUtf8Boundary(bytes, start) || !isUtf8Boundary(bytes, end)) {
    return undefined;
  }

  return {
    start: bytes.subarray(0, start).toString("utf8").length,
    end: bytes.subarray(0, end).toString("utf8").length,
  };
}
