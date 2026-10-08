// Run a freshly packaged web-ad diagnostic bundle through Chrome's real DOM,
// WebAssembly, input, SDK-loader, and Web Audio paths. Provider scripts are
// intercepted at their documented URL and answered with test-only fakes.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { spawn } from "node:child_process";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import path from "node:path";

const providers = new Set(["none", "crazygames", "gamemonetize", "gamedistribution", "poki"]);
const argv = process.argv.slice(2);
const bundle = path.resolve(argv[0] || "samples/web_ad_lifecycle/build/web");
const evidenceRoot = path.resolve(argv[1] || "target/web-ad-browser-acceptance");
const provider = argv[2] || "";
const withAudio = argv.includes("--audio");
const pointerKind = argv.includes("--pointer=touch") ? "touch" : "mouse";
const sdkFailure = argv.includes("--sdk-failure");
const moduleFailure = argv.includes("--module-failure");
assert.ok(providers.has(provider), "usage: node tools/run_web_ad_browser_acceptance.mjs <bundle> <evidence-dir> <profile> [--audio] [--pointer=touch] [--sdk-failure] [--module-failure]");
assert.ok(!(sdkFailure && moduleFailure), "choose either --sdk-failure or --module-failure");
await mkdir(evidenceRoot, { recursive: true });
const runRoot = await mkdtemp(path.join(evidenceRoot, `${provider}-${withAudio ? "audio" : "noaudio"}-${pointerKind}-`));
const profileDir = await mkdtemp(path.join(runRoot, "chrome-"));
const browserPath = process.env.STASIS_BROWSER_EXECUTABLE || "C:/Program Files/Google/Chrome/Application/chrome.exe";
const ffmpegPath = process.env.STASIS_FFMPEG_EXECUTABLE || "D:/code/ffmpeg-9.0.1-essentials_build/ffmpeg-9.0.1-essentials_build/bin/ffmpeg.exe";
const sdkUrls = {
  crazygames: "https://sdk.crazygames.com/crazygames-sdk-v3.js",
  gamemonetize: "https://api.gamemonetize.com/sdk.js",
  gamedistribution: "https://html5.api.gamedistribution.com/main.min.js",
  poki: "https://game-cdn.poki.com/scripts/v2/poki-sdk.js",
};
const urlProviders = new Map(Object.entries(sdkUrls).map(([name, url]) => [`${new URL(url).origin}${new URL(url).pathname}`, name]));
const providerForUrl = value => {
  try {
    const parsed = new URL(value);
    return urlProviders.get(`${parsed.origin}${parsed.pathname}`);
  } catch { return undefined; }
};
async function encodeScreencast(runDirectory, frames) {
  assert.ok(frames.length >= 2, `expected a real Chrome screencast, got ${frames.length} frames`);
  const firstTime = Number.isFinite(frames[0].timestamp) ? frames[0].timestamp : frames[0].capturedAtMs / 1000;
  const normalizedTimes = frames.map(frame => {
    const time = Number.isFinite(frame.timestamp) ? frame.timestamp : frame.capturedAtMs / 1000;
    return Math.max(0, time - firstTime);
  });
  const concatLines = ["ffconcat version 1.0"];
  for (let index = 0; index < frames.length; index += 1) {
    const name = `capture-${String(index + 1).padStart(5, "0")}.jpg`;
    await writeFile(path.join(runDirectory, name), Buffer.from(frames[index].data, "base64"));
    const nextDelta = index + 1 < frames.length ? normalizedTimes[index + 1] - normalizedTimes[index] : 0.1;
    const duration = nextDelta > 0 ? nextDelta : 0.001;
    concatLines.push(`file '${name}'`, `duration ${duration.toFixed(6)}`);
  }
  const lastName = `capture-${String(frames.length).padStart(5, "0")}.jpg`;
  concatLines.push(`file '${lastName}'`);
  const concatPath = path.join(runDirectory, "capture.ffconcat");
  const videoPath = path.join(runDirectory, "browser.mp4");
  await writeFile(concatPath, `${concatLines.join("\n")}\n`);
  const result = await new Promise((resolve, reject) => {
    const child = spawn(ffmpegPath, [
      "-y", "-f", "concat", "-safe", "0", "-i", concatPath,
      "-vf", "pad=ceil(iw/2)*2:ceil(ih/2)*2", "-fps_mode", "vfr",
      "-c:v", "libx264", "-pix_fmt", "yuv420p", "-movflags", "+faststart", videoPath,
    ], { cwd: runDirectory, stdio: ["ignore", "ignore", "pipe"] });
    let stderr = "";
    child.stderr.on("data", chunk => { stderr += chunk; });
    child.once("error", reject);
    child.once("exit", code => resolve({ code, stderr }));
  });
  assert.equal(result.code, 0, `ffmpeg failed to encode the real screencast: ${result.stderr}`);
  return { frameCount: frames.length, durationSeconds: normalizedTimes.at(-1) || 0, video: videoPath };
}
const server = createServer(async (request, response) => {
  try {
    const requestPath = decodeURIComponent(new URL(request.url, "http://localhost").pathname);
    const relativePath = requestPath === "/" ? "index.html" : requestPath.replace(/^\/+/, "");
    const file = path.resolve(bundle, relativePath);
    const relative = path.relative(bundle, file);
    if (relative.startsWith("..") || path.isAbsolute(relative)) {
      response.writeHead(403).end();
      return;
    }
    if (moduleFailure && relative === "ad_lifecycle.js") {
      moduleRequests.push({ path: relative, status: 404 });
      response.writeHead(404, { "Content-Type": "text/plain; charset=utf-8", "Cache-Control": "no-store" }).end("intercepted missing ad lifecycle module");
      return;
    }
    const bytes = await readFile(file);
    const contentType = file.endsWith(".html") ? "text/html; charset=utf-8"
      : file.endsWith(".wasm") ? "application/wasm"
      : file.endsWith(".js") ? "text/javascript; charset=utf-8"
        : file.endsWith(".json") ? "application/json; charset=utf-8"
          : file.endsWith(".woff2") ? "font/woff2" : "application/octet-stream";
    response.writeHead(200, { "Content-Type": contentType, "Cache-Control": "no-store" });
    response.end(bytes);
  } catch {
    response.writeHead(404).end();
  }
});
await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));

const browser = spawn(browserPath, [
  "--headless=new", "--no-sandbox", "--disable-gpu-sandbox", "--no-first-run",
  "--no-default-browser-check", "--use-angle=swiftshader", "--enable-unsafe-swiftshader",
  "--autoplay-policy=document-user-activation-required", "--window-size=1024,768",
  "--remote-debugging-port=0", `--user-data-dir=${profileDir}`, "about:blank",
], { stdio: "ignore" });
const delay = milliseconds => new Promise(resolve => setTimeout(resolve, milliseconds));
async function until(action, message = "browser acceptance condition timed out", timeout = 20_000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    try {
      const value = await action();
      if (value) return value;
    } catch { /* runtime may not be ready yet */ }
    await delay(50);
  }
  throw new Error(message);
}

let socket;
let call;
const cdpFailures = [];
const expectedModuleErrors = [];
const expectedSdkErrors = [];
const interceptedScripts = [];
const moduleRequests = [];
const screencastFrames = [];
let screencastStarted = false;
try {
  const port = await until(async () => (await readFile(path.join(profileDir, "DevToolsActivePort"), "utf8")).split("\n")[0], "Chrome DevTools endpoint did not start");
  const pages = await fetch(`http://127.0.0.1:${port}/json/list`).then(response => response.json());
  socket = new WebSocket(pages.find(page => page.type === "page").webSocketDebuggerUrl);
  await new Promise(resolve => socket.addEventListener("open", resolve, { once: true }));
  let nextId = 0;
  const pending = new Map();
  call = (method, params = {}) => new Promise((resolve, reject) => {
    const id = ++nextId;
    const timer = setTimeout(() => { pending.delete(id); reject(new Error(`${method} timed out`)); }, 20_000);
    pending.set(id, message => {
      clearTimeout(timer);
      message.error ? reject(new Error(message.error.message)) : resolve(message.result);
    });
    socket.send(JSON.stringify({ id, method, params }));
  });
  const evaluate = async expression => {
    const result = await call("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true });
    if (result.exceptionDetails) throw new Error(result.exceptionDetails.exception?.description || result.exceptionDetails.text);
    return result.result.value;
  };
  const fakeSdk = selected => {
    const prelude = `(() => {
      const proof = window.__webAdProof;
      const finishList = [];
      proof.sdkEvaluations.push({ provider: ${JSON.stringify(selected)}, readyOptions: null });
      proof.fake.configureNextBreak = mode => { proof.fake.nextBreakMode = mode || {}; };
      const takeBreakMode = () => {
        const mode = proof.fake.nextBreakMode || {};
        proof.fake.nextBreakMode = null;
        return mode;
      };
      const addBreak = (kind, finish) => {
        proof.sdkBreaks.push({ provider: ${JSON.stringify(selected)}, kind });
        finishList.push(finish);
      };
      proof.fake.finish = () => {
        const finish = finishList.shift();
        if (!finish) throw new Error("no pending fake ad break");
        finish();
      };
      ${selected === "crazygames" ? `
        const game = {
          loadingStart: () => proof.lifecycle.push("loadingStart"),
          loadingStop: () => proof.lifecycle.push("loadingStop"),
          gameplayStart: () => proof.lifecycle.push("gameplayStart"),
          gameplayStop: () => proof.lifecycle.push("gameplayStop"),
        };
        const ad = { requestAd(kind, callbacks) {
          const mode = takeBreakMode();
          const finish = () => mode.noFill ? callbacks.adError({ errorCode: "unfilled" }) : callbacks.adFinished();
          addBreak(mode.noFill ? kind + "-no-fill" : kind, finish);
          if (mode.noFill) setTimeout(finish, 100);
          else setTimeout(() => callbacks.adStarted(), 750);
        }};
        window.CrazyGames = { SDK: { game, ad, init: () => Promise.resolve() } };
      ` : ""}
      ${selected === "poki" ? `
        const makeBreak = (kind, start) => {
          const mode = takeBreakMode();
          proof.pokiEvents.push({ type: "break-call", kind, noFill: mode.noFill === true });
          return new Promise((resolve, reject) => {
            addBreak(mode.noFill ? kind + "-no-fill" : kind, () => {
              if (mode.reject) reject(new Error("intercepted test rejection"));
              else resolve(mode.noFill ? undefined : kind === "rewarded");
            });
            if (mode.noFill) setTimeout(() => resolve(undefined), 100);
            else setTimeout(() => {
              proof.pokiEvents.push({ type: "break-start-callback", kind });
              start();
            }, 750);
          });
        };
        window.PokiSDK = {
          init: () => Promise.resolve(),
          gameLoadingFinished: () => proof.lifecycle.push("gameLoadingFinished"),
          gameplayStart: () => proof.lifecycle.push("gameplayStart"),
          gameplayStop: () => proof.lifecycle.push("gameplayStop"),
          commercialBreak: start => makeBreak("commercial", start),
          rewardedBreak: start => makeBreak("rewarded", start),
        };
      ` : ""}
      ${selected === "gamemonetize" ? `
        const options = window.SDK_OPTIONS;
        proof.sdkEvaluations.at(-1).readyOptions = {
          gameId: options?.gameId || "",
          hasHandler: typeof options?.onEvent === "function",
        };
        window.sdk = { showBanner() {
          const mode = takeBreakMode();
          const emit = name => {
            proof.sdkEvents.push({ provider: "gamemonetize", name });
            options.onEvent({ name });
          };
          addBreak(mode.noStart ? "showBanner-no-start-timeout" : "showBanner", () => emit("SDK_GAME_START"));
          if (!mode.noStart) setTimeout(() => emit("SDK_GAME_PAUSE"), 100);
        }};
        queueMicrotask(() => {
          proof.sdkEvents.push({ provider: "gamemonetize", name: "SDK_READY" });
          options?.onEvent?.({ name: "SDK_READY" });
        });
      ` : ""}
      ${selected === "gamedistribution" ? `
        const options = window.GD_OPTIONS;
        proof.sdkEvaluations.at(-1).readyOptions = {
          gameId: options?.gameId || "",
          hasHandler: typeof options?.onEvent === "function",
        };
        const emit = name => {
          proof.sdkEvents.push({ provider: "gamedistribution", name });
          options.onEvent({ name });
        };
        window.gdsdk = { showAd(requestKind) {
          const mode = takeBreakMode();
          const activation = proof.watchPointerUps.at(-1) || null;
          proof.showAdCalls.push({
            requestKind: requestKind || "midgame",
            activation,
            userActivation: navigator.userActivation?.isActive === true,
          });
          let resolvePromise;
          let rejectPromise;
          const promise = new Promise((resolve, reject) => { resolvePromise = resolve; rejectPromise = reject; });
          const finish = () => {
            if (mode.rewardProof) emit("SDK_REWARDED_WATCH_COMPLETE");
            if (!mode.noFill) emit("SDK_GAME_START");
            if (mode.reject) rejectPromise(new Error("intercepted test rejection"));
            else resolvePromise();
          };
          addBreak(mode.noFill ? (requestKind || "midgame") + "-no-fill" : requestKind || "midgame", finish);
          if (mode.noFill) setTimeout(finish, 100);
          else if (!mode.noStart) setTimeout(() => emit("SDK_GAME_PAUSE"), 100);
          return promise;
        }};
        queueMicrotask(() => {
          proof.sdkEvents.push({ provider: "gamedistribution", name: "SDK_READY" });
          options?.onEvent?.({ name: "SDK_READY" });
        });
      ` : ""}
    })();`;
    return prelude;
  };

  socket.addEventListener("message", event => {
    const message = JSON.parse(event.data);
    if (message.method === "Runtime.exceptionThrown") cdpFailures.push(message.params.exceptionDetails.text);
    if (message.method === "Runtime.consoleAPICalled" && ["error", "assert"].includes(message.params.type)) {
      const detail = message.params.args.map(argument => argument.value || argument.description || "").join(" ");
      if (moduleFailure && /ad_lifecycle\.js.*404|404.*ad_lifecycle\.js|failed to load module script/i.test(detail)) expectedModuleErrors.push(detail);
      else if (sdkFailure && /ERR_BLOCKED_BY_CLIENT/i.test(detail)) expectedSdkErrors.push(detail);
      else cdpFailures.push(detail);
    }
    if (message.method === "Page.screencastFrame") {
      const frame = message.params;
      screencastFrames.push({
        data: frame.data,
        timestamp: Number.isFinite(frame.metadata?.timestamp) ? frame.metadata.timestamp : null,
        capturedAtMs: performance.now(),
      });
      void call("Page.screencastFrameAck", { sessionId: frame.sessionId }).catch(error => cdpFailures.push(String(error)));
    }
    if (message.method === "Fetch.requestPaused") {
      const paused = message.params;
      const selectedProvider = providerForUrl(paused.request.url);
      void (async () => {
        try {
          if (selectedProvider !== provider) {
            interceptedScripts.push({ provider: selectedProvider || "unknown", url: paused.request.url, unexpected: true });
            await call("Fetch.failRequest", { requestId: paused.requestId, errorReason: "BlockedByClient" });
            return;
          }
          if (sdkFailure) {
            interceptedScripts.push({ provider: selectedProvider, url: paused.request.url, unexpected: false, failed: true });
            await call("Fetch.failRequest", { requestId: paused.requestId, errorReason: "BlockedByClient" });
            return;
          }
          interceptedScripts.push({ provider: selectedProvider, url: paused.request.url, unexpected: false });
          const source = fakeSdk(selectedProvider);
          new Function(source);
          await call("Fetch.fulfillRequest", {
            requestId: paused.requestId,
            responseCode: 200,
            responseHeaders: [
              { name: "Content-Type", value: "application/javascript; charset=utf-8" },
              { name: "Access-Control-Allow-Origin", value: "*" },
              { name: "Cache-Control", value: "no-store" },
            ],
            body: Buffer.from(source).toString("base64"),
          });
        } catch (error) { cdpFailures.push(String(error)); }
      })();
    }
    const callback = pending.get(message.id);
    if (callback) { pending.delete(message.id); callback(message); }
  });

  await call("Page.enable");
  await call("Runtime.enable");
  await call("Network.enable");
  if (pointerKind === "touch") {
    await call("Emulation.setDeviceMetricsOverride", { width: 390, height: 844, deviceScaleFactor: 1, mobile: true });
    await call("Emulation.setTouchEmulationEnabled", { enabled: true, maxTouchPoints: 1 });
  }
  const patterns = Object.values(sdkUrls).map(url => ({ urlPattern: `${url}*`, requestStage: "Request" }));
  await call("Fetch.enable", { patterns });
  await call("Page.addScriptToEvaluateOnNewDocument", { source: `
    window.__webAdProof = {
      failures: [], imports: [], guest: null, sdkEvaluations: [], sdkBreaks: [],
      lifecycle: [], sdkEvents: [], pokiEvents: [], showAdCalls: [], watchPointerUps: [], importCalls: {}, requestKinds: [], adRequestCalls: [], fake: {},
      preloadedSdkGlobals: {
        CrazyGames: typeof window.CrazyGames !== "undefined",
        PokiSDK: typeof window.PokiSDK !== "undefined",
        sdk: typeof window.sdk !== "undefined",
        gdsdk: typeof window.gdsdk !== "undefined",
        SDK_OPTIONS: typeof window.SDK_OPTIONS !== "undefined",
        GD_OPTIONS: typeof window.GD_OPTIONS !== "undefined",
      },
      audio: { contexts: [], stateEvents: [], resumeCalls: [], resumeErrors: [], resumeSettlements: [] }, gestureEvents: [],
    };
    const proof = window.__webAdProof;
    for (const type of ["pointerdown", "pointerup", "click", "keydown"]) {
      document.addEventListener(type, event => proof.gestureEvents.push({
        type, trusted: event.isTrusted, target: event.target?.id || event.target?.tagName || null,
        code: event.code || null, key: event.key || null, repeat: event.repeat === true,
        pointerType: event.pointerType || null, activation: navigator.userActivation?.isActive === true,
        audio: proof.audio.contexts.at(-1)?.state || "none",
      }), true);
    }
    window.addEventListener("pointerdown", event => proof.gestureEvents.push({
      type: "window-pointerdown", trusted: event.isTrusted, bubbles: event.bubbles,
      activation: navigator.userActivation?.isActive === true,
      audio: proof.audio.contexts.at(-1)?.state || "none",
    }));
    const OriginalContext = globalThis.AudioContext;
    if (OriginalContext) {
      globalThis.AudioContext = class extends OriginalContext {
        constructor(...args) {
          super(...args);
          proof.audio.contexts.push(this);
          this.addEventListener("statechange", () => proof.audio.stateEvents.push(this.state));
        }
        resume(...args) {
          proof.audio.resumeCalls.push({ state: this.state, activation: navigator.userActivation?.isActive === true });
          return super.resume(...args).then(value => {
            proof.audio.resumeSettlements.push(this.state);
            return value;
          }, error => {
            proof.audio.resumeErrors.push(String(error));
            throw error;
          });
        }
      };
    }
    document.addEventListener("pointerup", event => {
      if (!event.target?.closest?.("#stasis-ad-watch")) return;
      proof.watchPointerUps.push({
        trusted: event.isTrusted,
        type: event.type,
        pointerType: event.pointerType,
        primary: event.isPrimary,
        button: event.button,
        eventPhase: event.eventPhase,
        hasCurrentTarget: Boolean(event.currentTarget),
        activation: navigator.userActivation?.isActive === true,
      });
    }, true);
    const originalInstantiate = WebAssembly.instantiate;
    WebAssembly.instantiate = async function(bytes, imports) {
      const module = await WebAssembly.compile(bytes);
      proof.imports = WebAssembly.Module.imports(module).map(record => record.name);
      const api = imports?.env;
      for (const name of ["stasis_jit_ad_request", "stasis_jit_ad_poll", "stasis_jit_ad_gameplay_blocked", "stasis_jit_ad_take_reward", "stasis_jit_ad_release", "stasis_jit_portal_lifecycle"]) {
        if (typeof api?.[name] !== "function") continue;
        const original = api[name];
        api[name] = (...args) => {
          proof.importCalls[name] = (proof.importCalls[name] || 0) + 1;
          if (name === "stasis_jit_ad_request") proof.requestKinds.push(args[0] | 0);
          const result = original(...args);
          if (name === "stasis_jit_ad_request") proof.adRequestCalls.push({ kind: args[0] | 0, handle: result | 0 });
          return result;
        };
      }
      const result = await originalInstantiate(bytes, imports);
      proof.guest = (result.instance || result).exports;
      proof.readGlobal = name => {
        let hash = 2166136261;
        for (const byte of new TextEncoder().encode(name)) hash = Math.imul(hash ^ byte, 16777619) >>> 0;
        return proof.guest.__stasis_global_get_i32(hash | 0);
      };
      return result;
    };
    addEventListener("error", event => proof.failures.push(event.message));
    addEventListener("unhandledrejection", event => proof.failures.push(String(event.reason)));
  ` });
  await call("Page.navigate", { url: `http://127.0.0.1:${server.address().port}/` });
  await until(() => evaluate("document.body.dataset.ready === 'true'"), "the packaged Wasm game did not become ready");
  assert.equal(await evaluate("document.body.dataset.mainResult"), "0");
  assert.equal(await evaluate("STASIS_GAME.portal.provider"), provider);
  assert.ok(await evaluate("Object.values(__webAdProof.preloadedSdkGlobals).every(value => value === false)"), "the test preloaded an SDK global before runtime startup");
  assert.ok(await evaluate("typeof __webAdProof.readGlobal === 'function'"), "guest global reader was not installed");
  const requiredImports = [
    "stasis_jit_ad_request", "stasis_jit_ad_poll", "stasis_jit_ad_gameplay_blocked",
    "stasis_jit_ad_take_reward", "stasis_jit_ad_release", "stasis_jit_portal_lifecycle",
  ];
  const imports = await evaluate("__webAdProof.imports");
  for (const name of requiredImports) assert.ok(imports.includes(name), `missing Wasm import ${name}`);
  await call("Page.startScreencast", { format: "jpeg", quality: 72, maxWidth: 1024, maxHeight: 768, everyNthFrame: 1 });
  screencastStarted = true;

  const screenshot = async label => {
    const result = await call("Page.captureScreenshot", { format: "png", fromSurface: true });
    const bytes = Buffer.from(result.data, "base64");
    assert.ok(bytes.length > 1_000, `${label}: screenshot is empty`);
    const destination = path.join(runRoot, `frame-${String(frameNumber++).padStart(2, "0")}.png`);
    await writeFile(destination, bytes);
    frames.push({ label, file: path.basename(destination), bytes: bytes.length });
  };
  let frameNumber = 1;
  const frames = [];
  const readCounters = () => evaluate(`(() => {
    const read = __webAdProof.readGlobal;
    return {
      state: read("browser_ad_state"), terminal: read("browser_ad_last_terminal_state"),
      handle: read("browser_ad_handle"), simulation: read("browser_ad_simulation_steps"),
      polls: read("browser_ad_poll_ticks"), renders: read("browser_ad_render_count"),
      rewards: read("browser_ad_rewards_granted"), secondTakeZero: read("browser_ad_second_take_zero"),
    };
  })()`);
  const takeSnapshot = async () => ({ ...(await readCounters()), hostBlocked: await evaluate("document.body.dataset.adGameplayBlocked === 'true'") });
  const waitCounters = (predicate, label, timeout = 10_000) => until(async () => {
    const snapshot = await takeSnapshot();
    return predicate(snapshot) ? snapshot : null;
  }, label, timeout);
  const canvasPoint = async () => evaluate(`(() => {
    const rect = document.querySelector("canvas").getBoundingClientRect();
    return { x: Math.round(rect.left + rect.width / 2), y: Math.round(rect.top + rect.height / 2) };
  })()`);
  const tap = async ({ x, y }) => {
    if (pointerKind === "touch") {
      await call("Input.dispatchTouchEvent", { type: "touchStart", touchPoints: [{ x, y, id: 1, radiusX: 2, radiusY: 2, force: 1 }] });
      await delay(90);
      await call("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] });
      return;
    }
    await call("Input.dispatchMouseEvent", { type: "mouseMoved", x, y });
    await call("Input.dispatchMouseEvent", { type: "mousePressed", x, y, button: "left", buttons: 1, clickCount: 1 });
    await delay(90);
    await call("Input.dispatchMouseEvent", { type: "mouseReleased", x, y, button: "left", buttons: 0, clickCount: 1 });
  };
  const pressKey = async (code, key) => {
    const virtualKeyCode = key === "Enter" ? 13 : key.toUpperCase().charCodeAt(0);
    await call("Input.dispatchKeyEvent", { type: "keyDown", code, key, windowsVirtualKeyCode: virtualKeyCode });
    await delay(80);
    await call("Input.dispatchKeyEvent", { type: "keyUp", code, key, windowsVirtualKeyCode: virtualKeyCode });
  };
  const activateGameDistributionRequest = async expectedCallCount => {
    assert.equal(await evaluate("document.getElementById('stasis-ad-controls').hidden"), false, "GameDistribution did not show its activation control");
    await evaluate("document.getElementById('stasis-ad-watch').focus()");
    await pressKey("Enter", "Enter");
    assert.equal(await evaluate("__webAdProof.showAdCalls.length"), expectedCallCount - 1, "keyboard activation issued an unsupported GameDistribution call");
    const watchRect = await evaluate(`(() => { const r = document.getElementById("stasis-ad-watch").getBoundingClientRect(); return { x: Math.round(r.left + r.width / 2), y: Math.round(r.top + r.height / 2) }; })()`);
    await tap(watchRect);
    await until(() => evaluate(`__webAdProof.showAdCalls.length === ${expectedCallCount}`), "trusted Watch ad release did not call GameDistribution showAd");
    const activation = await evaluate(`__webAdProof.showAdCalls[${expectedCallCount - 1}]`);
    assert.equal(activation.userActivation, true, "GameDistribution showAd lacked active browser user activation");
    assert.ok(activation.activation?.trusted, "GameDistribution showAd was not issued from a trusted pointer release");
    assert.equal(activation.activation?.type, "pointerup");
    assert.ok(activation.activation?.hasCurrentTarget && activation.activation?.eventPhase > 0);
    assert.equal(activation.activation?.pointerType, pointerKind);
    assert.equal(activation.activation?.activation, true);
    return activation;
  };
  await screenshot("loaded");
  const initial = await readCounters();
  await delay(450);
  const idleLater = await readCounters();

  const requestsBeforeInput = await evaluate("__webAdProof.requestKinds.length");
  const audioBeforeGesture = await evaluate("__webAdProof.audio.contexts.at(-1)?.state || 'none'");
  const point = await canvasPoint();
  await tap(point);
  let audioAfterGesture = await evaluate("__webAdProof.audio.contexts.at(-1)?.state || 'none'");
  if (withAudio) {
    try {
      await until(() => evaluate("__webAdProof.audio.contexts.at(-1)?.state === 'running'"), "real pointer gesture did not resume the fixture AudioContext", 2_000);
    } catch (error) {
      const diagnostic = await evaluate(`({
        contexts: __webAdProof.audio.contexts.map(context => context.state),
        stateEvents: __webAdProof.audio.stateEvents,
        resumeCalls: __webAdProof.audio.resumeCalls,
        resumeErrors: __webAdProof.audio.resumeErrors,
        resumeSettlements: __webAdProof.audio.resumeSettlements,
        gestureEvents: __webAdProof.gestureEvents,
        requestKinds: __webAdProof.requestKinds,
        sdkScripts: __webAdProof.sdkEvaluations,
        failures: __webAdProof.failures,
        ready: document.body.dataset.ready || null,
        gpuError: document.body.dataset.gpuError || null,
        audioStateDataset: document.body.dataset.audioState || null,
        userActivation: navigator.userActivation?.isActive === true,
        activeElement: document.activeElement?.id || document.activeElement?.tagName || null,
      })`);
      await writeFile(path.join(runRoot, "audio-gesture-failure.json"), JSON.stringify(diagnostic, null, 2));
      throw new Error(`${error.message}; diagnostic: ${JSON.stringify(diagnostic)}`);
    }
    audioAfterGesture = await evaluate("__webAdProof.audio.contexts.at(-1)?.state || 'none'");
  }
  let snapshots = [];
  let adScriptCount = interceptedScripts.length;
  let manualAudioPause = null;
  if (moduleFailure) {
    await until(() => moduleRequests.length > 0, "dynamic ad lifecycle module request was not intercepted");
    const unavailable = await waitCounters(value => value.state === 5 || value.terminal === 5,
      "module-load failure did not resolve the guest request as unavailable");
    assert.equal(await evaluate("document.body.dataset.mainResult"), "0", "game main did not return successfully after adapter import failure");
    assert.equal(await evaluate("document.body.dataset.ready"), "true", "game did not reach ready after adapter import failure");
    assert.equal(moduleRequests.length, 1);
    assert.deepEqual(moduleRequests[0], { path: "ad_lifecycle.js", status: 404 });
    assert.equal(await evaluate("document.body.dataset.adStatus || ''"), "Ads are unavailable. You can keep playing.");
    assert.equal(interceptedScripts.length, 0, "module-load failure attempted to load an SDK script");
    assert.equal(await evaluate("__webAdProof.sdkEvaluations.length"), 0);
    assert.equal(await evaluate("__webAdProof.sdkBreaks.length"), 0);
    assert.equal(unavailable.handle, 0, "module-load failure left an ad handle outstanding");
    assert.equal(unavailable.state, 5, "module-load failure did not expose the unavailable guest state");
    assert.equal(await evaluate("document.body.dataset.adGameplayBlocked === 'true'"), false);
    assert.ok(unavailable.simulation > initial.simulation, "module-load failure stopped game simulation");
    assert.ok(unavailable.renders > idleLater.renders, "module-load failure stopped rendering");
    await screenshot("module-unavailable-playable");
    snapshots.push({ label: "module-unavailable-playable", counters: unavailable });
  } else if (sdkFailure) {
    await until(() => interceptedScripts.some(item => item.failed), "selected SDK request was not intercepted as a failure");
    await delay(100);
    const unavailable = await readCounters();
    assert.equal(interceptedScripts.length, 1);
    assert.ok(interceptedScripts.every(item => item.provider === provider && item.failed && !item.unexpected), "SDK failure run intercepted an unexpected script");
    assert.equal(await evaluate("__webAdProof.sdkEvaluations.length"), 0, "failed SDK script executed fake adapter code");
    assert.equal(unavailable.handle, 0, "SDK load failure left a guest handle outstanding");
    assert.ok(unavailable.state === 5 || unavailable.terminal === 5, "SDK load failure did not resolve as unavailable");
    assert.equal(await evaluate("document.body.dataset.adGameplayBlocked === 'true'"), false);
    assert.ok(unavailable.simulation > initial.simulation, "SDK load failure stopped game simulation");
    assert.ok(unavailable.renders > idleLater.renders, "SDK load failure stopped rendering");
    assert.equal(await evaluate("__webAdProof.sdkBreaks.length"), 0);
    if (withAudio) assert.equal(audioAfterGesture, "running", "failed SDK load changed the resumed AudioContext");
    await screenshot("sdk-unavailable-playable");
    snapshots.push({ label: "sdk-unavailable-playable", counters: unavailable, audio: await evaluate("__webAdProof.audio.contexts.at(-1)?.state || 'none'") });
  } else if (provider === "none") {
    await delay(450);
    const noneLater = await readCounters();
    assert.equal(interceptedScripts.length, 0, "none profile requested an ad SDK script");
    assert.equal(await evaluate("__webAdProof.sdkBreaks.length"), 0);
    assert.equal(noneLater.handle, 0, "none profile left an ad handle open");
    assert.ok(noneLater.state === 5 || noneLater.terminal === 5, "none profile did not report the unsupported ad outcome");
    assert.ok(noneLater.simulation > initial.simulation, "none profile did not continue simulation");
    assert.ok(noneLater.renders > idleLater.renders, "none profile did not render frames");
    await screenshot("none-running");
    snapshots.push({ label: "none-running", counters: noneLater });
  } else {
    await waitCounters(value => value.handle > 0, "guest did not receive an ad handle");
    const requesting = await waitCounters(value => value.state === 1, "guest did not observe the requesting state");
    assert.equal(requesting.hostBlocked, true, "requesting did not block guest gameplay");
    const requestStable = await readCounters();
    await delay(240);
    const requestLater = await readCounters();
    assert.equal(requestLater.simulation, requestStable.simulation, "simulation advanced while ad request was pending");
    assert.ok(requestLater.polls > requestStable.polls, "guest polling stopped while request was pending");
    assert.ok(requestLater.renders > requestStable.renders, "host rendering stopped while request was pending");
    if (withAudio) {
      assert.equal(audioAfterGesture, "running", "audio was not active after the real canvas gesture");
      assert.equal(await evaluate("__webAdProof.audio.contexts.at(-1)?.state"), "running", "audio suspended before physical ad start");
    }
    await screenshot("requesting");
    snapshots.push({ label: "requesting", counters: requestLater, audio: await evaluate("__webAdProof.audio.contexts.at(-1)?.state || 'none'") });

    if (provider === "gamedistribution") {
      await activateGameDistributionRequest(1);
      await screenshot("activation");
    }

    let started;
    try {
      started = await waitCounters(value => value.state === 2, "SDK did not report actual ad start", 12_000);
    } catch (error) {
      const diagnostic = await evaluate(`({
        counters: (() => { const read = __webAdProof.readGlobal; return {
          state: read("browser_ad_state"), terminal: read("browser_ad_last_terminal_state"),
          handle: read("browser_ad_handle"), simulation: read("browser_ad_simulation_steps"),
          polls: read("browser_ad_poll_ticks"), renders: read("browser_ad_render_count"),
        }; })(),
        requests: __webAdProof.adRequestCalls, requestKinds: __webAdProof.requestKinds,
        sdkScripts: ${JSON.stringify(interceptedScripts)}, sdkEvaluations: __webAdProof.sdkEvaluations,
        sdkBreaks: __webAdProof.sdkBreaks, pokiEvents: __webAdProof.pokiEvents,
        lifecycle: __webAdProof.lifecycle, failures: __webAdProof.failures,
        activeElement: document.activeElement?.id || document.activeElement?.tagName || null,
      })`);
      await writeFile(path.join(runRoot, "provider-start-failure.json"), JSON.stringify(diagnostic, null, 2));
      throw new Error(`${error.message}; browser snapshot: ${JSON.stringify(diagnostic)}`);
    }
    if (withAudio) {
      await until(() => evaluate("__webAdProof.audio.contexts.at(-1)?.state === 'suspended'"), "audio was not suspended on actual ad start");
    }
    await screenshot("playing");
    const playingStable = await readCounters();
    const requestsBeforeHeldKey = await evaluate("__webAdProof.requestKinds.length");
    const breaksBeforeHeldKey = await evaluate("__webAdProof.sdkBreaks.length");
    await call("Input.dispatchKeyEvent", { type: "keyDown", code: "KeyR", key: "r", windowsVirtualKeyCode: 82 });
    await delay(300);
    const playingLater = await readCounters();
    assert.equal(playingLater.simulation, playingStable.simulation, "guest simulation advanced during the visible ad");
    assert.ok(playingLater.polls > playingStable.polls, "guest polling stopped during the visible ad");
    assert.ok(playingLater.renders > playingStable.renders, "rendering stopped during the visible ad");
    assert.equal(playingLater.handle, playingStable.handle);
    assert.equal(await evaluate("__webAdProof.requestKinds.length"), requestsBeforeHeldKey, "blocked held input issued another guest request");
    assert.equal(await evaluate("__webAdProof.sdkBreaks.length"), breaksBeforeHeldKey, "blocked input issued another provider call");
    if (withAudio) assert.equal(await evaluate("__webAdProof.audio.contexts.at(-1)?.state"), "suspended");
    snapshots.push({ label: "playing", counters: playingLater, audio: await evaluate("__webAdProof.audio.contexts.at(-1)?.state || 'none'") });
    await evaluate("__webAdProof.fake.finish()");
    const finished = await waitCounters(value => value.handle === 0 && value.terminal === 3, "guest did not observe and release the finished request");
    await waitCounters(value => value.hostBlocked === false, "host did not clear the gameplay block after actual terminal");
    if (withAudio) await until(() => evaluate("__webAdProof.audio.contexts.at(-1)?.state === 'running'"), "audio did not resume after the completed break");
    await call("Input.dispatchKeyEvent", {
      type: "keyDown", code: "KeyR", key: "r", windowsVirtualKeyCode: 82, autoRepeat: true,
    });
    await delay(150);
    const resumed = await readCounters();
    assert.ok(resumed.simulation > finished.simulation, "guest simulation did not resume after the completed break");
    assert.ok(resumed.simulation - finished.simulation <= 15, "guest simulation caught up in a burst after the completed break");
    assert.ok(resumed.renders > finished.renders, "rendering did not resume after the completed break");
    assert.equal(await evaluate("__webAdProof.requestKinds.length"), requestsBeforeHeldKey, "a repeated key held across the break triggered a new guest request");
    assert.equal(await evaluate("__webAdProof.sdkBreaks.length"), breaksBeforeHeldKey, "a repeated key held across the break triggered another provider call");
    await call("Input.dispatchKeyEvent", { type: "keyUp", code: "KeyR", key: "r", windowsVirtualKeyCode: 82 });
    await screenshot("finished");
    snapshots.push({ label: "finished", counters: resumed, audio: await evaluate("__webAdProof.audio.contexts.at(-1)?.state || 'none'") });

    if (provider === "crazygames" || provider === "poki") {
      await pressKey("KeyR", "r");
      const rewardRequest = await waitCounters(value => value.handle > 0 && value.state === 1, "rewarded request did not receive a pending handle");
      let manuallySuspendedBeforeAdStart = null;
      if (withAudio) {
        manuallySuspendedBeforeAdStart = await evaluate("(async () => { const context = __webAdProof.audio.contexts.at(-1); await context.suspend(); return context.state; })()");
        assert.equal(manuallySuspendedBeforeAdStart, "suspended", "manual audio suspension before ad start did not take effect");
      }
      await waitCounters(value => value.state === 2, "rewarded request did not reach actual start", 12_000);
      await evaluate("__webAdProof.fake.finish()");
      const rewardFinished = await waitCounters(value => value.handle === 0 && value.terminal === 3 && value.rewards === 1 && value.secondTakeZero === 1,
        "verified reward was not consumed exactly once");
      if (withAudio) {
        const afterAd = await evaluate("__webAdProof.audio.contexts.at(-1)?.state || 'none'");
        assert.equal(afterAd, "suspended", "ending an ad resumed audio that had been manually suspended before ad start");
        await pressKey("KeyA", "a");
        await until(() => evaluate("__webAdProof.audio.contexts.at(-1)?.state === 'running'"), "fresh user gesture did not resume manually suspended audio");
        manualAudioPause = {
          requestHandle: rewardRequest.handle,
          beforeAdStart: manuallySuspendedBeforeAdStart,
          afterAdTerminal: afterAd,
          afterFreshGesture: await evaluate("__webAdProof.audio.contexts.at(-1)?.state || 'none'"),
        };
      }
      await screenshot("reward-consumed");
      snapshots.push({ label: "reward-consumed", counters: rewardFinished, audio: await evaluate("__webAdProof.audio.contexts.at(-1)?.state || 'none'") });
    } else if (provider === "gamedistribution") {
      await evaluate("__webAdProof.fake.configureNextBreak({})");
      await call("Input.dispatchKeyEvent", { type: "keyDown", code: "KeyR", key: "r", windowsVirtualKeyCode: 82 });
      try {
        await waitCounters(value => value.handle > 0 && value.state === 1, "promise-only rewarded request did not remain pending");
      } catch (error) {
        const diagnostic = await evaluate(`(() => {
          const game = STASIS_GAME;
          const host = new Int32Array(__webAdProof.guest.memory.buffer, game.memory.host_i32.offset, game.memory.host_i32.length);
          const read = __webAdProof.readGlobal;
          return {
            counters: {
              state: read("browser_ad_state"), terminal: read("browser_ad_last_terminal_state"),
              handle: read("browser_ad_handle"), rewardLatched: read("browser_ad_reward_latched"),
              rewards: read("browser_ad_rewards_granted"),
            },
            keyR: host[32 + 21], requestKinds: __webAdProof.requestKinds,
            adRequestCalls: __webAdProof.adRequestCalls,
            showAdCalls: __webAdProof.showAdCalls, sdkEvents: __webAdProof.sdkEvents,
            gestures: __webAdProof.gestureEvents, activeElement: document.activeElement?.id || document.activeElement?.tagName,
            controlsHidden: document.getElementById("stasis-ad-controls")?.hidden,
            status: document.getElementById("stasis-ad-status")?.textContent,
            hostBlocked: document.body.dataset.adGameplayBlocked || null,
            failures: __webAdProof.failures,
          };
        })()`);
        await writeFile(path.join(runRoot, "gd-reward-request-failure.json"), JSON.stringify(diagnostic, null, 2));
        await screenshot("gd-reward-request-failure");
        throw new Error(`${error.message}; diagnostic: ${JSON.stringify(diagnostic)}`);
      } finally {
        await call("Input.dispatchKeyEvent", { type: "keyUp", code: "KeyR", key: "r", windowsVirtualKeyCode: 82 });
      }
      assert.equal(await evaluate("__webAdProof.requestKinds.at(-1)"), 1, "GameDistribution guest did not issue a rewarded request");
      await activateGameDistributionRequest(2);
      await waitCounters(value => value.state === 2, "promise-only rewarded request did not reach actual start", 12_000);
      assert.equal((await evaluate("__webAdProof.showAdCalls[1]")).requestKind, "rewarded", "reward request did not use showAd('rewarded')");
      await evaluate("__webAdProof.fake.finish()");
      const promiseOnly = await waitCounters(value => value.handle === 0 && value.terminal === 3, "promise-only rewarded break did not settle");
      assert.equal(promiseOnly.rewards, 0, "GameDistribution promise/resume alone granted a reward");
      assert.equal(promiseOnly.secondTakeZero, 0);
      assert.equal(await evaluate("__webAdProof.sdkEvents.filter(event => event.name === 'SDK_REWARDED_WATCH_COMPLETE').length"), 0,
        "promise-only GameDistribution flow unexpectedly emitted reward proof");
      await screenshot("reward-promise-without-proof");
      snapshots.push({ label: "reward-promise-without-proof", counters: promiseOnly });

      await evaluate("__webAdProof.fake.configureNextBreak({ rewardProof: true, reject: true })");
      await pressKey("KeyR", "r");
      await waitCounters(value => value.handle > 0 && value.state === 1, "proof-before-rejection request did not remain pending");
      assert.equal(await evaluate("__webAdProof.requestKinds.at(-1)"), 1, "GameDistribution guest did not issue its second rewarded request");
      await activateGameDistributionRequest(3);
      await waitCounters(value => value.state === 2, "proof-before-rejection request did not reach actual start", 12_000);
      assert.equal((await evaluate("__webAdProof.showAdCalls[2]")).requestKind, "rewarded");
      await evaluate("__webAdProof.fake.finish()");
      const rewardFinished = await waitCounters(value => value.handle === 0 && value.terminal === 4 && value.rewards === 1 && value.secondTakeZero === 1,
        "GameDistribution event proof was not consumed once after the later promise rejection");
      assert.equal(await evaluate("__webAdProof.sdkEvents.filter(event => event.name === 'SDK_REWARDED_WATCH_COMPLETE').length"), 1);
      await screenshot("reward-proof-before-rejection");
      snapshots.push({ label: "reward-proof-before-rejection", counters: rewardFinished });
    } else {
      await pressKey("KeyR", "r");
      await delay(150);
      const unsupportedReward = await readCounters();
      assert.ok(await evaluate("__webAdProof.requestKinds.includes(1)"), `${provider} guest did not issue a rewarded request`);
      assert.equal(unsupportedReward.rewards, 0, `${provider} incorrectly granted a reward`);
      assert.equal(await evaluate("__webAdProof.sdkBreaks.length"), 1, `${provider} issued an undocumented rewarded call`);
      await screenshot("reward-unavailable");
      snapshots.push({ label: "reward-unavailable", counters: unsupportedReward });
    }

    const beforeNoFill = await readCounters();
    if (provider === "gamemonetize") await evaluate("__webAdProof.fake.configureNextBreak({ noStart: true })");
    else await evaluate("__webAdProof.fake.configureNextBreak({ noFill: true })");
    await tap(point);
    const noFillRequest = await waitCounters(value => value.handle > 0 && value.state === 1,
      "no-fill/no-start diagnostic request did not enter the polling state");
    if (provider === "gamedistribution") await activateGameDistributionRequest(4);
    const noFillTerminal = await waitCounters(value => value.handle === 0 && value.terminal === (provider === "crazygames" || provider === "gamemonetize" ? 4 : 3),
      "no-fill/no-start path did not return to a terminal playable state", provider === "gamemonetize" ? 18_000 : 10_000);
    await waitCounters(value => !value.hostBlocked, "no-fill/no-start path kept the host gameplay block active");
    await delay(150);
    const noFillLater = await readCounters();
    assert.ok(noFillLater.simulation > noFillTerminal.simulation, "simulation did not continue after no-fill/no-start");
    assert.ok(noFillLater.simulation - noFillTerminal.simulation <= 15, "game simulation caught up in a burst after the failed/no-fill request");
    assert.ok(noFillLater.renders > noFillTerminal.renders, "rendering did not continue after no-fill/no-start");
    if (withAudio) assert.equal(await evaluate("__webAdProof.audio.contexts.at(-1)?.state"), "running", "no-fill/no-start changed the already-running audio context");
    await screenshot("no-fill-resumed");
    snapshots.push({
      label: provider === "gamemonetize" ? "no-start-timeout-resumed" : "no-fill-resumed",
      request: noFillRequest,
      counters: noFillLater,
      audio: await evaluate("__webAdProof.audio.contexts.at(-1)?.state || 'none'"),
    });

    adScriptCount = interceptedScripts.length;
    assert.equal(adScriptCount, 1, "selected profile did not load exactly one provider SDK script");
    assert.ok(interceptedScripts.every(item => item.provider === provider && !item.unexpected), "package requested an unexpected provider SDK");
    assert.equal(await evaluate("__webAdProof.sdkEvaluations.length"), 1);
    const sdkEvaluation = await evaluate("__webAdProof.sdkEvaluations[0]");
    if (provider === "gamemonetize" || provider === "gamedistribution") {
      assert.ok(sdkEvaluation.readyOptions?.hasHandler, "SDK did not capture its event handler during script evaluation");
      assert.ok(sdkEvaluation.readyOptions.gameId, "portal SDK did not receive its public game ID");
    }
    if (provider === "crazygames") {
      const lifecycle = await evaluate("__webAdProof.lifecycle");
      for (const event of ["loadingStart", "loadingStop", "gameplayStart", "gameplayStop"]) assert.ok(lifecycle.includes(event), `missing CrazyGames lifecycle ${event}`);
      assert.deepEqual(lifecycle.slice(0, 2), ["loadingStart", "loadingStop"], "CrazyGames loading lifecycle was not emitted at the loading boundary");
      assert.ok(lifecycle.indexOf("gameplayStart") < lifecycle.indexOf("gameplayStop"), "CrazyGames gameplay lifecycle was not ordered around the break");
      assert.ok(lifecycle.lastIndexOf("gameplayStart") > lifecycle.indexOf("gameplayStop"), "CrazyGames gameplay did not restart after the break");
    }
    if (provider === "poki") {
      const lifecycle = await evaluate("__webAdProof.lifecycle");
      assert.equal(lifecycle.includes("loadingStart"), false, "Poki received unsupported loading-start notification");
      for (const event of ["gameLoadingFinished", "gameplayStart", "gameplayStop"]) assert.ok(lifecycle.includes(event), `missing Poki lifecycle ${event}`);
      assert.ok(lifecycle.indexOf("gameLoadingFinished") < lifecycle.indexOf("gameplayStart"), "Poki loading completion was not emitted before gameplay");
      assert.ok(lifecycle.lastIndexOf("gameplayStart") > lifecycle.indexOf("gameplayStop"), "Poki gameplay did not restart after the break");
    }
    assert.ok(requesting.handle > 0 && started.state === 2 && finished.terminal === 3);
  }

  assert.deepEqual(await evaluate("__webAdProof.failures"), []);
    assert.deepEqual(cdpFailures, []);
  await call("Page.stopScreencast");
  screencastStarted = false;
  const video = await encodeScreencast(runRoot, screencastFrames);
  const hashes = {};
  for (const name of ["index.html", "game.js", "game.wasm", "ad_lifecycle.js"]) {
    try { hashes[name] = createHash("sha256").update(await readFile(path.join(bundle, name))).digest("hex"); }
    catch { /* none profile intentionally omits the ad module */ }
  }
  const receipt = {
    provider, pointerKind, variant: withAudio ? "audio" : "noaudio",
    sdkFailure, moduleFailure, moduleRequests, expectedModuleErrors, expectedSdkErrors,
    browser: await call("Browser.getVersion").catch(() => null),
    package: bundle, hashes, imports, sdkScripts: interceptedScripts, sdkEvaluations: await evaluate("__webAdProof.sdkEvaluations"),
    sdkBreaks: await evaluate("__webAdProof.sdkBreaks"), sdkEvents: await evaluate("__webAdProof.sdkEvents"),
    pokiEvents: await evaluate("__webAdProof.pokiEvents"), lifecycle: await evaluate("__webAdProof.lifecycle"),
    gameRequests: await evaluate("__webAdProof.importCalls"), audio: await evaluate(`({
      states: __webAdProof.audio.stateEvents,
      current: __webAdProof.audio.contexts.at(-1)?.state || "none",
      events: document.body.dataset.audioEvents || "0",
    })`),
    gdActivation: await evaluate("__webAdProof.showAdCalls"), initial, idleLater,
    audioBeforeGesture, audioAfterGesture, manualAudioPause,
    requestsBeforeInput, requestKinds: await evaluate("__webAdProof.requestKinds"),
    adRequestCalls: await evaluate("__webAdProof.adRequestCalls"), snapshots, frames,
    visualEvidence: { pngFrames: frames.map(frame => frame.file), video, captureMethod: "timestamped Chrome Page.screencastFrame events" },
  };
  await writeFile(path.join(runRoot, "receipt.json"), JSON.stringify(receipt, null, 2));
  console.log(JSON.stringify({ ...receipt, evidence: runRoot }, null, 2));
} finally {
  if (screencastStarted && call) await call("Page.stopScreencast").catch(() => {});
  socket?.close();
  if (browser.exitCode === null) {
    const exited = new Promise(resolve => browser.once("exit", resolve));
    browser.kill();
    await Promise.race([exited, delay(2_000)]);
  }
  server.close();
  if (profileDir.startsWith(runRoot + path.sep)) {
    await rm(profileDir, { recursive: true, force: true, maxRetries: 2 }).catch(() => {});
  }
}
