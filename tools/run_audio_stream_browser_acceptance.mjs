// Run against a freshly packaged development samples/audio_stream_pcm bundle.
// Capture rendered WebAudio PCM, not the submitted AudioBuffer contents.
import assert from "node:assert/strict";
import { readFile, writeFile, mkdir, mkdtemp, rm } from "node:fs/promises";
import { createServer } from "node:http";
import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import path from "node:path";

const bundle = path.resolve(process.argv[2] || "samples/audio_stream_pcm/build/web");
const evidence = path.resolve(process.argv[3] || "target/audio-stream-browser");
await mkdir(evidence, { recursive: true });
const profile = await mkdtemp(path.join(evidence, "chrome-"));
const browserPath = process.env.STASIS_BROWSER_EXECUTABLE || "C:/Program Files/Google/Chrome/Application/chrome.exe";
const server = createServer(async (request, response) => {
  const file = new URL(request.url, "http://localhost").pathname;
  if (!["/", "/index.html", "/game.js", "/game.wasm"].includes(file)) {
    response.writeHead(404).end(); return;
  }
  try {
    response.setHeader("Content-Type", file.endsWith(".wasm") ? "application/wasm" : file.endsWith(".js") ? "text/javascript" : "text/html");
    response.end(await readFile(path.join(bundle, file === "/" ? "index.html" : file.slice(1))));
  } catch { response.writeHead(404).end(); }
});
await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
const browser = spawn(browserPath, ["--headless=new", "--no-sandbox", "--disable-gpu-sandbox", "--no-first-run", "--no-default-browser-check",
  "--autoplay-policy=document-user-activation-required", "--use-angle=swiftshader", "--enable-unsafe-swiftshader",
  "--remote-debugging-port=0", `--user-data-dir=${profile}`, "about:blank"], { stdio: "ignore" });
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
async function until(action) {
  const deadline = Date.now() + 15000;
  while (Date.now() < deadline) {
    try { const value = await action(); if (value) return value; } catch {}
    await delay(50);
  }
  throw new Error("browser acceptance condition timed out");
}
let socket;
try {
  const port = await until(async () => (await readFile(path.join(profile, "DevToolsActivePort"), "utf8")).split("\n")[0]);
  const pages = await fetch(`http://127.0.0.1:${port}/json/list`).then(r => r.json());
  socket = new WebSocket(pages.find(page => page.type === "page").webSocketDebuggerUrl);
  await new Promise(resolve => socket.addEventListener("open", resolve, { once: true }));
  let nextId = 0;
  const pending = new Map();
  socket.addEventListener("message", event => {
    const message = JSON.parse(event.data);
    const callback = pending.get(message.id);
    if (callback) { pending.delete(message.id); callback(message); }
  });
  const call = (method, params = {}) => new Promise((resolve, reject) => {
    const id = ++nextId;
    const timer = setTimeout(() => { pending.delete(id); reject(new Error(`${method} timed out`)); }, 15000);
    pending.set(id, message => { clearTimeout(timer); message.error ? reject(new Error(message.error.message)) : resolve(message.result); });
    socket.send(JSON.stringify({ id, method, params }));
  });
  const evaluate = async expression => {
    const result = await call("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true });
    if (result.exceptionDetails) throw new Error(JSON.stringify(result.exceptionDetails));
    return result.result.value;
  };
  await call("Page.enable");
  await call("Page.addScriptToEvaluateOnNewDocument", { source: `
    window.audioProof = { contexts: [], chunks: [], failures: [], required: [] };
    const originalInstantiate = WebAssembly.instantiate;
    WebAssembly.instantiate = async function(bytes, imports) {
      audioProof.required = WebAssembly.Module.imports(await WebAssembly.compile(bytes)).map(i => i.name);
      const result = await originalInstantiate(bytes, imports);
      audioProof.guest = result.instance.exports;
      audioProof.api = imports.env;
      return result;
    };
    const OriginalContext = AudioContext;
    window.AudioContext = class extends OriginalContext {
      constructor(...args) {
        if (audioProof.failDevice) throw new Error("injected device failure");
        super(...args);
        audioProof.contexts.push(this);
        const tap = this.createGain();
        const capture = this.createScriptProcessor(1024, 2, 2);
        tap.connect(capture);
        capture.connect(this.destination);
        capture.onaudioprocess = event => {
          if (audioProof.chunks.length < 100) audioProof.chunks.push([
            Array.from(event.inputBuffer.getChannelData(0)), Array.from(event.inputBuffer.getChannelData(1))]);
        };
        const originalSource = this.createBufferSource.bind(this);
        this.createBufferSource = () => {
          const source = originalSource();
          const connect = source.connect.bind(source);
          source.connect = (destination, ...rest) => {
            if (destination === this.destination) connect(tap);
            return connect(destination, ...rest);
          };
          return source;
        };
      }
    };
    addEventListener("error", event => audioProof.failures.push(event.message));
    addEventListener("unhandledrejection", event => audioProof.failures.push(String(event.reason)));
  ` });
  await call("Page.navigate", { url: `http://127.0.0.1:${server.address().port}/` });
  await until(() => evaluate("document.body.dataset.ready === 'true'"));
  assert.equal(await evaluate("document.body.dataset.mainResult"), "0");
  const required = ["init", "shutdown", "is_available", "get_sample_rate", "get_channels", "get_queued_frames", "get_underruns", "push_f32_interleaved",
    "play", "stop", "voice_is_playing", "voice_set_paused", "voice_set_volume_pan"].map(name => `stasis_jit_audio_${name}`);
  const imports = await evaluate("audioProof.required");
  for (const name of required) assert.ok(imports.includes(name), name);
  assert.equal(await evaluate("audioProof.contexts.at(-1).state"), "suspended");
  await call("Input.dispatchMouseEvent", { type: "mousePressed", x: 100, y: 100, button: "left", clickCount: 1 });
  await call("Input.dispatchMouseEvent", { type: "mouseReleased", x: 100, y: 100, button: "left", clickCount: 1 });
  await until(() => evaluate("audioProof.contexts.at(-1).state === 'running'"));
  const stages = [];
  async function capture(label, amplitude) {
    await delay(250); // Drain previously owned PCM before measuring new guest settings.
    await evaluate("audioProof.chunks = []");
    await until(() => evaluate("audioProof.chunks.length >= 12"));
    const chunks = await evaluate("audioProof.chunks");
    const rate = await evaluate("audioProof.contexts.at(-1).sampleRate");
    let peak = 0, nonzero = 0, ratioError = 0, crossings = 0, previous = 0;
    const pcm = [];
    for (const [left, right] of chunks) for (let i = 0; i < left.length; i++) {
      pcm.push(left[i], right[i]);
      peak = Math.max(peak, Math.abs(left[i]));
      ratioError = Math.max(ratioError, Math.abs(right[i] - left[i] * 0.5));
      if (Math.abs(left[i]) > 0.01) {
        nonzero++;
        if (previous * left[i] < 0) crossings++;
        previous = left[i];
      }
    }
    assert.ok(ratioError < 0.00001, `${label}: stereo ratio ${ratioError}`);
    if (amplitude === 0) assert.equal(peak, 0, label);
    else {
      assert.ok(nonzero > 4096, `${label}: no sustained rendered PCM`);
      assert.ok(Math.abs(peak - amplitude) < 0.08, `${label}: amplitude ${peak}`);
      assert.ok(Math.abs(crossings - nonzero * 960 / rate) < 4, `${label}: incorrect 480 Hz period`);
    }
    const bytes = Buffer.from(new Float32Array(pcm).buffer);
    await writeFile(path.join(evidence, `${label}.f32le`), bytes);
    stages.push({ label, frames: pcm.length / 2, peak, nonzero, ratioError, crossings,
      sha256: createHash("sha256").update(bytes).digest("hex") });
  }
  await capture("gesture", 0.5);
  if (!process.argv.includes("--startup-only")) {
    await evaluate("audioProof.guest.volume.value = 0.25");
    await capture("volume", 0.25);
    await evaluate("audioProof.guest.muted.value = 1");
    await capture("mute", 0);
    await evaluate("audioProof.guest.muted.value = 0; audioProof.guest.on_code_swap()");
    await capture("swap-reopen", 0.25);
    await evaluate("dispatchEvent(new PageTransitionEvent('pagehide', { persisted: true }))");
    await until(() => evaluate("audioProof.contexts.at(-1).state === 'suspended'"));
    await delay(250);
    assert.ok(await evaluate("audioProof.api.stasis_jit_audio_get_queued_frames() <= 4800"));
    await evaluate("dispatchEvent(new PageTransitionEvent('pageshow', { persisted: true }))");
    await capture("resume", 0.25);
    await evaluate("audioProof.api.stasis_jit_audio_shutdown()");
    assert.equal(await evaluate("audioProof.api.stasis_jit_audio_is_available()"), 0);
    assert.equal(await evaluate("audioProof.api.stasis_jit_audio_get_queued_frames()"), 0);
    await evaluate("audioProof.failDevice = true");
    assert.equal(await evaluate("audioProof.api.stasis_jit_audio_init(48000, 2, 1024)"), 0);
    assert.equal(await evaluate("audioProof.api.stasis_jit_audio_push_f32_interleaved(0, 1)"), 0);
    await evaluate("audioProof.failDevice = false; audioProof.guest.on_code_swap()");
    await capture("device-recovery", 0.25);
  }
  assert.deepEqual(await evaluate("audioProof.failures"), []);
  const sampleRate = await evaluate("audioProof.contexts.at(-1).sampleRate");
  await call("Page.reload");
  await until(() => evaluate("document.body.dataset.ready === 'true' && document.body.dataset.mainResult === '0'"));
  await call("Input.dispatchMouseEvent", { type: "mousePressed", x: 100, y: 100, button: "left", clickCount: 1 });
  await call("Input.dispatchMouseEvent", { type: "mouseReleased", x: 100, y: 100, button: "left", clickCount: 1 });
  await capture("reload", 0.5);
  assert.deepEqual(await evaluate("audioProof.failures"), []);
  const hashes = {};
  for (const file of ["game.js", "game.wasm", "stasis_provenance.json"]) {
    hashes[file] = createHash("sha256").update(await readFile(path.join(bundle, file))).digest("hex");
  }
  const receipt = { browser: await call("Browser.getVersion"), imports: required, hashes, sampleRate, stages,
    capture: "Rendered AudioContext graph via ScriptProcessor tap; stereo float32 little-endian at device sample rate", physicalListening: false };
  await writeFile(path.join(evidence, "receipt.json"), JSON.stringify(receipt, null, 2));
  console.log(JSON.stringify(receipt, null, 2));
} finally {
  socket?.close();
  if (browser.exitCode === null) {
    const exited = new Promise(resolve => browser.once("exit", resolve));
    browser.kill();
    await Promise.race([exited, delay(2000)]);
  }
  server.close();
  if (profile.startsWith(evidence + path.sep)) {
    await rm(profile, { recursive: true, force: true, maxRetries: 2 }).catch(() => {});
  }
}

