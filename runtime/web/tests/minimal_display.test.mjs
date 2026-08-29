import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";

const source = fs.readFileSync(new URL("../game_minimal.js", import.meta.url), "utf8")
  .replace("__STASIS_IMPORTS__", "");

async function loadMinimal({ logical = [640, 360], css = logical, dpr = 1 } = {}) {
  const transforms = [];
  const raf = [];
  let currentDpr = dpr;
  const body = { dataset: {} };
  const canvas = {
    width: logical[0], height: logical[1], dataset: {}, style: {},
    getContext: () => ({
      setTransform(...value) { transforms.push(value); },
      fillRect() {}, fillText() {},
    }),
    getBoundingClientRect: () => ({ left: 0, top: 0, width: css[0], height: css[1] }),
    addEventListener() {},
  };
  const instance = { exports: { main: () => 0, tick() {}, render() {} } };
  const context = {
    document: {
      body, hidden: false,
      getElementById(id) {
        if (id === "stasis-canvas") return canvas;
        if (id === "stasis-hud") return null;
        if (id === "stasis-error") return { textContent: "" };
        if (id === "stasis-loading") return { dataset: {}, textContent: "" };
        if (id === "stasis-loading-status") return { textContent: "" };
        return null;
      },
      addEventListener() {},
    },
    window: null, get devicePixelRatio() { return currentDpr; }, performance: { now: () => 0 },
    fetch: async () => ({ ok: true, arrayBuffer: async () => new ArrayBuffer(0) }),
    WebAssembly: { instantiate: async () => ({ instance }) },
    requestAnimationFrame: callback => { raf.push(callback); return raf.length; },
    cancelAnimationFrame() {}, addEventListener() {}, console,
    TextDecoder, TextEncoder, setTimeout, clearTimeout,
  };
  context.window = { STASIS_GAME: { strings: {} }, STASIS_REFIT_VIEWPORT() {} };
  vm.runInNewContext(source, context, { filename: "runtime/web/game_minimal.js" });
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(body.dataset.ready, "true");
  raf.shift()();
  return {
    body, canvas, transforms,
    setDpr(value) { currentDpr = value; },
    frame() { raf.shift()(); }
  };
}

test("minimal runtime keeps logical layout and scales its physical backing", async () => {
  const runtime = await loadMinimal({ logical: [320, 180], css: [640, 360], dpr: 2 });
  assert.equal(runtime.body.dataset.logicalWidth, "320");
  assert.equal(runtime.body.dataset.logicalHeight, "180");
  assert.equal(runtime.body.dataset.cssWidth, "640");
  assert.equal(runtime.body.dataset.cssHeight, "360");
  assert.equal(runtime.body.dataset.backingWidth, "1280");
  assert.equal(runtime.body.dataset.backingHeight, "720");
  assert.equal(runtime.body.dataset.densityTier, "4");
  assert.ok(runtime.transforms.some(value => value[0] === 4 && value[3] === 4));
});

test("minimal runtime exposes the same backing caps and fallback reason", async () => {
  const runtime = await loadMinimal({ logical: [10000, 10000], css: [10000, 10000], dpr: 3 });
  assert.ok(Number(runtime.body.dataset.backingWidth) <= 8192);
  assert.ok(Number(runtime.body.dataset.backingHeight) <= 8192);
  assert.ok(Number(runtime.body.dataset.backingBytes) <= 64 * 1024 * 1024);
  assert.notEqual(runtime.body.dataset.backingFallback, "none");
  assert.equal(runtime.body.dataset.backingCap, "capped");
});

test("minimal runtime advances density generation within one stable tier", async () => {
  const runtime = await loadMinimal({ logical: [640, 360], css: [640, 360], dpr: 1.1 });
  const firstGeneration = Number(runtime.body.dataset.densityGeneration);
  const firstTier = runtime.body.dataset.densityTier;
  const firstRasterScale = runtime.body.dataset.rasterScale;

  runtime.setDpr(1.2);
  runtime.frame();

  assert.equal(runtime.body.dataset.densityTier, firstTier);
  assert.notEqual(runtime.body.dataset.rasterScale, firstRasterScale);
  assert.equal(Number(runtime.body.dataset.densityGeneration), firstGeneration + 1);
});
