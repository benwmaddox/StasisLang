import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";
import { fakeWebGL2 } from "./fake_webgl2.mjs";

const source = fs.readFileSync(new URL("../game.js", import.meta.url), "utf8");
const LAYOUT_DIGEST = "a".repeat(64);
const IMPORTS = ["print_i32", "reject_code_swap", "sin_fast"];

function metadata({ imports = IMPORTS, layoutDigest = LAYOUT_DIGEST } = {}) {
  const config = {
    strings: {}, memory: {}, assets: {},
    collectionViewAbiVersion: 2,
    renderContractVersion: 7,
    renderConstructionLifecycleVersion: 0,
    spriteAtlasPageSize: 512,
    host_exports: { abi_version: 1, functions: [] },
  };
  return {
    schemaVersion: 1,
    config,
    imports: imports.slice().sort(),
    entrypoints: {
      main: { parameters: [], returnType: "i32" },
      tick: { parameters: [], returnType: "i32" },
      render: { parameters: [], returnType: "i32" },
      on_code_swap: { parameters: [], returnType: "void" },
    },
    layoutDigest,
    stateLayout: {},
    snapshotSupported: true,
    replayCompatibility: {
      state_snapshot: {
        support: "canonical_bytes",
        required_bytes: 4,
        size_operation: "stasis_replay_state_snapshot_size",
        write_operation: "stasis_replay_state_snapshot_write",
        restore_operation: "stasis_replay_state_snapshot_restore",
      },
    },
    hookPresent: true,
    provenance: { compiler: "runtime-test" },
  };
}

function fakeWasm(specs) {
  class Module {
    constructor(bytes) {
      this.spec = specs.get(new Uint8Array(bytes)[0]);
      if (!this.spec) throw new Error("unknown fake Wasm module");
    }
    static imports(module) { return module.spec.imports.map(name => ({ module: "env", name, kind: "function" })); }
    static exports() {
      return ["memory", "__stasis_collection_view_abi_version", "main", "tick", "render", "on_code_swap",
        "stasis_replay_state_snapshot_size", "stasis_replay_state_snapshot_write", "stasis_replay_state_snapshot_restore"]
        .map((name, index) => ({ name, kind: index === 0 ? "memory" : index === 1 ? "global" : "function" }));
    }
  }
  return {
    Module,
    Global: WebAssembly.Global,
    Memory: WebAssembly.Memory,
    compile: async bytes => new Module(bytes),
    instantiate: async (module, imports) => {
      const spec = module.spec;
      const memory = new WebAssembly.Memory({ initial: 1 });
      const getState = () => new DataView(memory.buffer).getInt32(0, true);
      const setState = value => new DataView(memory.buffer).setInt32(0, value | 0, true);
      const exports = {
        memory,
        __stasis_collection_view_abi_version: new WebAssembly.Global({ value: "i32", mutable: false }, 2),
        main() {
          if (spec.mainValue !== undefined) setState(spec.mainValue);
          if (spec.mainDelta) setState(getState() + spec.mainDelta);
          imports.env.sin_fast(0);
          return 0;
        },
        tick() { setState(getState() + 1); imports.env.print_i32(getState()); return 0; },
        render() { return 0; },
        on_code_swap() {
          setState(getState() + (spec.hookDelta || 0));
          if (spec.hookEffect === "reject") imports.env.reject_code_swap();
          if (spec.hookEffect === "print") imports.env.print_i32(7);
        },
        stasis_replay_state_snapshot_size() { return 4; },
        stasis_replay_state_snapshot_write(pointer, capacity) {
          if (capacity !== 4) return -1;
          new DataView(memory.buffer).setInt32(pointer, getState(), true);
          return 4;
        },
        stasis_replay_state_snapshot_restore(pointer, length) {
          if (length !== 4) return -1;
          setState(new DataView(memory.buffer).getInt32(pointer, true));
          return 4;
        },
      };
      return { instance: { exports } };
    },
  };
}

async function loadRuntime() {
  const specs = new Map([
    [1, { imports: IMPORTS, mainValue: 7 }],
    [2, { imports: IMPORTS, mainDelta: 1000, hookDelta: 100 }],
    [3, { imports: IMPORTS, hookDelta: 100, hookEffect: "reject" }],
    [4, { imports: IMPORTS, hookDelta: 100, hookEffect: "print" }],
    [5, { imports: ["sin_fast", "storage_save_i32"] }],
    [6, { imports: ["print_i32", "print_i32", "reject_code_swap", "sin_fast"] }],
  ]);
  const frameCallbacks = [];
  const logs = [];
  const memory = new WebAssembly.Memory({ initial: 1 });
  const canvas = {
    width: 640, height: 360, dataset: {}, style: {}, parentElement: { style: {} },
    getContext: kind => kind === "webgl2" ? fakeWebGL2() : { fillRect() {}, fillText() {}, drawImage() {}, measureText: () => ({ width: 0 }) },
    getBoundingClientRect: () => ({ left: 0, top: 0, width: 640, height: 360 }),
    addEventListener() {}, setPointerCapture() {}, focus() {}, requestFullscreen: async () => {},
  };
  const loading = { dataset: {} };
  const document = {
    body: { dataset: {} }, hidden: false, fullscreenElement: null, hasFocus: () => true,
    fonts: { ready: Promise.resolve(), add() {}, delete() {} },
    getElementById(id) {
      if (id === "stasis-canvas") return canvas;
      if (id === "stasis-hud") return { textContent: "" };
      if (id === "stasis-loading") return loading;
      if (id === "stasis-loading-status") return { textContent: "" };
      if (id === "stasis-error") return { textContent: "" };
      if (id === "stasis-audio") return { addEventListener() {}, disabled: false };
      return null;
    },
    addEventListener() {},
    createElement: () => ({ width: 0, height: 0, getContext: () => ({ fillRect() {}, fillText() {}, drawImage() {}, measureText: () => ({ width: 0 }) }) }),
  };
  const initialMetadata = metadata();
  let fetches = 0;
  const window = {
    STASIS_PLAYGROUND_BOOT: { wasmBytes: Uint8Array.of(1), layoutDigest: LAYOUT_DIGEST, metadata: initialMetadata },
    screen: { width: 640, height: 360 },
  };
  const context = {
    document, window, screen: window.screen, devicePixelRatio: 1,
    performance: { now: () => 0 },
    WebAssembly: fakeWasm(specs),
    fetch: async () => { fetches += 1; return { ok: false, status: 404 }; },
    requestAnimationFrame: callback => { frameCallbacks.push(callback); return frameCallbacks.length; },
    cancelAnimationFrame() {}, addEventListener() {}, console: { log: value => logs.push(value) },
    Image: class { addEventListener() {} decode() { return Promise.resolve(); } set src(value) { this.value = value; } get src() { return this.value; } },
    FontFace: class { load() { return Promise.resolve(this); } },
    AudioContext: class { constructor() { this.state = "running"; this.currentTime = 0; this.destination = {}; } close() {} resume() {} },
    TextDecoder, TextEncoder, setTimeout, clearTimeout,
  };
  context.addEventListener = () => {};
  vm.runInNewContext(source, context, { filename: "runtime/web/game.js" });
  await context.window.STASIS_RUNTIME_PROMISE;
  await context.window.STASIS_PLAYGROUND.ready;
  const api = context.window.STASIS_PLAYGROUND;
  const runFrame = async () => {
    const callback = frameCallbacks.shift();
    assert.equal(typeof callback, "function", "a frame should be queued");
    callback(0);
    await new Promise(resolve => setImmediate(resolve));
  };
  const request = (moduleId, metadataValue = initialMetadata) => ({
    wasmBytes: Uint8Array.of(moduleId),
    config: metadataValue.config,
    layoutDigest: metadataValue.layoutDigest,
    metadata: metadataValue,
  });
  return { api, document, logs, fetchCount: () => fetches, runFrame, request, makeMetadata: metadata };
}

async function stageSwap(runtime, request) {
  const promise = runtime.api.swap(request);
  const assertion = assert.doesNotReject(promise);
  await new Promise(resolve => setImmediate(resolve));
  await runtime.runFrame();
  await assertion;
  return promise;
}

const readState = snapshot => new DataView(snapshot.bytes.buffer, snapshot.bytes.byteOffset, snapshot.bytes.byteLength).getInt32(0, true);

test("playground boots from bytes and atomically restores state before swapping", async () => {
  const runtime = await loadRuntime();
  assert.equal(runtime.fetchCount(), 0, "playground boot must not fetch a packaged Wasm URL");
  assert.equal(readState(runtime.api.snapshot()), 7);
  runtime.api.pause(true);
  const committed = await stageSwap(runtime, runtime.request(2));
  assert.equal(committed.generation, 1);
  assert.equal(committed.layoutDigest, LAYOUT_DIGEST);
  assert.equal(runtime.api.generation, 1);
  assert.equal(readState(runtime.api.snapshot()), 107, "candidate hook runs after canonical restore");
  const stepped = runtime.api.step();
  await runtime.runFrame();
  const stepResult = await stepped;
  assert.equal(stepResult.stepped, true);
  assert.equal(stepResult.generation, 1);
  assert.equal(readState(runtime.api.snapshot()), 108, "paused stepping advances exactly one tick");
  assert.deepEqual(runtime.logs, [108], "published candidates regain the regular host import bridge");
});

test("failed or effectful swap hooks leave the active state and generation untouched", async () => {
  for (const moduleId of [3, 4]) {
    const runtime = await loadRuntime();
    runtime.api.pause(true);
    const failed = runtime.api.swap(runtime.request(moduleId));
    const failure = assert.rejects(failed, /candidate rejected code swap|blocked during hook/);
    await new Promise(resolve => setImmediate(resolve));
    await runtime.runFrame();
    await failure;
    assert.equal(runtime.api.generation, 0);
    assert.equal(readState(runtime.api.snapshot()), 7, "rejected candidate writes stay isolated");
  }
});

test("playground rejects changed layouts and imports outside its local host allowlist", async () => {
  const runtime = await loadRuntime();
  const mismatch = runtime.makeMetadata({ layoutDigest: "b".repeat(64) });
  await assert.rejects(runtime.api.swap(runtime.request(2, mismatch)), /exact layout digest/);
  const unsafe = runtime.makeMetadata({ imports: ["sin_fast", "storage_save_i32"] });
  await assert.rejects(runtime.api.swap(runtime.request(5, unsafe)), /not available in the browser sandbox/);
  assert.equal(runtime.api.generation, 0);
  assert.equal(readState(runtime.api.snapshot()), 7);
});

test("playground compares imported symbols as a set when Wasm repeats an import entry", async () => {
  const runtime = await loadRuntime();
  runtime.api.pause(true);
  const committed = await stageSwap(runtime, runtime.request(6));
  assert.equal(committed.generation, 1);
  assert.equal(runtime.api.generation, 1);
  assert.equal(readState(runtime.api.snapshot()), 7);
});
