import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";
import { performance as nodePerformance } from "node:perf_hooks";
import { installCollectionViewAbi } from "./collection_view_abi.mjs";

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

function fakeGl(stats, available = true, throwing = false, textureThrow = false,
  maxTextureSize = 4096, textureFailureAt = 0, glErrorAt = 0) {
  if (!available) return null;
  const gl = {
    VERTEX_SHADER: 1, FRAGMENT_SHADER: 2, COMPILE_STATUS: 3, LINK_STATUS: 4,
    ARRAY_BUFFER: 5, STATIC_DRAW: 6, DYNAMIC_DRAW: 7, FLOAT: 8,
    COLOR_BUFFER_BIT: 9, BLEND: 10, SRC_ALPHA: 11, ONE_MINUS_SRC_ALPHA: 12,
    TRIANGLE_STRIP: 13, TEXTURE_2D: 14, TEXTURE_WRAP_S: 15, TEXTURE_WRAP_T: 16,
    TEXTURE_MIN_FILTER: 17, TEXTURE_MAG_FILTER: 18, CLAMP_TO_EDGE: 19, LINEAR: 20,
    RGBA: 21, UNSIGNED_BYTE: 22, TEXTURE0: 23, UNPACK_FLIP_Y_WEBGL: 24,
    LINEAR_MIPMAP_LINEAR: 25, NO_ERROR: 0,
    createShader: () => { if (throwing) throw new Error("fake shader failure"); return {}; }, createProgram: () => ({}), createVertexArray: () => ({}), createBuffer: () => ({}), createTexture: () => { stats.createdTextures += 1; return {}; },
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
    texParameteri() {}, pixelStorei() {}, texImage2D(_target, _level, _internal, width, height) { stats.texImageCalls += 1; stats.pageSizes.push([width, height]); if (textureThrow || (textureFailureAt && stats.texImageCalls === textureFailureAt)) throw new Error("fake texture failure"); }, texSubImage2D(...args) { stats.texSubImageCalls += 1; const source = args[args.length - 1]; stats.textureUploads.push({ width: Number(source?.width) || 0, height: Number(source?.height) || 0 }); if (textureThrow) throw new Error("fake texture failure"); }, generateMipmap() {}, activeTexture() {}, bindTexture() {}, getError: () => { stats.getErrorCalls += 1; return glErrorAt && stats.getErrorCalls === glErrorAt ? 1280 : 0; },
    isContextLost: () => stats.contextLost, getParameter: () => maxTextureSize,
    enable() {}, disable() {}, scissor(x, y, width, height) { stats.scissors.push([x, y, width, height]); }, blendFunc() {}, blendFuncSeparate() {}, drawArraysInstanced(_mode, _first, _vertices, count) {
      stats.instanced += 1;
      stats.instances.push(count);
    }
  };
  return gl;
}

async function loadRuntime({ rects = 0, rectSizes = null, rectAlpha = 1, ordered = null, clips = [], sprites = 0, spriteHandles = [], spriteSize = null, spriteSizes = null, spriteUv = [0.1, 0.2, 0.9, 0.8], spriteXOffset = null, spritePivot = [4, 5], spriteScale = [1, 1], instanceFlags = 0, runMetadata = [0, 0, 0, 0, 0], webgl = true, throwing = false, textureThrow = false, textureFailureAt = 0, glErrorAt = 0, imageReady = true, timing = false, realTime = false, dpr = 1, cssExtent = [640, 360], imageExtent = [16, 16], assetMetadata = {}, assets = {}, createImageBitmap = null, imageDecode = null, fetchBlob = null, hudQuery = "", atlasBudgetBytes = undefined, maxTextureSize = 4096, expectReady = true } = {}) {
  const memory = new WebAssembly.Memory({ initial: 16 });
  const i32 = new Int32Array(memory.buffer, 0, I32_COUNT);
  const f32 = new Float32Array(memory.buffer, F32_OFFSET, F32_COUNT);
  const stats = { instanced: 0, instances: [], uploadedFloats: [], uploads: [], uniforms: [], viewports: [], scissors: [], transforms: [], imageArgs: [], images: 0, fills: 0, events: [], clipRects: [], clipCalls: 0, restores: 0, contextLost: false, imageDecodeCalls: 0, imageConstructed: 0, bitmapCalls: [], createdTextures: 0, texImageCalls: 0, texSubImageCalls: 0, pageSizes: [], textureUploads: [], getErrorCalls: 0, deletedTextures: 0 };
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
  const rasterStats = { draws: 0, images: [], clears: [], textFills: [], transforms: [], saves: 0, restores: 0, canvases: 0 };
  let activeOffscreen;
  const rasterContext = {
    imageSmoothingEnabled: true, imageSmoothingQuality: "high", fillRect() {},
    clearRect(...args) { rasterStats.clears.push(args); }, drawImage(...args) { rasterStats.draws += 1; rasterStats.images.push(args); },
    measureText(text) { return { width: String(text).length * 8, actualBoundingBoxDescent: 4 }; },
    fillText(...args) { rasterStats.textFills.push({ canvas: activeOffscreen, width: activeOffscreen?.width || 0, height: activeOffscreen?.height || 0, args }); },
    setTransform(...args) { rasterStats.transforms.push(args); },
    save() { rasterStats.saves += 1; }, restore() { rasterStats.restores += 1; }
  };
  const gl = fakeGl(stats, true, throwing, textureThrow, maxTextureSize, textureFailureAt, glErrorAt);
  const canvasListeners = new Map();
  const canvas = {
    width: 640, height: 360, style: {}, parentElement: { style: {} },
    dataset: {},
    getContext: kind => kind === "2d" ? context2d : gl,
    getBoundingClientRect: () => ({ left: 0, top: 0, width: cssExtent[0], height: cssExtent[1] }),
    addEventListener(type, callback) { canvasListeners.set(type, callback); }, setPointerCapture() {}, focus() {}, requestFullscreen: async () => {}
  };
  const hud = { textContent: "", hidden: false, dataset: {}, setAttribute(name, value) { this[name] = value; } };
  const body = { dataset: {} };
  const errorBox = { textContent: "" };
  const offscreenListeners = new Map();
  const makeOffscreen = () => {
    rasterStats.canvases += 1;
    const surface = {
      width: 0, height: 0,
      getContext: kind => kind === "2d" ? rasterContext : gl,
      addEventListener(type, callback) { offscreenListeners.set(type, callback); }
    };
    activeOffscreen = surface;
    return surface;
  };
  const offscreen = makeOffscreen();
  const document = {
    body, hidden: false, fullscreenElement: null,
    fonts: { ready: Promise.resolve(), add() {} }, hasFocus: () => true,
    getElementById(id) {
      if (id === "stasis-canvas") return canvas;
      if (id === "stasis-hud") return hud;
      if (id === "stasis-error") return errorBox;
      if (id === "stasis-audio") return { addEventListener() {}, disabled: false, textContent: "" };
      return null;
    },
    createElement: () => makeOffscreen(),
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
        f32[baseF] = index + 0.5 + (spriteXOffset?.value || 0); f32[baseF + 1] = 2; f32[baseF + 2] = 8; f32[baseF + 3] = 10;
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
  const game = {
    memory: {
      gfx_cmd_i32: { offset: 0, length: I32_COUNT },
      gfx_cmd_f32: { offset: F32_OFFSET, length: F32_COUNT },
      host_i32: { offset: 900000, length: 768 },
      host_f32: { offset: 903072, length: 64 },
      "run.font": { hash: 101, handle: 1101, offset: 910000, length: 1, stride: 4, type_id: 1 },
      "run.handle": { hash: 102, handle: 1102, offset: 910004, length: 1, stride: 4, type_id: 1 },
      "run.width": { hash: 103, handle: 1103, offset: 910008, length: 1, stride: 4, type_id: 2 },
      "run.height": { hash: 104, handle: 1104, offset: 910012, length: 1, stride: 4, type_id: 2 },
    },
    views: { "101": { font: "run.font", handle: "run.handle", width: "run.width", height: "run.height" } },
    strings: {}, assets, asset_metadata: assetMetadata,
  };
  if (atlasBudgetBytes !== undefined) game.atlasBudgetBytes = atlasBudgetBytes;
  installCollectionViewAbi(game, instance.exports);
  const raf = [];
  const windowListeners = new Map();
  const location = { search: hudQuery, origin: "https://example.test", protocol: "https:", host: "example.test", hash: "" };
  const contextObject = {
    document, screen: { width: 640, height: 360 }, devicePixelRatio: 1,
    location,
    URLSearchParams,
    performance: { now: () => realTime ? nodePerformance.now() : now }, WebAssembly: { Global: WebAssembly.Global, instantiate: async (_bytes, imports) => { env = imports.env; return { instance }; } },
    fetch: async source => ({ ok: true, arrayBuffer: async () => new ArrayBuffer(0), blob: async () => fetchBlob ? fetchBlob(source) : { source } }),
    requestAnimationFrame: callback => { raf.push(callback); return raf.length; }, cancelAnimationFrame() {},
    addEventListener(type, callback) { windowListeners.set(type, callback); }, console, Image: class { constructor() { stats.imageConstructed += 1; this.complete = imageReady; this.naturalWidth = imageReady ? imageExtent[0] : 0; this.naturalHeight = imageReady ? imageExtent[1] : 0; } decode() { stats.imageDecodeCalls += 1; return imageDecode ? imageDecode(this) : Promise.resolve(); } }, FontFace: class { load() { return Promise.resolve(this); } },
    AudioContext: class { constructor() { this.state = "running"; this.currentTime = 0; this.destination = {}; } close() {} resume() {} },
    TextDecoder, TextEncoder, setTimeout, clearTimeout, devicePixelRatio: dpr,
  };
  if (createImageBitmap) {
    contextObject.createImageBitmap = async (source, options) => {
      stats.bitmapCalls.push({ source, options });
      return createImageBitmap(source, options, stats.bitmapCalls.length);
    };
  }
  contextObject.window = { STASIS_GAME: game, screen: contextObject.screen };
  vm.runInNewContext(source, contextObject, { filename: "runtime/web/game.js" });
  const expectedFailure = expectReady
    ? null : assert.rejects(contextObject.window.STASIS_RUNTIME_PROMISE);
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));
  if (expectReady) assert.equal(body.dataset.ready, "true");
  else await expectedFailure;
  return {
    stats, body, errorBox, hud, rasterStats, canvas, offscreen, env, contextObject, frame: () => raf.shift()(now),
    setPresentation(width, height, nextDpr = contextObject.devicePixelRatio) {
      cssExtent[0] = width;
      cssExtent[1] = height;
      contextObject.devicePixelRatio = nextDpr;
    },
    setTextFixture(font, text, textId = 1) {
      contextObject.window.STASIS_GAME.strings[textId] = text;
      textFixture = { font, handle: env.stasis_jit_gfx_cache_text(font, textId) };
    },
    replaceTextFixture(font, text) {
      game.strings[1] = text;
      const replaced = env.stasis_jit_text_run_replace_from(101, 0, 1, font, 1);
      textFixture = { font, handle: new DataView(memory.buffer).getInt32(910004, true) };
      return replaced;
    },
    dispatchKey: event => windowListeners.get("keydown")?.(event),
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

test("ordered clipping clamps negative clip extents before WebGL scissor", async () => {
  const scale = 16384;
  const runtime = await loadRuntime({
    clips: [[25, 30, -40, -24]],
    ordered: [5 * scale, 6 * scale]
  });
  runtime.frame();
  assert.deepEqual(runtime.stats.scissors, [[25, 330, 0, 0]]);
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

test("invalid atlas budgets fail visibly with the manifest field name", async () => {
  for (const atlasBudgetBytes of [0, -1, 1.5, "4096", null, Number.MAX_SAFE_INTEGER + 1]) {
    const runtime = await loadRuntime({ atlasBudgetBytes, expectReady: false });
    assert.equal(runtime.body.dataset.ready, "false");
    assert.match(runtime.errorBox.textContent, /web\.atlas_budget_bytes/);
    assert.match(runtime.body.dataset.gpuError, /web\.atlas_budget_bytes/);
    assert.equal(runtime.stats.createdTextures, 0);
  }
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
  assert.equal(runtime.body.dataset.assetCacheBytes, String(32 * 32 * 4));

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
  assert.equal(runtime.body.dataset.assetCacheBytes, String(32 * 32 * 4), "obsolete landscape tiers are released");
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
  assert.equal(runtime.stats.deletedTextures, 0, "transactional refresh reuses the retained page");
  runtime.stats.instanced = 0;
  runtime.frame();
  assert.equal(runtime.stats.instanced, 1);
  runtime.env.gfx_release_sprite(1);
  assert.equal(replacement.closed, true);
  assert.equal(runtime.stats.deletedTextures, 1);
});

test("budget-failed density refresh rolls back staged atlas state before lower-tier recovery", async () => {
  const pageBytes = 512 * 512 * 4;
  const pending = [];
  const bitmaps = [];
  const makeBitmap = (width, height) => {
    const bitmap = { width, height, closed: false, close() { this.closed = true; } };
    bitmaps.push(bitmap);
    return bitmap;
  };
  const runtime = await loadRuntime({
    sprites: 1, spriteHandles: [1], spriteSize: [200, 200], dpr: 1,
    atlasBudgetBytes: pageBytes,
    assets: { "": "refresh-budget.svg" },
    assetMetadata: { "": { encoding: "svg", prepared_width: 200, prepared_height: 200 } },
    createImageBitmap: (_source, options, call) => {
      if (call === 1) return makeBitmap(options.resizeWidth, options.resizeHeight);
      return new Promise(resolve => pending.push({ resolve, options }));
    }
  });
  runtime.frame();
  assert.equal(runtime.body.dataset.atlasPages, "1");
  assert.equal(runtime.body.dataset.assetAtlasBytes, String(pageBytes));

  runtime.contextObject.devicePixelRatio = 2;
  runtime.frame();
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(pending.length, 1);
  const highTier = makeBitmap(pending[0].options.resizeWidth, pending[0].options.resizeHeight);
  pending[0].resolve(highTier);
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));
  runtime.stats.instanced = 0;
  runtime.frame();
  assert.equal(runtime.stats.instanced, 1, "the retained low tier still draws after rejection");
  assert.equal(runtime.body.dataset.assetPreparedWidth, "200");
  assert.equal(runtime.body.dataset.assetRefreshState, "failed");
  assert.match(runtime.body.dataset.assetRefreshError, /atlas memory budget exhausted/);
  assert.match(runtime.body.dataset.gpuError, /atlas memory budget exhausted/);
  assert.equal(runtime.body.dataset.atlasPages, "1");
  assert.equal(runtime.body.dataset.assetAtlasBytes, String(pageBytes));
  assert.equal(runtime.stats.createdTextures, 1, "the rejected 1024px page is checked before createTexture");
  assert.equal(runtime.stats.deletedTextures, 0);
  assert.equal(bitmaps[0].closed, false);
  assert.equal(highTier.closed, true, "the failed replacement cache lease is cleaned up");

  runtime.contextObject.devicePixelRatio = 1;
  runtime.frame();
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(runtime.body.dataset.assetPreparedWidth, "200");
  assert.equal(runtime.body.dataset.assetRefreshState, "none");
  assert.equal(runtime.body.dataset.gpuError, undefined);
  assert.equal(runtime.body.dataset.atlasPages, "1");
  assert.equal(runtime.body.dataset.assetAtlasBytes, String(pageBytes));
  runtime.stats.instanced = 0;
  runtime.frame();
  assert.equal(runtime.stats.instanced, 1);
  assert.equal(bitmaps[0].closed, false);
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

test("atlas efficiency measurement: separate images versus an authored sheet", async () => {
  const spriteCount = 18;
  const drawOrder = [
    ...Array.from({ length: spriteCount }, (_, index) => index + 1),
    ...Array.from({ length: 128 }, (_, index) => index % 2 ? spriteCount : 1)
  ];
  const separate = await loadRuntime({
    sprites: drawOrder.length, spriteHandles: drawOrder, spriteSize: [252, 252],
    spriteUv: [0, 0, 1, 1], realTime: true
  });
  separate.frame();
  const separateFirst = {
    pages: separate.stats.pageSizes, allocatedBytes: Number(separate.body.dataset.atlasAllocatedBytes),
    pixelUploads: Number(separate.body.dataset.atlasUploadCount),
    pixelUploadBytes: Number(separate.body.dataset.atlasUploadBytes),
    instanceUploadBytes: Number(separate.body.dataset.uploadedBytes),
    draws: Number(separate.body.dataset.drawCalls),
    binds: Number(separate.body.dataset.textureBinds),
    transitions: Number(separate.body.dataset.atlasTransitions)
  };
  assert.deepEqual(separateFirst.pages, Array(6).fill(0).map(() => [512, 512]));
  assert.equal(separateFirst.allocatedBytes, 6 * 512 * 512 * 4);
  assert.equal(separateFirst.pixelUploads, spriteCount + 6); // Entries plus white texel per page.
  assert.ok(separateFirst.draws > 100);
  assert.equal(separateFirst.instanceUploadBytes, drawOrder.length * 64);

  const sheet = await loadRuntime({
    sprites: drawOrder.length, spriteHandles: Array(drawOrder.length).fill(1),
    spriteSize: [1512, 756], spriteUv: [0, 0, 1, 1], realTime: true
  });
  sheet.frame();
  const sheetFirst = {
    pages: sheet.stats.pageSizes, allocatedBytes: Number(sheet.body.dataset.atlasAllocatedBytes),
    pixelUploads: Number(sheet.body.dataset.atlasUploadCount),
    pixelUploadBytes: Number(sheet.body.dataset.atlasUploadBytes),
    instanceUploadBytes: Number(sheet.body.dataset.uploadedBytes),
    draws: Number(sheet.body.dataset.drawCalls),
    binds: Number(sheet.body.dataset.textureBinds),
    transitions: Number(sheet.body.dataset.atlasTransitions)
  };
  assert.deepEqual(sheetFirst.pages, [[2048, 2048]]);
  assert.equal(sheetFirst.allocatedBytes, 2048 * 2048 * 4);
  assert.equal(sheetFirst.pixelUploads, 2);
  assert.equal(sheetFirst.draws, 1);
  assert.equal(sheetFirst.instanceUploadBytes, drawOrder.length * 64);

  const firstSeparateUploadCount = Number(separate.body.dataset.atlasUploadCount);
  const firstSheetUploadCount = Number(sheet.body.dataset.atlasUploadCount);
  const separateReplay = [];
  const sheetReplay = [];
  for (let frame = 0; frame < 25; frame += 1) {
    separate.frame();
    sheet.frame();
    separateReplay.push(Number(separate.body.dataset.hostReplayMs));
    sheetReplay.push(Number(sheet.body.dataset.hostReplayMs));
  }
  assert.equal(Number(separate.body.dataset.atlasUploadCount), firstSeparateUploadCount);
  assert.equal(Number(sheet.body.dataset.atlasUploadCount), firstSheetUploadCount);
  const median = values => [...values].sort((left, right) => left - right)[12];
  if (process.env.STASIS_ATLAS_REPORT === "1") {
    console.log("ATLAS_MEASUREMENT " + JSON.stringify({
      source: "runtime/web/game.js", imageCount: spriteCount, imageSize: [252, 252],
      padding: 2, paddedArea: spriteCount * 256 * 256,
      sheetSize: [1512, 756], drawCount: drawOrder.length,
      separate: { ...separateFirst, medianHostReplayMs: median(separateReplay) },
      sheet: { ...sheetFirst, medianHostReplayMs: median(sheetReplay) }
    }));
  }
});

test("atlas efficiency measurement: mixed sizes, game assets, and animation residency", async () => {
  const mixedSizes = [
    ...Array(6).fill([252, 252]), ...Array(4).fill([120, 240]),
    ...Array(4).fill([64, 96]), ...Array(4).fill([32, 48])
  ];
  const mixedOrder = [
    ...Array.from({ length: mixedSizes.length }, (_, index) => index + 1),
    ...Array.from({ length: 128 }, (_, index) => index % 2 ? mixedSizes.length : 1)
  ];
  const mixed = await loadRuntime({
    sprites: mixedOrder.length, spriteHandles: mixedOrder, spriteSizes: mixedSizes,
    spriteUv: [0, 0, 1, 1]
  });
  mixed.frame();
  const mixedPaddedArea = mixedSizes.reduce((total, [width, height]) =>
    total + (width + 4) * (height + 4), 0);
  assert.ok(mixedPaddedArea < 2048 * 2048);
  const mixedResult = {
    sizes: mixedSizes, paddedArea: mixedPaddedArea, pages: mixed.stats.pageSizes,
    allocatedBytes: Number(mixed.body.dataset.atlasAllocatedBytes),
    pixelUploads: Number(mixed.body.dataset.atlasUploadCount),
    pixelUploadBytes: Number(mixed.body.dataset.atlasUploadBytes),
    instanceUploadBytes: Number(mixed.body.dataset.uploadedBytes),
    draws: Number(mixed.body.dataset.drawCalls),
    binds: Number(mixed.body.dataset.textureBinds),
    transitions: Number(mixed.body.dataset.atlasTransitions)
  };

  const pngDimensions = path => {
    const bytes = fs.readFileSync(new URL(path, import.meta.url));
    assert.equal(bytes.toString("ascii", 1, 4), "PNG");
    return [bytes.readUInt32BE(16), bytes.readUInt32BE(20)];
  };
  const background = pngDimensions("../../../samples/asset_breakout/assets/arena_background.png");
  const smoke = pngDimensions("../../../samples/windows_launch_smoke/assets/smoke.png");
  assert.deepEqual(background, [1672, 941]);
  assert.deepEqual(smoke, [64, 64]);
  const gameOrder = Array.from({ length: 128 }, (_, index) => index % 2 ? 2 : 1);
  const game = await loadRuntime({
    sprites: gameOrder.length, spriteHandles: gameOrder, spriteSizes: [background, smoke],
    spriteUv: [0, 0, 1, 1]
  });
  game.frame();
  assert.deepEqual(game.stats.pageSizes, [[2048, 2048]]);
  assert.equal(Number(game.body.dataset.drawCalls), 1);
  const gameResult = {
    assets: [{ path: "samples/asset_breakout/assets/arena_background.png", size: background },
      { path: "samples/windows_launch_smoke/assets/smoke.png", size: smoke }],
    paddedArea: (background[0] + 4) * (background[1] + 4)
      + (smoke[0] + 4) * (smoke[1] + 4),
    pages: game.stats.pageSizes, allocatedBytes: Number(game.body.dataset.atlasAllocatedBytes),
    pixelUploads: Number(game.body.dataset.atlasUploadCount),
    pixelUploadBytes: Number(game.body.dataset.atlasUploadBytes),
    instanceUploadBytes: Number(game.body.dataset.uploadedBytes),
    draws: Number(game.body.dataset.drawCalls),
    binds: Number(game.body.dataset.textureBinds),
    transitions: Number(game.body.dataset.atlasTransitions)
  };

  const uv = [0, 0, 0.5, 0.5];
  const offset = { value: 0 };
  const animated = await loadRuntime({
    sprites: 1, spriteHandles: [1], spriteSize: [64, 64], spriteUv: uv,
    spriteXOffset: offset
  });
  animated.frame();
  const originalQuad = animated.stats.uploads.at(-1);
  const uploadCount = Number(animated.body.dataset.atlasUploadCount);
  uv[0] = 0.5;
  uv[2] = 1;
  offset.value = 30;
  animated.frame();
  const movedQuad = animated.stats.uploads.at(-1);
  assert.notEqual(movedQuad[0], originalQuad[0]);
  assert.notEqual(movedQuad[4], originalQuad[4]);
  assert.equal(Number(animated.body.dataset.atlasUploadCount), uploadCount);
  if (process.env.STASIS_ATLAS_REPORT === "1") {
    console.log("ATLAS_MEASUREMENT " + JSON.stringify({
      mixed: mixedResult, representative: gameResult,
      animation: { pixelUploadsBefore: uploadCount,
        pixelUploadsAfter: Number(animated.body.dataset.atlasUploadCount),
        instanceBytesPerFrame: Number(animated.body.dataset.uploadedBytes) }
    }));
  }
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

test("an exact atlas budget admits nine 1024px pages", async () => {
  const pageCount = 9;
  const handles = Array.from({ length: pageCount }, (_, index) => index + 1);
  const pageBytes = 1024 * 1024 * 4;
  const runtime = await loadRuntime({
    sprites: pageCount,
    spriteHandles: handles,
    spriteSizes: handles.map(() => [1016, 1016]),
    atlasBudgetBytes: pageCount * pageBytes,
  });
  runtime.frame();
  assert.equal(runtime.body.dataset.atlasPages, String(pageCount));
  assert.equal(runtime.body.dataset.assetAtlasBytes, String(pageCount * pageBytes));
  assert.equal(runtime.stats.createdTextures, pageCount);
  assert.equal(runtime.stats.deletedTextures, 0);
});

test("omitting the atlas budget leaves page allocation unlimited", async () => {
  const pageCount = 9;
  const handles = Array.from({ length: pageCount }, (_, index) => index + 1);
  const runtime = await loadRuntime({
    sprites: pageCount,
    spriteHandles: handles,
    spriteSizes: handles.map(() => [1016, 1016]),
    maxTextureSize: 4096,
  });
  runtime.frame();
  assert.equal(runtime.body.dataset.atlasPages, String(pageCount));
  assert.equal(runtime.body.dataset.assetAtlasBytes, String(pageCount * 1024 * 1024 * 4));
  assert.equal(runtime.stats.createdTextures, pageCount);
});

test("three 4096px pages remain available without a global page or byte cap", async () => {
  const pageCount = 3;
  const handles = Array.from({ length: pageCount }, (_, index) => index + 1);
  const runtime = await loadRuntime({
    sprites: pageCount,
    spriteHandles: handles,
    spriteSizes: handles.map(() => [4090, 4090]),
    maxTextureSize: 4096,
  });
  runtime.frame();
  const pageBytes = 4096 * 4096 * 4;
  assert.equal(runtime.body.dataset.atlasPages, String(pageCount));
  assert.equal(runtime.body.dataset.assetAtlasBytes, String(pageCount * pageBytes));
  assert.equal(runtime.stats.createdTextures, pageCount);
});

test("an explicit atlas budget above the legacy cap admits three supported 4096px pages", async () => {
  const pageCount = 3;
  const pageBytes = 4096 * 4096 * 4;
  const handles = Array.from({ length: pageCount }, (_, index) => index + 1);
  const runtime = await loadRuntime({
    sprites: pageCount,
    spriteHandles: handles,
    spriteSizes: handles.map(() => [4090, 4090]),
    atlasBudgetBytes: pageCount * pageBytes,
    maxTextureSize: 4096,
  });
  runtime.frame();
  assert.equal(runtime.body.dataset.gpuError, undefined);
  assert.equal(runtime.body.dataset.atlasPages, String(pageCount));
  assert.equal(runtime.body.dataset.assetAtlasBytes, String(pageCount * pageBytes));
  assert.ok(Number(runtime.body.dataset.assetAtlasBytes) > 128 * 1024 * 1024);
});

test("configured residency accounts for full and source atlas variants", async () => {
  const pageBytes = 512 * 512 * 4;
  const runtime = await loadRuntime({
    sprites: 64, spriteHandles: Array(64).fill(1), spriteSize: [16, 16],
    spriteUv: [0, 0, 0.5, 0.5], atlasBudgetBytes: pageBytes,
    assets: { "": "budget-sheet.svg" },
    assetMetadata: { "": { encoding: "svg", prepared_width: 64, prepared_height: 32 } },
    createImageBitmap: (_source, options) => ({
      width: options.resizeWidth, height: options.resizeHeight, close() {}
    })
  });
  runtime.frame();
  assert.equal(runtime.body.dataset.gpuError, undefined);
  assert.equal(runtime.body.dataset.atlasPages, "1");
  assert.equal(runtime.body.dataset.atlasLiveEntries, "2");
  assert.equal(runtime.body.dataset.assetAtlasBytes, String(pageBytes));
  runtime.env.gfx_release_sprite(1);
  assert.equal(runtime.body.dataset.assetAtlasBytes, "0");
  assert.equal(runtime.stats.deletedTextures, 1);
});

test("configured solid and text residency returns bytes when the text resource is released", async () => {
  const pageBytes = 512 * 512 * 4;
  const runtime = await loadRuntime({ rects: 1, atlasBudgetBytes: pageBytes * 2 });
  runtime.frame();
  assert.equal(runtime.body.dataset.gpuError, undefined);
  assert.equal(runtime.body.dataset.atlasPages, "1");
  assert.equal(runtime.body.dataset.assetAtlasBytes, String(pageBytes));
  const font = runtime.env.load_font(0, 18);
  assert.equal(runtime.env.font_status(font), 2);
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(runtime.env.font_status(font), 3);
  runtime.setTextFixture(font, "budget text");
  runtime.frame();
  assert.equal(runtime.body.dataset.gpuError, undefined);
  assert.equal(runtime.body.dataset.atlasLiveEntries, "2");
  assert.equal(runtime.body.dataset.assetAtlasBytes, String(pageBytes));
  runtime.env.gfx_release_font(font);
  runtime.frame();
  assert.equal(runtime.body.dataset.atlasLiveEntries, "1");
  assert.equal(runtime.body.dataset.assetAtlasBytes, String(pageBytes));
  assert.equal(runtime.body.dataset.atlasPages, "1");
});

test("released atlas pages return budget capacity for later allocation", async () => {
  const handles = [1];
  const pageBytes = 512 * 512 * 4;
  const runtime = await loadRuntime({
    sprites: 1,
    spriteHandles: handles,
    spriteSize: [256, 256],
    atlasBudgetBytes: pageBytes,
  });
  runtime.frame();
  assert.equal(runtime.body.dataset.assetAtlasBytes, String(pageBytes));
  assert.equal(runtime.body.dataset.atlasPages, "1");

  runtime.env.gfx_release_sprite(1);
  assert.equal(runtime.body.dataset.assetAtlasBytes, "0");
  assert.equal(runtime.stats.deletedTextures, 1);

  const replacement = runtime.env.gfx_load_sprite(0, 256, 256);
  handles.fill(replacement);
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));
  runtime.frame();
  assert.equal(runtime.body.dataset.gpuError, undefined);
  assert.equal(runtime.body.dataset.assetAtlasBytes, String(pageBytes));
  assert.equal(runtime.body.dataset.atlasPages, "1");
  assert.equal(runtime.stats.createdTextures, 2);
  assert.equal(runtime.stats.deletedTextures, 1);
});

test("releasing one page preserves the other page and returns only its budget bytes", async () => {
  const handles = [1, 2];
  const pageBytes = 1024 * 1024 * 4;
  const runtime = await loadRuntime({
    sprites: 2,
    spriteHandles: handles,
    spriteSize: [1016, 1016],
    atlasBudgetBytes: 2 * pageBytes,
  });
  runtime.frame();
  assert.equal(runtime.body.dataset.assetAtlasBytes, String(2 * pageBytes));
  assert.equal(runtime.body.dataset.atlasPages, "2");

  runtime.env.gfx_release_sprite(1);
  handles[0] = 2;
  runtime.frame();
  assert.equal(runtime.body.dataset.assetAtlasBytes, String(pageBytes));
  assert.equal(runtime.body.dataset.atlasPages, "1");
  assert.equal(runtime.stats.deletedTextures, 1);

  handles[0] = runtime.env.gfx_load_sprite(0, 1016, 1016);
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));
  runtime.frame();
  assert.equal(runtime.body.dataset.gpuError, undefined);
  assert.equal(runtime.body.dataset.assetAtlasBytes, String(2 * pageBytes));
  assert.equal(runtime.body.dataset.atlasPages, "2");
  assert.equal(runtime.stats.createdTextures, 3);
  assert.equal(runtime.stats.deletedTextures, 1);
});

test("atlas budget rejects the next page before its GPU texture allocation", async () => {
  const pageCount = 9;
  const handles = Array.from({ length: pageCount }, (_, index) => index + 1);
  const pageBytes = 1024 * 1024 * 4;
  const runtime = await loadRuntime({
    sprites: pageCount,
    spriteHandles: handles,
    spriteSizes: handles.map(() => [1016, 1016]),
    atlasBudgetBytes: pageCount * pageBytes - 1,
  });
  runtime.frame();
  assert.equal(
    runtime.body.dataset.gpuError,
    `Error: WebGL2 atlas memory budget exhausted (web.atlas_budget_bytes): budget=${pageCount * pageBytes - 1} current=${(pageCount - 1) * pageBytes} requested=${pageBytes} pages=${pageCount - 1}`
  );
  assert.equal(runtime.stats.createdTextures, pageCount - 1);
  assert.equal(runtime.stats.texImageCalls, pageCount - 1);
  assert.equal(runtime.body.dataset.backend, "WebGL2");
  assert.equal(runtime.body.dataset.assetAtlasBytes, String((pageCount - 1) * pageBytes));
});

test("an over-MAX sprite is a visible GPU failure and never a missing-sprite placeholder", async () => {
  const runtime = await loadRuntime({
    sprites: 1, spriteHandles: [1], spriteSize: [4095, 4095], maxTextureSize: 4096
  });
  runtime.frame();
  assert.match(runtime.body.dataset.gpuError, /MAX_TEXTURE_SIZE/);
  assert.equal(runtime.stats.instanced, 0);
  assert.equal(runtime.stats.images, 0);
  assert.equal(runtime.body.dataset.assetAtlasBytes, "0");
  assert.equal(runtime.stats.createdTextures, 0);
});

test("a successful frame preserves an unrelated resource GPU error", async () => {
  const handles = [1];
  const runtime = await loadRuntime({
    sprites: 1, spriteHandles: handles, spriteSize: [4095, 4095], spriteUv: [0, 0, 1, 1], maxTextureSize: 4096
  });
  assert.match(runtime.body.dataset.gpuError, /MAX_TEXTURE_SIZE/);

  const healthyHandle = runtime.env.gfx_load_sprite(0, 16, 16);
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));
  handles[0] = healthyHandle;
  runtime.stats.instanced = 0;
  runtime.frame();
  assert.equal(runtime.stats.instanced, 1);
  assert.match(runtime.body.dataset.gpuError, /MAX_TEXTURE_SIZE/);

  runtime.env.gfx_release_sprite(1);
  runtime.frame();
  assert.equal(runtime.body.dataset.gpuError, undefined);
});

test("a one-shot atlas upload WebGL error is visible and deletes its new texture", async () => {
  const runtime = await loadRuntime({ sprites: 1, spriteHandles: [1], glErrorAt: 2 });
  runtime.frame();
  assert.match(runtime.body.dataset.gpuError, /WebGL error \(1280\)/);
  assert.equal(runtime.stats.instanced, 0);
  assert.equal(runtime.stats.images, 0);
  assert.equal(runtime.stats.createdTextures, 1);
  assert.equal(runtime.stats.deletedTextures, 1);
  assert.equal(runtime.body.dataset.assetAtlasBytes, "0");
});

test("texture failure and context loss never select another renderer", async () => {
  const failed = await loadRuntime({ sprites: 64, spriteHandles: Array(64).fill(1), textureThrow: true });
  failed.frame();
  assert.equal(failed.stats.instanced, 0);
  assert.equal(failed.stats.images, 0);
  assert.match(failed.body.dataset.gpuError, /fake texture failure/);
  assert.ok(failed.stats.createdTextures > 0);
  assert.equal(failed.stats.deletedTextures, failed.stats.createdTextures);

  const recovered = await loadRuntime({ sprites: 64, spriteHandles: Array(64).fill(1) });
  recovered.frame();
  recovered.loseContext();
  assert.equal(recovered.stats.deletedTextures, 1);
  recovered.frame();
  assert.equal(recovered.stats.instanced, 1);
  assert.equal(recovered.stats.images, 0);
  recovered.restoreContext();
  recovered.frame();
  assert.equal(recovered.stats.instanced, 2);
  assert.equal(recovered.body.dataset.assetAtlasBytes, String(512 * 512 * 4));
});

test("failed context restore disposes pages rebuilt before the failing resource", async () => {
  const runtime = await loadRuntime({
    sprites: 2, spriteHandles: [1, 2], spriteSize: [1016, 1016], textureFailureAt: 4
  });
  runtime.frame();
  assert.equal(runtime.stats.createdTextures, 2);
  runtime.loseContext();
  assert.equal(runtime.stats.deletedTextures, 2);

  // Restore call 1 creates page 3 for the first resource, then fails while
  // creating page 4 for the second. Both the partially rebuilt page and the
  // just-created failing texture must be deleted.
  runtime.restoreContext();
  assert.equal(runtime.stats.createdTextures, 4);
  assert.equal(runtime.stats.deletedTextures, 4);
  assert.match(runtime.body.dataset.gpuError, /fake texture failure/);

  // A later restore can recover and clears only the restore-owned error.
  runtime.restoreContext();
  assert.equal(runtime.body.dataset.gpuError, undefined);
  runtime.frame();
  assert.equal(runtime.stats.images, 0);
  assert.equal(runtime.body.dataset.atlasPages, "2");
});

test("prepared text rasterizes at the physical tier while submitting logical metrics", async () => {
  const runtime = await loadRuntime({ dpr: 4 });
  const font = runtime.env.load_font(0, 18);
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(runtime.env.font_status(font), 3);
  runtime.setTextFixture(font, "density");
  runtime.frame();

  const logicalWidth = "density".length * 8;
  const logicalHeight = 18 + 4;
  const tier = 4;
  const fill = runtime.rasterStats.textFills.at(-1);
  assert.deepEqual([fill.width, fill.height], [logicalWidth * tier, logicalHeight * tier]);
  assert.deepEqual(fill.args, ["density", 0, 18]);
  assert.deepEqual(runtime.rasterStats.transforms.at(-1), [tier, 0, 0, tier, 0, 0]);
  assert.equal(runtime.rasterStats.saves, runtime.rasterStats.restores);

  const upload = runtime.stats.textureUploads.find(({ width, height }) => width === logicalWidth * tier + 4
    && height === logicalHeight * tier + 4);
  assert.deepEqual([upload.width, upload.height], [logicalWidth * tier + 4, logicalHeight * tier + 4]);
  const quad = runtime.stats.uploads.at(-1);
  assert.deepEqual(quad.slice(2, 4), [logicalWidth, logicalHeight]);
  const uvWidth = (quad[6] - quad[4]) * 512;
  const uvHeight = (quad[7] - quad[5]) * 512;
  assert.equal(uvWidth, logicalWidth * tier);
  assert.equal(uvHeight, logicalHeight * tier);
  assert.equal(runtime.body.dataset.preparedTextBytes, String(logicalWidth * tier * logicalHeight * tier * 4));
});

test("same density tier reuses text while a tier transition rebuilds its physical resource", async () => {
  const runtime = await loadRuntime({ dpr: 1.1 });
  const font = runtime.env.load_font(0, 18);
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(runtime.env.font_status(font), 3);
  runtime.setTextFixture(font, "reuse");
  runtime.frame();
  const fillsAfterFirst = runtime.rasterStats.textFills.length;
  const textUpload = (tier, logicalWidth = 40, logicalHeight = 22) =>
    runtime.stats.textureUploads.filter(({ width, height }) =>
      width === Math.ceil(logicalWidth * tier) + 4
      && height === Math.ceil(logicalHeight * tier) + 4).length;
  const uploadsAfterFirst = textUpload(1.25);
  assert.equal(uploadsAfterFirst, 1);
  const bytesAtTier125 = runtime.body.dataset.preparedTextBytes;
  assert.equal(runtime.body.dataset.densityTier, "1.25");

  runtime.setPresentation(640, 360, 1.2);
  runtime.frame();
  assert.equal(runtime.body.dataset.densityTier, "1.25");
  assert.equal(runtime.rasterStats.textFills.length, fillsAfterFirst);
  assert.equal(textUpload(1.25), uploadsAfterFirst);
  assert.equal(runtime.body.dataset.preparedTextBytes, bytesAtTier125);

  runtime.setPresentation(640, 360, 2);
  runtime.frame();
  assert.equal(runtime.body.dataset.densityTier, "2");
  assert.equal(runtime.rasterStats.textFills.length, fillsAfterFirst + 1);
  assert.equal(textUpload(2), 1);
  assert.equal(runtime.body.dataset.preparedTextBytes, String(40 * 2 * 22 * 2 * 4));
  assert.deepEqual(runtime.stats.uploads.at(-1).slice(2, 4), [40, 22]);
});

test("text sampling covers fitted and nonuniform backing axes beyond density tiers", async () => {
  for (const [width, height, scale] of [[1280, 360, 2], [640, 900, 3], [5760, 360, 9], [5500, 360, 5500 / 640]]) {
    const runtime = await loadRuntime({ cssExtent: [width, height] });
    const font = runtime.env.load_font(0, 18);
    await new Promise(resolve => setImmediate(resolve));
    runtime.setTextFixture(font, "physical");
    runtime.frame();
    const fill = runtime.rasterStats.textFills.at(-1);
    assert.deepEqual([fill.width, fill.height], [Math.ceil(64 * scale), Math.ceil(22 * scale)]);
    assert.deepEqual(fill.args, ["physical", 0, 18]);
    assert.deepEqual(runtime.rasterStats.transforms.at(-1),
      [fill.width / 64, 0, 0, fill.height / 22, 0, 0]);
    assert.deepEqual(runtime.stats.uploads.at(-1).slice(2, 4), [64, 22]);
    assert.equal(runtime.body.dataset.preparedTextBytes, String(Math.ceil(64 * scale) * Math.ceil(22 * scale) * 4));
  }
});

test("text rebuilds when the larger backing axis changes within an unchanged density tier", async () => {
  const runtime = await loadRuntime({ cssExtent: [1280, 360] });
  const draw = () => {
    runtime.env.web_begin_frame(0, 0, 0);
    runtime.env.web_draw_text(4, 8, "x");
    runtime.frame();
  };
  draw();
  assert.equal(runtime.body.dataset.densityTier, "1");
  assert.deepEqual(runtime.rasterStats.transforms.at(-1), [2, 0, 0, 2, 0, 0]);
  runtime.setPresentation(1920, 360, 1);
  draw();
  assert.equal(runtime.body.dataset.densityTier, "1");
  assert.equal(runtime.rasterStats.textFills.length, 2);
  assert.deepEqual(runtime.rasterStats.transforms.at(-1), [3, 0, 0, 3, 0, 0]);
  assert.equal(runtime.body.dataset.preparedTextBytes, String(56 * 22 * 3 * 3 * 4));
  runtime.setPresentation(1920, 720, 1);
  draw();
  assert.equal(runtime.body.dataset.densityTier, "2");
  assert.equal(runtime.rasterStats.textFills.length, 2, "unchanged text scale reuses its resource");
});

test("dynamic text replacement and restore retain physical sampling and logical placement", async () => {
  const runtime = await loadRuntime({ cssExtent: [5760, 360] });
  const font = runtime.env.load_font(0, 18);
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(runtime.replaceTextFixture(font, "old"), 1);
  runtime.frame();
  assert.equal(runtime.replaceTextFixture(font, "new value"), 1);
  runtime.frame();
  const fill = runtime.rasterStats.textFills.at(-1);
  assert.deepEqual([fill.width, fill.height], [72 * 9, 22 * 9]);
  const quad = runtime.stats.uploads.at(-1).slice(0, 4);
  assert.deepEqual(quad, [4, 8, 72, 22]);
  assert.equal(runtime.replaceTextFixture(font, "x".repeat(4097)), 0);
  runtime.loseContext();
  runtime.restoreContext();
  runtime.frame();
  assert.deepEqual(runtime.stats.uploads.at(-1).slice(0, 4), quad);
  assert.equal(runtime.rasterStats.textFills.at(-1), fill);
  assert.ok(runtime.stats.textureUploads.filter(upload => upload.width === 652 && upload.height === 202).length >= 2);
});

test("context restoration reuploads physical text without changing its logical quad", async () => {
  const runtime = await loadRuntime({ dpr: 2 });
  const font = runtime.env.load_font(0, 18);
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(runtime.env.font_status(font), 3);
  runtime.setTextFixture(font, "restore");
  runtime.frame();
  const textUpload = ({ width, height }) => width === 116 && height === 48;
  const uploadsBeforeRestore = runtime.stats.textureUploads.filter(textUpload);
  assert.equal(uploadsBeforeRestore.length, 1);
  const firstUpload = uploadsBeforeRestore[0];
  runtime.loseContext();
  runtime.restoreContext();
  const uploadsAfterRestore = runtime.stats.textureUploads.filter(textUpload);
  assert.equal(uploadsAfterRestore.length, 2);
  assert.deepEqual(uploadsAfterRestore[1], firstUpload);
  runtime.frame();
  assert.deepEqual(runtime.stats.uploads.at(-1).slice(2, 4), [56, 22]);
});

test("legacy direct text follows the physical density tier without changing its logical size", async () => {
  const runtime = await loadRuntime({ dpr: 1.1 });
  const drawScore = () => {
    runtime.env.web_begin_frame(0, 0, 0);
    runtime.env.web_draw_text(4, 8, "x");
    runtime.frame();
  };
  drawScore();
  assert.equal(runtime.body.dataset.densityTier, "1.25");
  const firstFill = runtime.rasterStats.textFills.at(-1);
  assert.deepEqual([firstFill.width, firstFill.height], [70, 28]);
  assert.deepEqual(firstFill.args, ["score x", 0, 18]);
  assert.ok(runtime.stats.textureUploads.some(({ width, height }) => width === 74 && height === 32));
  assert.equal(runtime.body.dataset.preparedTextBytes, String(70 * 28 * 4));

  runtime.setPresentation(640, 360, 2);
  drawScore();
  assert.equal(runtime.body.dataset.densityTier, "2");
  assert.equal(runtime.rasterStats.textFills.length, 2);
  assert.ok(runtime.stats.textureUploads.some(({ width, height }) => width === 116 && height === 48));
  assert.equal(runtime.body.dataset.preparedTextBytes, String(112 * 44 * 4));
  assert.deepEqual(runtime.stats.uploads.at(-1).slice(2, 4), [56, 22]);
});

test("text over the WebGL texture extent fails before Canvas allocation without a logical fallback", async () => {
  const runtime = await loadRuntime({ dpr: 4, maxTextureSize: 64 });
  const font = runtime.env.load_font(0, 18);
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(runtime.env.font_status(font), 3);
  const canvasesBefore = runtime.rasterStats.canvases;
  const textureUploadsBefore = runtime.stats.textureUploads.length;
  const textFillsBefore = runtime.rasterStats.textFills.length;
  const createdTexturesBefore = runtime.stats.createdTextures;
  runtime.setTextFixture(font, "x");
  runtime.frame();
  assert.match(runtime.body.dataset.gpuError, /available texture extent/);
  assert.equal(runtime.stats.instanced, 0);
  assert.equal(runtime.rasterStats.canvases, canvasesBefore);
  assert.equal(runtime.stats.createdTextures, createdTexturesBefore);
  assert.equal(runtime.stats.textureUploads.length, textureUploadsBefore);
  assert.equal(runtime.rasterStats.textFills.length, textFillsBefore);
  assert.equal(runtime.body.dataset.preparedTextEntries ?? "0", "0");
  assert.equal(runtime.body.dataset.preparedTextBytes ?? "0", "0");
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
  assert.equal(runtime.env.font_status(font), 2);
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(runtime.env.font_status(font), 3);
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

test("performance HUD is hidden by default while telemetry remains available", async () => {
  const runtime = await loadRuntime({ timing: true });
  assert.equal(runtime.hud.hidden, true);
  assert.equal(runtime.hud.dataset.visible, "false");
  runtime.frame();
  assert.equal(runtime.body.dataset.tickMs, "2.000");
  assert.match(runtime.hud.textContent, /frame work/);
  let prevented = false;
  runtime.dispatchKey({ code: "F3", preventDefault() { prevented = true; } });
  assert.equal(prevented, true);
  assert.equal(runtime.hud.hidden, false);
  assert.equal(runtime.hud.dataset.visible, "true");
  assert.equal(runtime.hud["aria-hidden"], "false");
  runtime.dispatchKey({ code: "F3", preventDefault() {} });
  assert.equal(runtime.hud.hidden, true);
  assert.equal(runtime.hud.dataset.visible, "false");
});

test("performance HUD query opt-in starts development overlay visible", async () => {
  const runtime = await loadRuntime({ hudQuery: "?stasis-hud=1" });
  assert.equal(runtime.hud.hidden, false);
  assert.equal(runtime.hud.dataset.visible, "true");
  assert.equal(runtime.hud["aria-hidden"], "false");
});


test("PNG sampling covers both backing axes including fractional and above-tier fits", async () => {
  for (const [width, height, scale] of [[640, 360, 1], [1280, 720, 2], [800, 360, 1.25], [1280, 360, 2], [5500, 360, 5500 / 640]]) {
    const runtime = await loadRuntime({ cssExtent: [width, height], sprites: 64,
      spriteHandles: Array(64).fill(1), spriteSize: [16, 16], spriteUv: [0, 0, 1, 1],
      imageExtent: [256, 256], assets: { "": "detail.png" },
      assetMetadata: { "": { encoding: "png", prepared_width: 256, prepared_height: 256 } } });
    runtime.frame();
    const extent = Math.ceil(16 * scale);
    assert.equal(runtime.body.dataset.assetPreparedWidth, String(extent));
    assert.equal(runtime.body.dataset.assetPreparedHeight, String(extent));
    assert.equal(runtime.body.dataset.assetCacheBytes, String(extent * extent * 4));
    assert.deepEqual(runtime.stats.uploads.at(-1).slice(2, 4), [8, 10]);
    assert.ok(runtime.stats.textureUploads.some(upload => upload.width === extent + 4 && upload.height === extent + 4));
  }
});

test("PNG sampling resize and restoration preserve logical atlas geometry", async () => {
  const runtime = await loadRuntime({ cssExtent: [1280, 360], sprites: 64,
    spriteHandles: Array(64).fill(1), spriteSize: [16, 16], spriteUv: [0, 0, 1, 1],
    imageExtent: [256, 256], assets: { "": "detail.png" },
    assetMetadata: { "": { encoding: "png", prepared_width: 256, prepared_height: 256 } } });
  runtime.frame();
  const quad = runtime.stats.uploads.at(-1).slice(0, 4);
  runtime.setPresentation(1920, 360, 1);
  runtime.frame();
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));
  runtime.frame();
  assert.equal(runtime.body.dataset.assetPreparedWidth, "48");
  assert.deepEqual(runtime.stats.uploads.at(-1).slice(0, 4), quad);
  const count = runtime.body.dataset.spriteRasterCount;
  runtime.setPresentation(1920, 720, 1);
  runtime.frame();
  await new Promise(resolve => setImmediate(resolve));
  runtime.frame();
  assert.equal(runtime.body.dataset.spriteRasterCount, count);
  runtime.loseContext();
  runtime.restoreContext();
  runtime.frame();
  assert.equal(runtime.body.dataset.assetPreparedWidth, "48");
  assert.deepEqual(runtime.stats.uploads.at(-1).slice(0, 4), quad);
  assert.ok(runtime.stats.textureUploads.filter(upload => upload.width === 52 && upload.height === 52).length >= 2);
});


test("PNG sheet crops stay in logical source coordinates at physical density", async () => {
  for (const dpr of [1, 1.25, 2]) {
    const runtime = await loadRuntime({ dpr, sprites: 64, spriteHandles: Array(64).fill(1),
      spriteSize: [16, 16], spriteUv: [0.25, 0, 0.75, 1], imageExtent: [64, 64],
      assets: { "": "sheet.png" },
      assetMetadata: { "": { encoding: "png", prepared_width: 64, prepared_height: 64 } } });
    runtime.frame();
    const uv = runtime.stats.uploads.at(-1).slice(4, 8);
    assert.equal(Math.round((uv[2] - uv[0]) * 512), 8 * dpr);
    assert.equal(Math.round((uv[3] - uv[1]) * 512), 16 * dpr);
  }
});
