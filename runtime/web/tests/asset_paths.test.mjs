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
  let env;
  const memory = new WebAssembly.Memory({ initial: 1 });
  const context2d = {
    fillRect() {}, fillText() {}, save() {}, restore() {}, beginPath() {}, moveTo() {},
    lineTo() {}, stroke() {}, drawImage() {}, translate() {}, rotate() {},
    measureText(value) {
      measurements.push({ font: this.font, value });
      return { width: value.length * 7 };
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
  return { env, imageSources, measurements, animationFrames, addedFonts, document, errorBox, runtimePromise };
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
