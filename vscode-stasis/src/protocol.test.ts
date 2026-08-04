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
