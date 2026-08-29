import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";

const source = fs.readFileSync(new URL("../game.js", import.meta.url), "utf8");

async function runSequence(first, second, options = {}) {
  const events = new Map();
  const visualViewportEvents = new Map();
  const raf = [];
  const ticks = [];
  let now = 0;
  const memory = new WebAssembly.Memory({ initial: 2 });
  const game = { memory: {
    host_i32: { offset: 0, length: 768 },
    host_f32: { offset: 768 * 4, length: 64 }
  }, strings: {}, assets: {} };
  const requestGlobals = options.backingRequest ? {
    host_req_seq: { value: 0 },
    host_req_flags: { value: 0 },
    host_req_window_w_px: { value: first[0] },
    host_req_window_h_px: { value: first[1] }
  } : {};
  let canvasWidth = first[0];
  let canvasHeight = first[1];
  const context = {
    fillRect() {}, fillText() {}, save() {}, restore() {}, beginPath() {}, moveTo() {}, lineTo() {}, stroke() {},
    drawImage() {}, translate() {}, rotate() {}
  };
  const canvas = {
    width: canvasWidth,
    height: canvasHeight,
    style: {},
    parentElement: { style: {} },
    listeners: new Map(),
    getContext: () => context,
    getBoundingClientRect: () => ({ left: 0, top: 0, width: canvasWidth, height: canvasHeight }),
    addEventListener(type, listener) { this.listeners.set(type, listener); },
    setPointerCapture() {},
    focus() {},
    requestFullscreen: async () => {},
  };
  const body = { dataset: {} };
  const errorBox = { textContent: "" };
  const document = {
    body,
    hidden: false,
    fullscreenElement: null,
    fonts: { ready: Promise.resolve(), add() {} },
    hasFocus: () => true,
    getElementById(id) {
      if (id === "stasis-canvas") return canvas;
      if (id === "stasis-hud") return { textContent: "" };
      if (id === "stasis-audio") return { addEventListener() {}, disabled: false, textContent: "" };
      if (id === "stasis-error") return errorBox;
      return null;
    },
    addEventListener(type, listener) { events.set(`document:${type}`, listener); },
  };
  let screenWidth = options.desktop?.[0] ?? first[0];
  let screenHeight = options.desktop?.[1] ?? first[1];
  const screen = { get width() { return screenWidth; }, get height() { return screenHeight; } };
  const visualViewport = {
    width: first[0],
    height: first[1],
    addEventListener(type, listener) { visualViewportEvents.set(type, listener); }
  };
  const instance = {
    exports: {
      memory,
      ...requestGlobals,
      main: () => 0,
      tick: () => {
        const i32 = new Int32Array(memory.buffer, 0, 768);
        const f32 = new Float32Array(memory.buffer, 768 * 4, 64);
        if (i32[547]) actionCount += 1;
        ticks.push({
          resized: i32[11], displayGeneration: i32[30], desktop: [i32[12], i32[13]], native: [i32[22], i32[23]], drawable: [i32[24], i32[25]],
          logical: [f32[50], f32[51]], pointer: [f32[0], f32[1]], pointerCount: i32[7], wentDown: i32[546], wentUp: i32[547], actionCount
        });
      },
      render: () => 0,
    }
  };
  let actionCount = 0;
  const contextObject = {
    document, screen, devicePixelRatio: options.dpr ?? 1, performance: { now: () => now },
    WebAssembly: { instantiate: async () => ({ instance }) },
    fetch: async () => ({ ok: true, arrayBuffer: async () => new ArrayBuffer(0) }),
    requestAnimationFrame: callback => { raf.push(callback); return raf.length; },
    cancelAnimationFrame() {},
    addEventListener(type, listener) { events.set(`window:${type}`, listener); },
    console, Image: class {}, FontFace: class { load() { return Promise.resolve(this); } },
    AudioContext: class { constructor() { this.state = "running"; this.currentTime = 0; this.destination = {}; } close() {} resume() {} },
    TextDecoder, setTimeout, clearTimeout, STASIS_GAME: game,
  };
  contextObject.window = {
    STASIS_GAME: game,
    screen,
    visualViewport,
    STASIS_REFIT_VIEWPORT: options.backingRequest
      ? () => { canvasWidth = canvas.width; canvasHeight = canvas.height; }
      : undefined
  };
  vm.runInNewContext(source, contextObject, { filename: "runtime/web/game.js" });
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(raf.length, 1, `runtime schedules its first RAF: ${errorBox.textContent}`);
  const frame = () => { now += 16; raf.shift()(now); };
  frame();
  if (options.desktopOnly) return ticks;
  const dispatch = (type, event = {}) => canvas.listeners.get(type)?.(event);
  dispatch("pointerdown", { pointerId: 7, clientX: first[0] - 1, clientY: first[1] - 1 });
  frame();
  const down = ticks.at(-1);
  assert.equal(down.pointerCount, 1);
  assert.equal(down.wentDown, 1);
  assert.equal(down.wentUp, 0);
  assert.equal(down.logical[0], first[0]);
  assert.equal(down.logical[1], first[1]);
  assert.deepEqual(down.pointer, [first[0] - 1, first[1] - 1]);
  if (options.backingRequest) {
    screenWidth = second[0]; screenHeight = second[1];
    requestGlobals.host_req_seq.value = 1;
    requestGlobals.host_req_flags.value = 4;
    requestGlobals.host_req_window_w_px.value = second[0];
    requestGlobals.host_req_window_h_px.value = second[1];
    frame();
    const backingResize = ticks.at(-1);
    assert.equal(backingResize.resized, 1);
    assert.equal(backingResize.displayGeneration, 2);
    assert.deepEqual(backingResize.drawable, first, "CSS extent remains the layout authority");
    assert.deepEqual(backingResize.logical, second, "intentional logical resize reaches the same HostFrame");
    requestGlobals.host_req_seq.value = 2;
    frame();
    const unchangedRequest = ticks.at(-1);
    assert.equal(unchangedRequest.resized, 0, "same-size maximized request does not report a resize");
    assert.equal(unchangedRequest.displayGeneration, 2);
    return ticks;
  }
  screenWidth = second[0]; screenHeight = second[1];
  canvasWidth = second[0]; canvasHeight = second[1];
  visualViewport.width = second[0]; visualViewport.height = second[1];
  visualViewportEvents.get("resize")();
  events.get("window:resize")();
  events.get("window:orientationchange")();
  assert.deepEqual([screen.width, screen.height], second);
  frame();
  const resized = ticks.at(-1);
  assert.equal(resized.resized, 1);
  assert.equal(resized.displayGeneration, 2);
  assert.deepEqual(resized.native, second);
  assert.deepEqual(resized.drawable, second);
  assert.deepEqual(resized.logical, first, "guest canvas remains selected logical size");
  dispatch("pointerup", { pointerId: 7, clientX: second[0] - 1, clientY: second[1] - 1 });
  frame();
  const up = ticks.at(-1);
  assert.equal(up.resized, 0);
  assert.equal(up.wentUp, 1);
  assert.deepEqual(up.pointer, [
    Math.round((second[0] - 1) * first[0] / second[0]),
    Math.round((second[1] - 1) * first[1] / second[1])
  ]);
  assert.equal(up.actionCount, 1, "release increments the action counter exactly once");
  dispatch("pointerleave");
  frame();
  const quiet = ticks.at(-1);
  assert.equal(quiet.resized, 0);
  assert.equal(quiet.displayGeneration, 2);
  assert.equal(quiet.pointerCount, 0);
  assert.equal(quiet.wentUp, 0);
  assert.equal(quiet.actionCount, 1, "quiet frames do not repeat the action");
  return ticks;
}

test("web HostFrame portrait to landscape preserves actionable release", async () => {
  await runSequence([360, 720], [720, 360]);
});

test("web HostFrame landscape to portrait preserves actionable release", async () => {
  await runSequence([720, 360], [360, 720]);
});

test("web HostFrame reports synchronous intentional backing resize", async () => {
  await runSequence([360, 720], [320, 640], { backingRequest: true });
});

test("web HostFrame keeps desktop physical size distinct from fitted canvas backing", async () => {
  const ticks = await runSequence([960, 540], [960, 540], {
    desktop: [1920, 1080], dpr: 2, desktopOnly: true
  });
  assert.deepEqual(ticks.at(-1).desktop, [3840, 2160]);
  assert.deepEqual(ticks.at(-1).native, [960, 540]);
  assert.deepEqual(ticks.at(-1).drawable, [1920, 1080]);
});

test("web HostFrame falls back to CSS extent when desktop screen metrics are invalid", async () => {
  const ticks = await runSequence([960, 540], [960, 540], {
    desktop: [NaN, Infinity], dpr: 2, desktopOnly: true
  });
  assert.deepEqual(ticks.at(-1).desktop, [960, 540]);
  assert.deepEqual(ticks.at(-1).native, [960, 540]);
  assert.deepEqual(ticks.at(-1).drawable, [1920, 1080]);
});
