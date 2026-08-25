import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";

const source = fs.readFileSync(new URL("../game.js", import.meta.url), "utf8");
const MAGIC = 1196967473;

function fakeGl(stats, available = true, throwing = false, textureThrow = false) {
  if (!available) return null;
  const gl = {
    VERTEX_SHADER: 1, FRAGMENT_SHADER: 2, COMPILE_STATUS: 3, LINK_STATUS: 4,
    ARRAY_BUFFER: 5, STATIC_DRAW: 6, DYNAMIC_DRAW: 7, FLOAT: 8,
    COLOR_BUFFER_BIT: 9, BLEND: 10, SRC_ALPHA: 11, ONE_MINUS_SRC_ALPHA: 12,
    TRIANGLE_STRIP: 13, TEXTURE_2D: 14, TEXTURE_WRAP_S: 15, TEXTURE_WRAP_T: 16,
    TEXTURE_MIN_FILTER: 17, TEXTURE_MAG_FILTER: 18, CLAMP_TO_EDGE: 19, LINEAR: 20,
    RGBA: 21, UNSIGNED_BYTE: 22, TEXTURE0: 23, UNPACK_FLIP_Y_WEBGL: 24,
    LINEAR_MIPMAP_LINEAR: 25, NO_ERROR: 0,
    createShader: () => { if (throwing) throw new Error("fake shader failure"); return {}; }, createProgram: () => ({}), createVertexArray: () => ({}), createBuffer: () => ({}), createTexture: () => ({}),
    deleteTexture() {}, deleteBuffer() {}, deleteVertexArray() {}, deleteProgram() {},
    shaderSource() {}, compileShader() {}, getShaderParameter: () => true,
    attachShader() {}, linkProgram() {}, getProgramParameter: () => true,
    bindVertexArray() {}, bindBuffer() {}, bufferData() {},
    bufferSubData(_target, _offset, _values, _sourceOffset, length) {
      stats.uploadedFloats.push(length);
      stats.uploads.push(Array.from(_values.subarray(_sourceOffset, _sourceOffset + length)));
    },
    enableVertexAttribArray() {}, disableVertexAttribArray() {}, vertexAttrib4f() {},
    vertexAttribPointer() {}, vertexAttribDivisor() {}, getUniformLocation: () => ({}),
    viewport() {}, clearColor() {}, clear() {}, useProgram() {}, uniform2f() {}, uniform1i() {},
    texParameteri() {}, pixelStorei() {}, texImage2D() { if (textureThrow) throw new Error("fake texture failure"); }, texSubImage2D() { if (textureThrow) throw new Error("fake texture failure"); }, generateMipmap() {}, activeTexture() {}, bindTexture() {}, getError: () => 0,
    isContextLost: () => stats.contextLost,
    enable() {}, blendFunc() {}, blendFuncSeparate() {}, drawArraysInstanced(_mode, _first, _vertices, count) {
      stats.instanced += 1;
      stats.instances.push(count);
    }
  };
  return gl;
}

async function loadRuntime({ rects = 0, ordered = null, sprites = 0, spriteHandles = [], spriteSize = null, webgl = true, throwing = false, textureThrow = false, imageReady = true, timing = false } = {}) {
  const memory = new WebAssembly.Memory({ initial: 16 });
  const i32 = new Int32Array(memory.buffer, 0, 20000);
  const f32 = new Float32Array(memory.buffer, 100000, 100000);
  const stats = { instanced: 0, instances: [], uploadedFloats: [], uploads: [], images: 0, fills: 0, events: [], contextLost: false };
  let now = 0;
  const context2d = {
    globalAlpha: 1,
    fillRect() { stats.fills += 1; stats.events.push("fill"); if (timing) now += 4; },
    fillText() {}, drawImage() { stats.images += 1; stats.events.push("image"); },
    save() {}, restore() {}, beginPath() {}, moveTo() {}, lineTo() {},
    stroke() { stats.events.push("stroke"); }, translate() {}, rotate() {}
  };
  const rasterStats = { draws: 0 };
  const rasterContext = {
    imageSmoothingEnabled: true, imageSmoothingQuality: "high",
    clearRect() {}, drawImage() { rasterStats.draws += 1; }, save() {}, restore() {}
  };
  const gl = fakeGl(stats, webgl, throwing, textureThrow);
  const canvas = {
    width: 640, height: 360, style: {}, parentElement: { style: {} },
    getContext: kind => kind === "2d" ? context2d : gl,
    getBoundingClientRect: () => ({ left: 0, top: 0, width: 640, height: 360 }),
    addEventListener() {}, setPointerCapture() {}, focus() {}, requestFullscreen: async () => {}
  };
  const hud = { textContent: "" };
  const body = { dataset: {} };
  const offscreenListeners = new Map();
  const offscreen = {
    width: 0, height: 0,
    getContext: kind => kind === "2d" ? rasterContext : gl,
    addEventListener(type, callback) { offscreenListeners.set(type, callback); }
  };
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
    createElement: () => offscreen,
    addEventListener() {}
  };
  let env;
  const instance = { exports: {
    memory,
    main: () => {
      if (timing) env.web_draw_rect(1, 2, 3, 4, 10, 20, 30);
      for (let index = 0; index < Math.max(1, new Set(spriteHandles).size); index += 1) {
        env.gfx_load_sprite(0, spriteSize?.[0], spriteSize?.[1]);
      }
      return 0;
    },
    tick: () => { if (timing) now += 2; },
    render: () => {
      if (timing) now += 3;
      if (!rects && !sprites) return;
      i32[0] = MAGIC; i32[1] = sprites ? 5 : 4; i32[2] = 0; i32[3] = ordered ? 1 : 0; i32[4] = sprites; i32[7] = 0; i32[24] = rects;
      if (ordered) {
        i32[3] = 1; i32[22] = ordered.length;
        ordered.forEach((encoded, index) => { i32[18464 + index] = encoded; });
      }
      for (let index = 0; index < rects; index += 1) {
        const base = 79996 - index * 8;
        f32[base] = index; f32[base + 1] = 1; f32[base + 2] = 2; f32[base + 3] = 2;
        f32[base + 4] = 1; f32[base + 5] = 0; f32[base + 6] = 0; f32[base + 7] = 1;
      }
      for (let index = 0; index < sprites; index += 1) {
        const baseI = 32 + index * 3;
        const baseF = 80004 + index * 8;
        i32[baseI] = spriteHandles[index] || 1;
        i32[baseI + 1] = index * 10;
        i32[baseI + 2] = 180;
        f32[baseF] = index + 0.5; f32[baseF + 1] = 2; f32[baseF + 2] = 8; f32[baseF + 3] = 10;
        f32[baseF + 4] = 0.1; f32[baseF + 5] = 0.2; f32[baseF + 6] = 0.9; f32[baseF + 7] = 0.8;
      }
    }
  }};
  const raf = [];
  const contextObject = {
    document, screen: { width: 640, height: 360 }, devicePixelRatio: 1,
    performance: { now: () => now }, WebAssembly: { instantiate: async (_bytes, imports) => { env = imports.env; return { instance }; } },
    fetch: async () => ({ ok: true, arrayBuffer: async () => new ArrayBuffer(0) }),
    requestAnimationFrame: callback => { raf.push(callback); return raf.length; }, cancelAnimationFrame() {},
    addEventListener() {}, console, Image: class { constructor() { this.complete = imageReady; this.naturalWidth = imageReady ? 16 : 0; this.naturalHeight = imageReady ? 16 : 0; } decode() { return Promise.resolve(); } }, FontFace: class { load() { return Promise.resolve(this); } },
    AudioContext: class { constructor() { this.state = "running"; this.currentTime = 0; this.destination = {}; } close() {} resume() {} },
    TextDecoder, TextEncoder, setTimeout, clearTimeout
  };
  contextObject.window = { STASIS_GAME: { memory: { gfx_cmd_i32: { offset: 0, length: 20000 }, gfx_cmd_f32: { offset: 100000, length: 100000 } }, strings: {}, assets: {} }, screen: contextObject.screen };
  vm.runInNewContext(source, contextObject, { filename: "runtime/web/game.js" });
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(body.dataset.ready, "true");
  return {
    stats, body, hud, rasterStats, frame: () => raf.shift()(now),
    loseContext: () => { stats.contextLost = true; offscreenListeners.get("webglcontextlost")?.({ preventDefault() {} }); },
    restoreContext: () => { stats.contextLost = false; offscreenListeners.get("webglcontextrestored")?.({}); }
  };
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

test("large same-handle sprite run uploads the private 64-byte records", async () => {
  const runtime = await loadRuntime({ sprites: 64, spriteHandles: Array(64).fill(1) });
  runtime.frame();
  assert.equal(runtime.stats.instanced, 1);
  assert.deepEqual(runtime.stats.instances, [64]);
  assert.deepEqual(runtime.stats.uploadedFloats, [64 * 16]);
  assert.deepEqual(runtime.stats.uploads[0].slice(0, 4), [
    0.5, 2, 8, 10
  ]);
  assert.deepEqual(runtime.stats.uploads[0].slice(8, 16), [
    1, 1, 1, new Float32Array([180 / 255])[0], 0, 1, 0, 0
  ]);
  const atlasUv = runtime.stats.uploads[0].slice(4, 8);
  assert.ok(atlasUv[0] > 0 && atlasUv[1] > 0 && atlasUv[2] < 1 && atlasUv[3] < 1);
  assert.ok(atlasUv[0] < atlasUv[2] && atlasUv[1] < atlasUv[3]);
  assert.equal(runtime.stats.images, 1);
});

test("requested sprite dimensions rasterize before Canvas fallback", async () => {
  const runtime = await loadRuntime({ sprites: 1, spriteHandles: [1], spriteSize: [4, 4] });
  runtime.frame();
  assert.equal(runtime.stats.images, 1);
  assert.ok(runtime.rasterStats.draws >= 1);
});

test("sprite handle changes and interleaved primitives split in source order", async () => {
  const first = Array.from({ length: 64 }, (_, index) => 2 * 16384 + index);
  const second = Array.from({ length: 64 }, (_, index) => 2 * 16384 + 64 + index);
  const runtime = await loadRuntime({
    rects: 1, sprites: 128, spriteHandles: [...Array(64).fill(1), ...Array(64).fill(2)],
    ordered: [...first, 4 * 16384, ...second]
  });
  runtime.frame();
  assert.equal(runtime.stats.instanced, 2);
  assert.deepEqual(runtime.stats.instances, [64, 64]);
  assert.deepEqual(runtime.stats.events, ["image", "fill", "image"]);
});

test("different handles sharing one atlas page batch together", async () => {
  const handles = Array.from({ length: 64 }, (_, index) => (index % 4) + 1);
  const runtime = await loadRuntime({ sprites: 64, spriteHandles: handles });
  runtime.frame();
  assert.equal(runtime.stats.instanced, 1);
  assert.deepEqual(runtime.stats.instances, [64]);
  assert.equal(runtime.body.dataset.atlasPages, "1");
  assert.equal(runtime.body.dataset.atlasLiveEntries, "4");
});

test("atlas page boundaries split the adjacent sprite run", async () => {
  const handles = [...Array(64).fill(1), ...Array(64).fill(2), ...Array(64).fill(3), ...Array(64).fill(4)];
  const runtime = await loadRuntime({ sprites: 256, spriteHandles: handles, spriteSize: [256, 256] });
  runtime.frame();
  assert.equal(runtime.stats.instanced, 4);
  assert.deepEqual(runtime.stats.instances, [64, 64, 64, 64]);
  assert.equal(runtime.body.dataset.atlasPages, "4");
});

test("oversize sprite runs fall back to Canvas without partial GPU submission", async () => {
  const runtime = await loadRuntime({
    sprites: 64, spriteHandles: Array(64).fill(1), spriteSize: [2048, 2048]
  });
  runtime.frame();
  assert.equal(runtime.stats.instanced, 0);
  assert.equal(runtime.stats.images, 64);
  assert.equal(runtime.body.dataset.atlasPages, "0");
});

test("sprite texture failure and context loss replay through Canvas", async () => {
  const failed = await loadRuntime({ sprites: 64, spriteHandles: Array(64).fill(1), textureThrow: true });
  failed.frame();
  assert.equal(failed.stats.instanced, 0);
  assert.equal(failed.stats.images, 64);

  const recovered = await loadRuntime({ sprites: 64, spriteHandles: Array(64).fill(1) });
  recovered.frame();
  recovered.loseContext();
  recovered.frame();
  assert.equal(recovered.stats.instanced, 1);
  assert.equal(recovered.stats.images, 65);
  recovered.restoreContext();
  recovered.frame();
  assert.equal(recovered.stats.instanced, 2);
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
