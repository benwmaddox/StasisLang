import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";

const html = fs.readFileSync(new URL("../index.html", import.meta.url), "utf8");
const fitter = html.match(/<script>\s*([\s\S]*?)\s*<\/script>/)?.[1];
assert.ok(fitter, "index.html has an inline viewport fitter");
const runtime = fs.readFileSync(new URL("../game.js", import.meta.url), "utf8");

function runFitter({ layoutWidth = 393, layoutHeight = 844, visualWidth = 393, visualHeight = 650, backingWidth = 640, backingHeight = 360, safe = {}, visual = true } = {}) {
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
  const canvas = {
    width: backingWidth,
    height: backingHeight,
    style: canvasStyle,
    parentElement: { style: shellStyle }
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
  assert.equal(fit.shellStyle.width, "393px");
  assert.equal(fit.shellStyle.height, "221.0625px");
  const beforeScroll = { ...fit.shellStyle };
  fit.visualViewport.offsetTop = 12;
  fit.dispatchVisual("scroll");
  assert.deepEqual(fit.shellStyle, beforeScroll, "origin-only scroll does not translate the grid-centered shell");
  assert.equal(fit.rootStyle.values["--stasis-visible-offset-top"], "12px");
  fit.visualViewport.height = 640;
  fit.dispatchVisual("scroll");
  assert.equal(fit.rootStyle.values["--stasis-visible-height"], "640px", "scroll can still refit when the visible extent changes");
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

function integratedRuntime() {
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
    width: 640,
    height: 360,
    dataset: {},
    style: canvasStyle,
    parentElement: { style: shellStyle },
    listeners: new Map(),
    getContext: () => ({
      fillRect() {}, fillText() {}, save() {}, restore() {}, beginPath() {}, moveTo() {}, lineTo() {}, stroke() {},
      drawImage() {}, translate() {}, rotate() {}
    }),
    getBoundingClientRect() {
      return { left: 0, top: 0, width: parseFloat(shellStyle.width) || 0, height: parseFloat(shellStyle.height) || 0 };
    },
    addEventListener(type, listener) { this.listeners.set(type, listener); },
    setPointerCapture() {},
    focus() {},
    requestFullscreen: async () => {}
  };
  const visualViewport = {
    width: 393,
    height: 650,
    offsetLeft: 0,
    offsetTop: 0,
    addEventListener(type, listener) { visualListeners.set(type, listener); },
    dispatchEvent(event) { visualListeners.get(event.type)?.(event); }
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
    host_req_window_w_px: { value: 640 },
    host_req_window_h_px: { value: 360 }
  };
  const ticks = [];
  const instance = { exports: {
    memory,
    ...request,
    main: () => 0,
    tick: () => {
      const i32 = new Int32Array(memory.buffer, 0, 768);
      const f32 = new Float32Array(memory.buffer, 768 * 4, 64);
      ticks.push({
        resized: i32[11], generation: i32[30], drawable: [i32[24], i32[25]], logical: [f32[50], f32[51]]
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
    documentElement: { clientWidth: 393, clientHeight: 844, style: rootStyle },
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
    innerWidth: 393,
    innerHeight: 844,
    addEventListener: eventTarget.addEventListener,
    dispatchEvent: eventTarget.dispatchEvent
  };
  const context = {
    document,
    window,
    screen: { width: 393, height: 844 },
    devicePixelRatio: 1,
    performance: { now: () => 0 },
    WebAssembly: { instantiate: async () => ({ instance }) },
    fetch: async () => ({ ok: true, arrayBuffer: async () => new ArrayBuffer(0) }),
    requestAnimationFrame: callback => { raf.push(callback); return raf.length; },
    cancelAnimationFrame() {},
    addEventListener: eventTarget.addEventListener,
    dispatchEvent: eventTarget.dispatchEvent,
    Event: class { constructor(type) { this.type = type; } },
    getComputedStyle: () => ({ getPropertyValue: name => ({
      "padding-top": "24px", "padding-bottom": "34px", "padding-left": "0px", "padding-right": "0px"
    }[name] || "0px") }),
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
  return { canvas, canvasStyle, shellStyle, rootStyle, visualViewport, request, ticks, raf, mutations, listeners, context };
}

async function startIntegratedRuntime() {
  const fixture = integratedRuntime();
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));
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
  assert.deepEqual(ticks.at(-1), { resized: 1, generation: 2, drawable: [296, 592], logical: [320, 640] });

  visualViewport.height = 600;
  visualViewport.dispatchEvent(new fixture.context.Event("scroll"));
  raf.shift()(48);
  assert.equal(ticks.at(-1).resized, 1);
  assert.equal(ticks.at(-1).generation, 3);

  visualViewport.offsetTop = 100;
  visualViewport.dispatchEvent(new fixture.context.Event("scroll"));
  raf.shift()(64);
  assert.equal(rootStyle.values["--stasis-visible-offset-top"], "100px");
  assert.equal(ticks.at(-1).resized, 0);
  assert.equal(ticks.at(-1).generation, 3);

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
