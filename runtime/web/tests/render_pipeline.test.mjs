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
    deleteTexture() { stats.deletedTextures += 1; }, deleteBuffer() {}, deleteVertexArray() {}, deleteProgram() {},
    shaderSource() {}, compileShader() {}, getShaderParameter: () => true,
    attachShader() {}, linkProgram() {}, getProgramParameter: () => true,
    bindVertexArray() {}, bindBuffer() {}, bufferData() {},
    bufferSubData(_target, _offset, _values, _sourceOffset, length) {
      stats.uploadedFloats.push(length);
      stats.uploads.push(Array.from(_values.subarray(_sourceOffset, _sourceOffset + length)));
    },
    enableVertexAttribArray() {}, disableVertexAttribArray() {}, vertexAttrib4f() {},
    vertexAttribPointer() {}, vertexAttribDivisor() {}, getUniformLocation: () => ({}),
    viewport() {}, clearColor() {}, clear() {}, useProgram() {}, uniform2f(_location, width, height) {
      stats.uniforms.push([width, height]);
    }, uniform1i() {},
    texParameteri() {}, pixelStorei() {}, texImage2D() { if (textureThrow) throw new Error("fake texture failure"); }, texSubImage2D() { if (textureThrow) throw new Error("fake texture failure"); }, generateMipmap() {}, activeTexture() {}, bindTexture() {}, getError: () => 0,
    isContextLost: () => stats.contextLost,
    enable() {}, blendFunc() {}, blendFuncSeparate() {}, drawArraysInstanced(_mode, _first, _vertices, count) {
      stats.instanced += 1;
      stats.instances.push(count);
    }
  };
  return gl;
}

async function loadRuntime({ rects = 0, ordered = null, clips = [], sprites = 0, spriteHandles = [], spriteSize = null, spriteSizes = null, spriteUv = [0.1, 0.2, 0.9, 0.8], webgl = true, throwing = false, textureThrow = false, imageReady = true, timing = false, dpr = 1, cssExtent = [640, 360], imageExtent = [16, 16], assetMetadata = {}, assets = {}, createImageBitmap = null, imageDecode = null, fetchBlob = null } = {}) {
  const memory = new WebAssembly.Memory({ initial: 16 });
  const i32 = new Int32Array(memory.buffer, 0, 35120);
  const f32 = new Float32Array(memory.buffer, 100000, 126084);
  const stats = { instanced: 0, instances: [], uploadedFloats: [], uploads: [], uniforms: [], transforms: [], imageArgs: [], images: 0, fills: 0, events: [], clipRects: [], clipCalls: 0, restores: 0, contextLost: false, imageDecodeCalls: 0, imageConstructed: 0, bitmapCalls: [], deletedTextures: 0 };
  let now = 0;
  const context2d = {
    globalAlpha: 1,
    setTransform(...value) { stats.transforms.push(value); },
    fillRect() { stats.fills += 1; stats.events.push("fill"); if (timing) now += 4; },
    fillText() {}, drawImage(...args) { stats.images += 1; stats.imageArgs.push(args); stats.events.push("image"); },
    save() {}, restore() { stats.restores += 1; }, beginPath() {}, moveTo() {}, lineTo() {},
    rect(x, y, width, height) { stats.clipRects.push([x, y, width, height]); },
    clip() { stats.clipCalls += 1; },
    stroke() { stats.events.push("stroke"); }, translate() {}, rotate() {}
  };
  const rasterStats = { draws: 0, images: [], clears: [] };
  const rasterContext = {
    imageSmoothingEnabled: true, imageSmoothingQuality: "high",
    clearRect(...args) { rasterStats.clears.push(args); }, drawImage(...args) { rasterStats.draws += 1; rasterStats.images.push(args); }, save() {}, restore() {}
  };
  const gl = fakeGl(stats, webgl, throwing, textureThrow);
  const canvas = {
    width: 640, height: 360, style: {}, parentElement: { style: {} },
    dataset: {},
    getContext: kind => kind === "2d" ? context2d : gl,
    getBoundingClientRect: () => ({ left: 0, top: 0, width: cssExtent[0], height: cssExtent[1] }),
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
        const dimensions = spriteSizes?.[index] || spriteSize;
        env.gfx_load_sprite(0, dimensions?.[0], dimensions?.[1]);
      }
      return 0;
    },
    tick: () => { if (timing) now += 2; },
    render: () => {
      if (timing) now += 3;
      if (!rects && !sprites && !clips.length) return;
      i32[0] = MAGIC; i32[1] = clips.length ? 6 : sprites ? 5 : 4; i32[2] = 0; i32[3] = ordered ? 1 : 0; i32[4] = sprites; i32[7] = 0; i32[24] = rects; i32[27] = clips.length;
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
        f32[baseF + 4] = spriteUv[0]; f32[baseF + 5] = spriteUv[1];
        f32[baseF + 6] = spriteUv[2]; f32[baseF + 7] = spriteUv[3];
      }
      clips.forEach((clip, index) => {
        const base = 125060 + index * 4;
        f32[base] = clip[0]; f32[base + 1] = clip[1];
        f32[base + 2] = clip[2]; f32[base + 3] = clip[3];
      });
    }
  }};
  const raf = [];
  const contextObject = {
    document, screen: { width: 640, height: 360 }, devicePixelRatio: 1,
    performance: { now: () => now }, WebAssembly: { instantiate: async (_bytes, imports) => { env = imports.env; return { instance }; } },
    fetch: async source => ({ ok: true, arrayBuffer: async () => new ArrayBuffer(0), blob: async () => fetchBlob ? fetchBlob(source) : { source } }),
    requestAnimationFrame: callback => { raf.push(callback); return raf.length; }, cancelAnimationFrame() {},
    addEventListener() {}, console, Image: class { constructor() { stats.imageConstructed += 1; this.complete = imageReady; this.naturalWidth = imageReady ? imageExtent[0] : 0; this.naturalHeight = imageReady ? imageExtent[1] : 0; } decode() { stats.imageDecodeCalls += 1; return imageDecode ? imageDecode(this) : Promise.resolve(); } }, FontFace: class { load() { return Promise.resolve(this); } },
    AudioContext: class { constructor() { this.state = "running"; this.currentTime = 0; this.destination = {}; } close() {} resume() {} },
    TextDecoder, TextEncoder, setTimeout, clearTimeout, devicePixelRatio: dpr,
  };
  if (createImageBitmap) {
    contextObject.createImageBitmap = async (source, options) => {
      stats.bitmapCalls.push({ source, options });
      return createImageBitmap(source, options, stats.bitmapCalls.length);
    };
  }
  contextObject.window = { STASIS_GAME: { memory: { gfx_cmd_i32: { offset: 0, length: 35120 }, gfx_cmd_f32: { offset: 100000, length: 126084 }, host_i32: { offset: 230000, length: 768 }, host_f32: { offset: 233072, length: 64 } }, strings: {}, assets, asset_metadata: assetMetadata }, screen: contextObject.screen };
  vm.runInNewContext(source, contextObject, { filename: "runtime/web/game.js" });
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(body.dataset.ready, "true");
  return {
    stats, body, hud, rasterStats, offscreen, env, contextObject, frame: () => raf.shift()(now),
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

test("WebGL targets use physical framebuffers with logical shader dimensions and identity composite", async () => {
  const runtime = await loadRuntime({ rects: 64, cssExtent: [800, 450], dpr: 2 });
  runtime.frame();
  assert.deepEqual(runtime.stats.uniforms[0], [640, 360]);
  assert.equal(runtime.offscreen.width, 1600);
  assert.equal(runtime.offscreen.height, 900);
  assert.ok(runtime.stats.transforms.some(value => value[0] === 1 && value[3] === 1));
});

test("ordered clipping saves and restores nested Canvas2D state", async () => {
  const scale = 16384;
  const runtime = await loadRuntime({
    clips: [[10, 12, 100, 80], [25, 30, 40, 24]],
    ordered: [5 * scale, 5 * scale + 1, 6 * scale, 6 * scale]
  });
  runtime.frame();
  assert.deepEqual(runtime.stats.clipRects, [[10, 12, 100, 80], [25, 30, 40, 24]]);
  assert.equal(runtime.stats.clipCalls, 2);
  assert.equal(runtime.stats.restores, 2);
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
  assert.equal(runtime.body.dataset.assetAtlasWidth, "512");
  assert.equal(runtime.body.dataset.assetAtlasHeight, "512");
  assert.equal(runtime.body.dataset.assetAtlasBytes, String(512 * 512 * 4));
});

test("requested sprite dimensions rasterize before Canvas fallback", async () => {
  const runtime = await loadRuntime({ sprites: 1, spriteHandles: [1], spriteSize: [4, 4] });
  runtime.frame();
  assert.equal(runtime.stats.images, 1);
  assert.ok(runtime.rasterStats.draws >= 1);
});

test("density changes select one bounded sprite tier and reuse its cache", async () => {
  const runtime = await loadRuntime({
    sprites: 1, spriteHandles: [1], spriteSize: [16, 16], imageExtent: [64, 64], dpr: 1,
    assetMetadata: { "": {
      encoding: "svg", source_sha256: "source-master", prepared_sha256: "prepared-tier-1"
    } }
  });
  runtime.frame();
  assert.equal(runtime.body.dataset.assetPreparedWidth, "16");
  assert.equal(runtime.body.dataset.assetPreparedTier, "1");
  assert.equal(runtime.body.dataset.spriteRasterCount, "1");

  runtime.contextObject.devicePixelRatio = 2;
  runtime.frame();
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(runtime.body.dataset.assetPreparedWidth, "32");
  assert.equal(runtime.body.dataset.assetPreparedTier, "2");
  assert.equal(runtime.body.dataset.assetDensityInvalidations, "1");
  assert.equal(runtime.body.dataset.spriteRasterCount, "2");
  assert.equal(runtime.body.dataset.spriteDecodedCount, "1", "density rebuild reuses the decoded source");

  runtime.env.gfx_load_sprite(0, 16, 16);
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(runtime.body.dataset.spriteCacheHits, "1");
  assert.equal(runtime.body.dataset.spriteRasterCount, "2");
});

test("uncapped sprite tiers ceil logical coverage", async () => {
  const runtime = await loadRuntime({
    webgl: false, sprites: 1, spriteHandles: [1], spriteSize: [5, 5], dpr: 1.1,
    assets: { "": "small.svg" },
    assetMetadata: { "": { encoding: "svg", prepared_width: 64, prepared_height: 64 } },
    createImageBitmap: (_source, options) => ({
      width: options.resizeWidth, height: options.resizeHeight, close() {}
    })
  });
  runtime.frame();
  assert.equal(runtime.body.dataset.assetPreparedTier, "1.25");
  assert.equal(runtime.body.dataset.assetPreparedWidth, "7");
  assert.equal(runtime.body.dataset.assetPreparedHeight, "7");
  assert.equal(runtime.stats.bitmapCalls[0].options.resizeWidth, 7);
  assert.equal(runtime.stats.bitmapCalls[0].options.resizeHeight, 7);
  assert.equal(runtime.body.dataset.assetFallback, "none");
});

test("large aspect-ratio sprite tiers use one uniform dimension cap", async () => {
  const runtime = await loadRuntime({
    webgl: false, sprites: 1, spriteHandles: [1], spriteSize: [4000, 1000], dpr: 3,
    assets: { "": "large.svg" },
    assetMetadata: { "": {
      encoding: "svg", prepared_width: 12000, prepared_height: 3000
    } },
    createImageBitmap: (_source, options) => ({
      width: options.resizeWidth, height: options.resizeHeight, close() {}
    })
  });
  runtime.frame();
  assert.equal(runtime.body.dataset.assetPreparedTier, "3");
  assert.equal(runtime.body.dataset.assetPreparedWidth, "8192");
  assert.equal(runtime.body.dataset.assetPreparedHeight, "2048");
  assert.equal(runtime.body.dataset.assetPreparedBytes, String(8192 * 2048 * 4));
  assert.equal(runtime.body.dataset.assetFallback, "raster-dimension");
  assert.equal(runtime.stats.bitmapCalls[0].options.resizeWidth, 8192);
  assert.equal(runtime.stats.bitmapCalls[0].options.resizeHeight, 2048);
  assert.equal(8192 / 2048, 4);
  assert.notEqual(runtime.stats.bitmapCalls[0].options.resizeHeight, 3000);
});

test("optimized sprite preparation resizes a Blob without constructing or decoding an Image", async () => {
  const bitmaps = [];
  const runtime = await loadRuntime({
    sprites: 1, spriteHandles: [1], spriteSize: [16, 16], assets: { "": "sprite.svg" },
    assetMetadata: { "": {
      encoding: "svg", prepared_width: 64, prepared_height: 64, prepared_bytes: 455,
      source_bytes: 4096, source_sha256: "source-master", prepared_sha256: "prepared-master"
    } },
    createImageBitmap: (_source, options) => {
      const bitmap = {
        width: options.resizeWidth, height: options.resizeHeight, closed: false,
        close() { this.closed = true; }
      };
      bitmaps.push(bitmap);
      return bitmap;
    }
  });
  runtime.frame();
  assert.equal(runtime.stats.imageConstructed, 0);
  assert.equal(runtime.stats.imageDecodeCalls, 0);
  assert.equal(runtime.stats.bitmapCalls[0].options.resizeWidth, 16);
  assert.equal(runtime.stats.bitmapCalls[0].options.resizeHeight, 16);
  assert.equal(runtime.stats.bitmapCalls[0].options.resizeQuality, "high");
  assert.equal(runtime.body.dataset.assetPreparedWidth, "16");
  assert.equal(runtime.body.dataset.assetPreparedBytes, String(16 * 16 * 4));
  assert.equal(runtime.body.dataset.assetPreparedFileBytes, "455");
  assert.equal(runtime.body.dataset.assetSourceWidth, "64");
  assert.equal(runtime.body.dataset.assetSourceHeight, "64");
  assert.equal(runtime.body.dataset.assetSourceBytes, "4096");
  assert.equal(runtime.body.dataset.assetDecodedWidth, "16");
  assert.equal(runtime.body.dataset.assetDecodedHeight, "16");
  assert.equal(runtime.body.dataset.assetDecodedBytes, String(16 * 16 * 4));
  assert.equal(runtime.body.dataset.assetCacheBytes, String(16 * 16 * 4));
  assert.equal(bitmaps[0].closed, false);
});

test("optimized sprite preparation preserves aspect ratio in a centered tier surface", async () => {
  const bitmaps = [];
  const runtime = await loadRuntime({
    webgl: false, sprites: 1, spriteHandles: [1], spriteSize: [16, 16], spriteUv: [0, 0, 1, 1],
    assets: { "": "wide.svg" },
    assetMetadata: { "": { encoding: "svg", prepared_width: 64, prepared_height: 32 } },
    createImageBitmap: (_source, options) => {
      const bitmap = {
        width: options.resizeWidth, height: options.resizeHeight, closed: false,
        close() { this.closed = true; }
      };
      bitmaps.push(bitmap);
      return bitmap;
    }
  });
  runtime.frame();
  assert.equal(runtime.stats.imageConstructed, 0);
  assert.equal(runtime.stats.imageDecodeCalls, 0);
  assert.deepEqual([
    runtime.stats.bitmapCalls[0].options.resizeWidth,
    runtime.stats.bitmapCalls[0].options.resizeHeight
  ], [16, 8]);
  assert.deepEqual(runtime.rasterStats.clears[0], [0, 0, 16, 16]);
  assert.deepEqual(runtime.rasterStats.images[0].slice(1), [0, 4, 16, 8]);
  assert.equal(runtime.stats.imageArgs[0][0], runtime.offscreen);
  assert.equal(runtime.offscreen.width, 16);
  assert.equal(runtime.offscreen.height, 16);
  assert.deepEqual(runtime.stats.imageArgs[0].slice(1), [-4, -5, 8, 10]);
  assert.equal(bitmaps[0].closed, false);
  assert.equal(runtime.body.dataset.assetPreparedWidth, "16");
  assert.equal(runtime.body.dataset.assetPreparedHeight, "16");
  assert.equal(runtime.body.dataset.assetDecodedWidth, "16");
  assert.equal(runtime.body.dataset.assetDecodedHeight, "8");
  assert.equal(runtime.body.dataset.assetDecodedBytes, String(16 * 8 * 4));
});

test("optimized contained sprite sheets use the unpadded bitmap for Canvas2D partial UVs", async () => {
  let bitmap;
  const runtime = await loadRuntime({
    webgl: false, sprites: 1, spriteHandles: [1], spriteSize: [16, 16], spriteUv: [0, 0, 0.5, 0.5],
    assets: { "": "wide-sheet.svg" },
    assetMetadata: { "": { encoding: "svg", prepared_width: 64, prepared_height: 32 } },
    createImageBitmap: (_source, options) => {
      bitmap = {
        width: options.resizeWidth, height: options.resizeHeight, closeCount: 0,
        close() { this.closeCount += 1; }
      };
      return bitmap;
    }
  });
  runtime.frame();

  const draw = runtime.stats.imageArgs[0];
  assert.equal(draw[0], bitmap);
  assert.deepEqual(draw.slice(1, 5), [0, 0, 8, 4]);
  assert.equal(bitmap.closeCount, 0);
  assert.equal(runtime.body.dataset.assetPreparedWidth, "16");
  assert.equal(runtime.body.dataset.assetPreparedHeight, "16");
  assert.equal(runtime.body.dataset.assetDecodedWidth, "16");
  assert.equal(runtime.body.dataset.assetDecodedHeight, "8");
  assert.equal(runtime.body.dataset.assetCacheBytes, String((16 * 16 + 16 * 8) * 4));

  runtime.env.gfx_release_sprite(1);
  assert.equal(bitmap.closeCount, 1);
  assert.equal(runtime.body.dataset.assetCacheBytes, "0");
  runtime.env.gfx_release_sprite(1);
  assert.equal(bitmap.closeCount, 1);
});

test("optimized contained sprite sheets use unpadded source dimensions in the WebGL atlas", async () => {
  let bitmap;
  const runtime = await loadRuntime({
    sprites: 64, spriteHandles: Array(64).fill(1), spriteSize: [16, 16], spriteUv: [0, 0, 0.5, 0.5],
    assets: { "": "wide-sheet.svg" },
    assetMetadata: { "": { encoding: "svg", prepared_width: 64, prepared_height: 32 } },
    createImageBitmap: (_source, options) => {
      bitmap = {
        width: options.resizeWidth, height: options.resizeHeight, closeCount: 0,
        close() { this.closeCount += 1; }
      };
      return bitmap;
    }
  });
  runtime.frame();

  const uv = runtime.stats.uploads[0].slice(4, 8);
  assert.deepEqual(uv.map(value => Math.round(value * 512)), [2, 2, 10, 6]);
  assert.equal(runtime.body.dataset.atlasPages, "1");
  assert.equal(runtime.body.dataset.atlasLiveEntries, "1");
  assert.equal(bitmap.closeCount, 0);

  runtime.env.gfx_release_sprite(1);
  assert.equal(bitmap.closeCount, 1);
  assert.equal(runtime.stats.deletedTextures, 1);
});

test("density refresh keeps the old sprite drawable until the replacement commits", async () => {
  const pending = [];
  const bitmaps = [];
  const makeBitmap = (width, height) => {
    const bitmap = { width, height, closed: false, close() { this.closed = true; } };
    bitmaps.push(bitmap);
    return bitmap;
  };
  const runtime = await loadRuntime({
    webgl: false, sprites: 1, spriteHandles: [1], spriteSize: [16, 16], dpr: 1,
    assets: { "": "refresh.svg" },
    assetMetadata: { "": { encoding: "svg", prepared_width: 64, prepared_height: 64 } },
    createImageBitmap: (_source, options, call) => {
      if (call === 1) return makeBitmap(options.resizeWidth, options.resizeHeight);
      return new Promise(resolve => pending.push({ resolve, options }));
    }
  });
  runtime.frame();
  runtime.stats.images = 0;
  runtime.contextObject.devicePixelRatio = 2;
  runtime.frame();
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(pending.length, 1);
  assert.equal(runtime.stats.images, 1);
  assert.equal(runtime.body.dataset.assetPreparedWidth, "16");
  assert.equal(runtime.body.dataset.assetPreparedTier, "1");
  assert.equal(runtime.body.dataset.assetReady, "true");
  assert.equal(runtime.body.dataset.assetRefreshState, "pending");
  assert.equal(runtime.body.dataset.assetRefreshFallback, "pending");
  assert.equal(bitmaps[0].closed, false);

  const replacement = makeBitmap(pending[0].options.resizeWidth, pending[0].options.resizeHeight);
  pending[0].resolve(replacement);
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(runtime.body.dataset.assetPreparedWidth, "32");
  assert.equal(runtime.body.dataset.assetPreparedTier, "2");
  assert.equal(runtime.body.dataset.assetRefreshState, "none");
  assert.equal(bitmaps[0].closed, true);
  assert.equal(replacement.closed, false);
  runtime.stats.images = 0;
  runtime.frame();
  assert.equal(runtime.stats.images, 1);
});

test("failed density refresh retains the old sprite cache and atlas ownership", async () => {
  const pending = [];
  const bitmaps = [];
  const makeBitmap = (width, height) => {
    const bitmap = { width, height, closed: false, close() { this.closed = true; } };
    bitmaps.push(bitmap);
    return bitmap;
  };
  const runtime = await loadRuntime({
    sprites: 64, spriteHandles: Array(64).fill(1), spriteSize: [16, 16], dpr: 1,
    imageDecode: () => Promise.reject(new Error("tier decode failed")),
    assets: { "": "refresh-failure.svg" },
    assetMetadata: { "": { encoding: "svg", prepared_width: 64, prepared_height: 64 } },
    createImageBitmap: (_source, options, call) => {
      if (call === 1) return makeBitmap(options.resizeWidth, options.resizeHeight);
      return new Promise((resolve, reject) => pending.push({ resolve, reject, options }));
    }
  });
  runtime.frame();
  assert.equal(runtime.body.dataset.atlasLiveEntries, "1");
  assert.equal(runtime.body.dataset.assetCacheBytes, String(16 * 16 * 4));
  runtime.stats.images = 0;
  runtime.contextObject.devicePixelRatio = 2;
  runtime.frame();
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(pending.length, 1);
  assert.equal(runtime.stats.images, 1);
  assert.equal(runtime.body.dataset.assetPreparedWidth, "16");
  assert.equal(runtime.body.dataset.assetRefreshState, "pending");
  assert.equal(runtime.body.dataset.atlasLiveEntries, "1");
  assert.equal(runtime.body.dataset.assetCacheBytes, String(16 * 16 * 4));
  assert.equal(runtime.stats.deletedTextures, 0);

  pending[0].reject(new Error("tier decode failed"));
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));
  runtime.stats.images = 0;
  runtime.frame();
  assert.equal(runtime.stats.images, 1);
  assert.equal(runtime.body.dataset.assetReady, "true");
  assert.equal(runtime.body.dataset.assetPreparedWidth, "16");
  assert.equal(runtime.body.dataset.assetRefreshState, "failed");
  assert.equal(runtime.body.dataset.assetRefreshError, "tier decode failed");
  assert.equal(runtime.body.dataset.assetRefreshFallback, "refresh-error");
  assert.equal(runtime.body.dataset.atlasLiveEntries, "1");
  assert.equal(runtime.body.dataset.assetCacheBytes, String(16 * 16 * 4));
  assert.equal(runtime.stats.deletedTextures, 0);
  assert.equal(bitmaps[0].closed, false);

  runtime.contextObject.devicePixelRatio = 3;
  runtime.frame();
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(pending.length, 2);
  assert.equal(runtime.body.dataset.assetRefreshState, "pending");
  assert.equal(runtime.body.dataset.assetPreparedWidth, "16");
  assert.equal(runtime.stats.deletedTextures, 0);
  const replacement = makeBitmap(pending[1].options.resizeWidth, pending[1].options.resizeHeight);
  pending[1].resolve(replacement);
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(runtime.body.dataset.assetPreparedWidth, "48");
  assert.equal(runtime.body.dataset.assetRefreshState, "none");
  assert.equal(bitmaps[0].closed, true);
  assert.equal(replacement.closed, false);
  assert.equal(runtime.stats.deletedTextures, 1);
  runtime.stats.images = 0;
  runtime.frame();
  assert.equal(runtime.stats.images, 1);
  runtime.env.gfx_release_sprite(1);
  assert.equal(replacement.closed, true);
  assert.equal(runtime.stats.deletedTextures, 2);
});

test("equivalent density scales reuse one stable requested-tier preparation", async () => {
  const runtime = await loadRuntime({
    sprites: 1, spriteHandles: [1], spriteSize: [16, 16], dpr: 1.1,
    assets: { "": "sprite.svg" },
    assetMetadata: { "": { encoding: "svg", prepared_width: 64, prepared_height: 64 } },
    createImageBitmap: (_source, options) => ({
      width: options.resizeWidth, height: options.resizeHeight, close() {}
    })
  });
  runtime.frame();
  assert.equal(runtime.body.dataset.assetPreparedTier, "1.25");
  assert.equal(runtime.body.dataset.assetPreparedWidth, "20");
  assert.equal(runtime.body.dataset.spriteRasterCount, "1");
  const firstDensityGeneration = Number(runtime.body.dataset.densityGeneration);
  const firstRasterScale = runtime.body.dataset.rasterScale;
  runtime.contextObject.devicePixelRatio = 1.2;
  runtime.frame();
  runtime.env.gfx_load_sprite(0, 16, 16);
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(runtime.body.dataset.spriteCacheHits, "1");
  assert.equal(runtime.body.dataset.spriteRasterCount, "1");
  assert.equal(runtime.body.dataset.densityTier, "1.25");
  assert.notEqual(runtime.body.dataset.rasterScale, firstRasterScale);
  assert.equal(Number(runtime.body.dataset.densityGeneration), firstDensityGeneration + 1);
});

test("stale density preparation cannot overwrite a newer tier and closes its bitmap", async () => {
  const pending = [];
  const bitmaps = [];
  const makeBitmap = (width, height) => {
    const bitmap = { width, height, closed: false, close() { this.closed = true; } };
    bitmaps.push(bitmap);
    return bitmap;
  };
  const runtime = await loadRuntime({
    sprites: 1, spriteHandles: [1], spriteSize: [16, 16], dpr: 1,
    assets: { "": "stale.svg" },
    assetMetadata: { "": { encoding: "svg", prepared_width: 64, prepared_height: 64 } },
    createImageBitmap: (_source, options, call) => {
      if (call === 1) return makeBitmap(options.resizeWidth, options.resizeHeight);
      return new Promise(resolve => pending.push({ resolve, options }));
    }
  });
  runtime.frame();
  runtime.contextObject.devicePixelRatio = 2;
  runtime.frame();
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(pending.length, 1);
  runtime.contextObject.devicePixelRatio = 3;
  runtime.frame();
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(pending.length, 2);
  const stale = makeBitmap(pending[0].options.resizeWidth, pending[0].options.resizeHeight);
  pending[0].resolve(stale);
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(stale.closed, true);
  assert.equal(runtime.body.dataset.assetPreparedWidth, "16");
  assert.equal(runtime.body.dataset.spriteStaleCount, "1");
  const current = makeBitmap(pending[1].options.resizeWidth, pending[1].options.resizeHeight);
  pending[1].resolve(current);
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(runtime.body.dataset.assetPreparedWidth, "48");
  assert.equal(runtime.body.dataset.assetPreparedTier, "3");
  assert.equal(runtime.body.dataset.assetGeneration, "3");
  assert.equal(current.closed, false);
});

test("shared pending sprite preparation keeps a remaining waiter alive", async () => {
  const pending = [];
  const bitmaps = [];
  const makeBitmap = (width, height) => {
    const bitmap = { width, height, closed: false, close() { this.closed = true; } };
    bitmaps.push(bitmap);
    return bitmap;
  };
  const runtime = await loadRuntime({
    sprites: 2, spriteHandles: [1, 2], spriteSize: [16, 16], dpr: 1,
    assets: { "": "shared.svg" },
    assetMetadata: { "": { encoding: "svg", prepared_width: 64, prepared_height: 64 } },
    createImageBitmap: (_source, options, call) => {
      if (call === 1) return makeBitmap(options.resizeWidth, options.resizeHeight);
      return new Promise(resolve => pending.push({ resolve, options }));
    }
  });
  runtime.frame();
  runtime.contextObject.devicePixelRatio = 2;
  runtime.frame();
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(pending.length, 1);

  runtime.env.gfx_release_sprite(1);
  const remaining = makeBitmap(pending[0].options.resizeWidth, pending[0].options.resizeHeight);
  pending[0].resolve(remaining);
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));
  runtime.stats.images = 0;
  runtime.frame();

  assert.equal(runtime.body.dataset.assetPreparedWidth, "32");
  assert.equal(runtime.stats.images, 1);
  assert.equal(remaining.closed, false);
  runtime.env.gfx_release_sprite(2);
  assert.equal(remaining.closed, true);
  assert.ok(bitmaps.length >= 2);
});

test("raster source underprovision is explicit instead of browser upscaling", async () => {
  const runtime = await loadRuntime({
    webgl: false, sprites: 1, spriteHandles: [1], spriteSize: [16, 16], imageExtent: [8, 4],
    assets: { "": "sprite.png" },
    assetMetadata: { "": { encoding: "png", prepared_width: 8, prepared_height: 4 } },
    createImageBitmap: () => { throw new Error("PNG source must not be enlarged"); }
  });
  runtime.frame();
  assert.equal(runtime.stats.imageConstructed, 1);
  assert.equal(runtime.stats.imageDecodeCalls, 1);
  assert.deepEqual(runtime.rasterStats.clears[0], [0, 0, 16, 16]);
  assert.deepEqual(runtime.rasterStats.images[0].slice(1), [4, 6, 8, 4]);
  assert.equal(runtime.body.dataset.assetFallback, "source-underprovisioned");
  assert.equal(runtime.body.dataset.assetPreparedWidth, "16");
  assert.equal(runtime.body.dataset.assetPreparedHeight, "16");
  assert.equal(runtime.body.dataset.assetDecodedWidth, "8");
  assert.equal(runtime.body.dataset.assetDecodedHeight, "4");
  assert.equal(runtime.body.dataset.assetDecodedBytes, String(8 * 4 * 4));
  assert.equal(runtime.body.dataset.assetSourceWidth, "8");
  assert.equal(runtime.body.dataset.assetSourceHeight, "4");
  assert.equal(runtime.body.dataset.assetCacheBytes, String(16 * 16 * 4));
});

test("underprovisioned raster content keeps logical size across density tiers", async () => {
  const runtime = await loadRuntime({
    webgl: false, sprites: 1, spriteHandles: [1], spriteSize: [16, 16], dpr: 1,
    imageExtent: [8, 4], assets: { "": "density.png" },
    assetMetadata: { "": { encoding: "png", prepared_width: 8, prepared_height: 4 } },
    createImageBitmap: () => { throw new Error("PNG source must not be enlarged"); }
  });
  runtime.frame();
  assert.deepEqual(runtime.rasterStats.images[0].slice(1), [4, 6, 8, 4]);
  assert.equal(runtime.body.dataset.assetFallback, "source-underprovisioned");

  runtime.rasterStats.images.length = 0;
  runtime.contextObject.devicePixelRatio = 2;
  runtime.frame();
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));

  assert.deepEqual(runtime.rasterStats.images[0].slice(1), [8, 12, 16, 8]);
  assert.equal(runtime.body.dataset.assetPreparedWidth, "32");
  assert.equal(runtime.body.dataset.assetPreparedHeight, "32");
  assert.equal(runtime.body.dataset.assetDecodedWidth, "8");
  assert.equal(runtime.body.dataset.assetDecodedHeight, "4");
  assert.equal(runtime.body.dataset.assetFallback, "source-underprovisioned");
  assert.equal(runtime.body.dataset.assetCacheBytes, String(32 * 32 * 4));
});

test("underprovisioned sprite sheets use raw source regions for Canvas2D", async () => {
  const runtime = await loadRuntime({
    webgl: false, sprites: 1, spriteHandles: [1], spriteSize: [96, 96], spriteUv: [0, 0, 0.5, 0.5],
    imageExtent: [2, 2], assets: { "": "sheet.png" },
    assetMetadata: { "": { encoding: "png", prepared_width: 96, prepared_height: 96 } }
  });
  runtime.frame();
  const draw = runtime.stats.imageArgs[0];
  assert.deepEqual(draw.slice(1, 5), [0, 0, 1, 1]);
  assert.equal(runtime.body.dataset.assetPreparedWidth, "96");
  assert.equal(runtime.body.dataset.assetPreparedHeight, "96");
  assert.equal(runtime.body.dataset.assetDecodedWidth, "2");
  assert.equal(runtime.body.dataset.assetDecodedHeight, "2");
  assert.equal(runtime.body.dataset.assetFallback, "source-underprovisioned");
});

test("underprovisioned sprite sheets use raw source regions in the WebGL atlas", async () => {
  const runtime = await loadRuntime({
    sprites: 64, spriteHandles: Array(64).fill(1), spriteSize: [96, 96], spriteUv: [0, 0, 0.5, 0.5],
    imageExtent: [2, 2], assets: { "": "sheet.png" },
    assetMetadata: { "": { encoding: "png", prepared_width: 96, prepared_height: 96 } }
  });
  runtime.frame();
  const uv = runtime.stats.uploads[0].slice(4, 8);
  assert.deepEqual(uv.map(value => Math.round(value * 512)), [2, 2, 3, 3]);
  assert.equal(runtime.body.dataset.atlasPages, "1");
  assert.equal(runtime.body.dataset.atlasLiveEntries, "1");
  assert.equal(runtime.body.dataset.assetPreparedWidth, "96");
  assert.equal(runtime.body.dataset.assetDecodedWidth, "2");
});

test("released atlas allocations are reused by a later sprite variant", async () => {
  const handles = Array.from({ length: 64 }, (_, index) => (index % 4) + 1);
  const runtime = await loadRuntime({ sprites: 64, spriteHandles: handles, spriteSize: [16, 16] });
  runtime.frame();
  assert.equal(runtime.body.dataset.atlasPages, "1");
  assert.equal(runtime.body.dataset.atlasLiveEntries, "4");
  const atlasBytes = runtime.body.dataset.assetAtlasBytes;
  runtime.env.gfx_release_sprite(1);
  const replacement = runtime.env.gfx_load_sprite(0, 16, 16);
  assert.equal(replacement, 5);
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));
  handles.fill(5);
  runtime.frame();
  assert.equal(runtime.body.dataset.atlasPages, "1");
  assert.equal(runtime.body.dataset.atlasLiveEntries, "4");
  assert.equal(runtime.body.dataset.assetAtlasBytes, atlasBytes);
  assert.equal(runtime.body.dataset.assetAtlasFallback, "none");
  assert.equal(runtime.stats.deletedTextures, 0);
});

test("staggered density refreshes recycle atlas space between separate commits", async () => {
  const handles = Array.from({ length: 64 }, (_, index) => (index % 4) + 1);
  const pending = [];
  const makeBitmap = (width, height) => ({ width, height, close() {} });
  const runtime = await loadRuntime({
    sprites: 64, spriteHandles: handles,
    spriteSizes: [[80, 80], [81, 80], [82, 80], [83, 80]],
    assets: { "": "staggered.svg" },
    assetMetadata: { "": { encoding: "svg", prepared_width: 512, prepared_height: 512 } },
    createImageBitmap: (_source, options, call) => {
      if (call <= 4) return makeBitmap(options.resizeWidth, options.resizeHeight);
      return new Promise(resolve => pending.push({ resolve, options }));
    }
  });
  runtime.frame();
  const stablePages = Number(runtime.body.dataset.atlasPages);
  const stableBytes = runtime.body.dataset.assetAtlasBytes;
  assert.equal(stablePages, 1);
  assert.equal(runtime.body.dataset.atlasLiveEntries, "4");
  assert.equal(runtime.body.dataset.assetAtlasFallback, "none");

  const flush = () => new Promise(resolve => setImmediate(resolve));
  let transition = 0;
  for (const dpr of [2, 1, 2, 1, 2, 1, 2, 1, 2, 1, 2, 1, 2, 1]) {
    transition += 1;
    runtime.contextObject.devicePixelRatio = dpr;
    runtime.frame();
    await flush();
    await flush();
    assert.equal(pending.length, 4);
    for (let index = 0; index < 4; index += 1) {
      const replacement = pending.shift();
      replacement.resolve(makeBitmap(
        replacement.options.resizeWidth, replacement.options.resizeHeight
      ));
      await flush();
      await flush();
      const before = runtime.stats.instanced;
      runtime.frame();
      assert.ok(runtime.stats.instanced > before);
      assert.equal(runtime.body.dataset.atlasLiveEntries, "4");
      assert.ok(Number(runtime.body.dataset.atlasPages) <= stablePages);
      assert.equal(runtime.body.dataset.assetAtlasFallback, "none");
      assert.equal(runtime.body.dataset.backend, "Canvas2D + WebGL2");
    }
    assert.equal(runtime.body.dataset.atlasPages, String(stablePages));
    assert.equal(runtime.body.dataset.assetAtlasBytes, stableBytes);
    assert.equal(runtime.body.dataset.atlasLiveEntries, "4");
    assert.equal(runtime.body.dataset.assetAtlasGeneration, String(transition + 1));
  }
});

test("many staggered density refreshes keep mixed atlas pages bounded", async () => {
  const resourceCount = 20;
  const handles = Array.from({ length: resourceCount * 64 }, (_, index) => (Math.floor(index / 64) % resourceCount) + 1);
  const sizes = Array.from({ length: resourceCount }, (_, index) => [60 + index, 60 + index]);
  const pending = [];
  const makeBitmap = (width, height) => ({ width, height, close() {} });
  const runtime = await loadRuntime({
    sprites: resourceCount * 64, spriteHandles: handles, spriteSizes: sizes,
    assets: { "": "many-staggered.svg" },
    assetMetadata: { "": { encoding: "svg", prepared_width: 512, prepared_height: 512 } },
    createImageBitmap: (_source, options, call) => {
      if (call <= resourceCount) return makeBitmap(options.resizeWidth, options.resizeHeight);
      return new Promise(resolve => pending.push({ resolve, options }));
    }
  });
  runtime.frame();
  let maximumPages = Number(runtime.body.dataset.atlasPages);
  let maximumBytes = Number(runtime.body.dataset.assetAtlasBytes);
  const flush = () => new Promise(resolve => setImmediate(resolve));
  let transition = 0;
  for (const dpr of [2, 1, 2, 1, 2, 1, 2, 1]) {
    transition += 1;
    runtime.contextObject.devicePixelRatio = dpr;
    runtime.frame();
    await flush();
    await flush();
    assert.equal(pending.length, resourceCount);
    for (let index = 0; index < resourceCount; index += 1) {
      const replacement = pending.shift();
      replacement.resolve(makeBitmap(
        replacement.options.resizeWidth, replacement.options.resizeHeight
      ));
      await flush();
      await flush();
      const before = runtime.stats.instanced;
      runtime.frame();
      assert.ok(runtime.stats.instanced > before,
        `GPU batch missing at transition ${transition}, resource ${index}, backend ${runtime.body.dataset.backend}, pages ${runtime.body.dataset.atlasPages}, images ${runtime.stats.images}`);
      assert.equal(runtime.body.dataset.atlasLiveEntries, String(resourceCount));
      assert.equal(runtime.body.dataset.assetAtlasFallback, "none");
      assert.equal(runtime.body.dataset.backend, "Canvas2D + WebGL2");
      maximumPages = Math.max(maximumPages, Number(runtime.body.dataset.atlasPages));
      maximumBytes = Math.max(maximumBytes, Number(runtime.body.dataset.assetAtlasBytes));
    }
    assert.equal(runtime.body.dataset.atlasLiveEntries, String(resourceCount));
    assert.equal(runtime.body.dataset.assetAtlasGeneration, String(transition + 1));
  }
  assert.ok(maximumPages <= 3, `atlas pages grew to ${maximumPages}`);
  assert.ok(maximumBytes <= 3 * 512 * 512 * 4, `atlas bytes grew to ${maximumBytes}`);
  assert.equal(runtime.body.dataset.assetAtlasFallback, "none");
  assert.equal(runtime.body.dataset.backend, "Canvas2D + WebGL2");
});

test("releasing the latest Image fallback clears its retained resource receipt", async () => {
  const runtime = await loadRuntime({
    webgl: false, sprites: 1, spriteHandles: [1], spriteSize: [16, 16], imageExtent: [8, 4],
    assets: { "": "released.png" },
    assetMetadata: { "": { encoding: "png", prepared_width: 8, prepared_height: 4 } }
  });
  runtime.frame();
  assert.equal(runtime.body.dataset.assetReady, "true");
  assert.equal(runtime.body.dataset.assetPreparedWidth, "16");
  assert.equal(runtime.body.dataset.assetCacheBytes, String(16 * 16 * 4));
  assert.equal(runtime.body.dataset.assetSource, "released.png");

  runtime.env.gfx_release_sprite(1);
  assert.equal(runtime.body.dataset.assetReady, "false");
  assert.equal(runtime.body.dataset.assetCacheBytes, "0");
  assert.equal(runtime.body.dataset.assetPreparedWidth, "16");
  assert.equal(runtime.body.dataset.assetDecodedWidth, "8");
  assert.equal(runtime.body.dataset.assetSourceWidth, "8");

  runtime.frame();
  assert.equal(runtime.body.dataset.assetReady, "false");
  assert.equal(runtime.body.dataset.assetCacheBytes, "0");
  assert.equal(runtime.body.dataset.assetPreparedWidth, "16");
  assert.equal(runtime.body.dataset.assetSource, "released.png");
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
  assert.equal(runtime.body.dataset.assetAtlasFallback, "dimension");
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
