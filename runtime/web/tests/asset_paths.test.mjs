import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";

const source = fs.readFileSync(new URL("../game.js", import.meta.url), "utf8");

async function loadRuntime(game) {
  const imageSources = [];
  let env;
  const memory = new WebAssembly.Memory({ initial: 1 });
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
  const screen = { width: 640, height: 360 };
  const contextObject = {
    document,
    screen,
    devicePixelRatio: 1,
    performance: { now: () => 0 },
    WebAssembly: {
      Global: WebAssembly.Global,
      Memory: WebAssembly.Memory,
      instantiate: async (_bytes, imports) => {
        env = imports.env;
        return { instance };
      },
    },
    fetch: async () => ({ ok: true, arrayBuffer: async () => new ArrayBuffer(0) }),
    requestAnimationFrame: () => 1,
    cancelAnimationFrame() {},
    addEventListener() {},
    console,
    Image: class {
      addEventListener() {}
      decode() { return Promise.resolve(); }
      set src(value) {
        this.value = value;
        imageSources.push(value);
      }
      get src() { return this.value; }
    },
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
  assert.equal(typeof env?.gfx_load_sprite, "function", errorBox.textContent);
  return { env, imageSources };
}

test("web asset paths normalize fallback values and preserve explicit overrides", async () => {
  const game = {
    memory: {},
    strings: {
      "1": "../assets/missing-map.png",
      "2": "../../assets/empty-map.png",
      "3": "../assets/override.png",
      "4": "../assets/explicit-empty.png",
    },
  };
  const { env, imageSources } = await loadRuntime(game);

  env.gfx_load_sprite(1);
  game.assets = {};
  env.gfx_load_sprite(2);
  game.assets = {
    "assets/override.png": "assets/packed/override-123.png",
    "assets/explicit-empty.png": "",
  };
  env.gfx_load_sprite(3);
  env.gfx_load_sprite(4);

  assert.deepEqual(imageSources, [
    "assets/missing-map.png",
    "assets/empty-map.png",
    "assets/packed/override-123.png",
    "",
  ]);
});
