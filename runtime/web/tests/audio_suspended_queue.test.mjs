import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";
import { fakeWebGL2 } from "./fake_webgl2.mjs";

const source = fs.readFileSync(new URL("../game.js", import.meta.url), "utf8");

async function createRuntime({ deviceFailure = false } = {}) {
  const windowEvents = new Map();
  const documentEvents = new Map();
  const memory = new WebAssembly.Memory({ initial: 2 });
  const starts = [];
  const buffers = [];
  const contexts = [];
  let failScheduling = false;
  let imports;
  let allowResume = false;

  class FakeAudioContext {
    constructor() {
      if (deviceFailure) throw new Error("audio device unavailable");
      this.state = "suspended";
      this.currentTime = 0;
      this.destination = {};
      contexts.push(this);
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
      if (failScheduling) throw new Error("audio output lost");
      const data = Array.from({ length: channels }, () => new Float32Array(frames));
      const buffer = { frames, data, getChannelData: channel => data[channel] };
      buffers.push(buffer);
      return buffer;
    }
    createBufferSource() {
      return {
        connect() { return this; },
        disconnect() {},
        stop() {},
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
    contexts,
    failScheduling: () => { failScheduling = true; },
    visibility: async hidden => {
      document.hidden = hidden;
      documentEvents.get("visibilitychange")();
      await new Promise(resolve => setImmediate(resolve));
    },
    pagehide: () => windowEvents.get("pagehide")({ persisted: false }),
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

test("Wasm instantiates with every public stream and effect voice import", async () => {
  const { imports } = await createRuntime();
  const names = ["init", "shutdown", "is_available", "get_sample_rate", "get_channels",
    "get_queued_frames", "get_underruns", "push_f32_interleaved", "play", "stop",
    "voice_is_playing", "voice_set_paused", "voice_set_volume_pan"];
  const uleb = value => {
    const bytes = [];
    do { const byte = value & 127; value >>>= 7; bytes.push(byte | (value ? 128 : 0)); } while (value);
    return bytes;
  };
  const string = value => [...uleb(value.length), ...Buffer.from(value)];
  const entries = names.flatMap(name => [...string("env"), ...string(`stasis_jit_audio_${name}`), 0, 0]);
  const section = [names.length, ...entries];
  const module = new WebAssembly.Module(Uint8Array.from([
    0, 97, 115, 109, 1, 0, 0, 0,
    1, 4, 1, 96, 0, 0, // A function type; JS host functions accept Wasm signatures.
    2, ...uleb(section.length), ...section,
  ]));
  assert.ok(new WebAssembly.Instance(module, { env: imports }));
  assert.equal(imports.stasis_jit_audio_play(-1, false, 1, 0), 0);
  assert.equal(imports.stasis_jit_audio_voice_is_playing(-1), 0);
  imports.stasis_jit_audio_stop(-1);
  imports.stasis_jit_audio_voice_set_paused(-1, true);
  imports.stasis_jit_audio_voice_set_volume_pan(-1, 0.5, 0);
});

test("public AudioStream imports share the legacy adapters and own stereo PCM", async () => {
  const runtime = await createRuntime();
  const names = ["init", "shutdown", "is_available", "get_sample_rate", "get_channels",
    "get_queued_frames", "get_underruns", "push_f32_interleaved"];
  for (const name of names) {
    assert.equal(typeof runtime.imports[`stasis_jit_audio_${name}`], "function", name);
    if (!name.startsWith("get_")) {
      assert.equal(runtime.imports[`stasis_jit_audio_${name}`], runtime.imports[`audio_${name}`]);
    }
  }
  const api = runtime.imports;
  assert.equal(api.stasis_jit_audio_init(48000, 2, 4800), 1);
  new Float32Array(runtime.memory.buffer, 0, 4).set([0.25, -0.5, 0.75, -1]);
  assert.equal(api.stasis_jit_audio_push_f32_interleaved(0, 2), 2);
  new Float32Array(runtime.memory.buffer, 0, 4).fill(0);
  await runtime.resume();
  assert.deepEqual(Array.from(runtime.buffers[0].data[0]), [0.25, 0.75]);
  assert.deepEqual(Array.from(runtime.buffers[0].data[1]), [-0.5, -1]);
  api.stasis_jit_audio_shutdown();
  assert.equal(api.stasis_jit_audio_is_available(), 0);
  assert.equal(api.stasis_jit_audio_push_f32_interleaved(0, 2), 0);
  assert.equal(api.stasis_jit_audio_init(44100, 1, 4410), 0);
  assert.equal(api.stasis_jit_audio_init(44100, 2, 4410), 1);
  assert.equal(api.stasis_jit_audio_get_sample_rate(), 44100);
  assert.equal(api.stasis_jit_audio_get_channels(), 2);
  assert.equal(api.stasis_jit_audio_get_queued_frames(), 0);
});

test("AudioStream device failure never reports successful initialization or pushes", async () => {
  const { imports } = await createRuntime({ deviceFailure: true });
  assert.equal(imports.stasis_jit_audio_init(48000, 2, 1024), 0);
  assert.equal(imports.stasis_jit_audio_is_available(), 0);
  assert.equal(imports.stasis_jit_audio_push_f32_interleaved(0, 2), 0);
});

test("scheduling failure drops owned PCM and marks the stream unavailable", async () => {
  const runtime = await createRuntime();
  const api = runtime.imports;
  api.stasis_jit_audio_init(48000, 2, 1024);
  await runtime.resume();
  runtime.failScheduling();
  assert.equal(api.stasis_jit_audio_push_f32_interleaved(0, 4), 0);
  assert.equal(api.stasis_jit_audio_is_available(), 0);
  assert.equal(api.stasis_jit_audio_get_queued_frames(), 0);
});

test("queue counts owned frames, drains with audio time, and suspends with visibility", async () => {
  const runtime = await createRuntime();
  const api = runtime.imports;
  api.stasis_jit_audio_init(48000, 2, 1024);
  await runtime.resume();
  assert.equal(api.stasis_jit_audio_push_f32_interleaved(1, 2), 0);
  assert.equal(api.stasis_jit_audio_push_f32_interleaved(-4, 2), 0);
  assert.equal(api.stasis_jit_audio_push_f32_interleaved(runtime.memory.buffer.byteLength, 2), 0);
  assert.equal(api.stasis_jit_audio_push_f32_interleaved(0, 480), 480);
  assert.equal(api.stasis_jit_audio_get_queued_frames(), 480);
  const context = runtime.contexts.at(-1);
  context.currentTime = 0.010;
  assert.equal(api.stasis_jit_audio_get_queued_frames(), 240);
  await runtime.visibility(true);
  assert.equal(context.state, "suspended");
  assert.equal(api.stasis_jit_audio_push_f32_interleaved(0, 8192), 4560);
  assert.equal(api.stasis_jit_audio_get_queued_frames(), 4800);
  await runtime.visibility(false);
  assert.equal(context.state, "running");
  context.currentTime = 1;
  assert.equal(api.stasis_jit_audio_get_queued_frames(), 0);
  assert.equal(api.stasis_jit_audio_push_f32_interleaved(0, 4), 4);
  assert.equal(api.stasis_jit_audio_get_underruns(), 1);
});

test("page close drops pending PCM and running queues apply backpressure", async () => {
  const runtime = await createRuntime();
  runtime.imports.stasis_jit_audio_init(48000, 2, 1024);
  runtime.imports.stasis_jit_audio_push_f32_interleaved(0, 2);
  runtime.pagehide();
  await runtime.resume();
  assert.equal(runtime.starts.length, 0);
  assert.equal(runtime.imports.stasis_jit_audio_is_available(), 0);
  runtime.imports.stasis_jit_audio_init(48000, 2, 1024);
  await runtime.resume();
  assert.equal(runtime.imports.stasis_jit_audio_push_f32_interleaved(0, 8192), 8192);
  assert.equal(runtime.imports.stasis_jit_audio_push_f32_interleaved(0, 1), 0);
});

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
  assert.equal(runtime.imports.audio_get_queued_frames(), 4800);
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
  assert.equal(runtime.imports.audio_get_queued_frames(), 6000);
});
