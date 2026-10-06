import assert from "node:assert/strict";
import test from "node:test";
import fs from "node:fs";
import vm from "node:vm";
import { fakeWebGL2 } from "./fake_webgl2.mjs";
import { installCollectionViewAbi } from "./collection_view_abi.mjs";

const source = fs.readFileSync(new URL("../game.js", import.meta.url), "utf8");

async function loadRuntime(initialStorage = new Map()) {
  const storage = new Map(initialStorage);
  let imports;
  const memory = new WebAssembly.Memory({ initial: 1 });
  const canvas = {
    width: 640, height: 360, style: {}, parentElement: { style: {} },
    getContext: kind => kind === "webgl2" ? fakeWebGL2() : ({ fillRect() {}, fillText() {}, save() {}, restore() {}, beginPath() {},
      moveTo() {}, lineTo() {}, stroke() {}, drawImage() {}, translate() {}, rotate() {} }),
    getBoundingClientRect: () => ({ left: 0, top: 0, width: 640, height: 360 }),
    addEventListener() {}, setPointerCapture() {}, focus() {}, requestFullscreen: async () => {},
  };
  const localStorage = {
    getItem(key) { return storage.has(key) ? storage.get(key) : null; },
    setItem(key, value) { storage.set(key, String(value)); },
  };
  const body = { dataset: {} };
  const document = {
    body, hidden: false, fullscreenElement: null,
    fonts: { ready: Promise.resolve(), add() {} }, hasFocus: () => true,
    getElementById(id) {
      if (id === "stasis-canvas") return canvas;
      if (id === "stasis-hud") return { textContent: "" };
      if (id === "stasis-error") return { textContent: "" };
      if (id === "stasis-audio") return { addEventListener() {}, disabled: false, textContent: "" };
      if (id === "stasis-loading") return { dataset: {}, textContent: "" };
      if (id === "stasis-loading-status") return { textContent: "" };
      return null;
    },
    addEventListener() {}, createElement: () => ({ getContext: () => ({}) }),
  };
  const instance = { exports: { memory, main: () => 0, tick: () => 0, render: () => 0 } };
  const game = {
    strings: { "1": "project-a", "2": "score", "3": "project-b", "4": "bad/key" },
    memory: {
      payload: { hash: 7, handle: 7007, offset: 0, length: 8, stride: 1, byte_backed: true, type_id: 5 },
    },
    assets: {},
  };
  installCollectionViewAbi(game, instance.exports);
  const context = {
    document,
    window: {
      STASIS_GAME: game,
    },
    localStorage,
    location: { origin: "https://example.test", protocol: "https:", host: "example.test", hash: "" },
    crypto: { getRandomValues(bytes) { bytes.fill(0x2a); return bytes; } },
    WebAssembly: {
      Memory: WebAssembly.Memory, Global: WebAssembly.Global,
      instantiate: async (_bytes, values) => { imports = values.env; return { instance }; },
    },
    fetch: async () => ({ ok: true, arrayBuffer: async () => new ArrayBuffer(0) }),
    requestAnimationFrame() { return 1; }, cancelAnimationFrame() {},
    addEventListener() {}, console, performance: { now: () => 0 }, screen: { width: 640, height: 360 },
    Image: class {}, FontFace: class { load() { return Promise.resolve(this); } },
    AudioContext: class { constructor() { this.state = "running"; this.currentTime = 0; this.destination = {}; } close() {} resume() {} },
    TextDecoder, TextEncoder, URLSearchParams, setTimeout, clearTimeout,
    STASIS_CHARACTERIZATION_TEST: true,
  };
  vm.runInNewContext(source, context, { filename: "runtime/web/game.js" });
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));
  await context.window.STASIS_RUNTIME_PROMISE;
  return { context, env: context.window.__STASIS_CHARACTERIZATION__, imports, storage, memory, body };
}

test("web storage persists by project/key and keeps projects isolated", async () => {
  const runtime = await loadRuntime();
  const { env, imports, storage } = runtime;
  assert.equal(env.storageKey(1, 2), "stasis:project-a:score");
  assert.equal(env.storageKey(3, 2), "stasis:project-b:score");
  const separatorKey = env.storageKey(1, 4);
  assert.equal(separatorKey, "stasis:project-a:bad/key");
  assert.match(separatorKey, /^stasis:/, "browser keys stay in the runtime namespace");
  env.storageSet(env.storageKey(1, 2), "42");
  env.storageSet(env.storageKey(3, 2), "7");
  env.storageSet(separatorKey, "safe");
  assert.equal(env.storageGet(env.storageKey(1, 2)), "42");
  assert.equal(env.storageGet(env.storageKey(3, 2)), "7");
  assert.equal(imports.storage_load_i32(1, 2, 0), 42);
  assert.equal(imports.storage_load_i32(3, 2, 0), 7);
  assert.equal(storage.size, 3);
  assert.notEqual(env.storageKey(1, 2), env.storageKey(3, 2));
  assert.equal(env.storageGet(env.storageKey(3, 4)), null, "separator-bearing keys stay project-scoped");
});

test("web storage load uses the runtime fallback for missing and corrupt values", async () => {
  const runtime = await loadRuntime();
  const { env, imports } = runtime;
  env.storageSet(env.storageKey(1, 2), "not-an-integer");
  assert.equal(imports.storage_load_i32(1, 2, 19), 19);
  assert.equal(imports.storage_load_i32(1, 99, 23), 23);
  env.storageSet(env.storageKey(1, 2), "-12");
  assert.equal(imports.storage_load_i32(1, 2, 19), -12);
});

test("web storage saves printable ASCII atomically", async () => {
  const runtime = await loadRuntime();
  const { env, imports, storage, memory } = runtime;
  const existingKey = env.storageKey(1, 2);
  const missingKey = env.storageKey(3, 2);
  const bytes = new Uint8Array(memory.buffer);
  const save = (scope, key, reference, length) =>
    imports.stasis_jit_storage_save_ascii(scope, key, reference, length);

  env.storageSet(existingKey, "preserved");
  for (const byte of [0, 21, 31, 127, 128, 233]) {
    bytes.fill(0);
    bytes[0] = byte;
    assert.equal(save(1, 2, 7007, 1), 0, `reject byte ${byte}`);
    assert.equal(env.storageGet(existingKey), "preserved", `keep existing value for byte ${byte}`);
    assert.equal(save(3, 2, 7007, 1), 0, `reject byte ${byte} for missing key`);
    assert.equal(storage.has(missingKey), false, `do not create a key for byte ${byte}`);
  }

  for (const [text, payload] of [[" ", [32]], ["~", [126]], [" ~", [32, 126]], ["", []]]) {
    bytes.fill(0);
    bytes.set(payload);
    assert.equal(save(1, 2, 7007, payload.length), 1);
    assert.equal(env.storageGet(existingKey), text);
    assert.equal(imports.stasis_jit_storage_load_ascii(1, 2, 7007, 8), payload.length);
    assert.equal(String.fromCharCode(...bytes.slice(0, payload.length)), text);
  }
  assert.equal(save(3, 2, 7007, 0), 1, "save a valid empty value for a missing key");
  assert.equal(storage.has(missingKey), true);
  assert.equal(env.storageGet(missingKey), "");

  env.storageSet(existingKey, "malformed-preserved");
  storage.delete(missingKey);
  for (const [reference, length] of [[9999, 0], [7007, -1], [7007, 9], [7007, 1.5], [7007, Number.NaN]]) {
    assert.equal(save(1, 2, reference, length), 0, `reject malformed span ${reference}/${length}`);
    assert.equal(env.storageGet(existingKey), "malformed-preserved");
    assert.equal(save(3, 2, reference, length), 0, `reject malformed span for missing key ${reference}/${length}`);
    assert.equal(storage.has(missingKey), false);
  }
});

test("web storage keeps a volatile fallback when browser storage is denied", async () => {
  const runtime = await loadRuntime();
  const { env, context } = runtime;
  context.localStorage.getItem = () => { throw new Error("storage denied"); };
  context.localStorage.setItem = () => { throw new Error("storage denied"); };
  assert.equal(env.storageSet(env.storageKey(1, 4), "ephemeral"), 1);
  assert.equal(env.storageGet(env.storageKey(1, 4)), "ephemeral");
  assert.equal(env.storageGet(env.storageKey(3, 4)), null);
});
