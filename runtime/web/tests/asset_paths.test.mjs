import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";

const source = fs.readFileSync(new URL("../game.js", import.meta.url), "utf8");

async function loadRuntime(game, options = {}) {
  const imageSources = [];
  const measurements = [];
  const animationFrames = [];
  const addedFonts = [];
  const fontSources = [];
  let env;
  const memory = new WebAssembly.Memory({ initial: 1 });
  const context2d = {
    fontKerning: "auto",
    textBaseline: "alphabetic",
    fillRect() {}, fillText() {}, save() {}, restore() {}, beginPath() {}, moveTo() {},
    lineTo() {}, stroke() {}, drawImage() {}, translate() {}, rotate() {},
    measureText(value) {
      measurements.push({ font: this.font, value });
      return options.measureText?.({
        font: this.font, fontKerning: this.fontKerning, textBaseline: this.textBaseline, value
      }) || { width: value.length * 7 };
    }
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
    fonts: {
      ready: options.fontsReady || Promise.resolve(),
      add(font) { addedFonts.push(font); },
    },
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
      main: () => { options.main?.(env); return 0; },
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
    requestAnimationFrame: () => {
      animationFrames.push(true);
      options.onFrame?.();
      return 1;
    },
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
    FontFace: class {
      constructor(family, source) {
        this.family = family;
        this.source = source;
        fontSources.push(source);
      }
      load() { return options.fontLoad ? options.fontLoad(this) : Promise.resolve(this); }
    },
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
  const runtimePromise = contextObject.window.STASIS_RUNTIME_PROMISE;
  runtimePromise?.catch(() => {});
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(typeof env?.gfx_load_sprite, "function", errorBox.textContent);
  return {
    env, memory, imageSources, measurements, animationFrames, addedFonts, fontSources,
    document, errorBox, runtimePromise,
  };
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

test("web rooted sprite and font paths use package-relative asset keys", async () => {
  const game = {
    memory: {},
    strings: {
      "1": "/assets/rooted-fallback.png",
      "2": "/assets/rooted.png",
      "3": "/assets/rooted.ttf",
      "4": "/textures/absolute.png",
    },
    assets: {
      "assets/rooted.png": "assets/packed/rooted-123.png",
      "assets/rooted.ttf": "assets/packed/rooted-456.ttf",
    },
  };
  const { env, imageSources, fontSources } = await loadRuntime(game);

  env.gfx_load_sprite(1);
  env.gfx_load_sprite(2);
  env.load_font(3, 20);
  env.gfx_load_sprite(4);

  assert.deepEqual(imageSources, [
    "assets/rooted-fallback.png",
    "assets/packed/rooted-123.png",
    "/textures/absolute.png",
  ]);
  assert.deepEqual(fontSources, ["url(assets/packed/rooted-456.ttf)"]);
});

test("web measure_text uses the registered Canvas font and string handle", async () => {
  const game = {
    memory: {},
    strings: { "1": "marble" },
  };
  const { env, measurements } = await loadRuntime(game);

  const font = env.load_font(7, 20);
  assert.equal(typeof env.measure_text, "function");
  assert.equal(env.measure_text(font, 1), 42);
  assert.deepEqual(measurements, [{ font: "20px stasis-font-1", value: "marble" }]);
});

test("web cached text matches native pixel-height metrics before the first frame", async () => {
  const game = {
    memory: {
      "run.font": { offset: 0, length: 1, stride: 4, type_id: 1 },
      "run.handle": { offset: 4, length: 1, stride: 4, type_id: 1 },
      "run.width": { offset: 8, length: 1, stride: 4, type_id: 2 },
      "run.height": { offset: 12, length: 1, stride: 4, type_id: 2 },
    },
    views: {
      "101": {
        font: "run.font", handle: "run.handle", width: "run.width", height: "run.height",
      },
    },
    strings: { "1": "assets/font.ttf", "2": "GAMBIT GUARD" },
  };
  let loadedFont = 0;
  const result = await loadRuntime(game, {
    main: env => {
      loadedFont = env.load_font(1, 24);
      assert.equal(env.stasis_jit_text_run_load_from(101, 0, 1, loadedFont, 2), 1);
    },
    measureText: metrics => {
      assert.equal(metrics.textBaseline, "alphabetic");
      assert.equal(metrics.fontKerning, "none");
      if (metrics.font === "1000px stasis-font-1") {
        return { width: 500, fontBoundingBoxAscent: 1011, fontBoundingBoxDescent: 353 };
      }
      assert.ok(Math.abs(Number.parseFloat(metrics.font) - 17.5953079) < 0.0001);
      return { width: 139.25, actualBoundingBoxDescent: 0.25 };
    },
  });
  await result.runtimePromise;

  const view = new DataView(result.memory.buffer);
  assert.equal(view.getInt32(0, true), loadedFont);
  assert.ok(view.getInt32(4, true) > 0);
  assert.equal(view.getFloat32(8, true), 139.25);
  assert.ok(Math.abs(view.getFloat32(12, true) - 18.038856) < 0.0001);
  assert.deepEqual(result.measurements.map(({ font, value }) => ({ font, value })), [
    { font: "1000px stasis-font-1", value: "Mg" },
    { font: "17.595307917888565px stasis-font-1", value: "GAMBIT GUARD" },
  ]);
});

test("web startup fails visibly when a declared font cannot load", async () => {
  const result = await loadRuntime({
    memory: {},
    strings: { "7": "assets/missing.ttf" },
  }, {
    main: env => env.load_font(7, 20),
    fontLoad: () => Promise.reject(new Error("font fetch failed")),
  });
  await assert.rejects(result.runtimePromise, /font fetch failed/);

  const { document, errorBox, animationFrames } = result;
  assert.equal(document.body.dataset.ready, "false");
  assert.match(errorBox.textContent, /font fetch failed/);
  assert.deepEqual(animationFrames, []);
});

test("web startup waits for every FontFace load before the fonts-ready signal", async () => {
  const releases = [];
  let fontsReadyResolved = false;
  let frameCount = 0;
  const runtime = loadRuntime({
    memory: {},
    strings: { "7": "assets/first.ttf", "8": "assets/second.ttf" },
  }, {
    main: env => {
      env.load_font(7, 20);
      env.load_font(8, 24);
    },
    fontLoad: font => new Promise(resolve => releases.push(() => resolve(font))),
    fontsReady: Promise.resolve().then(() => { fontsReadyResolved = true; }),
    onFrame: () => { frameCount += 1; },
  });

  await new Promise(resolve => setImmediate(resolve));
  assert.equal(releases.length, 2);
  assert.equal(fontsReadyResolved, true);
  assert.equal(frameCount, 0);

  releases.forEach(release => release());
  const afterFontsResolve = await runtime;
  assert.equal(frameCount, 1);
  assert.equal(afterFontsResolve.addedFonts.length, 2);
});
