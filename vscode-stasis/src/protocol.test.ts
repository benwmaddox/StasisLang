import { strict as assert } from "node:assert";
import { test } from "node:test";
import { displayRuntimeValue, isLiveResponse } from "./protocol";
import {
  parseToolchainCapabilities,
  requireEditorToolchain,
  resolveToolchainExecutable,
} from "./toolchain";
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

test("toolchain capability parsing rejects the pre-LSP command surface", () => {
  assert.deepEqual(parseToolchainCapabilities("Commands:\n  run   Run a project\n  tui   Open the TUI\n"), {
    lsp: false,
    dap: false,
  });
  assert.deepEqual(parseToolchainCapabilities("Commands:\n  lsp   Run the language server\n  dap   Run the debug adapter\n"), {
    lsp: true,
    dap: true,
  });
});

test("explicit and locally packaged toolchains outrank PATH", () => {
  assert.equal(resolveToolchainExecutable(" C:/toolchains/stasis.exe ", "D:/repo/stasis.exe"), "C:/toolchains/stasis.exe");
  assert.equal(resolveToolchainExecutable("", " D:/repo/stasis.exe "), "D:/repo/stasis.exe");
  assert.equal(resolveToolchainExecutable(undefined, undefined), "stasis");
});

test("toolchain probe rejects an executable without editor protocols", async () => {
  await assert.rejects(
    requireEditorToolchain(process.execPath, process.cwd()),
    /missing lsp and dap/,
  );
});
