import assert from "node:assert/strict";
import test from "node:test";
import fs from "node:fs";
import vm from "node:vm";
import { fakeWebGL2 } from "./fake_webgl2.mjs";

const source = fs.readFileSync(new URL("../game.js", import.meta.url), "utf8");

async function loadRuntime({ blocked = false, headless = false, recording = false } = {}) {
  const memory = new WebAssembly.Memory({ initial: 1 });
  const globalListeners = new Map();
  const canvasListeners = new Map();
  const opened = [];
  const navigations = [];
  let imports;
  const canvas = {
    width: 640, height: 360, style: {}, dataset: {}, parentElement: { style: {} },
    getAttribute() { return null; },
    getContext: kind => kind === "webgl2" ? fakeWebGL2() : ({ fillRect() {}, fillText() {}, save() {}, restore() {}, beginPath() {},
      moveTo() {}, lineTo() {}, stroke() {}, drawImage() {}, translate() {}, rotate() {} }),
    getBoundingClientRect: () => ({ left: 0, top: 0, right: 640, bottom: 360, width: 640, height: 360 }),
    addEventListener(name, listener) { canvasListeners.set(name, listener); },
    setPointerCapture() {}, focus() {}, requestFullscreen: async () => {},
  };
  const body = { dataset: {} };
  const documentListeners = new Map();
  const document = {
    body, hidden: false, fullscreenElement: null,
    fonts: { ready: Promise.resolve(), add() {} }, hasFocus: () => true,
    getElementById(id) {
      if (id === "stasis-canvas") return canvas;
      if (id === "stasis-hud") return { textContent: "" };
      if (id === "stasis-error") return { textContent: "" };
      if (id === "stasis-audio") return { addEventListener() {}, disabled: false, textContent: "" };
      if (id === "stasis-loading") return { dataset: {}, textContent: "" };
      if (id === "stasis-loading-status") return { textContent: "" };
      return null;
    },
    addEventListener(name, listener) { documentListeners.set(name, listener); },
    createElement: () => ({ getContext: () => ({}) }),
  };
  const popup = () => {
    const popupDocument = {
      body: { append() {} },
      createElement() {
        return {
          href: "", target: "", rel: "", referrerPolicy: "",
          click() { navigations.push({ href: this.href, target: this.target, rel: this.rel, referrerPolicy: this.referrerPolicy }); },
          remove() {},
        };
      },
    };
    return { document: popupDocument, opener: {}, close() {} };
  };
  const strings = {
    1: "https://www.maddoxlabs.com/",
    2: "http://example.test/path?x=1#part",
    3: "mailto:test@example.test",
    4: "https://example.test/line\nbreak",
    5: `https://example.test/${"é".repeat(1014)}`,
    6: "https://example.test/%zz",
    7: "https://user@example.test/",
    8: "HTTPS://example.test/",
    9: "https:\\example.test/path",
    10: "https://127.1/path",
    11: "https://01.2.3.4/path",
    12: "https://example.test:0/path",
    13: "https://maddoxé.test/path",
    14: "https://[::ffff:127.0.0.1]/path",
    15: "https://[::1]:443/path",
  };
  const instance = { exports: { memory, main: () => 0, tick: () => 0, render: () => 0 } };
  const window = {
    STASIS_GAME: { strings, memory: {}, globals: {}, views: {}, assets: {}, headless, recording },
    open(url, target) {
      opened.push({ url, target });
      return blocked ? null : popup();
    },
  };
  const context = {
    document, window, navigator: { userActivation: { isActive: true }, clipboard: {} },
    localStorage: { getItem() { return null; }, setItem() {} },
    WebAssembly: {
      Memory: WebAssembly.Memory, Global: WebAssembly.Global,
      instantiate: async (_bytes, values) => { imports = values.env; return { instance }; },
    },
    fetch: async () => ({ ok: true, arrayBuffer: async () => new ArrayBuffer(0) }),
    requestAnimationFrame() { return 1; }, cancelAnimationFrame() {},
    addEventListener(name, listener) { globalListeners.set(name, listener); },
    console, performance: { now: () => 0 }, screen: { width: 640, height: 360 },
    Image: class {}, FontFace: class { load() { return Promise.resolve(this); } },
    AudioContext: class { constructor() { this.state = "running"; this.currentTime = 0; this.destination = {}; } close() {} resume() {} },
    TextDecoder, TextEncoder, URL, URLSearchParams, setTimeout, clearTimeout, ArrayBuffer,
    STASIS_CHARACTERIZATION_TEST: true,
  };
  vm.runInNewContext(source, context, { filename: "runtime/web/game.js" });
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));
  await window.STASIS_RUNTIME_PROMISE;
  return { body, canvasListeners, context, documentListeners, globalListeners, imports, navigations, opened, testing: window.__STASIS_CHARACTERIZATION__ };
}

const pointerEvent = () => ({
  clientX: 10, clientY: 10, pointerId: 1, pointerType: "mouse",
});

test("external URL import accepts bounded HTTP(S) only", async () => {
  const runtime = await loadRuntime();
  const open = runtime.imports.stasis_jit_open_external_url;
  for (const id of [1, 2, 15]) {
    runtime.canvasListeners.get("pointerdown")(pointerEvent());
    assert.equal(open(id), 1);
    runtime.canvasListeners.get("pointerup")(pointerEvent());
  }
  assert.deepEqual(runtime.navigations.map(value => value.href), [
    "https://www.maddoxlabs.com/",
    "http://example.test/path?x=1#part",
    "https://[::1]:443/path",
  ]);
  assert.ok(runtime.navigations.every(value => value.target === "_self"));
  assert.ok(runtime.navigations.every(value => value.rel === "noopener noreferrer"));
  assert.ok(runtime.navigations.every(value => value.referrerPolicy === "no-referrer"));
  assert.ok(runtime.opened.every(value => value.url === "about:blank" && value.target === "_blank"));
});

test("external URL import rejects unsupported, unsafe, and oversized values", async () => {
  const runtime = await loadRuntime();
  const open = runtime.imports.stasis_jit_open_external_url;
  for (const id of [3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 999]) {
    runtime.testing.markExternalActionGesture();
    assert.equal(open(id), -1, `string ${id}`);
    assert.equal(runtime.body.dataset.externalUrlResult, "invalid");
  }
  assert.equal(runtime.opened.length, 0);
});

test("external URL request consumes one input edge and reports a blocked popup", async () => {
  const runtime = await loadRuntime({ blocked: true });
  const open = runtime.imports.stasis_jit_open_external_url;
  runtime.canvasListeners.get("pointerdown")(pointerEvent());
  assert.equal(open(1), 0);
  assert.equal(runtime.body.dataset.externalUrlResult, "blocked");
  assert.equal(open(1), 0, "held input cannot retry the consumed request");
  assert.equal(runtime.opened.length, 1);

  runtime.canvasListeners.get("pointerdown")(pointerEvent());
  assert.equal(open(1), 0, "a duplicate down event while held is not a new edge");
  assert.equal(runtime.opened.length, 1);
  runtime.canvasListeners.get("pointerup")(pointerEvent());
  runtime.canvasListeners.get("pointerdown")(pointerEvent());
  assert.equal(open(1), 0, "a later pointer edge may make one new attempt");
  assert.equal(runtime.opened.length, 2);
});

test("external URL request expires after its frame and is unavailable without activation", async () => {
  const runtime = await loadRuntime();
  const open = runtime.imports.stasis_jit_open_external_url;
  runtime.testing.markExternalActionGesture();
  runtime.testing.clearExternalActionGesture();
  assert.equal(open(1), 0);
  assert.equal(runtime.body.dataset.externalUrlResult, "ignored");

  runtime.testing.markExternalActionGesture();
  runtime.context.navigator.userActivation.isActive = false;
  assert.equal(open(1), 0);
  assert.equal(runtime.body.dataset.externalUrlResult, "unavailable");
  assert.equal(runtime.opened.length, 0);
});

test("headless and recording web modes never open a browser", async () => {
  for (const mode of [{ headless: true }, { recording: true }]) {
    const runtime = await loadRuntime(mode);
    runtime.testing.markExternalActionGesture();
    assert.equal(runtime.imports.stasis_jit_open_external_url(1), 0);
    assert.equal(runtime.body.dataset.externalUrlResult, "unavailable");
    assert.equal(runtime.opened.length, 0);
  }
});
