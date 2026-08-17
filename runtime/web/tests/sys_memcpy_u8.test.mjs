import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";

const source = fs.readFileSync(new URL("../game.js", import.meta.url), "utf8");

async function loadRuntime(game, memory) {
  const context2d = {
    fillRect() {}, fillText() {}, save() {}, restore() {}, beginPath() {}, moveTo() {},
    lineTo() {}, stroke() {}, drawImage() {}, translate() {}, rotate() {}
  };
  const canvas = {
    width: 640,
    height: 360,
    style: {},
    parentElement: { style: {} },
    getContext: () => context2d,
    getBoundingClientRect: () => ({ left: 0, top: 0, width: 640, height: 360 }),
    addEventListener() {},
    setPointerCapture() {},
    focus() {},
    requestFullscreen: async () => {},
  };
  const errorBox = { textContent: "" };
  const document = {
    body: { dataset: {} },
    hidden: false,
    fullscreenElement: null,
    fonts: { ready: Promise.resolve(), add() {} },
    hasFocus: () => true,
    getElementById(id) {
      if (id === "stasis-canvas") return canvas;
      if (id === "stasis-hud") return { textContent: "" };
      if (id === "stasis-audio") return { addEventListener() {}, disabled: false, textContent: "" };
      if (id === "stasis-error") return errorBox;
      return null;
    },
    addEventListener() {},
  };
  const instance = {
    exports: {
      memory,
      main: () => 0,
      tick: () => 0,
      render: () => 0,
    }
  };
  let env;
  const webAssembly = {
    Global: WebAssembly.Global,
    Memory: WebAssembly.Memory,
    instantiate: async (_bytes, imports) => {
      env = imports.env;
      return { instance };
    },
  };
  const screen = { width: 640, height: 360 };
  const contextObject = {
    document, screen, devicePixelRatio: 1, performance: { now: () => 0 },
    WebAssembly: webAssembly,
    fetch: async () => ({ ok: true, arrayBuffer: async () => new ArrayBuffer(0) }),
    requestAnimationFrame: () => 1,
    cancelAnimationFrame() {},
    addEventListener() {},
    console,
    Image: class {},
    FontFace: class { load() { return Promise.resolve(this); } },
    AudioContext: class {
      constructor() { this.state = "running"; this.currentTime = 0; this.destination = {}; }
      close() {}
      resume() {}
    },
    TextDecoder,
    TextEncoder,
    setTimeout,
    clearTimeout,
  };
  contextObject.window = { STASIS_GAME: game, screen };
  vm.runInNewContext(source, contextObject, { filename: "runtime/web/game.js" });
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(typeof env?.sys_memcpy_u8, "function", errorBox.textContent);
  return env.sys_memcpy_u8;
}

test("web sys_memcpy_u8 matches native registered-buffer and literal semantics", async () => {
  const memory = new WebAssembly.Memory({ initial: 1 });
  const game = {
    memory: {
      source: { hash: 101, offset: 0, type_id: 5, length: 8, stride: 1 },
      destination: { hash: 202, offset: 16, type_id: 5, length: 8, stride: 1 },
      collision: { hash: 303, offset: 32, type_id: 5, length: 4, stride: 1 },
      invalid: { hash: 404, offset: -1, type_id: 5, length: 4, stride: 1 },
    },
    strings: {
      "303": "literal loses",
      "404": "invalid layout still wins",
      "505": "éA",
    },
    assets: {},
  };
  const bytes = new Uint8Array(memory.buffer);
  bytes.set([1, 2, 3, 4, 5], 0);
  bytes.set([7, 8, 9, 10], 32);
  const copy = await loadRuntime(game, memory);

  copy(202, 1, 101, 1, 5);
  assert.deepEqual(Array.from(bytes.slice(16, 24)), [0, 2, 3, 4, 5, 0, 0, 0]);

  bytes.fill(0, 16, 24);
  copy(202, 0, 303, 0, 4);
  assert.deepEqual(Array.from(bytes.slice(16, 20)), [7, 8, 9, 10]);

  bytes.fill(0, 16, 24);
  copy(202, 0, 505, 0, 4);
  assert.deepEqual(Array.from(bytes.slice(16, 20)), [0xc3, 0xa9, 0x41, 0]);

  bytes.set([1, 2, 3, 4, 5], 0);
  copy(101, 1, 101, 0, 4);
  assert.deepEqual(Array.from(bytes.slice(0, 5)), [1, 1, 2, 3, 4]);

  bytes.fill(9, 16, 24);
  copy(202, 0, 404, 0, 2);
  copy(202, 7, 505, 0, 3);
  copy(202, 0, 505, 0, 0);
  assert.deepEqual(Array.from(bytes.slice(16, 24)), [0, 0, 9, 9, 9, 9, 9, 0xc3]);
});
