import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";
import { fakeWebGL2 } from "./fake_webgl2.mjs";

const html = fs.readFileSync(new URL("../index.html", import.meta.url), "utf8");
const fitter = html.match(/<script>\s*([\s\S]*?)\s*<\/script>/)?.[1];
assert.ok(fitter, "index.html has an inline viewport fitter");
const runtime = fs.readFileSync(new URL("../game.js", import.meta.url), "utf8");

test("shared web shell keeps the canvas keyboard reachable", () => {
  const canvasTag = html.match(/<canvas\b[^>]*>/gi)
    ?.find(tag => /\bid=["']stasis-canvas["']/i.test(tag));
  assert.ok(canvasTag, "index.html has the game canvas");
  assert.match(canvasTag, /\btabindex=["']0["']/i);
});

function runFitter({ layoutWidth = 393, layoutHeight = 844, visualWidth = 393, visualHeight = 650, backingWidth = 640, backingHeight = 360, logicalWidth = null, logicalHeight = null, safe = {}, visual = true } = {}) {
  const windowListeners = new Map();
  const visualListeners = new Map();
  const mutations = [];
  const mutationOptions = [];
  const rootStyle = {
    values: {},
    setProperty(name, value) { this.values[name] = value; }
  };
  const shellStyle = {};
  const canvasStyle = {};
  const dataset = logicalWidth === null || logicalHeight === null ? undefined : {
    logicalWidth: String(logicalWidth), logicalHeight: String(logicalHeight)
  };
  const canvas = {
    width: backingWidth,
    height: backingHeight,
    dataset,
    style: canvasStyle,
    parentElement: { style: shellStyle },
    getAttribute(name) {
      if (name === "data-logical-width") return dataset?.logicalWidth ?? null;
      if (name === "data-logical-height") return dataset?.logicalHeight ?? null;
      return null;
    }
  };
  const visualViewport = visual ? {
    width: visualWidth,
    height: visualHeight,
    offsetLeft: 0,
    offsetTop: 0,
    addEventListener(type, listener) { visualListeners.set(type, listener); }
  } : undefined;
  const document = {
    body: {},
    documentElement: { clientWidth: layoutWidth, clientHeight: layoutHeight, style: rootStyle },
    getElementById(id) { return id === "stasis-canvas" ? canvas : null; }
  };
  const context = {
    document,
    window: { visualViewport, innerWidth: layoutWidth, innerHeight: layoutHeight },
    getComputedStyle: () => ({
      getPropertyValue(name) {
        const side = name.slice("padding-".length);
        return String(safe[side] || 0);
      }
    }),
    addEventListener(type, listener) {
      const listeners = windowListeners.get(type) || [];
      listeners.push(listener);
      windowListeners.set(type, listeners);
    },
    MutationObserver: class {
      constructor(callback) { mutations.push(callback); }
      observe(_target, options) { mutationOptions.push(options); }
    }
  };
  vm.runInNewContext(fitter, context, { filename: "runtime/web/index.html" });
  return {
    window: context.window,
    canvas,
    shellStyle,
    rootStyle,
    windowListeners,
    visualListeners,
    mutations,
    mutationOptions,
    canvasStyle,
    visualViewport,
    dispatch(type) { for (const listener of windowListeners.get(type) || []) listener(); },
    dispatchVisual(type) { visualListeners.get(type)?.(); }
  };
}

test("shared web shell uses the visible viewport and preserves backing size", () => {
  const fit = runFitter({ backingWidth: 160, backingHeight: 900, safe: { top: 24, right: 0, bottom: 34, left: 0 } });
  assert.equal(fit.rootStyle.values["--stasis-visible-width"], "393px");
  assert.equal(fit.rootStyle.values["--stasis-visible-height"], "650px");
  assert.equal(fit.rootStyle.values["--stasis-visible-offset-top"], "0px");
  assert.deepEqual({ ...fit.window.STASIS_AVAILABLE_VIEWPORT }, { width: 393, height: 592 });
  assert.ok(Math.abs(parseFloat(fit.shellStyle.width) - 105.24444444444444) < 1e-9);
  assert.equal(fit.shellStyle.height, "592px");
  assert.notStrictEqual(fit.shellStyle, fit.canvasStyle);
  assert.equal(fit.canvasStyle.width, undefined);
  assert.equal(fit.canvas.width, 160);
  assert.equal(fit.canvas.height, 900);
  assert.equal(fit.visualListeners.size, 2);
  assert.equal(fit.windowListeners.get("resize")?.length, 1);
  assert.equal(fit.windowListeners.get("orientationchange")?.length, 1);
  assert.equal(fit.mutationOptions.length, 1);
  assert.equal(fit.mutationOptions[0].attributes, true);
  assert.deepEqual(Array.from(fit.mutationOptions[0].attributeFilter), ["data-logical-width", "data-logical-height"]);
  assert.equal(parseFloat(fit.shellStyle.width) <= 393 && 592 <= 650 - 24 - 34, true);
  assert.equal(parseFloat(fit.shellStyle.width) <= 393 && 592 <= 844, true, "the VM models 393x844 layout and 393x650 visual viewports");
});

test("visual viewport and orientation changes refit once without duplicate listeners", () => {
  const fit = runFitter({ layoutWidth: 844, layoutHeight: 393, visualWidth: 844, visualHeight: 393 });
  fit.visualViewport.width = 393;
  fit.visualViewport.height = 650;
  fit.dispatchVisual("resize");
  assert.deepEqual({ ...fit.window.STASIS_AVAILABLE_VIEWPORT }, { width: 393, height: 650 });
  assert.equal(fit.shellStyle.width, "393px");
  assert.ok(Math.abs(parseFloat(fit.shellStyle.height) - 221.0625) < 1e-9);
  const beforeScroll = { ...fit.shellStyle };
  fit.visualViewport.offsetTop = 12;
  fit.dispatchVisual("scroll");
  assert.deepEqual(fit.shellStyle, beforeScroll, "origin-only scroll does not translate the grid-centered shell");
  assert.equal(fit.rootStyle.values["--stasis-visible-offset-top"], "12px");
  fit.visualViewport.height = 640;
  fit.dispatchVisual("scroll");
  assert.equal(fit.rootStyle.values["--stasis-visible-height"], "640px", "scroll can still refit when the visible extent changes");
  assert.deepEqual({ ...fit.window.STASIS_AVAILABLE_VIEWPORT }, { width: 393, height: 640 });
  fit.dispatch("resize");
  fit.dispatch("orientationchange");
  assert.equal(fit.windowListeners.get("resize")?.length, 1);
  assert.equal(fit.windowListeners.get("orientationchange")?.length, 1);
  assert.equal(fit.visualListeners.get("resize") !== undefined, true);
});

test("visual offset moves the containing body box without clipping a tall shell", () => {
  const fit = runFitter({ backingWidth: 160, backingHeight: 900, safe: { top: 24, bottom: 34 } });
  fit.visualViewport.offsetTop = 100;
  fit.dispatchVisual("scroll");
  const visibleTop = 100;
  const visibleHeight = 650;
  const contentHeight = visibleHeight - 24 - 34;
  const shellTop = visibleTop + 24 + (contentHeight - parseFloat(fit.shellStyle.height)) / 2;
  const shellBottom = shellTop + parseFloat(fit.shellStyle.height);
  assert.equal(fit.rootStyle.values["--stasis-visible-offset-top"], "100px");
  assert.ok(shellTop >= visibleTop && shellBottom <= visibleTop + visibleHeight);
  assert.equal(parseFloat(fit.shellStyle.height), contentHeight);
});

test("intrinsic backing mutation changes fit ratio without changing the backing", () => {
  const fit = runFitter();
  fit.canvas.width = 320;
  fit.canvas.height = 240;
  fit.mutations[0]();
  assert.equal(fit.shellStyle.width, "393px");
  assert.equal(fit.shellStyle.height, "294.75px");
  assert.equal(fit.canvas.width, 320);
  assert.equal(fit.canvas.height, 240);
});

test("layout viewport fallback works when visualViewport is unavailable", () => {
  const fit = runFitter({ layoutWidth: 480, layoutHeight: 800, visual: false });
  assert.equal(fit.rootStyle.values["--stasis-visible-width"], "480px");
  assert.equal(fit.rootStyle.values["--stasis-visible-height"], "800px");
  assert.equal(fit.visualListeners.size, 0);
  assert.equal(fit.windowListeners.get("resize")?.length, 1);
});

function assertAuthoredViewportFit(name, logical, viewport, unusedAxis) {
  const fit = runFitter({
    layoutWidth: viewport[0], layoutHeight: viewport[1],
    visualWidth: viewport[0], visualHeight: viewport[1],
    logicalWidth: logical[0], logicalHeight: logical[1]
  });
  const width = parseFloat(fit.shellStyle.width);
  const height = parseFloat(fit.shellStyle.height);
  const scaleX = width / logical[0];
  const scaleY = height / logical[1];
  assert.ok(width <= viewport[0] && height <= viewport[1], `${name} stays inside the visible viewport`);
  assert.ok(Math.abs(scaleX - scaleY) < 1e-12, `${name} uses one uniform scale`);
  const unusedX = viewport[0] - width;
  const unusedY = viewport[1] - height;
  assert.ok(unusedAxis === "x" ? unusedX > 0 : unusedY > 0, `${name} leaves the expected unused axis`);
}

test("Sheep Herder authored viewport fits desktop and mobile orientations uniformly", () => {
  const logical = [1600, 900];
  assert.match(html, /body \{[\s\S]*?display: grid;[\s\S]*?place-items: center;/, "shell centers letterbox and pillarbox space");
  assertAuthoredViewportFit("desktop landscape", logical, [1440, 900], "y");
  assertAuthoredViewportFit("mobile portrait", logical, [390, 844], "y");
  assertAuthoredViewportFit("mobile landscape", logical, [844, 390], "x");
});

test("extreme valid aspect ratios remain uniformly contained", () => {
  for (const logical of [[1, 8192], [8192, 1]]) {
    const fit = runFitter({
      layoutWidth: 1, layoutHeight: 1, visualWidth: 1, visualHeight: 1,
      logicalWidth: logical[0], logicalHeight: logical[1]
    });
    const width = parseFloat(fit.shellStyle.width);
    const height = parseFloat(fit.shellStyle.height);
    const scaleX = width / logical[0];
    const scaleY = height / logical[1];
    assert.ok(width > 0 && height > 0 && width <= 1 && height <= 1);
    assert.ok(Math.abs(scaleX - scaleY) <= Number.EPSILON * Math.max(scaleX, scaleY, 1));
  }
});

test("index shell contract is safe-area aware and has one fitter", () => {
  assert.match(html, /viewport-fit=cover/);
  assert.match(html, /safe-area-inset-top/);
  assert.match(html, /safe-area-inset-bottom/);
  assert.match(html, /100svh/);
  assert.match(html, /100dvh/);
  assert.match(html, /position: relative;/);
  assert.doesNotMatch(html, /transform: translate\(var\(--stasis-visible-offset-left/);
  assert.match(html, /#stasis-loading \{[^}]*position: fixed; inset: 0;/);
  assert.match(html, /window\.STASIS_REFIT_VIEWPORT = fit/);
  assert.equal((html.match(/<script>\s*\(\(\) =>/g) || []).length, 1);
  assert.equal((html.match(/addEventListener\("resize", fit\)/g) || []).length, 2);
  assert.equal((html.match(/addEventListener\("orientationchange", fit\)/g) || []).length, 1);
});

function integratedRuntime({
  logical = [640, 360], viewport = [393, 650], layout = [393, 844],
  safe = { top: 24, bottom: 34, left: 0, right: 0 }, desktop = [393, 844],
  backing = [640, 360], metadata = logical, dpr = 1, requestFromMain = null
} = {}) {
  const listeners = new Map();
  const visualListeners = new Map();
  const raf = [];
  const mutations = [];
  const rootStyle = {
    values: {},
    setProperty(name, value) { this.values[name] = value; }
  };
  const shellStyle = {};
  const canvasStyle = {};
  const canvas = {
    width: backing[0],
    height: backing[1],
    dataset: { logicalWidth: String(metadata[0]), logicalHeight: String(metadata[1]) },
    style: canvasStyle,
    parentElement: { style: shellStyle },
    listeners: new Map(),
    getAttribute(name) {
      if (name === "data-logical-width") return this.dataset.logicalWidth;
      if (name === "data-logical-height") return this.dataset.logicalHeight;
      return null;
    },
    getContext: kind => kind === "webgl2" ? fakeWebGL2() : ({
      fillRect() {}, fillText() {}, save() {}, restore() {}, beginPath() {}, moveTo() {}, lineTo() {}, stroke() {},
      drawImage() {}, translate() {}, rotate() {}
    }),
    getBoundingClientRect() {
      const width = parseFloat(shellStyle.width) || 0;
      const height = parseFloat(shellStyle.height) || 0;
      const availableWidth = visualViewport.width - (safe.left || 0) - (safe.right || 0);
      const availableHeight = visualViewport.height - (safe.top || 0) - (safe.bottom || 0);
      const left = visualViewport.offsetLeft + (safe.left || 0) + (availableWidth - width) / 2;
      const top = visualViewport.offsetTop + (safe.top || 0) + (availableHeight - height) / 2;
      return { left, top, right: left + width, bottom: top + height, width, height };
    },
    addEventListener(type, listener) { this.listeners.set(type, listener); },
    setPointerCapture() {},
    focus() {},
    requestFullscreen: async () => {}
  };
  const initialBacking = [canvas.width, canvas.height];
  const visualViewport = {
    width: viewport[0],
    height: viewport[1],
    offsetLeft: 0,
    offsetTop: 0,
    addEventListener(type, listener) {
      const values = visualListeners.get(type) || [];
      values.push(listener);
      visualListeners.set(type, values);
    },
    dispatchEvent(event) {
      for (const listener of visualListeners.get(event.type) || []) listener(event);
    }
  };
  const body = { dataset: {} };
  const errorBox = { textContent: "" };
  const memory = new WebAssembly.Memory({ initial: 2 });
  const game = { memory: {
    host_i32: { offset: 0, length: 768 },
    host_f32: { offset: 768 * 4, length: 64 }
  }, strings: {}, assets: {} };
  const request = {
    host_req_seq: { value: 0 },
    host_req_flags: { value: 0 },
    host_req_window_w_px: { value: logical[0] },
    host_req_window_h_px: { value: logical[1] }
  };
  const ticks = [];
  const mainFrames = [];
  const instance = { exports: {
    memory,
    ...request,
    main: () => {
      const i32 = new Int32Array(memory.buffer, 0, 768);
      const f32 = new Float32Array(memory.buffer, 768 * 4, 64);
      mainFrames.push({
        version: i32[14], available: [f32[56], f32[57]], logical: [f32[50], f32[51]],
        backing: [i32[24], i32[25]]
      });
      if (requestFromMain) {
        request.host_req_seq.value += 1;
        request.host_req_flags.value = 4;
        request.host_req_window_w_px.value = requestFromMain[0];
        request.host_req_window_h_px.value = requestFromMain[1];
      }
      return 0;
    },
    tick: () => {
      const i32 = new Int32Array(memory.buffer, 0, 768);
      const f32 = new Float32Array(memory.buffer, 768 * 4, 64);
      ticks.push({
        resized: i32[11], generation: i32[30], drawable: [i32[24], i32[25]], logical: [f32[50], f32[51]],
        available: [f32[56], f32[57]], pointer: [f32[0], f32[1]]
      });
    },
    render: () => 0
  } };
  const eventTarget = {
    addEventListener(type, listener) {
      const values = listeners.get(type) || [];
      values.push(listener);
      listeners.set(type, values);
    },
    dispatchEvent(event) {
      for (const listener of listeners.get(event.type) || []) listener(event);
      return true;
    }
  };
  const document = {
    body,
    hidden: false,
    fullscreenElement: null,
    documentElement: { clientWidth: layout[0], clientHeight: layout[1], style: rootStyle },
    fonts: { ready: Promise.resolve(), add() {} },
    hasFocus: () => true,
    getElementById(id) {
      if (id === "stasis-canvas") return canvas;
      if (id === "stasis-error") return errorBox;
      return null;
    },
    addEventListener: eventTarget.addEventListener
  };
  const window = {
    STASIS_GAME: game,
    visualViewport,
    innerWidth: layout[0],
    innerHeight: layout[1],
    addEventListener: eventTarget.addEventListener,
    dispatchEvent: eventTarget.dispatchEvent
  };
  let currentDpr = dpr;
  const context = {
    document,
    window,
    screen: { width: desktop[0], height: desktop[1] },
    get devicePixelRatio() { return currentDpr; },
    performance: { now: () => 0 },
    WebAssembly: { instantiate: async () => ({ instance }) },
    fetch: async () => ({ ok: true, arrayBuffer: async () => new ArrayBuffer(0) }),
    requestAnimationFrame: callback => { raf.push(callback); return raf.length; },
    cancelAnimationFrame() {},
    addEventListener: eventTarget.addEventListener,
    dispatchEvent: eventTarget.dispatchEvent,
    Event: class { constructor(type) { this.type = type; } },
    getComputedStyle: () => ({ getPropertyValue: name => `${safe[name.slice("padding-".length)] || 0}px` }),
    MutationObserver: class {
      constructor(callback) { mutations.push(callback); }
      observe() {}
    },
    console,
    Image: class {},
    FontFace: class { load() { return Promise.resolve(this); } },
    AudioContext: class { constructor() { this.state = "running"; this.currentTime = 0; this.destination = {}; } close() {} resume() {} },
    TextDecoder,
    setTimeout,
    clearTimeout,
    STASIS_GAME: game
  };
  vm.runInNewContext(fitter, context, { filename: "runtime/web/index.html" });
  vm.runInNewContext(runtime, context, { filename: "runtime/web/game.js" });
  return {
    canvas, canvasStyle, shellStyle, rootStyle, visualViewport, request, ticks, mainFrames, raf, mutations, listeners, context, initialBacking,
    setDpr(value) { currentDpr = value; context.dispatchEvent(new context.Event("resize")); }
  };
}

async function startIntegratedRuntime() {
  const fixture = integratedRuntime();
  assert.deepEqual(fixture.initialBacking, [640, 360]);
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));
  assert.deepEqual(fixture.mainFrames[0].logical, [640, 360]);
  assert.equal(fixture.raf.length, 1);
  fixture.raf.shift()(16);
  return fixture;
}

test("integrated fitter and runtime share extent signaling and ordering", async () => {
  const fixture = await startIntegratedRuntime();
  const { canvas, request, ticks, raf, visualViewport, rootStyle, mutations, shellStyle } = fixture;
  request.host_req_seq.value = 1;
  request.host_req_flags.value = 4;
  request.host_req_window_w_px.value = 320;
  request.host_req_window_h_px.value = 640;
  raf.shift()(32);
  assert.deepEqual(ticks.at(-1), { resized: 1, generation: 2, drawable: [296, 592], logical: [320, 640], available: [393, 592], pointer: [0, 0] });

  visualViewport.height = 600;
  visualViewport.dispatchEvent(new fixture.context.Event("scroll"));
  raf.shift()(48);
  assert.equal(ticks.at(-1).resized, 1);
  assert.equal(ticks.at(-1).generation, 3);
  assert.deepEqual(ticks.at(-1).available, [393, 542]);

  visualViewport.offsetTop = 100;
  visualViewport.dispatchEvent(new fixture.context.Event("scroll"));
  raf.shift()(64);
  assert.equal(rootStyle.values["--stasis-visible-offset-top"], "100px");
  assert.equal(ticks.at(-1).resized, 0);
  assert.equal(ticks.at(-1).generation, 3);
  assert.deepEqual(ticks.at(-1).available, [393, 542]);

  request.host_req_seq.value = 2;
  request.host_req_flags.value = 4;
  request.host_req_window_w_px.value = 200;
  request.host_req_window_h_px.value = 400;
  raf.shift()(80);
  assert.equal(ticks.at(-1).resized, 1);
  assert.equal(ticks.at(-1).generation, 4);
  assert.deepEqual(ticks.at(-1).logical, [200, 400]);
  assert.equal(parseFloat(shellStyle.height) > 0, true);
  assert.ok(fixture.listeners.get("stasis-viewport-extent")?.length, "runtime listens to the fitter extent event");
});

test("portrait guest observes landscape availability before main and settles without feedback", async () => {
  const fixture = integratedRuntime({
    logical: [360, 720], viewport: [1280, 720], layout: [1280, 720],
    safe: {}, desktop: [2560, 1440], requestFromMain: [720, 360]
  });
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));

  assert.deepEqual(fixture.mainFrames, [{
    version: 4, available: [1280, 720], logical: [360, 720], backing: [360, 720]
  }]);
  assert.equal(fixture.shellStyle.width, "1280px");
  assert.equal(fixture.shellStyle.height, "640px");
  fixture.raf.shift()(16);
  assert.deepEqual(fixture.ticks.at(-1), {
    resized: 1, generation: 2, drawable: [1280, 640], logical: [720, 360],
    available: [1280, 720], pointer: [0, 0]
  });

  fixture.setDpr(2);
  fixture.raf.shift()(32);
  assert.deepEqual(fixture.ticks.at(-1).drawable, [2560, 1280]);
  const settledGeneration = fixture.ticks.at(-1).generation;
  fixture.raf.shift()(48);
  fixture.raf.shift()(64);
  assert.equal(fixture.ticks.at(-1).generation, settledGeneration);
  assert.equal(fixture.ticks.at(-1).resized, 0);
  assert.deepEqual(fixture.ticks.at(-1).available, [1280, 720]);

  const pinnedGeneration = fixture.ticks.at(-1).generation;
  fixture.visualViewport.width = 1200;
  fixture.visualViewport.height = 700;
  fixture.visualViewport.dispatchEvent(new fixture.context.Event("resize"));
  assert.equal(fixture.shellStyle.width, "1200px");
  assert.equal(fixture.shellStyle.height, "600px");
  fixture.raf.shift()(72);
  assert.equal(fixture.ticks.at(-1).generation, pinnedGeneration + 1);
  assert.equal(fixture.ticks.at(-1).resized, 1);
  assert.deepEqual(fixture.ticks.at(-1).drawable, [2400, 1200]);
  assert.deepEqual(fixture.ticks.at(-1).available, [1200, 700]);
  fixture.raf.shift()(76);
  assert.equal(fixture.ticks.at(-1).generation, pinnedGeneration + 1);
  assert.equal(fixture.ticks.at(-1).resized, 0);

  fixture.canvas.listeners.get("pointerdown")({
    pointerId: 9, pointerType: "mouse", clientX: 600, clientY: 350
  });
  fixture.raf.shift()(80);
  assert.deepEqual(fixture.ticks.at(-1).pointer, [360, 180], "CSS center maps to the landscape logical center");
});

test("pointer and touch map through a fitted Sheep Herder viewport", async () => {
  const fixture = integratedRuntime({
    logical: [1600, 900], viewport: [390, 844], layout: [390, 844], safe: {}, desktop: [390, 844]
  });
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));
  const bounds = fixture.canvas.getBoundingClientRect();
  assert.ok(bounds.top > 0 && bounds.bottom < 844, "portrait viewport is letterboxed without clipping");

  fixture.canvas.listeners.get("pointerdown")({
    pointerId: 10, pointerType: "mouse",
    clientX: bounds.left + bounds.width / 2,
    clientY: bounds.top + bounds.height / 2
  });
  fixture.raf.shift()(16);
  assert.deepEqual(fixture.ticks.at(-1).pointer, [800, 450]);

  fixture.canvas.listeners.get("pointerdown")({
    pointerId: 11, pointerType: "touch",
    clientX: bounds.left + bounds.width / 4,
    clientY: bounds.top + bounds.height / 4
  });
  fixture.raf.shift()(32);
  assert.deepEqual(fixture.ticks.at(-1).pointer, [400, 225]);
});

test("configured maximum logical size starts from a safe physical backing", async () => {
  const fixture = integratedRuntime({
    logical: [8192, 8192], backing: [640, 360], viewport: [1024, 768], layout: [1024, 768], safe: {}
  });
  assert.deepEqual(fixture.initialBacking, [640, 360]);
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));
  assert.deepEqual(fixture.mainFrames, [{
    version: 4, available: [1024, 768], logical: [8192, 8192], backing: [768, 768]
  }]);
  assert.ok(fixture.canvas.width * fixture.canvas.height * 4 <= 64 * 1024 * 1024);
});

test("invalid logical metadata falls back to safe intrinsic dimensions", async () => {
  const fixture = integratedRuntime({
    logical: [320, 240], metadata: [9000, "bad"], backing: [320, 240],
    viewport: [640, 480], layout: [640, 480], safe: {}
  });
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));
  assert.deepEqual(fixture.mainFrames[0].logical, [320, 240]);
});
