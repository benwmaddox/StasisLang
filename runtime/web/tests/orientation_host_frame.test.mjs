import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";

const source = fs.readFileSync(new URL("../game.js", import.meta.url), "utf8");

async function runSequence(first, second) {
  const events = new Map();
  const raf = [];
  const ticks = [];
  let now = 0;
  const memory = new WebAssembly.Memory({ initial: 2 });
  const game = { memory: {
    host_i32: { offset: 0, length: 768 },
    host_f32: { offset: 768 * 4, length: 64 }
  }, strings: {}, assets: {} };
  let canvasWidth = first[0];
  let canvasHeight = first[1];
  const context = {
    fillRect() {}, fillText() {}, save() {}, restore() {}, beginPath() {}, moveTo() {}, lineTo() {}, stroke() {},
    drawImage() {}, translate() {}, rotate() {}
  };
  const canvas = {
    width: canvasWidth,
    height: canvasHeight,
    style: {},
    parentElement: { style: {} },
    listeners: new Map(),
    getContext: () => context,
    getBoundingClientRect: () => ({ left: 0, top: 0, width: canvasWidth, height: canvasHeight }),
    addEventListener(type, listener) { this.listeners.set(type, listener); },
    setPointerCapture() {},
    focus() {},
    requestFullscreen: async () => {},
  };
  const body = { dataset: {} };
  const errorBox = { textContent: "" };
  const document = {
    body,
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
    addEventListener(type, listener) { events.set(`document:${type}`, listener); },
  };
  let screenWidth = first[0];
  let screenHeight = first[1];
  const screen = { get width() { return screenWidth; }, get height() { return screenHeight; } };
  const instance = {
    exports: {
      memory,
      main: () => 0,
      tick: () => {
        const i32 = new Int32Array(memory.buffer, 0, 768);
        const f32 = new Float32Array(memory.buffer, 768 * 4, 64);
        if (i32[547]) actionCount += 1;
        ticks.push({
          resized: i32[11], displayGeneration: i32[30], native: [i32[22], i32[23]], drawable: [i32[24], i32[25]],
          logical: [f32[50], f32[51]], pointerCount: i32[7], wentDown: i32[546], wentUp: i32[547], actionCount
        });
      },
      render: () => 0,
    }
  };
  let actionCount = 0;
  const contextObject = {
    document, screen, devicePixelRatio: 1, performance: { now: () => now },
    WebAssembly: { instantiate: async () => ({ instance }) },
    fetch: async () => ({ ok: true, arrayBuffer: async () => new ArrayBuffer(0) }),
    requestAnimationFrame: callback => { raf.push(callback); return raf.length; },
    cancelAnimationFrame() {},
    addEventListener(type, listener) { events.set(`window:${type}`, listener); },
    console, Image: class {}, FontFace: class { load() { return Promise.resolve(this); } },
    AudioContext: class { constructor() { this.state = "running"; this.currentTime = 0; this.destination = {}; } close() {} resume() {} },
    TextDecoder, setTimeout, clearTimeout, STASIS_GAME: game,
  };
  contextObject.window = { STASIS_GAME: game, screen };
  vm.runInNewContext(source, contextObject, { filename: "runtime/web/game.js" });
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(raf.length, 1, `runtime schedules its first RAF: ${errorBox.textContent}`);
  const frame = () => { now += 16; raf.shift()(now); };
  frame();
  const dispatch = (type, event = {}) => canvas.listeners.get(type)?.(event);
  dispatch("pointerdown", { pointerId: 7, clientX: 90, clientY: 180 });
  frame();
  const down = ticks.at(-1);
  assert.equal(down.pointerCount, 1);
  assert.equal(down.wentDown, 1);
  assert.equal(down.wentUp, 0);
  assert.equal(down.logical[0], first[0]);
  assert.equal(down.logical[1], first[1]);
  screenWidth = second[0]; screenHeight = second[1];
  canvasWidth = second[0]; canvasHeight = second[1];
  events.get("window:resize")();
  assert.deepEqual([screen.width, screen.height], second);
  frame();
  const resized = ticks.at(-1);
  assert.equal(resized.resized, 1);
  assert.equal(resized.displayGeneration, 2);
  assert.deepEqual(resized.native, second);
  assert.deepEqual(resized.drawable, second);
  assert.deepEqual(resized.logical, first, "guest canvas remains selected logical size");
  dispatch("pointerup", { pointerId: 7, clientX: 90, clientY: 180 });
  frame();
  const up = ticks.at(-1);
  assert.equal(up.resized, 0);
  assert.equal(up.wentUp, 1);
  assert.equal(up.actionCount, 1, "release increments the action counter exactly once");
  frame();
  const quiet = ticks.at(-1);
  assert.equal(quiet.resized, 0);
  assert.equal(quiet.displayGeneration, 2);
  assert.equal(quiet.pointerCount, 0);
  assert.equal(quiet.wentUp, 0);
  assert.equal(quiet.actionCount, 1, "quiet frames do not repeat the action");
  return ticks;
}

test("web HostFrame portrait to landscape preserves actionable release", async () => {
  await runSequence([360, 720], [720, 360]);
});

test("web HostFrame landscape to portrait preserves actionable release", async () => {
  await runSequence([720, 360], [360, 720]);
});