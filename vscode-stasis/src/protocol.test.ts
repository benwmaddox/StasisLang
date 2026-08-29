import { strict as assert } from "node:assert";
import { createHash } from "node:crypto";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { test } from "node:test";
import { displayRuntimeValue, isLiveResponse } from "./protocol";
import { verifyPackagedToolchain } from "./toolchain";
test("live response validation rejects unrelated stdout", () => {
  assert.equal(
    isLiveResponse({ schema_version: 1, request_id: 1, tick: 0, ok: true, kind: "status" }),
    true,
  );
  assert.equal(isLiveResponse({ kind: "status" }), false);
});

function loadJsonl(relativePath: string): Array<{ case: string; expect: string; payload: unknown }> {
  return fs
    .readFileSync(path.resolve(__dirname, "..", "..", relativePath), "utf8")
    .trim()
    .split("\n")
    .map(line => JSON.parse(line) as { case: string; expect: string; payload: unknown });
}

const LIVE_COMMAND_TYPES = new Set([
  "help", "status", "pause", "resume", "step", "capture_frame", "set_input_state",
  "cancel", "quit", "symbols", "read", "references", "diagnostics", "hover",
  "definition", "organize_imports", "quick_fixes", "inlay_hints", "call_hierarchy",
  "type_hierarchy", "rename_preview", "validate", "validation_snapshot",
  "validation_reinitialize", "validation_restore", "validation_clear", "complete",
  "palette", "edit", "edit_batch", "preview", "apply", "changes", "undo", "redo",
  "inspect", "inspect_all", "watch", "unwatch", "set", "print", "evaluate", "do",
  "cell_put", "cell_run", "cell_list", "cell_clear", "cell_persist",
]);

function isLiveRequestShape(value: unknown): boolean {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const candidate = value as Record<string, unknown>;
  return (
    typeof candidate.schema_version === "number" && Number.isInteger(candidate.schema_version) &&
    typeof candidate.request_id === "number" && Number.isInteger(candidate.request_id) &&
    typeof candidate.type === "string" && LIVE_COMMAND_TYPES.has(candidate.type)
  );
}

test("shared live protocol fixtures agree on request shape acceptance", () => {
  for (const record of loadJsonl("tests/characterization/live_protocol/v1/requests.jsonl")) {
    assert.equal(
      isLiveRequestShape(record.payload),
      true,
      `${record.case} must be accepted as JSON shape before Rust semantics`,
    );
  }
  for (const record of loadJsonl("tests/characterization/live_protocol/v1/malformed.jsonl")) {
    if (record.case.startsWith("request_")) {
      assert.equal(isLiveRequestShape(record.payload), false, `${record.case} shape`);
    }
  }
});

test("shared live protocol response fixtures agree on valid and malformed shapes", () => {
  for (const record of loadJsonl("tests/characterization/live_protocol/v1/responses.jsonl")) {
    assert.equal(isLiveResponse(record.payload), true, `${record.case} response shape`);
  }
  for (const record of loadJsonl("tests/characterization/live_protocol/v1/malformed.jsonl")) {
    if (record.case.startsWith("response_")) {
      assert.equal(isLiveResponse(record.payload), false, `${record.case} response shape`);
    }
  }
});

test("runtime values display scalars without protocol wrappers", () => {
  assert.equal(displayRuntimeValue({ type: "i32", value: 42 }), "42");
  assert.equal(displayRuntimeValue({ hp: 4, active: true }), '{"hp":4,"active":true}');
});

test("packaged toolchain is mandatory", async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "stasis-vsix-missing-"));
  try {
    await assert.rejects(
      verifyPackagedToolchain(root),
      /does not contain its pinned toolchain/,
    );
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("packaged toolchain paths cannot escape the immutable bundle", async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "stasis-vsix-traversal-"));
  try {
    fs.mkdirSync(path.join(root, "dist", "toolchain"), { recursive: true });
    fs.writeFileSync(
      path.join(root, "dist", "toolchain-manifest.json"),
      JSON.stringify({
        schema: 1,
        executable: "../stasis",
        identity: { schema: 1 },
        files: [
          { path: "../stasis", sha256: "0".repeat(64), role: "executable" },
          { path: "../graphics", sha256: "0".repeat(64), role: "graphics_runtime" },
        ],
      }),
    );
    await assert.rejects(
      verifyPackagedToolchain(root),
      /path escapes its bundle/,
    );
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("packaged toolchain rejects changed binaries before launch", async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "stasis-vsix-hash-"));
  try {
    const bundle = path.join(root, "dist", "toolchain");
    fs.mkdirSync(bundle, { recursive: true });
    fs.writeFileSync(path.join(bundle, "stasis"), "changed");
    fs.writeFileSync(path.join(bundle, "graphics"), "changed");
    fs.writeFileSync(
      path.join(root, "dist", "toolchain-manifest.json"),
      JSON.stringify({
        schema: 1,
        executable: "stasis",
        identity: { schema: 1 },
        files: [
          { path: "stasis", sha256: "0".repeat(64), role: "executable" },
          { path: "graphics", sha256: "0".repeat(64), role: "graphics_runtime" },
        ],
      }),
    );
    await assert.rejects(
      verifyPackagedToolchain(root),
      /toolchain is corrupt: stasis/,
    );
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("packaged toolchain rejects changed support files before launch", async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "stasis-vsix-support-hash-"));
  try {
    const bundle = path.join(root, "dist", "toolchain");
    fs.mkdirSync(bundle, { recursive: true });
    fs.writeFileSync(path.join(bundle, "stasis"), "executable");
    fs.writeFileSync(path.join(bundle, "graphics"), "runtime");
    fs.writeFileSync(path.join(bundle, "stdlib.stasis"), "changed");
    const sha256 = (value: string) => createHash("sha256").update(value).digest("hex");
    fs.writeFileSync(
      path.join(root, "dist", "toolchain-manifest.json"),
      JSON.stringify({
        schema: 1,
        executable: "stasis",
        identity: { schema: 1 },
        files: [
          { path: "stdlib.stasis", sha256: "0".repeat(64), role: "support" },
          { path: "stasis", sha256: sha256("executable"), role: "executable" },
          { path: "graphics", sha256: sha256("runtime"), role: "graphics_runtime" },
        ],
      }),
    );
    await assert.rejects(
      verifyPackagedToolchain(root),
      /toolchain is corrupt: stdlib.stasis/,
    );
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});
