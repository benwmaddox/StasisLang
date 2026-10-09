import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";
import { fakeWebGL2 } from "./fake_webgl2.mjs";

const source = fs.readFileSync(new URL("../game.js", import.meta.url), "utf8");
const LAYOUT_DIGEST = "a".repeat(64);
const IMPORTS = ["print_i32", "reject_code_swap", "sin_fast"];

function canonicalSnapshotDescriptor({ requiredBytes = 4, path = "vendor/stasis/stdlib/host_frame.stasis", offset = 0, bytes = 4 } = {}) {
  return {
    support: "canonical_bytes",
    required_bytes: requiredBytes,
    size_operation: "stasis_replay_state_snapshot_size",
    write_operation: "stasis_replay_state_snapshot_write",
    restore_operation: "stasis_replay_state_snapshot_restore",
    entries: [{ path, offset, bytes }],
  };
}

function metadata({ imports = IMPORTS, layoutDigest = LAYOUT_DIGEST, snapshotDescriptor = null } = {}) {
  const config = {
    strings: {},
    memory: {
      host_i32: { offset: 1024, length: 768 },
      host_f32: { offset: 4096, length: 64 },
    },
    assets: {},
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
      state_snapshot: snapshotDescriptor ?? canonicalSnapshotDescriptor(),
    },
    hookPresent: true,
    provenance: { compiler: "runtime-test" },
  };
}

function fakeWasm(specs, counters) {
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
        tick() {
          counters.ticks += 1;
          const hostI32 = new Int32Array(memory.buffer, 1024, 768);
          counters.hostFrames.push({ timeMs: hostI32[0], tickHz: hostI32[16], timeUs: hostI32[19] });
          counters.pointerFrames.push({ pointerCount: hostI32[7], isDown: hostI32[545], wentDown: hostI32[546] });
          setState(getState() + 1);
          imports.env.print_i32(getState());
          return 0;
        },
        render() { counters.renders += 1; return 0; },
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
  const counters = { ticks: 0, renders: 0, hostFrames: [], pointerFrames: [] };
  const memory = new WebAssembly.Memory({ initial: 1 });
  const canvasListeners = new Map();
  const canvas = {
    width: 640, height: 360, dataset: {}, style: {}, parentElement: { style: {} },
    getContext: kind => kind === "webgl2" ? fakeWebGL2() : { fillRect() {}, fillText() {}, drawImage() {}, measureText: () => ({ width: 0 }) },
    getBoundingClientRect: () => ({ left: 0, top: 0, width: 640, height: 360 }),
    addEventListener(type, callback) {
      if (!canvasListeners.has(type)) canvasListeners.set(type, []);
      canvasListeners.get(type).push(callback);
    },
    setPointerCapture() {}, focus() {}, requestFullscreen: async () => {},
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
    WebAssembly: fakeWasm(specs, counters),
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
  const runFrame = async (timestamp = 0) => {
    const callback = frameCallbacks.shift();
    assert.equal(typeof callback, "function", "a frame should be queued");
    callback(timestamp);
    await new Promise(resolve => setImmediate(resolve));
  };
  const dispatchCanvas = (type, event = {}) => {
    for (const callback of canvasListeners.get(type) || []) callback(event);
  };
  const request = (moduleId, metadataValue = initialMetadata) => ({
    wasmBytes: Uint8Array.of(moduleId),
    config: metadataValue.config,
    layoutDigest: metadataValue.layoutDigest,
    metadata: metadataValue,
  });
  return {
    api, document, logs, fetchCount: () => fetches, runFrame, request, makeMetadata: metadata,
    tickCount: () => counters.ticks, renderCount: () => counters.renders,
    hostFrames: () => counters.hostFrames,
    pointerFrames: () => counters.pointerFrames, dispatchCanvas,
  };
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
  const candidateMetadata = runtime.makeMetadata();
  const descriptor = candidateMetadata.replayCompatibility.state_snapshot;
  candidateMetadata.replayCompatibility.state_snapshot = {
    entries: descriptor.entries.map(({ path, offset, bytes }) => ({ bytes, offset, path })),
    restore_operation: descriptor.restore_operation,
    write_operation: descriptor.write_operation,
    size_operation: descriptor.size_operation,
    required_bytes: descriptor.required_bytes,
    support: descriptor.support,
  };
  const committed = await stageSwap(runtime, runtime.request(2, candidateMetadata));
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
  assert.equal(runtime.tickCount(), 1, "one paused step runs exactly one simulation tick");
  assert.equal(runtime.renderCount(), 1, "one paused step runs exactly one render");
  assert.deepEqual(runtime.logs, [108], "published candidates regain the regular host import bridge");
});

test("playground runs fixed 60 Hz ticks and caps rendering at display cadence", async () => {
  for (const { refreshRate, expectedRenders } of [
    { refreshRate: 60, expectedRenders: 60 },
    { refreshRate: 120, expectedRenders: 60 },
    { refreshRate: 30, expectedRenders: 30 },
  ]) {
    const runtime = await loadRuntime();
    await runtime.runFrame(0);
    const startingTicks = runtime.tickCount();
    const startingRenders = runtime.renderCount();
    for (let frame = 1; frame <= refreshRate; frame += 1) {
      await runtime.runFrame(frame * (1000 / refreshRate));
    }
    assert.equal(runtime.tickCount() - startingTicks, 60, `${refreshRate} Hz callbacks should advance 60 simulation ticks per second`);
    assert.equal(runtime.renderCount() - startingRenders, expectedRenders, `${refreshRate} Hz callbacks should render at the lesser of display rate and 60 Hz`);
  }
});

test("playground renders through 60 Hz callback jitter while ticks stay fixed", async () => {
  const runtime = await loadRuntime();
  await runtime.runFrame(0);
  const startingTicks = runtime.tickCount();
  const startingRenders = runtime.renderCount();
  let timestamp = 0;
  for (let frame = 0; frame < 60; frame += 1) {
    timestamp += frame % 2 === 0 ? 16 : (1000 / 60) + (2 / 3);
    await runtime.runFrame(timestamp);
  }
  assert.equal(runtime.tickCount() - startingTicks, 60, "jittering 60 Hz callbacks still advance 60 simulation ticks");
  assert.equal(runtime.renderCount() - startingRenders, 60, "presentation does not skip callbacks that precede a tick deadline by less than one millisecond");
});

test("render-only playground callbacks preserve pointer edges until a simulation tick", async () => {
  const runtime = await loadRuntime();
  await runtime.runFrame(0);
  const startingTicks = runtime.tickCount();
  const startingRenders = runtime.renderCount();
  const startingPointerFrames = runtime.pointerFrames().length;
  runtime.dispatchCanvas("pointerdown", {
    clientX: 120, clientY: 180, pointerId: 1, pointerType: "touch",
  });
  await runtime.runFrame(16);
  assert.equal(runtime.tickCount(), startingTicks, "the early callback renders without advancing guest code");
  assert.equal(runtime.renderCount(), startingRenders + 1, "the early callback still publishes a render");
  assert.equal(runtime.pointerFrames().length, startingPointerFrames, "render-only callbacks do not rewrite HostFrame input");

  await runtime.runFrame(1000 / 30);
  assert.equal(runtime.tickCount(), startingTicks + 2, "the next due callback catches up both fixed simulation steps");
  assert.deepEqual(runtime.pointerFrames().slice(-2), [
    { pointerCount: 1, isDown: 1, wentDown: 1 },
    { pointerCount: 1, isDown: 1, wentDown: 0 },
  ], "one-shot pointer edges survive rendering and are consumed by the first tick only");
});

test("playground bounds stalled catch-up and resumes on its fixed cadence", async () => {
  const runtime = await loadRuntime();
  await runtime.runFrame(0);
  const startingTicks = runtime.tickCount();
  const startingRenders = runtime.renderCount();
  await runtime.runFrame(10000);
  assert.equal(runtime.tickCount() - startingTicks, 5, "a long stall is capped at five catch-up ticks");
  assert.equal(runtime.renderCount() - startingRenders, 1, "catch-up updates publish one render");
  await runtime.runFrame(10000 + (1000 / 60));
  assert.equal(runtime.tickCount() - startingTicks, 6, "the next normal interval advances one tick");
  assert.equal(runtime.renderCount() - startingRenders, 2, "render cadence resumes without a second catch-up burst");
});

test("playground swaps commit between fixed updates before candidate execution", async () => {
  const runtime = await loadRuntime();
  await runtime.runFrame(0);
  const swap = runtime.api.swap(runtime.request(2));
  const assertion = assert.doesNotReject(swap);
  await new Promise(resolve => setImmediate(resolve));
  await runtime.runFrame(8);
  await assertion;
  assert.equal(runtime.api.generation, 1);
  assert.equal(runtime.tickCount(), 1, "the frame before the fixed update commits without advancing guest code");
  assert.equal(readState(runtime.api.snapshot()), 108, "the candidate restores active state before its hook runs");
  await runtime.runFrame(1000 / 60);
  assert.equal(runtime.tickCount(), 2);
  assert.equal(readState(runtime.api.snapshot()), 109, "the next due update executes the committed candidate");
});

test("playground pause and resume discard elapsed wall time instead of catching up", async () => {
  const runtime = await loadRuntime();
  await runtime.runFrame(0);
  const startingTicks = runtime.tickCount();
  runtime.api.pause(true);
  await runtime.runFrame(5000);
  assert.equal(runtime.tickCount(), startingTicks, "paused callbacks do not advance simulation");
  runtime.api.pause(false);
  await runtime.runFrame(10000);
  assert.equal(runtime.tickCount(), startingTicks + 1, "resuming starts a fresh clock without replaying paused time");
  await runtime.runFrame(10000 + (1000 / 60));
  assert.equal(runtime.tickCount(), startingTicks + 2, "the fixed cadence continues after resume");
});

test("paused playground steps advance HostFrame time by one fixed tick", async () => {
  const runtime = await loadRuntime();
  await runtime.runFrame(0);
  assert.deepEqual(runtime.hostFrames().at(-1), { timeMs: 0, tickHz: 60, timeUs: 0 });
  runtime.api.pause(true);
  await runtime.runFrame(5000);
  const step = runtime.api.step();
  await runtime.runFrame(9000);
  await step;
  assert.deepEqual(runtime.hostFrames().at(-1), { timeMs: 16, tickHz: 60, timeUs: 16666 });
  assert.equal(runtime.tickCount(), 2, "one paused step advances exactly one tick after the long pause");
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

test("playground rejects same-digest snapshots with different restore descriptors", async () => {
  for (const snapshotDescriptor of [
    canonicalSnapshotDescriptor({ requiredBytes: 8 }),
    canonicalSnapshotDescriptor({ path: "main.stasis", offset: 4 }),
  ]) {
    const runtime = await loadRuntime();
    runtime.api.pause(true);
    const before = readState(runtime.api.snapshot());
    const candidateMetadata = runtime.makeMetadata({ snapshotDescriptor });
    await assert.rejects(
      runtime.api.swap(runtime.request(2, candidateMetadata)),
      /canonical state snapshot descriptor/,
    );
    assert.equal(runtime.api.generation, 0, "an incompatible restore descriptor does not publish the candidate");
    assert.equal(readState(runtime.api.snapshot()), before, "the active state remains untouched");
  }
});

test("playground rechecks the prepared snapshot descriptor before commit", async () => {
  const runtime = await loadRuntime();
  runtime.api.pause(true);
  const before = readState(runtime.api.snapshot());
  const candidateMetadata = runtime.makeMetadata();
  const swap = runtime.api.swap(runtime.request(2, candidateMetadata));
  const failure = assert.rejects(swap, /snapshot descriptor changed/);
  await new Promise(resolve => setImmediate(resolve));
  candidateMetadata.replayCompatibility.state_snapshot.entries[0].path = "main.stasis";
  candidateMetadata.replayCompatibility.state_snapshot.entries[0].offset = 4;
  await runtime.runFrame();
  await failure;
  assert.equal(runtime.api.generation, 0, "commit rejects descriptor metadata changed after preparation");
  assert.equal(readState(runtime.api.snapshot()), before, "the active Wasm state is not restored into an incompatible candidate");
});

test("playground compares imported symbols as a set when Wasm repeats an import entry", async () => {
  const runtime = await loadRuntime();
  runtime.api.pause(true);
  const committed = await stageSwap(runtime, runtime.request(6));
  assert.equal(committed.generation, 1);
  assert.equal(runtime.api.generation, 1);
  assert.equal(readState(runtime.api.snapshot()), 7);
});
