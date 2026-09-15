import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";
import { fakeWebGL2 } from "./fake_webgl2.mjs";
import { installCollectionViewAbi } from "./collection_view_abi.mjs";

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
    getContext: kind => kind === "webgl2" ? fakeWebGL2() : context2d,
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
  installCollectionViewAbi(game, instance.exports);
  let env;
  const webAssembly = {
    Global: WebAssembly.Global,
    Memory: WebAssembly.Memory,
    instantiate: async (_bytes, imports) => {
      env = imports.env;
      return { instance };
    },
  };
  const contextObject = {
    document, screen: { width: 640, height: 360 }, devicePixelRatio: 1,
    performance: { now: () => 0 }, WebAssembly: webAssembly,
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
  contextObject.window = { STASIS_GAME: game, screen: contextObject.screen };
  vm.runInNewContext(source, contextObject, { filename: "runtime/web/game.js" });
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(typeof env?.sys_memcpy_i32, "function", errorBox.textContent);
  assert.equal(typeof env?.sys_memcpy_f32, "function", errorBox.textContent);
  return env;
}

function typedGame() {
  return {
    memory: {
      i32Source: { hash: 101, handle: 1101, offset: 0, type_id: 1, length: 5, stride: 8 },
      i32Destination: { hash: 202, handle: 1202, offset: 64, type_id: 1, length: 5, stride: 8 },
      f32Source: { hash: 303, handle: 1303, offset: 128, type_id: 2, length: 4, stride: 12 },
      f32Destination: { hash: 404, handle: 1404, offset: 192, type_id: 2, length: 4, stride: 12 },
      invalid: { hash: 505, handle: 1505, offset: 65535, type_id: 1, length: 4, stride: 8 },
    },
    strings: {},
    assets: {},
  };
}

test("web typed memcpy imports copy strided i32/f32 values by opaque handle", async () => {
  const memory = new WebAssembly.Memory({ initial: 1 });
  const game = typedGame();
  const view = new DataView(memory.buffer);
  for (let index = 0; index < 5; index += 1) view.setInt32(index * 8, index * 11 - 7, true);
  for (let index = 0; index < 4; index += 1) view.setFloat32(128 + index * 12, index + 0.25, true);
  const env = await loadRuntime(game, memory);

  env.sys_memcpy_i32(1202, 1, 1101, 0, 4);
  assert.deepEqual(
    Array.from({ length: 5 }, (_, index) => view.getInt32(64 + index * 8, true)),
    [0, -7, 4, 15, 26]
  );
  env.sys_memcpy_f32(1404, 0, 1303, 0, 4);
  assert.deepEqual(
    Array.from({ length: 4 }, (_, index) => view.getFloat32(192 + index * 12, true)),
    [0.25, 1.25, 2.25, 3.25]
  );
});

test("web typed memcpy uses opaque handles and preserves overlap", async () => {
  const memory = new WebAssembly.Memory({ initial: 1 });
  const game = typedGame();
  const view = new DataView(memory.buffer);
  for (let index = 0; index < 5; index += 1) view.setInt32(index * 8, index + 1, true);
  for (let index = 0; index < 4; index += 1) view.setFloat32(128 + index * 12, index + 10.5, true);
  const env = await loadRuntime(game, memory);

  env.sys_memcpy_i32(1101, 1, 1101, 0, 4);
  assert.deepEqual(
    Array.from({ length: 5 }, (_, index) => view.getInt32(index * 8, true)),
    [1, 1, 2, 3, 4]
  );
  env.sys_memcpy_f32(1404, 0, 1303, 1, 3);
  assert.deepEqual(
    Array.from({ length: 4 }, (_, index) => view.getFloat32(192 + index * 12, true)),
    [11.5, 12.5, 13.5, 0]
  );
});

test("web typed memcpy ignores invalid layouts and out-of-range elements", async () => {
  const memory = new WebAssembly.Memory({ initial: 1 });
  const game = typedGame();
  const view = new DataView(memory.buffer);
  view.setInt32(0, 99, true);
  view.setInt32(64, 77, true);
  view.setFloat32(128, 6.5, true);
  const env = await loadRuntime(game, memory);

  env.sys_memcpy_i32(1202, 4, 1101, 4, 3);
  assert.equal(view.getInt32(64 + 4 * 8, true), 0);
  env.sys_memcpy_i32(1505, 0, 1101, 0, 2);
  assert.equal(view.getInt32(64, true), 77);
  env.sys_memcpy_i32(1202, 0, 1505, 0, 2);
  assert.equal(view.getInt32(64, true), 0);
  env.sys_memcpy_f32(1404, 0, 1303, -1, 2);
  assert.equal(view.getFloat32(192, true), 0);
  env.sys_memcpy_f32(1404, 0, 1303, 0, 0);
  assert.equal(view.getFloat32(192, true), 0);
});
