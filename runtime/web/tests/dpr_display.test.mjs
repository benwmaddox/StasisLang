import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";

const source = fs.readFileSync(new URL("../game.js", import.meta.url), "utf8");

async function loadRuntime({ logical = [640, 360], css = logical, dpr = 1, includeHostF32 = true } = {}) {
  const memory = new WebAssembly.Memory({ initial: 2 });
  const hostI32 = { offset: 0, length: 768 };
  const hostF32 = { offset: 768 * 4, length: 64 };
  const game = {
    memory: { host_i32: hostI32, ...(includeHostF32 ? { host_f32: hostF32 } : {}) },
    strings: {}, assets: {}
  };
  const events = new Map();
  const raf = [];
  const transforms = [];
  const canvas = {
    width: logical[0], height: logical[1], dataset: {}, style: {},
    listeners: new Map(),
    getContext: () => ({
      setTransform(...value) { transforms.push(value); },
      fillRect() {}, fillText() {}, save() {}, restore() {}, beginPath() {},
      moveTo() {}, lineTo() {}, stroke() {}, drawImage() {}, translate() {}, rotate() {},
    }),
    getBoundingClientRect: () => ({ left: 10, top: 20, width: css[0], height: css[1], right: 10 + css[0], bottom: 20 + css[1] }),
    addEventListener(type, listener) { this.listeners.set(type, listener); },
    setPointerCapture() {}, focus() {}, requestFullscreen: async () => {},
  };
  const body = { dataset: {} };
  const errorBox = { textContent: "" };
  const ticks = [];
  const instance = { exports: {
    memory,
    main: () => 0,
    tick: () => {
      if (!includeHostF32) return;
      const i32 = new Int32Array(memory.buffer, hostI32.offset, hostI32.length);
      const f32 = new Float32Array(memory.buffer, hostF32.offset, hostF32.length);
      ticks.push({
        logical: [f32[50], f32[51]], css: [i32[22], i32[23]],
        backing: [i32[24], i32[25]], generation: i32[30], density: i32[31],
        pointer: [f32[0], f32[1]], normalized: [f32[4], f32[5]],
      });
    },
    render: () => 0,
  }};
  const document = {
    body, hidden: false, fullscreenElement: null,
    fonts: { ready: Promise.resolve(), add() {} }, hasFocus: () => true,
    getElementById(id) {
      if (id === "stasis-canvas") return canvas;
      if (id === "stasis-error") return errorBox;
      if (id === "stasis-hud") return null;
      return null;
    },
    addEventListener(type, listener) { events.set(`document:${type}`, listener); },
  };
  let currentDpr = dpr;
  const context = {
    document, window: null, screen: { width: css[0], height: css[1] },
    get devicePixelRatio() { return currentDpr; },
    performance: { now: () => 0 },
    WebAssembly: { instantiate: async () => ({ instance }) },
    fetch: async () => ({ ok: true, arrayBuffer: async () => new ArrayBuffer(0) }),
    requestAnimationFrame: callback => { raf.push(callback); return raf.length; },
    cancelAnimationFrame() {}, addEventListener(type, listener) { events.set(`window:${type}`, listener); },
    console, Image: class {}, FontFace: class { load() { return Promise.resolve(this); } },
    AudioContext: class { constructor() { this.state = "running"; this.currentTime = 0; this.destination = {}; } close() {} resume() {} },
    TextDecoder, TextEncoder, setTimeout, clearTimeout, STASIS_GAME: game,
  };
  context.window = { STASIS_GAME: game, screen: context.screen, visualViewport: undefined };
  vm.runInNewContext(source, context, { filename: "runtime/web/game.js" });
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(body.dataset.ready, "true", errorBox.textContent);
  assert.equal(raf.length, 1);
  const frame = () => raf.shift()(0);
  frame();
  return {
    body, canvas, ticks, transforms, frame, events,
    setDpr(value) { currentDpr = value; events.get("window:resize")?.(); },
    setCss(width, height) { css[0] = width; css[1] = height; events.get("window:resize")?.(); },
  };
}

test("web backing dimensions follow CSS extent times DPR tiers", async () => {
  for (const dpr of [1, 1.25, 1.5, 2, 3]) {
    const runtime = await loadRuntime({ logical: [640, 360], css: [640, 360], dpr });
    const frame = runtime.ticks.at(-1);
    assert.deepEqual(frame.logical, [640, 360]);
    assert.deepEqual(frame.css, [640, 360]);
    assert.deepEqual(frame.backing, [Math.round(640 * dpr), Math.round(360 * dpr)]);
    assert.equal(runtime.body.dataset.backingFallback, "none");
  }
});

test("logical metadata remains authoritative through resize and DPR changes", async () => {
  const runtime = await loadRuntime({ logical: [320, 200], css: [800, 500], dpr: 1 });
  const first = runtime.ticks.at(-1);
  runtime.setDpr(2);
  runtime.frame();
  const second = runtime.ticks.at(-1);
  assert.deepEqual(second.logical, [320, 200]);
  assert.deepEqual(second.backing, [1600, 1000]);
  assert.equal(second.generation, first.generation + 1);
  assert.equal(second.density, first.density + 1);
  assert.equal(runtime.body.dataset.logicalWidth, "320");
  assert.equal(runtime.canvas.dataset.logicalWidth, "320");
});

test("backing axis and byte caps are explicit and inspectable", async () => {
  const runtime = await loadRuntime({ logical: [10000, 10000], css: [10000, 10000], dpr: 3 });
  const frame = runtime.ticks.at(-1);
  assert.ok(frame.backing[0] <= 8192 && frame.backing[1] <= 8192);
  assert.ok(frame.backing[0] * frame.backing[1] * 4 <= 64 * 1024 * 1024);
  assert.notEqual(runtime.body.dataset.backingFallback, "none");
  assert.equal(runtime.body.dataset.backingCap, "capped");
});

test("DPR cap is explicit when the browser reports an extreme density", async () => {
  const runtime = await loadRuntime({ logical: [640, 360], css: [640, 360], dpr: 8 });
  assert.equal(runtime.body.dataset.devicePixelRatio, "4");
  assert.match(runtime.body.dataset.backingFallback, /dpr/);
  assert.equal(runtime.body.dataset.backingCap, "capped");
});

test("CSS pointer coordinates round trip into logical normalized coordinates", async () => {
  const runtime = await loadRuntime({ logical: [400, 200], css: [800, 400], dpr: 2 });
  runtime.canvas.listeners.get("pointerdown")({
    pointerId: 7, pointerType: "mouse", clientX: 10, clientY: 20,
  });
  runtime.frame();
  const frame = runtime.ticks.at(-1);
  assert.deepEqual(frame.pointer, [0, 0]);
  assert.deepEqual(frame.normalized, [0, 0]);
  runtime.canvas.listeners.get("pointerup")({
    pointerId: 7, pointerType: "mouse", clientX: 810, clientY: 420,
  });
  runtime.frame();
  assert.deepEqual(runtime.ticks.at(-1).pointer, [400, 200]);
  assert.deepEqual(runtime.ticks.at(-1).normalized, [1, 1]);
});

test("Canvas2D uses one logical-to-backing transform", async () => {
  const runtime = await loadRuntime({ logical: [320, 180], css: [640, 360], dpr: 2 });
  assert.ok(runtime.transforms.some(value => value[0] === 4 && value[3] === 4));
});

test("display backing still follows viewport changes when host_f32 is absent", async () => {
  const runtime = await loadRuntime({
    logical: [320, 180], css: [393, 221.0625], dpr: 3, includeHostF32: false
  });
  const initialGeneration = Number(runtime.body.dataset.displayGeneration);
  runtime.setCss(698.65625, 393);
  runtime.frame();
  assert.equal(runtime.body.dataset.logicalWidth, "320");
  assert.equal(runtime.body.dataset.logicalHeight, "180");
  assert.equal(runtime.body.dataset.cssWidth, "698.65625");
  assert.equal(runtime.body.dataset.cssHeight, "393");
  assert.equal(runtime.body.dataset.backingWidth, "2096");
  assert.equal(runtime.body.dataset.backingHeight, "1179");
  assert.equal(Number(runtime.body.dataset.displayGeneration), initialGeneration + 1);
});
