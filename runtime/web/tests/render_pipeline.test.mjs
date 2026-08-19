import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";

const source = fs.readFileSync(new URL("../game.js", import.meta.url), "utf8");
const MAGIC = 1196967473;

function fakeGl(stats, available = true, throwing = false) {
  if (!available) return null;
  const gl = {
    VERTEX_SHADER: 1, FRAGMENT_SHADER: 2, COMPILE_STATUS: 3, LINK_STATUS: 4,
    ARRAY_BUFFER: 5, STATIC_DRAW: 6, DYNAMIC_DRAW: 7, FLOAT: 8,
    COLOR_BUFFER_BIT: 9, BLEND: 10, SRC_ALPHA: 11, ONE_MINUS_SRC_ALPHA: 12,
    TRIANGLE_STRIP: 13,
    createShader: () => { if (throwing) throw new Error("fake shader failure"); return {}; }, createProgram: () => ({}), createVertexArray: () => ({}), createBuffer: () => ({}),
    shaderSource() {}, compileShader() {}, getShaderParameter: () => true,
    attachShader() {}, linkProgram() {}, getProgramParameter: () => true,
    bindVertexArray() {}, bindBuffer() {}, bufferData() {},
    bufferSubData(_target, _offset, _values, _sourceOffset, length) {
      stats.uploadedFloats.push(length);
    },
    enableVertexAttribArray() {},
    vertexAttribPointer() {}, vertexAttribDivisor() {}, getUniformLocation: () => ({}),
    viewport() {}, clearColor() {}, clear() {}, useProgram() {}, uniform2f() {},
    enable() {}, blendFunc() {}, drawArraysInstanced(_mode, _first, _vertices, count) {
      stats.instanced += 1;
      stats.instances.push(count);
    }
  };
  return gl;
}

async function loadRuntime({ rects = 0, ordered = null, webgl = true, throwing = false, timing = false } = {}) {
  const memory = new WebAssembly.Memory({ initial: 16 });
  const i32 = new Int32Array(memory.buffer, 0, 20000);
  const f32 = new Float32Array(memory.buffer, 100000, 100000);
  const stats = { instanced: 0, instances: [], uploadedFloats: [], images: 0, fills: 0, events: [] };
  let now = 0;
  const context2d = {
    globalAlpha: 1,
    fillRect() { stats.fills += 1; stats.events.push("fill"); if (timing) now += 4; },
    fillText() {}, drawImage() { stats.images += 1; stats.events.push("image"); },
    save() {}, restore() {}, beginPath() {}, moveTo() {}, lineTo() {},
    stroke() { stats.events.push("stroke"); }, translate() {}, rotate() {}
  };
  const gl = fakeGl(stats, webgl, throwing);
  const canvas = {
    width: 640, height: 360, style: {}, parentElement: { style: {} },
    getContext: kind => kind === "2d" ? context2d : gl,
    getBoundingClientRect: () => ({ left: 0, top: 0, width: 640, height: 360 }),
    addEventListener() {}, setPointerCapture() {}, focus() {}, requestFullscreen: async () => {}
  };
  const hud = { textContent: "" };
  const body = { dataset: {} };
  const document = {
    body, hidden: false, fullscreenElement: null,
    fonts: { ready: Promise.resolve(), add() {} }, hasFocus: () => true,
    getElementById(id) {
      if (id === "stasis-canvas") return canvas;
      if (id === "stasis-hud") return hud;
      if (id === "stasis-error") return { textContent: "" };
      if (id === "stasis-audio") return { addEventListener() {}, disabled: false, textContent: "" };
      return null;
    },
    createElement: () => ({ width: 0, height: 0, getContext: () => gl }),
    addEventListener() {}
  };
  let env;
  const instance = { exports: {
    memory,
    main: () => {
      if (timing) env.web_draw_rect(1, 2, 3, 4, 10, 20, 30);
      return 0;
    },
    tick: () => { if (timing) now += 2; },
    render: () => {
      if (timing) now += 3;
      if (!rects) return;
      i32[0] = MAGIC; i32[1] = 4; i32[2] = 0; i32[3] = ordered ? 1 : 0; i32[4] = 0; i32[7] = 0; i32[24] = rects;
      if (ordered) {
        i32[3] = 1; i32[22] = ordered.length;
        ordered.forEach((encoded, index) => { i32[18464 + index] = encoded; });
      }
      for (let index = 0; index < rects; index += 1) {
        const base = 79996 - index * 8;
        f32[base] = index; f32[base + 1] = 1; f32[base + 2] = 2; f32[base + 3] = 2;
        f32[base + 4] = 1; f32[base + 5] = 0; f32[base + 6] = 0; f32[base + 7] = 1;
      }
    }
  }};
  const raf = [];
  const contextObject = {
    document, screen: { width: 640, height: 360 }, devicePixelRatio: 1,
    performance: { now: () => now }, WebAssembly: { instantiate: async (_bytes, imports) => { env = imports.env; return { instance }; } },
    fetch: async () => ({ ok: true, arrayBuffer: async () => new ArrayBuffer(0) }),
    requestAnimationFrame: callback => { raf.push(callback); return raf.length; }, cancelAnimationFrame() {},
    addEventListener() {}, console, Image: class {}, FontFace: class { load() { return Promise.resolve(this); } },
    AudioContext: class { constructor() { this.state = "running"; this.currentTime = 0; this.destination = {}; } close() {} resume() {} },
    TextDecoder, TextEncoder, setTimeout, clearTimeout
  };
  contextObject.window = { STASIS_GAME: { memory: { gfx_cmd_i32: { offset: 0, length: 20000 }, gfx_cmd_f32: { offset: 100000, length: 100000 } }, strings: {}, assets: {} }, screen: contextObject.screen };
  vm.runInNewContext(source, contextObject, { filename: "runtime/web/game.js" });
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(body.dataset.ready, "true");
  return { stats, body, hud, frame: () => raf.shift()(now) };
}

test("large ordered rectangle run uses one instanced composite", async () => {
  const runtime = await loadRuntime({ rects: 64 });
  runtime.frame();
  assert.equal(runtime.stats.instanced, 1);
  assert.deepEqual(runtime.stats.instances, [64]);
  assert.deepEqual(runtime.stats.uploadedFloats, [64 * 8]);
  assert.equal(runtime.stats.images, 1);
  assert.equal(runtime.stats.fills, 0);
});

test("interleaved commands flush rectangle batches in source order", async () => {
  const first = Array.from({ length: 64 }, (_, index) => 4 * 16384 + index);
  const second = Array.from({ length: 64 }, (_, index) => 4 * 16384 + 64 + index);
  const runtime = await loadRuntime({ rects: 128, ordered: [...first, 16384, ...second] });
  runtime.frame();
  assert.equal(runtime.stats.instanced, 2);
  assert.deepEqual(runtime.stats.instances, [64, 64]);
  assert.deepEqual(runtime.stats.events, ["image", "stroke", "image"]);
});

test("WebGL failure permanently falls back to Canvas rectangles", async () => {
  const runtime = await loadRuntime({ rects: 64, throwing: true });
  runtime.frame();
  assert.equal(runtime.stats.instanced, 0);
  assert.equal(runtime.stats.fills, 64);
  assert.equal(runtime.stats.images, 0);
});

test("runtime publishes split timing phases and HUD labels", async () => {
  const runtime = await loadRuntime({ timing: true });
  for (let frame = 0; frame < 6; frame += 1) runtime.frame();
  assert.equal(runtime.body.dataset.tickMs, "2.000");
  assert.equal(runtime.body.dataset.wasmRenderMs, "3.000");
  assert.equal(runtime.body.dataset.browserReplayMs, "4.000");
  assert.equal(runtime.body.dataset.frameWorkMs, "9.000");
  assert.equal(runtime.body.dataset.renderMs, "7.000");
  assert.equal(runtime.body.dataset.worstRenderMs, "7.000");
  assert.match(runtime.hud.textContent, /guest render/);
  assert.match(runtime.hud.textContent, /host replay/);
  assert.match(runtime.hud.textContent, /frame work/);
});
