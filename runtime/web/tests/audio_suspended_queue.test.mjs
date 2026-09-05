import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";
import { fakeWebGL2 } from "./fake_webgl2.mjs";

const source = fs.readFileSync(new URL("../game.js", import.meta.url), "utf8");

async function createRuntime() {
  const windowEvents = new Map();
  const documentEvents = new Map();
  const memory = new WebAssembly.Memory({ initial: 2 });
  const starts = [];
  const buffers = [];
  let imports;
  let allowResume = false;

  class FakeAudioContext {
    constructor() {
      this.state = "suspended";
      this.currentTime = 0;
      this.destination = {};
    }
    resume() {
      if (allowResume) this.state = "running";
      return Promise.resolve();
    }
    suspend() {
      this.state = "suspended";
      return Promise.resolve();
    }
    close() {
      this.state = "closed";
      return Promise.resolve();
    }
    createBuffer(channels, frames) {
      const data = Array.from({ length: channels }, () => new Float32Array(frames));
      const buffer = { frames, data, getChannelData: channel => data[channel] };
      buffers.push(buffer);
      return buffer;
    }
    createBufferSource() {
      return {
        connect() { return this; },
        start(at = this.context?.currentTime || 0) { starts.push({ at, buffer: this.buffer }); },
        addEventListener() {},
      };
    }
  }

  const context2d = {
    fillRect() {}, fillText() {}, save() {}, restore() {}, beginPath() {}, moveTo() {},
    lineTo() {}, stroke() {}, drawImage() {}, translate() {}, rotate() {},
    measureText: () => ({ width: 0 }),
  };
  const canvas = {
    width: 480,
    height: 720,
    style: {},
    parentElement: { style: {} },
    getContext: kind => kind === "webgl2" ? fakeWebGL2() : context2d,
    getBoundingClientRect: () => ({ left: 0, top: 0, width: 480, height: 720 }),
    addEventListener() {},
    setPointerCapture() {},
    focus() {},
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
      if (id === "stasis-hud") return null;
      if (id === "stasis-error") return errorBox;
      return null;
    },
    addEventListener(type, listener) { documentEvents.set(type, listener); },
  };
  const game = { memory: {}, strings: {}, assets: {} };
  const instance = { exports: { memory, main: () => 0, tick() {}, render() {} } };
  const contextObject = {
    document,
    screen: { width: 480, height: 720 },
    devicePixelRatio: 1,
    performance: { now: () => 0 },
    WebAssembly: {
      Memory: WebAssembly.Memory,
      Global: WebAssembly.Global,
      instantiate: async (_bytes, value) => { imports = value.env; return { instance }; },
    },
    fetch: async () => ({ ok: true, arrayBuffer: async () => new ArrayBuffer(0) }),
    requestAnimationFrame() {},
    cancelAnimationFrame() {},
    addEventListener(type, listener) { windowEvents.set(type, listener); },
    console,
    Image: class {},
    FontFace: class { load() { return Promise.resolve(this); } },
    AudioContext: FakeAudioContext,
    TextDecoder,
    TextEncoder,
    setTimeout,
    clearTimeout,
    STASIS_GAME: game,
  };
  contextObject.window = { STASIS_GAME: game };
  vm.runInNewContext(source, contextObject, { filename: "runtime/web/game.js" });
  await contextObject.window.STASIS_RUNTIME_PROMISE;
  await new Promise(resolve => setImmediate(resolve));

  return {
    imports,
    memory,
    starts,
    buffers,
    resume: async () => {
      allowResume = true;
      windowEvents.get("pointerdown")({});
      await new Promise(resolve => setImmediate(resolve));
      await new Promise(resolve => setImmediate(resolve));
    },
  };
}

function writeStereo(memory, frames, value) {
  new Float32Array(memory.buffer, 0, frames * 2).fill(value);
}

test("suspended PCM queue is latency bounded, reported, and flushed in order", async () => {
  const runtime = await createRuntime();
  runtime.imports.audio_init(48000, 2);

  writeStereo(runtime.memory, 2048, 0.1);
  assert.equal(runtime.imports.audio_push_f32_interleaved(0, 2048), 2048);
  writeStereo(runtime.memory, 2048, 0.2);
  assert.equal(runtime.imports.audio_push_f32_interleaved(0, 2048), 2048);
  writeStereo(runtime.memory, 2048, 0.3);
  assert.equal(runtime.imports.audio_push_f32_interleaved(0, 2048), 704);
  assert.equal(runtime.imports.audio_get_queued_frames(), 4800);
  assert.equal(runtime.imports.audio_push_f32_interleaved(0, 2048), 0);
  assert.equal(runtime.starts.length, 0);

  await runtime.resume();

  assert.equal(runtime.starts.length, 3);
  assert.deepEqual(runtime.buffers.map(buffer => buffer.frames), [2048, 2048, 704]);
  assert.ok(Math.abs(runtime.buffers[0].data[0][0] - 0.1) < 0.000001);
  assert.ok(Math.abs(runtime.buffers[1].data[0][0] - 0.2) < 0.000001);
  assert.ok(Math.abs(runtime.buffers[2].data[0][0] - 0.3) < 0.000001);
  assert.deepEqual(runtime.starts.map(start => Math.round(start.at * 48000)), [240, 2288, 4336]);
  assert.equal(runtime.imports.audio_get_queued_frames(), 5040);
});

test("suspended PCM closure count is bounded for tiny pushes", async () => {
  const runtime = await createRuntime();
  runtime.imports.audio_init(48000, 2);
  writeStereo(runtime.memory, 1, 0.25);

  for (let index = 0; index < 32; index += 1) {
    assert.equal(runtime.imports.audio_push_f32_interleaved(0, 1), 1);
  }
  assert.equal(runtime.imports.audio_push_f32_interleaved(0, 1), 0);
  assert.equal(runtime.imports.audio_get_queued_frames(), 32);

  await runtime.resume();
  assert.equal(runtime.starts.length, 32);
});

test("running PCM scheduling preserves full pushes and queue timing", async () => {
  const runtime = await createRuntime();
  runtime.imports.audio_init(48000, 2);
  await runtime.resume();
  writeStereo(runtime.memory, 6000, 0.5);

  assert.equal(runtime.imports.audio_push_f32_interleaved(0, 6000), 6000);
  assert.equal(runtime.starts.length, 1);
  assert.equal(runtime.buffers[0].frames, 6000);
  assert.equal(runtime.imports.audio_get_queued_frames(), 6240);
});
