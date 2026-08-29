import assert from "node:assert/strict";
import test from "node:test";
import fs from "node:fs";
import vm from "node:vm";

const source = fs.readFileSync(new URL("../game.js", import.meta.url), "utf8");

test("browser mailbox checkpoints only bounded semantic metadata", () => {
  assert.match(source, /stasis_web_network_checkpoint: networkCheckpoint/);
  assert.match(source, /networkClient\.desiredSeat = seat/);
  assert.match(source, /networkClient\.lastSequence = lastSequence/);
  assert.match(source, /lastSequence > 0x7fffffff/);
  assert.match(source, /seat < -1 \|\| seat >= 8/);
  assert.match(source, /JSON\.stringify\(\{ seat, lastSequence \}\)/);
  assert.match(source, /networkCheckpointKey\(networkResumeCredential\(\)\)/);
  // The credential may select an opaque storage namespace, but is not stored
  // as checkpoint JSON or returned through the Stasis import.
  assert.doesNotMatch(source, /JSON\.stringify\(\{[^}]*credential/);
});

test("browser network runtime keeps credential and pairing secret adapter-only", () => {
  assert.match(source, /new WebSocket\(socketUrl, \["stasis-v1", secret, protocol\]\)/);
  assert.match(source, /const socketUrl = `\$\{currentLocation\.protocol/);
  assert.match(source, /const networkPairingSecret = \(\) =>/);
  assert.match(source, /const networkResumeCredential = \(\) =>/);
  assert.match(source, /networkLoadCheckpoint\(\);/);
  assert.doesNotMatch(source, /networkClient\.queue\.push\([^)]*secret/);
});

async function loadRuntime() {
  const storage = new Map();
  const sockets = [];
  const memory = new WebAssembly.Memory({ initial: 1 });
  let imports;
  class FakeWebSocket {
    static CONNECTING = 0;
    static OPEN = 1;
    static CLOSING = 2;
    static CLOSED = 3;
    constructor(url, protocols) {
      this.url = url;
      this.protocols = protocols;
      this.readyState = FakeWebSocket.CONNECTING;
      this.sent = [];
      sockets.push(this);
    }
    send(value) { this.sent.push(new Uint8Array(value)); }
    open() { this.readyState = FakeWebSocket.OPEN; this.onopen?.(); }
    receive(value) {
      const bytes = value instanceof Uint8Array ? value : new Uint8Array(value);
      this.onmessage?.({ data: bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) });
    }
  }
  const canvas = {
    width: 640, height: 360, style: {}, parentElement: { style: {} },
    getContext: () => ({ fillRect() {}, fillText() {}, save() {}, restore() {}, beginPath() {},
      moveTo() {}, lineTo() {}, stroke() {}, drawImage() {}, translate() {}, rotate() {} }),
    getBoundingClientRect: () => ({ left: 0, top: 0, width: 640, height: 360 }),
    addEventListener() {}, setPointerCapture() {}, focus() {}, requestFullscreen: async () => {},
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
  const localStorage = {
    getItem(key) { return storage.has(key) ? storage.get(key) : null; },
    setItem(key, value) { storage.set(key, String(value)); },
  };
  const instance = { exports: { memory, main: () => 0, tick: () => 0, render: () => 0 } };
  const context = {
    document,
    window: {
      STASIS_GAME: {
        strings: {},
        memory: { payload: { hash: 7, offset: 0, length: 8, stride: 1, byte_backed: true, type_id: 5 } },
        assets: {},
      },
    },
    localStorage,
    location: {
      origin: "https://example.test", protocol: "https:", host: "example.test",
      hash: "#secret=0123456789abcdef0123456789abcdef",
    },
    crypto: { getRandomValues(bytes) { bytes.fill(0x2a); return bytes; } },
    WebSocket: FakeWebSocket,
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
    ArrayBuffer,
    STASIS_CHARACTERIZATION_TEST: true,
  };
  vm.runInNewContext(source, context, { filename: "runtime/web/game.js" });
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));
  await context.window.STASIS_RUNTIME_PROMISE;
  return { testing: context.window.__STASIS_CHARACTERIZATION__, imports, storage, sockets, memory };
}

test("browser network checkpoints and mailbox execute bounded behavior in a VM", async () => {
  const runtime = await loadRuntime();
  const { testing, imports, storage, sockets, memory } = runtime;
  assert.equal(testing.networkConnect(), 0);
  assert.equal(sockets.length, 1);
  assert.deepEqual(Array.from(sockets[0].protocols.slice(0, 2)), ["stasis-v1", "0123456789abcdef0123456789abcdef"]);
  assert.match(sockets[0].protocols[2], /^stasis-resume-v1\.[0-9a-f]{32}$/);

  assert.equal(testing.networkCheckpoint(2, 17), 0);
  const checkpoint = [...storage.entries()].find(([key]) => key.startsWith("stasis:checkpoint:"));
  assert.ok(checkpoint, "checkpoint is persisted under a hashed key");
  assert.deepEqual(JSON.parse(checkpoint[1]), { seat: 2, lastSequence: 17 });
  assert.doesNotMatch(checkpoint[1], /0123456789abcdef/);

  new Uint8Array(memory.buffer).set([4, 5, 6], 0);
  assert.equal(testing.networkSend(7, 3), 0, "open or connecting payload is accepted");
  sockets[0].open();
  assert.deepEqual([...sockets[0].sent[0]], [4, 5, 6]);
  sockets[0].receive(new Uint8Array([8, 9]));
  assert.equal(testing.networkPoll(7, 8), 2);
  assert.deepEqual([...new Uint8Array(memory.buffer).slice(0, 2)], [8, 9]);
  assert.equal(imports.stasis_web_network_resume_seat(), 2);
  assert.equal(imports.stasis_web_network_last_sequence(), 17);
  assert.equal(testing.networkCheckpoint(-2, 1), -1);
  assert.equal(testing.networkCheckpoint(8, 1), -1);
});

test("browser network checkpoint corruption falls back without leaking stale metadata", async () => {
  const runtime = await loadRuntime();
  const { testing, storage } = runtime;
  assert.equal(testing.networkCheckpoint(3, 21), 0);
  const key = [...storage.keys()].find(value => value.startsWith("stasis:checkpoint:"));
  assert.ok(key);
  testing.networkClient.desiredSeat = -1;
  testing.networkClient.lastSequence = 0;
  storage.set(key, "{not-json");
  testing.networkLoadCheckpoint();
  assert.equal(testing.networkClient.desiredSeat, -1);
  assert.equal(testing.networkClient.lastSequence, 0);
});

test("browser network outbound mailbox rejects work beyond its bounded queue", async () => {
  const { testing, memory } = await loadRuntime();
  new Uint8Array(memory.buffer)[0] = 1;
  assert.equal(testing.networkConnect(), 0);
  for (let index = 0; index < 256; index += 1) {
    assert.equal(testing.networkSend(7, 1), 0, `queued payload ${index}`);
  }
  assert.equal(testing.networkSend(7, 1), -3);
  assert.equal(testing.networkClient.outbound.length, 256);
});
