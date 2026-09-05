import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";
import { fakeWebGL2 } from "./fake_webgl2.mjs";

const source = fs.readFileSync(new URL("../game.js", import.meta.url), "utf8");

const settle = async () => {
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));
};

async function createRuntime({ deferredDecode = false, stereoPanner = true } = {}) {
  const memory = new WebAssembly.Memory({ initial: 1 });
  const sources = [];
  const gains = [];
  const panners = [];
  let imports;
  let resolveDecode;
  const decoded = deferredDecode
    ? new Promise(resolve => { resolveDecode = resolve; })
    : Promise.resolve({ decoded: true });

  class FakeNode {
    constructor() { this.connections = []; }
    connect(target, output, input) {
      this.connections.push({ target, output, input });
      return target;
    }
    disconnect() { this.connections.length = 0; }
  }
  class FakeSource extends FakeNode {
    constructor() {
      super();
      this.listeners = new Map();
      this.playbackRate = { value: 1 };
      this.started = false;
      this.stopped = false;
    }
    start() { this.started = true; }
    stop() { this.stopped = true; }
    addEventListener(type, listener) { this.listeners.set(type, listener); }
    end() { this.listeners.get("ended")?.(); }
  }
  class FakeGain extends FakeNode {
    constructor() { super(); this.gain = { value: 1 }; }
  }
  class FakePanner extends FakeNode {
    constructor() { super(); this.pan = { value: 0 }; }
  }
  class FakeAudioContext {
    constructor() {
      this.state = "running";
      this.currentTime = 0;
      this.destination = new FakeNode();
    }
    resume() { this.state = "running"; return Promise.resolve(); }
    suspend() { this.state = "suspended"; return Promise.resolve(); }
    close() { this.state = "closed"; return Promise.resolve(); }
    decodeAudioData() { return decoded; }
    createBufferSource() {
      const value = new FakeSource();
      sources.push(value);
      return value;
    }
    createGain() {
      const value = new FakeGain();
      gains.push(value);
      return value;
    }
    createChannelMerger() { return new FakeNode(); }
    createStereoPanner() {
      if (!stereoPanner) throw new Error("stereo panner should be absent");
      const value = new FakePanner();
      panners.push(value);
      return value;
    }
  }
  if (!stereoPanner) delete FakeAudioContext.prototype.createStereoPanner;

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
  const game = { memory: {}, strings: { 7: "sound.wav" }, assets: {} };
  const instance = { exports: { memory, main: () => 0, tick() {}, render() {} } };
  const contextObject = {
    document: {
      body,
      hidden: false,
      fullscreenElement: null,
      fonts: { ready: Promise.resolve(), add() {} },
      hasFocus: () => true,
      getElementById(id) {
        if (id === "stasis-canvas") return canvas;
        if (id === "stasis-error") return { textContent: "" };
        return null;
      },
      addEventListener() {},
    },
    screen: { width: 480, height: 720 },
    devicePixelRatio: 1,
    performance: { now: () => 0 },
    WebAssembly: {
      Memory: WebAssembly.Memory,
      Global: WebAssembly.Global,
      instantiate: async (_bytes, value) => { imports = value.env; return { instance }; },
    },
    fetch: async uri => ({
      ok: true,
      arrayBuffer: async () => new ArrayBuffer(uri === "game.wasm" ? 8 : 4),
    }),
    requestAnimationFrame() {},
    cancelAnimationFrame() {},
    addEventListener() {},
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
  await settle();

  return {
    imports,
    sources,
    gains,
    panners,
    load: async () => {
      const handle = imports.audio_load_wav(7);
      await settle();
      return handle;
    },
    resolveDecode: () => resolveDecode?.({ decoded: true }),
  };
}

test("overlapping plays of one asset return distinct independently controlled voices", async () => {
  const runtime = await createRuntime();
  const asset = await runtime.load();
  const first = runtime.imports.audio_play(asset, false, 0.4, -0.25);
  const second = runtime.imports.audio_play(asset, false, 0.8, 0.5);
  assert.ok(first > 0);
  assert.ok(second > first);
  assert.notEqual(first, second);
  await settle();
  assert.equal(runtime.sources.length, 2);
  assert.equal(runtime.imports.audio_voice_is_playing(first), 1);
  assert.equal(runtime.imports.audio_voice_is_playing(second), 1);
  runtime.imports.audio_stop(first);
  assert.equal(runtime.sources[0].stopped, true);
  assert.equal(runtime.sources[1].stopped, false);
  assert.equal(runtime.imports.audio_voice_is_playing(first), 0);
  assert.equal(runtime.imports.audio_voice_is_playing(second), 1);
});

test("start and live updates apply clamped stereo pan and volume per voice", async () => {
  const runtime = await createRuntime();
  const asset = await runtime.load();
  const voice = runtime.imports.audio_play(asset, false, 0.25, -0.75);
  await settle();
  assert.equal(runtime.gains[0].gain.value, 0.25);
  assert.equal(runtime.panners[0].pan.value, -0.75);
  runtime.imports.audio_voice_set_volume_pan(voice, 2, -2);
  assert.equal(runtime.gains[0].gain.value, 1);
  assert.equal(runtime.panners[0].pan.value, -1);
});

test("missing StereoPanner uses deterministic equal-power gains", async () => {
  const runtime = await createRuntime({ stereoPanner: false });
  const asset = await runtime.load();
  const voice = runtime.imports.audio_play(asset, false, 0.5, 0);
  await settle();
  assert.ok(Math.abs(runtime.gains[1].gain.value - Math.SQRT1_2) < 0.000001);
  assert.ok(Math.abs(runtime.gains[2].gain.value - Math.SQRT1_2) < 0.000001);
  runtime.imports.audio_voice_set_volume_pan(voice, 0.5, 1);
  assert.ok(Math.abs(runtime.gains[1].gain.value) < 0.000001);
  assert.ok(Math.abs(runtime.gains[2].gain.value - 1) < 0.000001);
});

test("stopping before decode prevents a late source from starting", async () => {
  const runtime = await createRuntime({ deferredDecode: true });
  const asset = await runtime.load();
  const voice = runtime.imports.audio_play(asset, false, 1, 0);
  assert.equal(runtime.imports.audio_voice_is_playing(voice), 1);
  runtime.imports.audio_stop(voice);
  runtime.resolveDecode();
  await settle();
  assert.equal(runtime.sources.length, 0);
  assert.equal(runtime.imports.audio_voice_is_playing(voice), 0);
});

test("ended cleanup cannot remove a different overlapping voice", async () => {
  const runtime = await createRuntime();
  const asset = await runtime.load();
  const first = runtime.imports.audio_play(asset, false, 1, 0);
  const second = runtime.imports.audio_play(asset, false, 1, 0);
  await settle();
  runtime.sources[0].end();
  assert.equal(runtime.imports.audio_voice_is_playing(first), 0);
  assert.equal(runtime.imports.audio_voice_is_playing(second), 1);
});

test("pause preserves a live voice and resumes its playback cursor", async () => {
  const runtime = await createRuntime();
  const asset = await runtime.load();
  const voice = runtime.imports.audio_play(asset, true, 1, 0);
  await settle();
  runtime.imports.audio_voice_set_paused(voice, true);
  assert.equal(runtime.sources[0].playbackRate.value, 0);
  assert.equal(runtime.imports.audio_voice_is_playing(voice), 1);
  runtime.imports.audio_voice_set_paused(voice, false);
  assert.equal(runtime.sources[0].playbackRate.value, 1);
});

test("voice storage is bounded and stopped slots permit later voices", async () => {
  const runtime = await createRuntime();
  const asset = await runtime.load();
  const voices = Array.from({ length: 32 }, () => runtime.imports.audio_play(asset, true, 1, 0));
  assert.ok(voices.every(handle => handle > 0));
  assert.equal(new Set(voices).size, 32);
  assert.equal(runtime.imports.audio_play(asset, true, 1, 0), 0);
  runtime.imports.audio_stop(voices[0]);
  const replacement = runtime.imports.audio_play(asset, true, 1, 0);
  assert.ok(replacement > voices.at(-1));
  assert.equal(runtime.imports.audio_voice_is_playing(replacement), 1);
});
