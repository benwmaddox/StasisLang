import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";

const source = fs.readFileSync(new URL("../game.js", import.meta.url), "utf8");
const MAGIC = 1196967473;
const I32_COUNT = 67888;
const F32_COUNT = 146564;
const F32_OFFSET = 300000;
const ORDER_BASE = 51232;
const RUN_BASE = 18464;
const CLIP_BASE = 145540;

test("sprite loader publishes the opaque reference field", () => {
  assert.match(
    source,
    /stasis_jit_sprite_load_from:[\s\S]*?setViewField\(base, index, "sprite_ref", handle\)/,
  );
  assert.doesNotMatch(
    source,
    /return setViewField\(base, index, "handle", handle\)/,
  );
});

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
    viewport(_x, _y, width, height) { stats.viewports.push([width, height]); }, clearColor() {}, clear() {}, useProgram() {}, uniform2f(_location, width, height) {
      stats.uniforms.push([width, height]);
    }, uniform1i() {},
    texParameteri() {}, pixelStorei() {}, texImage2D() { if (textureThrow) throw new Error("fake texture failure"); }, texSubImage2D() { if (textureThrow) throw new Error("fake texture failure"); }, generateMipmap() {}, activeTexture() {}, bindTexture() {}, getError: () => 0,
    isContextLost: () => stats.contextLost, getParameter: () => 4096,
    enable() {}, disable() {}, scissor(x, y, width, height) { stats.scissors.push([x, y, width, height]); }, blendFunc() {}, blendFuncSeparate() {}, drawArraysInstanced(_mode, _first, _vertices, count) {
      stats.instanced += 1;
      stats.instances.push(count);
    }
  };
  return gl;
}

async function loadRuntime({ rects = 0, rectSizes = null, rectAlpha = 1, ordered = null, clips = [], sprites = 0, spriteHandles = [], spriteSize = null, spriteSizes = null, spriteUv = [0.1, 0.2, 0.9, 0.8], spritePivot = [4, 5], spriteScale = [1, 1], instanceFlags = 0, runMetadata = [0, 0, 0, 0, 0], webgl = true, throwing = false, textureThrow = false, imageReady = true, timing = false, dpr = 1, cssExtent = [640, 360], imageExtent = [16, 16], assetMetadata = {}, assets = {}, createImageBitmap = null, imageDecode = null, fetchBlob = null } = {}) {
  const memory = new WebAssembly.Memory({ initial: 16 });
  const i32 = new Int32Array(memory.buffer, 0, I32_COUNT);
  const f32 = new Float32Array(memory.buffer, F32_OFFSET, F32_COUNT);
  const stats = { instanced: 0, instances: [], uploadedFloats: [], uploads: [], uniforms: [], viewports: [], scissors: [], transforms: [], imageArgs: [], images: 0, fills: 0, events: [], clipRects: [], clipCalls: 0, restores: 0, contextLost: false, imageDecodeCalls: 0, imageConstructed: 0, bitmapCalls: [], deletedTextures: 0 };
  let now = 0;
  const context2d = {
    globalAlpha: 1,
    setTransform(...value) { stats.transforms.push(value); },
    fillRect() { stats.fills += 1; stats.events.push("fill"); if (timing) now += 4; },
    fillText() {}, drawImage(...args) { stats.images += 1; stats.imageArgs.push(args); stats.events.push("image"); },
    save() {}, restore() { stats.restores += 1; }, beginPath() {}, moveTo() {}, lineTo() {},
    rect(x, y, width, height) { stats.clipRects.push([x, y, width, height]); },
    clip() { stats.clipCalls += 1; },
    stroke() { stats.events.push("stroke"); }, translate() {}, rotate() {}, scale() {}
  };
  const rasterStats = { draws: 0, images: [], clears: [] };
  const rasterContext = {
    imageSmoothingEnabled: true, imageSmoothingQuality: "high",
    clearRect(...args) { rasterStats.clears.push(args); }, drawImage(...args) { rasterStats.draws += 1; rasterStats.images.push(args); },
    measureText(text) { return { width: String(text).length * 8, actualBoundingBoxDescent: 4 }; },
    fillText() {}, save() {}, restore() {}
  };
  const gl = fakeGl(stats, true, throwing, textureThrow);
  const canvasListeners = new Map();
  const canvas = {
    width: 640, height: 360, style: {}, parentElement: { style: {} },
    dataset: {},
    getContext: kind => kind === "2d" ? context2d : gl,
    getBoundingClientRect: () => ({ left: 0, top: 0, width: cssExtent[0], height: cssExtent[1] }),
    addEventListener(type, callback) { canvasListeners.set(type, callback); }, setPointerCapture() {}, focus() {}, requestFullscreen: async () => {}
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
  let textFixture = null;
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
      if (!rects && !sprites && !clips.length && !textFixture) return;
      i32[0] = MAGIC; i32[1] = 7; i32[2] = 0; i32[3] = ordered ? 1 : 0; i32[4] = sprites; i32[7] = 0; i32[24] = rects; i32[27] = clips.length;
      if (textFixture) {
        i32[7] = 1;
        i32[12320] = textFixture.font;
        i32[12321] = -textFixture.handle;
        i32[12322] = 0;
        f32[133252] = 4; f32[133253] = 8;
        f32[133254] = 1; f32[133255] = 1; f32[133256] = 1; f32[133257] = 1;
      }
      const encodedOrder = [];
      const runs = [];
      if (ordered) {
        i32[3] = 1;
        for (let position = 0; position < ordered.length;) {
          const encoded = ordered[position];
          const kind = Math.floor(encoded / 16384);
          if (kind !== 2) {
            encodedOrder.push(encoded);
            position += 1;
            continue;
          }
          const first = encoded % 16384;
          let count = 1;
          while (position + count < ordered.length
              && ordered[position + count] === 2 * 16384 + first + count) count += 1;
          const run = runs.length;
          runs.push([first, count]);
          encodedOrder.push(2 * 16384 + run);
          position += count;
        }
        i32[22] = encodedOrder.length;
        encodedOrder.forEach((encoded, index) => { i32[ORDER_BASE + index] = encoded; });
      } else if (sprites > 0) {
        runs.push([0, sprites]);
      }
      i32[29] = runs.length;
      runs.forEach(([first, count], index) => {
        const base = RUN_BASE + index * 8;
        i32[base] = first; i32[base + 1] = count; i32[base + 2] = -1;
        for (let field = 0; field < 5; field += 1) i32[base + 3 + field] = runMetadata[field];
      });
      for (let index = 0; index < rects; index += 1) {
        const base = 79996 - index * 8;
        const size = rectSizes?.[index] || [2, 2];
        f32[base] = index; f32[base + 1] = 1; f32[base + 2] = size[0]; f32[base + 3] = size[1];
        f32[base + 4] = 1; f32[base + 5] = 0; f32[base + 6] = 0; f32[base + 7] = rectAlpha;
      }
      for (let index = 0; index < sprites; index += 1) {
        const baseI = 32 + index * 3;
        const baseF = 80004 + index * 13;
        i32[baseI] = spriteHandles[index] || 1;
        i32[baseI + 1] = 0xffffffb4;
        i32[baseI + 2] = instanceFlags;
        f32[baseF] = index + 0.5; f32[baseF + 1] = 2; f32[baseF + 2] = 8; f32[baseF + 3] = 10;
        const dimensions = spriteSizes?.[(spriteHandles[index] || 1) - 1] || spriteSize || imageExtent;
        const partial = spriteUv[0] !== 0 || spriteUv[1] !== 0 || spriteUv[2] !== 1 || spriteUv[3] !== 1;
        f32[baseF + 4] = partial ? spriteUv[0] * dimensions[0] : 0;
        f32[baseF + 5] = partial ? spriteUv[1] * dimensions[1] : 0;
        f32[baseF + 6] = partial ? (spriteUv[2] - spriteUv[0]) * dimensions[0] : 0;
        f32[baseF + 7] = partial ? (spriteUv[3] - spriteUv[1]) * dimensions[1] : 0;
        f32[baseF + 8] = spritePivot[0]; f32[baseF + 9] = spritePivot[1];
        f32[baseF + 10] = spriteScale[0]; f32[baseF + 11] = spriteScale[1]; f32[baseF + 12] = index * 10;
      }
      clips.forEach((clip, index) => {
        const base = CLIP_BASE + index * 4;
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
  contextObject.window = { STASIS_GAME: { memory: { gfx_cmd_i32: { offset: 0, length: I32_COUNT }, gfx_cmd_f32: { offset: F32_OFFSET, length: F32_COUNT }, host_i32: { offset: 900000, length: 768 }, host_f32: { offset: 903072, length: 64 } }, strings: {}, assets, asset_metadata: assetMetadata }, screen: contextObject.screen };
  vm.runInNewContext(source, contextObject, { filename: "runtime/web/game.js" });
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(body.dataset.ready, "true");
  return {
    stats, body, hud, rasterStats, canvas, offscreen, env, contextObject, frame: () => raf.shift()(now),
    setPresentation(width, height, nextDpr = contextObject.devicePixelRatio) {
      cssExtent[0] = width;
      cssExtent[1] = height;
      contextObject.devicePixelRatio = nextDpr;
    },
    setTextFixture(font, text, textId = 1) {
      contextObject.window.STASIS_GAME.strings[textId] = text;
      textFixture = { font, handle: env.stasis_jit_gfx_cache_text(font, textId) };
    },
    loseContext: () => { stats.contextLost = true; canvasListeners.get("webglcontextlost")?.({ preventDefault() {} }); },
    restoreContext: () => { stats.contextLost = false; canvasListeners.get("webglcontextrestored")?.({}); }
  };
}

test("large ordered rectangle run uses one visible WebGL2 submission and no composite", async () => {
  const runtime = await loadRuntime({ rects: 64 });
  runtime.frame();
  assert.equal(runtime.stats.instanced, 1);
  assert.deepEqual(runtime.stats.instances, [64]);
  assert.deepEqual(runtime.stats.uploadedFloats, [64 * 16]);
  assert.equal(runtime.stats.images, 0);
  assert.equal(runtime.stats.fills, 0);
});

test("visible WebGL2 uses the physical framebuffer with logical shader dimensions", async () => {
  const runtime = await loadRuntime({ rects: 64, cssExtent: [800, 450], dpr: 2 });
  runtime.frame();
  assert.deepEqual(runtime.stats.uniforms[0], [640, 360]);
  assert.equal(runtime.canvas.width, 1600);
  assert.equal(runtime.canvas.height, 900);
  assert.ok(runtime.stats.viewports.some(value => value[0] === 1600 && value[1] === 900));
  assert.equal(runtime.stats.images, 0);
});

test("ordered clipping intersects nested logical clips through WebGL scissor", async () => {
  const scale = 16384;
  const runtime = await loadRuntime({
    clips: [[10, 12, 100, 80], [25, 30, 40, 24]],
    ordered: [5 * scale, 5 * scale + 1, 6 * scale, 6 * scale]
  });
  runtime.frame();
  assert.deepEqual(runtime.stats.scissors, [
    [10, 268, 100, 80], [25, 306, 40, 24], [10, 268, 100, 80]
  ]);
});

test("line barriers preserve source order between rectangle submissions", async () => {
  const first = Array.from({ length: 64 }, (_, index) => 4 * 16384 + index);
  const second = Array.from({ length: 64 }, (_, index) => 4 * 16384 + 64 + index);
  const runtime = await loadRuntime({ rects: 128, ordered: [...first, 16384, ...second] });
  runtime.frame();
  assert.equal(runtime.stats.instanced, 2);
  assert.deepEqual(runtime.stats.instances, [64, 64]);
  assert.equal(runtime.stats.images, 0);
});

test("WebGL initialization failure has an unsupported state and no fallback code", () => {
  assert.match(source, /dataset\.backend = "unsupported"/);
  assert.match(source, /WebGL2 is required by the Stasis Web renderer/);
  assert.doesNotMatch(source, /context\.drawImage\(target|context\.fillRect|context\.fillText/);
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
    1, 1, 1, new Float32Array([180 / 255])[0], 0, 1, 4, 5
  ]);
  const atlasUv = runtime.stats.uploads[0].slice(4, 8);
  assert.ok(atlasUv[0] > 0 && atlasUv[1] > 0 && atlasUv[2] < 1 && atlasUv[3] < 1);
  assert.ok(atlasUv[0] < atlasUv[2] && atlasUv[1] < atlasUv[3]);
  assert.equal(runtime.stats.images, 0);
  assert.equal(runtime.body.dataset.assetAtlasWidth, "512");
  assert.equal(runtime.body.dataset.assetAtlasHeight, "512");
  assert.equal(runtime.body.dataset.assetAtlasBytes, String(512 * 512 * 4));
});

test("requested sprite dimensions rasterize once before WebGL atlas upload", async () => {
  const runtime = await loadRuntime({ sprites: 1, spriteHandles: [1], spriteSize: [4, 4] });
  runtime.frame();
  assert.equal(runtime.stats.images, 0);
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

test("orientation and DPR settlement reuse decoded sprite ownership without tier thrash", async () => {
  const runtime = await loadRuntime({
    webgl: false, sprites: 1, spriteHandles: [1], spriteSize: [16, 16],
    cssExtent: [360, 720], dpr: 1, imageExtent: [64, 64],
    assets: { "": "orientation.svg" },
    assetMetadata: { "": { encoding: "svg", prepared_width: 64, prepared_height: 64 } }
  });
  runtime.frame();
  assert.equal(runtime.body.dataset.spriteRasterCount, "1");
  assert.equal(runtime.body.dataset.spriteDecodedCount, "1");
  assert.equal(runtime.body.dataset.assetCacheBytes, String(16 * 16 * 4));

  runtime.setPresentation(960, 480, 1);
  runtime.frame();
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(runtime.body.dataset.spriteRasterCount, "2");
  assert.equal(runtime.body.dataset.spriteDecodedCount, "1");
  assert.equal(runtime.body.dataset.assetCacheBytes, String(24 * 24 * 4));

  runtime.setPresentation(960, 480, 2);
  runtime.frame();
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(runtime.body.dataset.spriteRasterCount, "3");
  assert.equal(runtime.body.dataset.spriteDecodedCount, "1");
  const settledBytes = Number(runtime.body.dataset.assetCacheBytes);
  assert.equal(settledBytes, 48 * 48 * 4, "only the current landscape DPR tier remains owned");
  runtime.frame();
  runtime.frame();
  assert.equal(runtime.body.dataset.spriteRasterCount, "3", "settled frames do not create new tiers");
  assert.equal(runtime.body.dataset.assetCacheBytes, String(settledBytes));

  runtime.setPresentation(360, 720, 1);
  runtime.frame();
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(runtime.body.dataset.spriteRasterCount, "4", "one real orientation transition prepares one tier");
  assert.equal(runtime.body.dataset.spriteDecodedCount, "1");
  assert.equal(runtime.body.dataset.assetCacheBytes, String(16 * 16 * 4), "obsolete landscape tiers are released");
  runtime.frame();
  runtime.frame();
  assert.equal(runtime.body.dataset.spriteRasterCount, "4", "portrait settlement does not thrash");
  assert.equal(runtime.body.dataset.spriteStaleCount, "0");
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

test("release metadata projection preserves Canvas2D preparation and WebGL2 upload", async () => {
  const retained = {
    encoding: "svg", prepared_width: 64, prepared_height: 32,
    logical_width: 16, logical_height: 8,
  };
  const auditOnly = {
    path: "assets/wide.svg", prepared_bytes: 455, source_bytes: 4096,
    source_sha256: "source-master", prepared_sha256: "prepared-master",
  };
  const run = metadata => loadRuntime({
    sprites: 1, spriteHandles: [1], spriteSize: [16, 8], spriteUv: [0, 0, 1, 1],
    assets: { "": "wide.svg" }, assetMetadata: { "": metadata },
    createImageBitmap: (_source, options) => ({
      width: options.resizeWidth, height: options.resizeHeight, close() {}
    })
  });
  const projected = await run(retained);
  const diagnostic = await run({ ...retained, ...auditOnly });
  projected.frame();
  diagnostic.frame();

  for (const runtime of [projected, diagnostic]) {
    assert.deepEqual([
      runtime.stats.bitmapCalls[0].options.resizeWidth,
      runtime.stats.bitmapCalls[0].options.resizeHeight,
    ], [16, 8]);
    assert.ok(runtime.rasterStats.draws > 0);
    assert.ok(runtime.stats.instanced > 0);
    assert.equal(runtime.body.dataset.assetFallback, "none");
  }
  assert.equal(projected.rasterStats.draws, diagnostic.rasterStats.draws);
  assert.equal(projected.stats.instanced, diagnostic.stats.instanced);
  assert.equal(projected.body.dataset.assetPreparedFileBytes, "0");
  assert.equal(projected.body.dataset.assetSourceBytes, "0");
  assert.equal(diagnostic.body.dataset.assetPreparedFileBytes, "455");
  assert.equal(diagnostic.body.dataset.assetSourceBytes, "4096");
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
  assert.equal(runtime.stats.images, 0);
  assert.equal(runtime.stats.instanced, 1);
  assert.equal(bitmaps[0].closed, false);
  assert.equal(runtime.body.dataset.assetPreparedWidth, "16");
  assert.equal(runtime.body.dataset.assetPreparedHeight, "16");
  assert.equal(runtime.body.dataset.assetDecodedWidth, "16");
  assert.equal(runtime.body.dataset.assetDecodedHeight, "8");
  assert.equal(runtime.body.dataset.assetDecodedBytes, String(16 * 8 * 4));
});

test("optimized contained sprite sheets upload unpadded partial UVs to WebGL", async () => {
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

  assert.equal(runtime.stats.images, 0);
  assert.equal(runtime.stats.instanced, 1);
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
  const atlasPixels = uv.map(value => Math.round(value * 512));
  assert.deepEqual([atlasPixels[2] - atlasPixels[0], atlasPixels[3] - atlasPixels[1]], [8, 4]);
  assert.equal(runtime.body.dataset.atlasPages, "1");
  assert.equal(runtime.body.dataset.atlasLiveEntries, "2");
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
  runtime.stats.instanced = 0;
  runtime.contextObject.devicePixelRatio = 2;
  runtime.frame();
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(pending.length, 1);
  assert.equal(runtime.stats.instanced, 1);
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
  runtime.stats.instanced = 0;
  runtime.frame();
  assert.equal(runtime.stats.instanced, 1);
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
  runtime.stats.instanced = 0;
  runtime.contextObject.devicePixelRatio = 2;
  runtime.frame();
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(pending.length, 1);
  assert.equal(runtime.stats.instanced, 1);
  assert.equal(runtime.body.dataset.assetPreparedWidth, "16");
  assert.equal(runtime.body.dataset.assetRefreshState, "pending");
  assert.equal(runtime.body.dataset.atlasLiveEntries, "1");
  assert.equal(runtime.body.dataset.assetCacheBytes, String(16 * 16 * 4));
  assert.equal(runtime.stats.deletedTextures, 0);

  pending[0].reject(new Error("tier decode failed"));
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));
  runtime.stats.instanced = 0;
  runtime.frame();
  assert.equal(runtime.stats.instanced, 1);
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
  runtime.stats.instanced = 0;
  runtime.frame();
  assert.equal(runtime.stats.instanced, 1);
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
  runtime.frame();

  assert.equal(runtime.body.dataset.assetPreparedWidth, "32");
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

test("underprovisioned sprite sheets retain raw source proportions in WebGL", async () => {
  const runtime = await loadRuntime({
    webgl: false, sprites: 1, spriteHandles: [1], spriteSize: [96, 96], spriteUv: [0, 0, 0.5, 0.5],
    imageExtent: [2, 2], assets: { "": "sheet.png" },
    assetMetadata: { "": { encoding: "png", prepared_width: 96, prepared_height: 96 } }
  });
  runtime.frame();
  assert.equal(runtime.stats.images, 0);
  assert.equal(runtime.stats.instanced, 1);
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
  const atlasPixels = uv.map(value => Math.round(value * 512));
  assert.deepEqual([atlasPixels[2] - atlasPixels[0], atlasPixels[3] - atlasPixels[1]], [1, 1]);
  assert.equal(runtime.body.dataset.atlasPages, "1");
  assert.equal(runtime.body.dataset.atlasLiveEntries, "2");
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
  const stableEntries = runtime.body.dataset.atlasLiveEntries;
  assert.equal(stablePages, 1);
  assert.ok(Number(stableEntries) >= 4 && Number(stableEntries) <= 8);
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
      assert.equal(runtime.body.dataset.atlasLiveEntries, stableEntries);
      assert.ok(Number(runtime.body.dataset.atlasPages) <= 2);
      assert.equal(runtime.body.dataset.assetAtlasFallback, "none");
      assert.equal(runtime.body.dataset.backend, "WebGL2");
    }
    assert.ok(Number(runtime.body.dataset.atlasPages) <= 2);
    assert.ok(Number(runtime.body.dataset.assetAtlasBytes) <= Number(stableBytes) * 2);
    assert.equal(runtime.body.dataset.atlasLiveEntries, stableEntries);
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
      assert.equal(runtime.body.dataset.backend, "WebGL2");
      maximumPages = Math.max(maximumPages, Number(runtime.body.dataset.atlasPages));
      maximumBytes = Math.max(maximumBytes, Number(runtime.body.dataset.assetAtlasBytes));
    }
    assert.equal(runtime.body.dataset.atlasLiveEntries, String(resourceCount));
    assert.equal(runtime.body.dataset.assetAtlasGeneration, String(transition + 1));
  }
  assert.ok(maximumPages <= 3, `atlas pages grew to ${maximumPages}`);
  assert.ok(maximumBytes <= 3 * 512 * 512 * 4, `atlas bytes grew to ${maximumBytes}`);
  assert.equal(runtime.body.dataset.assetAtlasFallback, "none");
  assert.equal(runtime.body.dataset.backend, "WebGL2");
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

test("same-domain sprites and an interleaved solid rectangle share one ordered quad batch", async () => {
  const first = Array.from({ length: 64 }, (_, index) => 2 * 16384 + index);
  const second = Array.from({ length: 64 }, (_, index) => 2 * 16384 + 64 + index);
  const runtime = await loadRuntime({
    rects: 1, sprites: 128, spriteHandles: [...Array(64).fill(1), ...Array(64).fill(2)],
    ordered: [...first, 4 * 16384, ...second]
  });
  runtime.frame();
  assert.equal(runtime.stats.instanced, 1);
  assert.deepEqual(runtime.stats.instances, [129]);
  assert.equal(runtime.stats.images, 0);
  assert.equal(runtime.body.dataset.composites, "0");
});

test("A-B-C-A-C-B preserves translucent painter order in one same-page submission", async () => {
  const scale = 16384;
  const runtime = await loadRuntime({
    rects: 2, rectSizes: [[29, 53], [7, 41]], rectAlpha: 0.47,
    sprites: 4, spriteHandles: [1, 2, 1, 2],
    ordered: [2 * scale, 2 * scale + 1, 4 * scale, 2 * scale + 2, 4 * scale + 1, 2 * scale + 3]
  });
  runtime.frame();
  assert.equal(runtime.stats.instanced, 1);
  assert.deepEqual(runtime.stats.instances, [6]);
  assert.equal(runtime.body.dataset.composites, "0");
  assert.equal(runtime.body.dataset.atlasTransitions, "0");
  assert.equal(runtime.body.dataset.uploadedBytes, String(6 * 64));
  const records = runtime.stats.uploads[0];
  assert.deepEqual(records.slice(2 * 16 + 2, 2 * 16 + 4), [29, 53]);
  assert.equal(records[2 * 16 + 11], new Float32Array([0.47])[0]);
  assert.deepEqual(records.slice(4 * 16 + 2, 4 * 16 + 4), [7, 41]);
});

test("a leading solid rectangle adopts the next sprite binding domain", async () => {
  const sprites = Array.from({ length: 64 }, (_, index) => 2 * 16384 + index);
  const runtime = await loadRuntime({
    rects: 1, sprites: 64, spriteHandles: Array(64).fill(1),
    ordered: [4 * 16384, ...sprites]
  });
  runtime.frame();
  assert.equal(runtime.stats.instanced, 1);
  assert.deepEqual(runtime.stats.instances, [65]);
});

test("reserved run metadata and instance flags reject before replay", async () => {
  for (const configuration of [
    { runMetadata: [1, 0, 0, 0, 0] },
    { runMetadata: [0, 1, 0, 0, 0] },
    { runMetadata: [0, 0, 0, 1, 0] },
    { instanceFlags: 1 }
  ]) {
    const runtime = await loadRuntime({ sprites: 2, spriteHandles: [1, 1], ...configuration });
    runtime.frame();
    assert.equal(runtime.stats.instanced, 0);
    assert.equal(runtime.stats.images, 0);
    assert.equal(runtime.stats.fills, 0);
  }
});

test("non-center negative scale preserves semantic pivot in private records", async () => {
  const runtime = await loadRuntime({
    sprites: 64, spriteHandles: Array(64).fill(1), spritePivot: [2, 7], spriteScale: [-2, 0.5]
  });
  runtime.frame();
  const record = runtime.stats.uploads[0].slice(0, 16);
  assert.deepEqual(record.slice(0, 4), [6.5, 5.5, -16, 5]);
  assert.deepEqual(record.slice(14, 16), [-4, 3.5]);
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

test("oversize sprites within MAX_TEXTURE_SIZE use a dedicated WebGL atlas domain", async () => {
  const runtime = await loadRuntime({
    sprites: 64, spriteHandles: Array(64).fill(1), spriteSize: [2048, 2048]
  });
  runtime.frame();
  assert.equal(runtime.body.dataset.gpuError, undefined);
  assert.equal(runtime.stats.instanced, 1);
  assert.equal(runtime.stats.images, 0);
  assert.equal(runtime.body.dataset.atlasPages, "1");
  assert.equal(runtime.body.dataset.composites, "0");
});

test("texture failure and context loss never select another renderer", async () => {
  const failed = await loadRuntime({ sprites: 64, spriteHandles: Array(64).fill(1), textureThrow: true });
  failed.frame();
  assert.equal(failed.stats.instanced, 0);
  assert.equal(failed.stats.images, 0);
  assert.match(failed.body.dataset.gpuError, /fake texture failure/);

  const recovered = await loadRuntime({ sprites: 64, spriteHandles: Array(64).fill(1) });
  recovered.frame();
  recovered.loseContext();
  recovered.frame();
  assert.equal(recovered.stats.instanced, 1);
  assert.equal(recovered.stats.images, 0);
  recovered.restoreContext();
  recovered.frame();
  assert.equal(recovered.stats.instanced, 2);
});

test("prepared text LRU remains bounded and releases evicted atlas entries", async () => {
  const runtime = await loadRuntime();
  const drawScore = value => {
    runtime.env.web_begin_frame(0, 0, 0);
    runtime.env.web_draw_text(4, 8, value);
    runtime.frame();
  };

  for (let value = 0; value < 256; value += 1) drawScore(value);
  assert.equal(runtime.body.dataset.preparedTextEntries, "256");
  assert.equal(runtime.body.dataset.atlasLiveEntries, "257"); // plus the loaded sprite fixture

  // Refresh score 0 so inserting one more value evicts score 1 instead.
  drawScore(0);
  const uploadsBeforeEviction = Number(runtime.body.dataset.atlasUploadCount);
  drawScore(256);
  assert.equal(runtime.body.dataset.preparedTextEntries, "256");
  assert.equal(runtime.body.dataset.atlasLiveEntries, "257");
  assert.ok(Number(runtime.body.dataset.atlasPages) <= 3);
  assert.ok(Number(runtime.body.dataset.preparedTextBytes) <= 8 * 1024 * 1024);

  drawScore(0);
  assert.equal(Number(runtime.body.dataset.atlasUploadCount), uploadsBeforeEviction + 1,
    "the recently used entry should remain atlas-resident");
  drawScore(1);
  assert.equal(Number(runtime.body.dataset.atlasUploadCount), uploadsBeforeEviction + 2,
    "the evicted entry should require one new atlas upload");
  assert.equal(runtime.body.dataset.preparedTextEntries, "256");
  assert.equal(runtime.body.dataset.atlasLiveEntries, "257");
  assert.equal(runtime.stats.instanced, 260);
});

test("oversized prepared text is transient and releases its atlas page after drawing", async () => {
  const runtime = await loadRuntime();
  const font = runtime.env.load_font(0, 1024);
  for (let value = 0; value < 3; value += 1) {
    runtime.setTextFixture(font, `${String(value)}${"x".repeat(256)}`);
    runtime.frame();
    assert.equal(runtime.body.dataset.preparedTextEntries, "0");
    assert.equal(runtime.body.dataset.preparedTextBytes, "0");
    assert.equal(runtime.body.dataset.atlasLiveEntries, "1"); // loaded sprite fixture only
    assert.equal(runtime.body.dataset.atlasPages, "1");
  }
  assert.equal(runtime.stats.instanced, 3);
  assert.equal(runtime.stats.deletedTextures, 3);
});

test("runtime publishes split timing phases and HUD labels", async () => {
  const runtime = await loadRuntime({ timing: true });
  for (let frame = 0; frame < 6; frame += 1) runtime.frame();
  assert.equal(runtime.body.dataset.tickMs, "2.000");
  assert.equal(runtime.body.dataset.wasmRenderMs, "3.000");
  assert.equal(runtime.body.dataset.browserReplayMs, "0.000");
  assert.equal(runtime.body.dataset.frameWorkMs, "5.000");
  assert.equal(runtime.body.dataset.renderMs, "3.000");
  assert.equal(runtime.body.dataset.worstRenderMs, "3.000");
  assert.match(runtime.hud.textContent, /guest render/);
  assert.match(runtime.hud.textContent, /host replay/);
  assert.match(runtime.hud.textContent, /frame work/);
});
