import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";
import { fakeWebGL2 } from "./fake_webgl2.mjs";
import { installCollectionViewAbi } from "./collection_view_abi.mjs";

const source = fs.readFileSync(new URL("../game.js", import.meta.url), "utf8");

function stripFeature(sourceText, feature, enabled) {
  const begin = `// @stasis-feature ${feature} begin`;
  const end = `// @stasis-feature ${feature} end`;
  let inside = false;
  return sourceText.split(/\r?\n/).filter(line => {
    const trimmed = line.trim();
    if (trimmed === begin) { inside = true; return false; }
    if (trimmed === end) { inside = false; return false; }
    return enabled || !inside;
  }).join("\n");
}

async function createRuntime({
  deviceFailure = false,
  portalProvider,
  pauseAudioOnInit = false,
  audioEnabled = true,
  portalFactoryFailure = false,
  holdInitialAudioResume = false,
} = {}) {
  const windowEvents = new Map();
  const documentEvents = new Map();
  const canvasEvents = new Map();
  const adWatchEvents = new Map();
  const adCancelEvents = new Map();
  const addElementListener = (events, type, listener) => addListener(events, type, listener);
  const adWatchButton = {
    disabled: false,
    textContent: "Watch ad",
    addEventListener(type, listener) { addElementListener(adWatchEvents, type, listener); },
  };
  const adCancelButton = {
    addEventListener(type, listener) { addElementListener(adCancelEvents, type, listener); },
  };
  const adControls = { hidden: true };
  const adStatus = { textContent: "" };
  const addListener = (events, type, listener) => {
    if (!events.has(type)) events.set(type, []);
    events.get(type).push(listener);
  };
  const dispatch = (events, type, event = {}) => {
    for (const listener of events.get(type) || []) listener(event);
  };
  const memory = new WebAssembly.Memory({ initial: 2 });
  const starts = [];
  const buffers = [];
  const contexts = [];
  let failScheduling = false;
  let imports;
  let allowResume = false;
  let holdAudioResumes = holdInitialAudioResume;
  let nextFrame;
  let adBlocked = false;
  let lifecycleOptions;
  const pendingAudioResumes = [];
  const tickInputs = [];
  let tickCount = 0;
  let renderCount = 0;
  let disposeCount = 0;

  class FakeAudioContext {
    constructor() {
      if (deviceFailure) throw new Error("audio device unavailable");
      this.state = "suspended";
      this.currentTime = 0;
      this.destination = {};
      contexts.push(this);
    }
    resume() {
      return new Promise(resolve => {
        const finish = () => {
          if (allowResume) this.state = "running";
          resolve();
        };
        if (holdAudioResumes) pendingAudioResumes.push(finish);
        else finish();
      });
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
    addEventListener(type, listener) { addListener(canvasEvents, type, listener); },
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
      if (id === "stasis-ad-controls") return adControls;
      if (id === "stasis-ad-watch") return adWatchButton;
      if (id === "stasis-ad-cancel") return adCancelButton;
      if (id === "stasis-ad-status") return adStatus;
      return null;
    },
    addEventListener(type, listener) { addListener(documentEvents, type, listener); },
  };
  const hostApi = {
    request(kind) { return kind === 1 ? 71 : 70; },
    poll(handle) { return handle > 0 ? 1 : 4; },
    gameplayBlocked() { return adBlocked ? 1 : 0; },
    takeReward(handle) { return handle === 71 ? 1 : 0; },
    release() {},
    lifecycle() {},
    ready: Promise.resolve({ available: true }),
    resetGuest() {},
    dispose() { disposeCount += 1; },
  };
  const adModule = portalProvider ? {
    createAdLifecycle(options) {
      lifecycleOptions = options;
      if (portalFactoryFailure) throw new Error("injected module initialization failure");
      if (pauseAudioOnInit) options.onAudioPaused(true);
      return hostApi;
    },
  } : undefined;
  const game = {
    memory: {
      samples: { hash: 901, handle: 1901, offset: 0, length: 16384, stride: 4, type_id: 2 },
      host_i32: { hash: 902, handle: 1902, offset: 65536, length: 768, stride: 4, type_id: 1 },
      host_f32: { hash: 903, handle: 1903, offset: 68608, length: 64, stride: 4, type_id: 2 },
    },
    strings: {}, assets: {},
    portal: { provider: portalProvider || "none" },
    ...(portalProvider ? { adLifecycleUrl: "ad_lifecycle.js?hash=test" } : {}),
  };
  const instance = { exports: {
    memory,
    main: () => 0,
    tick() {
      tickCount += 1;
      const host = new Int32Array(memory.buffer, 65536, 768);
      tickInputs.push({ keyRight: host[32 + 79], pointerCount: host[7], pointerDown: host[545] });
    },
    render() { renderCount += 1; },
  } };
  installCollectionViewAbi(game, instance.exports);
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
    requestAnimationFrame(callback) { nextFrame = callback; return 1; },
    cancelAnimationFrame() {},
    addEventListener(type, listener) { addListener(windowEvents, type, listener); },
    console,
    Image: class {},
    FontFace: class { load() { return Promise.resolve(this); } },
    AudioContext: FakeAudioContext,
    TextDecoder,
    TextEncoder,
    setTimeout,
    clearTimeout,
    STASIS_GAME: game,
    STASIS_AD_LIFECYCLE: adModule,
  };
  contextObject.window = { STASIS_GAME: game, STASIS_AD_LIFECYCLE: adModule };
  vm.runInNewContext(audioEnabled ? source : stripFeature(source, "audio", false), contextObject, {
    filename: "runtime/web/game.js",
  });
  await contextObject.window.STASIS_RUNTIME_PROMISE;
  await new Promise(resolve => setImmediate(resolve));

  return {
    imports,
    memory,
    errorBox,
    starts,
    buffers,
    contexts,
    tickInputs,
    lifecycleCallbacks: () => lifecycleOptions,
    disposeCount: () => disposeCount,
    adControls,
    adStatus,
    dispatchAdWatch: (type, event) => dispatch(adWatchEvents, type, { ...event, currentTarget: adWatchButton }),
    dispatchAdCancel: type => dispatch(adCancelEvents, type, { currentTarget: adCancelButton }),
    adCancelListenerCount: () => (adCancelEvents.get("click") || []).length,
    setAdBlocked: blocked => {
      adBlocked = blocked;
      lifecycleOptions?.onGameplayBlocked(blocked);
    },
    setAdAudioPaused: paused => lifecycleOptions?.onAudioPaused(paused),
    runFrame: timestamp => {
      const callback = nextFrame;
      nextFrame = undefined;
      callback?.(timestamp);
      return Boolean(callback);
    },
    dispatchKey: (type, event) => dispatch(windowEvents, type, { preventDefault() {}, ...event }),
    dispatchCanvas: (type, event) => dispatch(canvasEvents, type, event),
    allowAudioResume: () => { allowResume = true; },
    holdNextAudioResume: () => { holdAudioResumes = true; },
    pendingAudioResumeCount: () => pendingAudioResumes.length,
    resolveAudioResume: index => {
      const finish = pendingAudioResumes.splice(index, 1)[0];
      assert.equal(typeof finish, "function", `pending audio resume ${index} exists`);
      finish();
    },
    resolveHeldAudioResumes: () => {
      holdAudioResumes = false;
      for (const finish of pendingAudioResumes.splice(0)) finish();
    },
    failScheduling: () => { failScheduling = true; },
    visibility: async hidden => {
      document.hidden = hidden;
      dispatch(documentEvents, "visibilitychange");
      await new Promise(resolve => setImmediate(resolve));
    },
    pagehide: (persisted = false) => dispatch(windowEvents, "pagehide", { persisted }),
    pageshow: (persisted = false) => dispatch(windowEvents, "pageshow", { persisted }),
    resume: async () => {
      allowResume = true;
      dispatch(windowEvents, "pointerdown");
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
  assert.equal(api.stasis_jit_audio_push_f32_interleaved(1901, 2), 2);
  new Float32Array(runtime.memory.buffer, 0, 4).fill(0);
  await runtime.resume();
  assert.deepEqual(Array.from(runtime.buffers[0].data[0]), [0.25, 0.75]);
  assert.deepEqual(Array.from(runtime.buffers[0].data[1]), [-0.5, -1]);
  api.stasis_jit_audio_shutdown();
  assert.equal(api.stasis_jit_audio_is_available(), 0);
  assert.equal(api.stasis_jit_audio_push_f32_interleaved(1901, 2), 0);
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
  assert.equal(imports.stasis_jit_audio_push_f32_interleaved(1901, 2), 0);
});

test("scheduling failure drops owned PCM and marks the stream unavailable", async () => {
  const runtime = await createRuntime();
  const api = runtime.imports;
  api.stasis_jit_audio_init(48000, 2, 1024);
  await runtime.resume();
  runtime.failScheduling();
  assert.equal(api.stasis_jit_audio_push_f32_interleaved(1901, 4), 0);
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
  assert.equal(api.stasis_jit_audio_push_f32_interleaved(1901, 480), 480);
  assert.equal(api.stasis_jit_audio_get_queued_frames(), 480);
  const context = runtime.contexts.at(-1);
  context.currentTime = 0.010;
  assert.equal(api.stasis_jit_audio_get_queued_frames(), 240);
  await runtime.visibility(true);
  assert.equal(context.state, "suspended");
  assert.equal(api.stasis_jit_audio_push_f32_interleaved(1901, 8192), 4560);
  assert.equal(api.stasis_jit_audio_get_queued_frames(), 4800);
  await runtime.visibility(false);
  assert.equal(context.state, "running");
  context.currentTime = 1;
  assert.equal(api.stasis_jit_audio_get_queued_frames(), 0);
  assert.equal(api.stasis_jit_audio_push_f32_interleaved(1901, 4), 4);
  assert.equal(api.stasis_jit_audio_get_underruns(), 1);
});

test("page close drops pending PCM and running queues apply backpressure", async () => {
  const runtime = await createRuntime();
  runtime.imports.stasis_jit_audio_init(48000, 2, 1024);
  runtime.imports.stasis_jit_audio_push_f32_interleaved(1901, 2);
  runtime.pagehide();
  await runtime.resume();
  assert.equal(runtime.starts.length, 0);
  assert.equal(runtime.imports.stasis_jit_audio_is_available(), 0);
  runtime.imports.stasis_jit_audio_init(48000, 2, 1024);
  await runtime.resume();
  assert.equal(runtime.imports.stasis_jit_audio_push_f32_interleaved(1901, 8192), 8192);
  assert.equal(runtime.imports.stasis_jit_audio_push_f32_interleaved(1901, 1), 0);
});

test("suspended PCM queue is latency bounded, reported, and flushed in order", async () => {
  const runtime = await createRuntime();
  runtime.imports.audio_init(48000, 2);

  writeStereo(runtime.memory, 2048, 0.1);
  assert.equal(runtime.imports.audio_push_f32_interleaved(1901, 2048), 2048);
  writeStereo(runtime.memory, 2048, 0.2);
  assert.equal(runtime.imports.audio_push_f32_interleaved(1901, 2048), 2048);
  writeStereo(runtime.memory, 2048, 0.3);
  assert.equal(runtime.imports.audio_push_f32_interleaved(1901, 2048), 704);
  assert.equal(runtime.imports.audio_get_queued_frames(), 4800);
  assert.equal(runtime.imports.audio_push_f32_interleaved(1901, 2048), 0);
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
    assert.equal(runtime.imports.audio_push_f32_interleaved(1901, 1), 1);
  }
  assert.equal(runtime.imports.audio_push_f32_interleaved(1901, 1), 0);
  assert.equal(runtime.imports.audio_get_queued_frames(), 32);

  await runtime.resume();
  assert.equal(runtime.starts.length, 32);
});

test("running PCM scheduling preserves full pushes and queue timing", async () => {
  const runtime = await createRuntime();
  runtime.imports.audio_init(48000, 2);
  await runtime.resume();
  writeStereo(runtime.memory, 6000, 0.5);

  assert.equal(runtime.imports.audio_push_f32_interleaved(1901, 6000), 6000);
  assert.equal(runtime.starts.length, 1);
  assert.equal(runtime.buffers[0].frames, 6000);
  assert.equal(runtime.imports.audio_get_queued_frames(), 6000);
});

test("ad imports delegate separately to gameplay and physical-audio lifecycle state", async () => {
  const runtime = await createRuntime({ portalProvider: "gamedistribution" });
  const api = runtime.imports;
  const callbacks = runtime.lifecycleCallbacks();
  assert.ok(callbacks, "selected portal initializes the host lifecycle manager");
  assert.equal(api.stasis_jit_ad_request(0), 70);
  assert.equal(api.stasis_jit_ad_request(1), 71);
  assert.equal(api.stasis_jit_ad_poll(71), 1);
  assert.equal(api.stasis_jit_ad_gameplay_blocked(), 0);
  assert.equal(api.stasis_jit_ad_take_reward(71), 1);
  api.stasis_jit_ad_release(71);
  api.stasis_jit_portal_lifecycle(1);

  await runtime.resume();
  const context = runtime.contexts.at(-1);
  assert.equal(context.state, "running");
  runtime.setAdBlocked(true);
  assert.equal(api.stasis_jit_ad_gameplay_blocked(), 1);
  assert.equal(context.state, "running", "Requesting blocks gameplay but does not suspend audio");
  runtime.setAdAudioPaused(true);
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(context.state, "suspended", "physical playback adds an independent audio reason");
});

test("ad and visibility reasons restore audio only after the last reason clears", async () => {
  const runtime = await createRuntime({ portalProvider: "poki" });
  await runtime.resume();
  const context = runtime.contexts.at(-1);
  assert.equal(context.state, "running");

  await runtime.visibility(true);
  runtime.setAdAudioPaused(true);
  await runtime.visibility(false);
  assert.equal(context.state, "suspended", "visibility recovery cannot resume during an ad");
  runtime.setAdAudioPaused(false);
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(context.state, "running");

  runtime.setAdAudioPaused(true);
  await runtime.visibility(true);
  runtime.setAdAudioPaused(false);
  assert.equal(context.state, "suspended", "ad recovery cannot resume while hidden");
  await runtime.visibility(false);
  assert.equal(context.state, "running");
});

test("pagehide and visibility suspension are independent lifecycle reasons", async () => {
  const runtime = await createRuntime();
  await runtime.resume();
  const context = runtime.contexts.at(-1);

  runtime.pagehide(true);
  await runtime.visibility(true);
  runtime.pageshow(true);
  assert.equal(context.state, "suspended", "pageshow cannot resume audio while visibility remains hidden");
  await runtime.visibility(false);
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(context.state, "running");

  await runtime.visibility(true);
  runtime.pagehide(true);
  await runtime.visibility(false);
  assert.equal(context.state, "suspended", "visibility recovery cannot resume audio while pagehide remains active");
  runtime.pageshow(true);
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(context.state, "running");
});

test("initially suspended audio stays suspended when the ad reason clears", async () => {
  const runtime = await createRuntime({ portalProvider: "gamedistribution" });
  assert.ok(runtime.lifecycleCallbacks(), "selected portal initializes lifecycle callbacks");
  assert.equal(runtime.contexts.at(-1).state, "suspended");
  runtime.allowAudioResume();
  runtime.setAdAudioPaused(true);
  runtime.setAdAudioPaused(false);
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(runtime.contexts.at(-1).state, "suspended", "ad end must preserve the initial suspended intent");
});

test("a real gesture retries resume while the pre-gesture AudioContext resume is still pending", async () => {
  const runtime = await createRuntime({ holdInitialAudioResume: true });
  assert.equal(runtime.pendingAudioResumeCount(), 1, "startup's no-gesture resume is pending");

  runtime.allowAudioResume();
  runtime.dispatchKey("keydown", { code: "KeyA", repeat: false });
  assert.equal(runtime.pendingAudioResumeCount(), 2, "the trusted gesture starts a fresh resume attempt");
  runtime.resolveHeldAudioResumes();
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));

  assert.equal(runtime.contexts.at(-1).state, "running");
});

test("an enableWebAudio promise resolving after ad start is suspended again", async () => {
  const runtime = await createRuntime({ portalProvider: "gamedistribution" });
  const context = runtime.contexts.at(-1);
  runtime.allowAudioResume();
  runtime.holdNextAudioResume();
  runtime.dispatchKey("pointerdown", {});
  runtime.setAdAudioPaused(true);
  runtime.resolveHeldAudioResumes();
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(context.state, "suspended", "late resume must not unmute beneath an active ad");
});

test("a pending trusted audio resume is restored after the ad without changing initial suspended intent", async () => {
  const runtime = await createRuntime({ portalProvider: "gamedistribution" });
  const context = runtime.contexts.at(-1);
  runtime.allowAudioResume();
  runtime.holdNextAudioResume();
  runtime.dispatchKey("pointerdown", {});
  runtime.setAdAudioPaused(true);
  runtime.resolveHeldAudioResumes();
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(context.state, "suspended", "the late gesture resume stays muted throughout playback");
  runtime.setAdAudioPaused(false);
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(context.state, "running", "the earlier user gesture intent resumes audio after playback");
});

test("a stale visibility resume resolving during a newer ad pause is suspended immediately", async () => {
  const runtime = await createRuntime({ portalProvider: "gamedistribution" });
  const context = runtime.contexts.at(-1);
  await runtime.resume();
  assert.equal(context.state, "running");

  runtime.holdNextAudioResume();
  await runtime.visibility(true);
  await runtime.visibility(false); // Resume A remains pending.
  await runtime.visibility(true);
  await runtime.visibility(false); // Resume B supersedes A.
  runtime.setAdAudioPaused(true);

  runtime.resolveAudioResume(0);
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(context.state, "suspended", "a stale resume cannot escape a newer physical ad pause");

  runtime.resolveHeldAudioResumes();
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(context.state, "suspended", "the current resume also honors the active ad reason");
  runtime.setAdAudioPaused(false);
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(context.state, "running", "the pre-pause running intent is restored after the ad");
});

test("the activation prompt calls the adapter synchronously on pointerup and keyboard users can cancel", async () => {
  const runtime = await createRuntime({ portalProvider: "gamedistribution" });
  let activatedWith;
  let canceled = false;
  runtime.lifecycleCallbacks().onGestureNeeded({
    visible: true,
    label: "Watch ad",
    message: "",
    activate(event) { activatedWith = event; return true; },
    cancel() { canceled = true; },
  });
  assert.equal(runtime.adControls.hidden, false);
  const event = { type: "pointerup", isTrusted: true, pointerType: "touch", eventPhase: 2 };
  runtime.dispatchAdWatch("pointerup", event);
  assert.equal(activatedWith.isTrusted, true, "pointerup is passed through before the handler returns");
  runtime.dispatchAdWatch("click", { detail: 0 });
  assert.match(runtime.adStatus.textContent, /mouse or touch release/i);
  runtime.dispatchAdCancel("click");
  assert.equal(runtime.adCancelListenerCount(), 1, "the Cancel action is registered exactly once");
  assert.equal(canceled, true, "the prompt exposes a keyboard-operable cancel button");
});

test("an audio-pruned portal build still handles adapter failure and disposes its manager", async () => {
  const failed = await createRuntime({
    portalProvider: "poki",
    audioEnabled: false,
    portalFactoryFailure: true,
  });
  assert.equal(failed.adStatus.textContent, "Ads are unavailable. You can keep playing.");
  assert.equal(failed.errorBox.textContent, "", "optional adapter failure does not fail game startup");
  assert.equal(failed.imports.stasis_jit_ad_request(0), 0);
  assert.equal(failed.imports.stasis_jit_ad_poll(1), 4);

  const working = await createRuntime({ portalProvider: "poki", audioEnabled: false });
  working.pagehide();
  assert.equal(working.disposeCount(), 1, "non-persisted pagehide disposes the manager without audio code");
});

test("blocked ad input neither accumulates nor replays held keys or pointers", async () => {
  const runtime = await createRuntime({ portalProvider: "gamedistribution" });
  runtime.setAdBlocked(true);
  runtime.dispatchKey("keydown", { code: "ArrowRight", repeat: false });
  runtime.dispatchCanvas("pointerdown", {
    pointerId: 1, pointerType: "mouse", clientX: 200, clientY: 200, isTrusted: true,
  });
  assert.equal(runtime.runFrame(16), true, "the host frame pump continues while gameplay is blocked");
  let input = runtime.tickInputs.at(-1);
  assert.deepEqual(input, { keyRight: 0, pointerCount: 0, pointerDown: 0 });

  runtime.setAdBlocked(false);
  runtime.runFrame(32);
  input = runtime.tickInputs.at(-1);
  assert.deepEqual(input, { keyRight: 0, pointerCount: 0, pointerDown: 0 }, "blocked events do not leak into the next tick");

  runtime.dispatchKey("keydown", { code: "ArrowRight", repeat: true });
  runtime.runFrame(40);
  assert.equal(runtime.tickInputs.at(-1).keyRight, 0, "a held key's repeat event does not replay after unblock");
  runtime.dispatchKey("keyup", { code: "ArrowRight" });
  runtime.dispatchKey("keydown", { code: "ArrowRight", repeat: false });
  runtime.runFrame(48);
  input = runtime.tickInputs.at(-1);
  assert.equal(input.keyRight, 1, "normal input resumes once the request is unblocked");
  runtime.setAdBlocked(true);
  runtime.setAdBlocked(false);
  runtime.runFrame(64);
  assert.equal(runtime.tickInputs.at(-1).keyRight, 0, "blocking clears already-held keys");
});
