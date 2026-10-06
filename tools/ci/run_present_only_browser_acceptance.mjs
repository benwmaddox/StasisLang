// Run against a freshly packaged samples/presentation_baseline Web bundle.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { spawn } from "node:child_process";
import { access, readFile, writeFile, mkdir, mkdtemp, rm } from "node:fs/promises";
import { createServer } from "node:http";
import path from "node:path";

const bundle = path.resolve(process.argv[2] || "samples/presentation_baseline/build/web");
const evidence = path.resolve(process.argv[3] || "target/task625-web");
await mkdir(evidence, { recursive: true });
const profile = await mkdtemp(path.join(evidence, "chrome-"));
const browserCandidates = [
  process.env.STASIS_BROWSER_EXECUTABLE,
  "C:/Program Files/Google/Chrome/Application/chrome.exe",
  "C:/Program Files (x86)/Google/Chrome/Application/chrome.exe",
];
let browserPath;
for (const candidate of browserCandidates) {
  if (!candidate) continue;
  try {
    await access(candidate);
    browserPath = candidate;
    break;
  } catch {}
}
if (!browserPath) throw new Error("Set STASIS_BROWSER_EXECUTABLE to a supported Chrome executable");

const allowed = new Set(["/", "/index.html", "/game.js", "/game.wasm"]);
const server = createServer(async (request, response) => {
  const requestPath = new URL(request.url, "http://localhost").pathname;
  if (!allowed.has(requestPath)) {
    response.writeHead(404).end();
    return;
  }
  const file = requestPath === "/" ? "index.html" : requestPath.slice(1);
  try {
    response.setHeader(
      "Content-Type",
      file.endsWith(".wasm") ? "application/wasm"
        : file.endsWith(".js") ? "text/javascript" : "text/html",
    );
    response.end(await readFile(path.join(bundle, file)));
  } catch {
    response.writeHead(404).end();
  }
});
await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));

const browser = spawn(browserPath, [
  "--headless=new", "--no-sandbox", "--disable-gpu-sandbox", "--no-first-run",
  "--no-default-browser-check", "--use-angle=swiftshader", "--enable-unsafe-swiftshader",
  "--remote-debugging-port=0", `--user-data-dir=${profile}`, "about:blank",
], { stdio: "ignore" });
const delay = milliseconds => new Promise(resolve => setTimeout(resolve, milliseconds));
async function until(action, label) {
  const deadline = Date.now() + 30_000;
  while (Date.now() < deadline) {
    try {
      const value = await action();
      if (value) return value;
    } catch {}
    await delay(50);
  }
  throw new Error(`presentation browser acceptance timed out: ${label}`);
}

let socket;
try {
  const port = await until(async () => (
    await readFile(path.join(profile, "DevToolsActivePort"), "utf8")
  ).split("\n")[0], "Chrome DevTools port");
  const pages = await fetch(`http://127.0.0.1:${port}/json/list`).then(response => response.json());
  socket = new WebSocket(pages.find(page => page.type === "page").webSocketDebuggerUrl);
  await new Promise(resolve => socket.addEventListener("open", resolve, { once: true }));

  let nextId = 0;
  const pending = new Map();
  const protocolFailures = [];
  socket.addEventListener("message", event => {
    const message = JSON.parse(event.data);
    if (message.method === "Runtime.exceptionThrown") {
      protocolFailures.push(message.params.exceptionDetails.text);
    }
    if (message.method === "Runtime.consoleAPICalled"
      && ["error", "assert"].includes(message.params.type)) {
      protocolFailures.push(message.params.args.map(argument => argument.value || argument.description).join(" "));
    }
    const callback = pending.get(message.id);
    if (callback) {
      pending.delete(message.id);
      callback(message);
    }
  });
  const call = (method, params = {}) => new Promise((resolve, reject) => {
    const id = ++nextId;
    const timer = setTimeout(() => {
      pending.delete(id);
      reject(new Error(`${method} timed out`));
    }, 20_000);
    pending.set(id, message => {
      clearTimeout(timer);
      message.error ? reject(new Error(message.error.message)) : resolve(message.result);
    });
    socket.send(JSON.stringify({ id, method, params }));
  });
  const evaluate = async expression => {
    const result = await call("Runtime.evaluate", {
      expression, returnByValue: true, awaitPromise: true,
    });
    if (result.exceptionDetails) throw new Error(JSON.stringify(result.exceptionDetails));
    return result.result.value;
  };

  await call("Page.enable");
  await call("Runtime.enable");
  await call("Page.addScriptToEvaluateOnNewDocument", { source: `
    window.__presentationProbe = {
      arm: false, samples: [], failures: [], imports: [], exports: [], guest: null,
      context: { lossCount: 0, restoreCount: 0, generation: 0, lost: false, restoreErrors: [] }
    };
    const proof = window.__presentationProbe;
    const originalInstantiate = WebAssembly.instantiate;
    WebAssembly.instantiate = async function(bytes, imports) {
      const module = bytes instanceof WebAssembly.Module ? bytes : await WebAssembly.compile(bytes);
      proof.imports = WebAssembly.Module.imports(module);
      proof.exports = WebAssembly.Module.exports(module).map(entry => entry.name);
      const result = await originalInstantiate(bytes, imports);
      proof.guest = (result.instance || result).exports;
      return result;
    };
    const requestFrame = window.requestAnimationFrame.bind(window);
    window.requestAnimationFrame = callback => requestFrame(timestamp => {
      const probe = window.__presentationProbe;
      const canvas = document.getElementById('stasis-canvas');
      const capture = Boolean(probe?.arm && callback.name === 'frame' && canvas);
      let gl = null;
      if (capture) {
        gl = canvas.getContext('webgl2');
        if (!gl || gl.isContextLost()) throw new Error('presentation probe could not acquire the live WebGL2 context');
        const canvasRect = canvas.getBoundingClientRect();
        const priorErrors = [];
        for (let attempt = 0; attempt < 16; attempt += 1) {
          const error = gl.getError();
          if (error === gl.NO_ERROR) break;
          priorErrors.push(error);
        }
        gl.colorMask(true, true, true, true);
        gl.disable(gl.SCISSOR_TEST);
        gl.viewport(0, 0, canvas.width, canvas.height);
        gl.clearColor(1, 0, 1, 1);
        gl.clear(gl.COLOR_BUFFER_BIT);
        probe.before = {
          width: canvas.width,
          height: canvas.height,
          logicalWidth: Number(canvas.dataset.logicalWidth),
          logicalHeight: Number(canvas.dataset.logicalHeight),
          viewport: {
            innerWidth: window.innerWidth,
            innerHeight: window.innerHeight,
            screenWidth: window.screen.width,
            screenHeight: window.screen.height,
            devicePixelRatio: globalThis.devicePixelRatio,
            visualWidth: window.visualViewport?.width ?? null,
            visualHeight: window.visualViewport?.height ?? null,
            visualScale: window.visualViewport?.scale ?? null,
            visualOffsetLeft: window.visualViewport?.offsetLeft ?? null,
            visualOffsetTop: window.visualViewport?.offsetTop ?? null
          },
          canvasRect: {
            left: canvasRect.left,
            top: canvasRect.top,
            width: canvasRect.width,
            height: canvasRect.height
          },
          display: {
            cssWidth: Number(document.body.dataset.cssWidth),
            cssHeight: Number(document.body.dataset.cssHeight),
            backingWidth: Number(document.body.dataset.backingWidth),
            backingHeight: Number(document.body.dataset.backingHeight),
            devicePixelRatio: Number(document.body.dataset.devicePixelRatio),
            effectiveDpr: Number(document.body.dataset.effectiveDpr),
            displayGeneration: Number(document.body.dataset.displayGeneration)
          },
          renderer: gl.getParameter(gl.RENDERER),
          version: gl.getParameter(gl.VERSION),
          shadingLanguage: gl.getParameter(gl.SHADING_LANGUAGE_VERSION),
          lost: gl.isContextLost(),
          priorErrors,
          contextGeneration: probe.context.generation
        };
      }
      try {
        callback(timestamp);
      } finally {
        if (capture) {
          const width = canvas.width;
          const height = canvas.height;
          const bytes = new Uint8Array(width * height * 4);
          gl.readPixels(0, 0, width, height, gl.RGBA, gl.UNSIGNED_BYTE, bytes);
          const logicalWidth = Number(canvas.dataset.logicalWidth);
          const logicalHeight = Number(canvas.dataset.logicalHeight);
          const left = Math.floor(80 * width / logicalWidth);
          const right = Math.ceil(240 * width / logicalWidth);
          const top = Math.floor(45 * height / logicalHeight);
          const bottom = Math.ceil(135 * height / logicalHeight);
          let redPixels = 0;
          let blackOutsidePixels = 0;
          let nonBlackOutsidePixels = 0;
          let transparentPixels = 0;
          let redBounds = null;
          const expectedRed = [230, 38, 20];
          const near = (actual, expected) => Math.abs(actual - expected) <= 2;
          const imageBytes = new Uint8ClampedArray(bytes.length);
          for (let y = 0; y < height; y += 1) {
            const sourceRow = height - 1 - y;
            const rowStart = sourceRow * width * 4;
            const imageStart = y * width * 4;
            imageBytes.set(bytes.subarray(rowStart, rowStart + width * 4), imageStart);
            for (let x = 0; x < width; x += 1) {
              const offset = rowStart + x * 4;
              const r = bytes[offset];
              const g = bytes[offset + 1];
              const b = bytes[offset + 2];
              const a = bytes[offset + 3];
              const inside = x >= left && x < right && y >= top && y < bottom;
              const black = r === 0 && g === 0 && b === 0 && a === 255;
              const red = near(r, expectedRed[0]) && near(g, expectedRed[1])
                && near(b, expectedRed[2]) && a === 255;
              if (a !== 255) transparentPixels += 1;
              if (red) {
                redPixels += 1;
                redBounds ||= { left: x, top: y, right: x, bottom: y };
                redBounds.left = Math.min(redBounds.left, x);
                redBounds.top = Math.min(redBounds.top, y);
                redBounds.right = Math.max(redBounds.right, x);
                redBounds.bottom = Math.max(redBounds.bottom, y);
              }
              if (!inside) {
                if (black) blackOutsidePixels += 1;
                else nonBlackOutsidePixels += 1;
              }
            }
          }
          const proofCanvas = document.createElement('canvas');
          proofCanvas.width = width;
          proofCanvas.height = height;
          const proofContext = proofCanvas.getContext('2d', { willReadFrequently: true });
          proofContext.putImageData(new ImageData(imageBytes, width, height), 0, 0);
          const pngBase64 = proofCanvas.toDataURL('image/png').split(',')[1];
          const layout = window.STASIS_GAME?.memory?.gfx_cmd_i32;
          const guestMemory = probe.guest?.memory;
          let command = null;
          if (layout && guestMemory && Number.isSafeInteger(layout.offset)
              && Number.isSafeInteger(layout.length) && layout.offset >= 0
              && layout.offset % 4 === 0 && layout.length >= 30
              && layout.offset + layout.length * 4 <= guestMemory.buffer.byteLength) {
            const words = new Int32Array(guestMemory.buffer, layout.offset, layout.length);
          command = { magic: words[0], version: words[1], flags: words[2], rectCount: words[24] };
          }
          probe.samples.push({
            before: probe.before,
            after: { width, height, lost: gl.isContextLost() },
            context: { ...probe.context },
            resources: {
              atlasBytes: Number(document.body.dataset.assetAtlasBytes),
              atlasGeneration: Number(document.body.dataset.assetAtlasGeneration),
              atlasUploadCount: Number(document.body.dataset.atlasUploadCount),
              atlasUploadBytes: Number(document.body.dataset.atlasUploadBytes)
            },
            gpuError: document.body.dataset.gpuError || "",
            command,
            rectangle: { left, top, right, bottom, redBounds },
            pixels: {
              total: width * height,
              expectedRectangle: (right - left) * (bottom - top),
              redPixels,
              blackOutsidePixels,
              nonBlackOutsidePixels,
              transparentPixels
            },
            glError: gl.getError(),
            pngBase64
          });
          probe.arm = false;
        }
      }
    });
    addEventListener('error', event => proof.failures.push(event.message));
    addEventListener('unhandledrejection', event => proof.failures.push(String(event.reason)));
  ` });

  await call("Page.navigate", { url: `http://127.0.0.1:${server.address().port}/` });
  await until(() => evaluate(
    "document.body.dataset.ready === 'true' && Number(document.body.dataset.frames) >= 2",
  ), "packaged guest startup");
  assert.equal(await evaluate("document.body.dataset.backend"), "WebGL2");
  assert.deepEqual(await evaluate("__presentationProbe.failures"), []);

  const cases = [
    { name: "landscape-16x9", width: 1280, height: 720, mobile: false },
    { name: "portrait-mobile", width: 450, height: 900, mobile: true },
    { name: "ultrawide", width: 1920, height: 500, mobile: false },
    { name: "resized-landscape", width: 1024, height: 700, mobile: false },
  ];
  const captures = [];
  for (const [index, viewport] of cases.entries()) {
    await call("Emulation.setDeviceMetricsOverride", {
      width: viewport.width,
      height: viewport.height,
      screenWidth: viewport.width,
      screenHeight: viewport.height,
      deviceScaleFactor: 1,
      mobile: viewport.mobile,
    });
    await evaluate("window.STASIS_REFIT_VIEWPORT?.()");
    await until(() => evaluate(`(() => {
      const canvas = document.getElementById('stasis-canvas');
      return canvas && canvas.width > 0 && canvas.height > 0
        && Number(document.body.dataset.backingWidth) === canvas.width
        && Number(document.body.dataset.backingHeight) === canvas.height;
    })()`), `${viewport.name} backing resize`);

    const beforeFrames = Number(await evaluate("document.body.dataset.frames"));
    await until(() => evaluate(`Number(document.body.dataset.frames) > ${beforeFrames}`),
      `${viewport.name} post-resize guest frame`);
    const beforeSamples = Number(await evaluate("__presentationProbe.samples.length"));
    await evaluate("__presentationProbe.arm = true");
    await until(() => evaluate(`__presentationProbe.samples.length > ${beforeSamples}`),
      `${viewport.name} poisoned PRESENT capture`);
    const sample = await evaluate("__presentationProbe.samples.shift()");
    assert.ok(sample, `${viewport.name} omitted its WebGL readback`);
    assert.equal(sample.before.lost, false, `${viewport.name} context was lost before rendering`);
    assert.equal(sample.after.lost, false, `${viewport.name} context was lost after rendering`);
    assert.deepEqual(sample.before.priorErrors, [],
      `${viewport.name} WebGL context had an error before the measured frame`);
    assert.equal(sample.before.logicalWidth, 640);
    assert.equal(sample.before.logicalHeight, 360);
    assert.equal(sample.command?.magic, 0x47584631,
      `${viewport.name} did not publish the canonical graphics command magic`);
    assert.equal(sample.command?.version, 8, `${viewport.name} did not publish a v8 frame`);
    assert.equal(sample.command?.flags, 2, `${viewport.name} fixture unexpectedly requested guest CLEAR`);
    assert.equal(sample.command?.rectCount, 1);
    assert.ok(sample.pixels.redPixels >= sample.pixels.expectedRectangle * 0.98,
      `${viewport.name} red fixture is incomplete: ${JSON.stringify(sample.pixels)}`);
    assert.equal(sample.pixels.nonBlackOutsidePixels, 0,
      `${viewport.name} physical margins were not opaque black: ${JSON.stringify(sample.pixels)}`);
    assert.equal(sample.pixels.transparentPixels, 0,
      `${viewport.name} framebuffer contains transparent pixels`);
    assert.equal(sample.glError, 0, `${viewport.name} left a WebGL error`);

    const targetPng = Buffer.from(sample.pngBase64, "base64");
    delete sample.pngBase64;
    const targetPath = path.join(evidence, `${viewport.name}-target.png`);
    await writeFile(targetPath, targetPng);
    const screenshot = await call("Page.captureScreenshot", { format: "png", fromSurface: true });
    const screenshotBytes = Buffer.from(screenshot.data, "base64");
    const viewportPath = path.join(evidence, `${viewport.name}-viewport.png`);
    await writeFile(viewportPath, screenshotBytes);
    captures.push({
      name: viewport.name,
      viewport: { width: viewport.width, height: viewport.height, mobile: viewport.mobile },
      target: sample,
      targetPng: path.basename(targetPath),
      targetPngSha256: createHash("sha256").update(targetPng).digest("hex"),
      viewportPng: path.basename(viewportPath),
      viewportPngSha256: createHash("sha256").update(screenshotBytes).digest("hex"),
    });
    if (index + 1 < cases.length) {
      await call("Emulation.clearDeviceMetricsOverride");
    }
  }

  const sampleCountBeforeRestore = Number(await evaluate("__presentationProbe.samples.length"));
  const restoreLifecycle = await evaluate(`new Promise((resolve, reject) => {
    const canvas = document.getElementById('stasis-canvas');
    const gl = canvas?.getContext('webgl2');
    const extension = gl?.getExtension('WEBGL_lose_context');
    if (!canvas || !gl || !extension) {
      reject(new Error('WEBGL_lose_context is unavailable for the live presentation context'));
      return;
    }
    const probe = window.__presentationProbe;
    const timeout = setTimeout(() => reject(new Error('WebGL context restore timed out')), 10000);
    canvas.addEventListener('webglcontextlost', event => {
      probe.context.lossCount += 1;
      probe.context.lost = true;
      event.preventDefault();
    }, { once: true });
    canvas.addEventListener('webglcontextrestored', () => {
      probe.context.restoreCount += 1;
      probe.context.generation += 1;
      probe.context.lost = false;
      const restoredGl = canvas.getContext('webgl2');
      for (let attempt = 0; attempt < 16; attempt += 1) {
        const error = restoredGl.getError();
        if (error === restoredGl.NO_ERROR) break;
        probe.context.restoreErrors.push(error);
      }
      probe.arm = true;
      clearTimeout(timeout);
      resolve({
        lossCount: probe.context.lossCount,
        restoreCount: probe.context.restoreCount,
        generation: probe.context.generation,
        lost: probe.context.lost,
        restoreErrors: [...probe.context.restoreErrors]
      });
    }, { once: true });
    extension.loseContext();
    setTimeout(() => extension.restoreContext(), 100);
  })`);
  assert.equal(restoreLifecycle.lossCount, 1, "live WebGL context did not emit one loss event");
  assert.equal(restoreLifecycle.restoreCount, 1, "live WebGL context did not emit one restore event");
  assert.equal(restoreLifecycle.generation, 1);
  assert.equal(restoreLifecycle.lost, false);
  assert.deepEqual(restoreLifecycle.restoreErrors, [], "restored WebGL resource preparation left GL errors");
  await until(() => evaluate(`__presentationProbe.samples.length > ${sampleCountBeforeRestore}`),
    "first guest PRESENT after WebGL context restore");
  const restoredSample = await evaluate("__presentationProbe.samples.shift()");
  assert.equal(restoredSample.before.contextGeneration, 1,
    "restored pixels were not captured from the recreated WebGL context");
  assert.equal(restoredSample.context.lossCount, 1);
  assert.equal(restoredSample.context.restoreCount, 1);
  assert.equal(restoredSample.context.lost, false);
  assert.deepEqual(restoredSample.context.restoreErrors, []);
  assert.deepEqual(restoredSample.before.priorErrors, []);
  assert.ok(restoredSample.resources.atlasBytes > 0,
    "the first restored guest PRESENT did not allocate its WebGL atlas resource");
  assert.ok(restoredSample.resources.atlasUploadCount > 0,
    "the first restored guest PRESENT did not upload its WebGL atlas resource");
  assert.ok(restoredSample.resources.atlasUploadBytes >= 16,
    "the first restored guest PRESENT did not upload the solid atlas pixels");
  assert.equal(restoredSample.gpuError, "", "restored frame retained a WebGL resource error");
  assert.equal(restoredSample.command?.magic, 0x47584631);
  assert.equal(restoredSample.command?.version, 8);
  assert.equal(restoredSample.command?.flags, 2,
    "the restored frame unexpectedly requested guest CLEAR");
  assert.equal(restoredSample.command?.rectCount, 1);
  assert.ok(restoredSample.pixels.redPixels >= restoredSample.pixels.expectedRectangle * 0.98,
    `restored red fixture is incomplete: ${JSON.stringify(restoredSample.pixels)}`);
  assert.equal(restoredSample.pixels.nonBlackOutsidePixels, 0,
    `restored physical margins were not opaque black: ${JSON.stringify(restoredSample.pixels)}`);
  assert.equal(restoredSample.pixels.transparentPixels, 0);
  assert.equal(restoredSample.glError, 0, "restored PRESENT left a WebGL error");
  const restoredTargetPng = Buffer.from(restoredSample.pngBase64, "base64");
  delete restoredSample.pngBase64;
  const restoredTargetPath = path.join(evidence, "restored-context-target.png");
  await writeFile(restoredTargetPath, restoredTargetPng);
  const restoredScreenshot = await call("Page.captureScreenshot", { format: "png", fromSurface: true });
  const restoredViewportPng = Buffer.from(restoredScreenshot.data, "base64");
  const restoredViewportPath = path.join(evidence, "restored-context-viewport.png");
  await writeFile(restoredViewportPath, restoredViewportPng);

  const hashes = {};
  for (const file of ["game.js", "game.wasm", "stasis_provenance.json"]) {
    hashes[file] = createHash("sha256").update(await readFile(path.join(bundle, file))).digest("hex");
  }
  const provenance = JSON.parse(await readFile(path.join(bundle, "stasis_provenance.json"), "utf8"));
  const failures = await evaluate("__presentationProbe.failures");
  assert.deepEqual(failures, []);
  assert.deepEqual(protocolFailures, []);
  const receipt = {
    schema: "stasis.present_only_browser_acceptance.v1",
    source: {
      bundle: path.relative(process.cwd(), bundle),
      sourceCommit: provenance.source_commit ?? null,
      dirtyState: provenance.dirty_state ?? null,
      developmentBuild: provenance.development_build ?? null,
      runtimeSources: provenance.runtime_sources ?? null,
      provenanceSha256: hashes["stasis_provenance.json"],
    },
    browser: await call("Browser.getVersion"),
    wasmImports: await evaluate("__presentationProbe.imports"),
    wasmExports: await evaluate("__presentationProbe.exports"),
    captures,
    pixelPreservationCoverage: "No-PRESENT and rejected-frame preservation is covered by the fake-GL unit suite only. The real browser fixture emits PRESENT every render; the harness does not mutate guest state to synthesize a no-PRESENT frame.",
    contextRestore: {
      lifecycle: restoreLifecycle,
      capture: restoredSample,
      targetPng: path.basename(restoredTargetPath),
      targetPngSha256: createHash("sha256").update(restoredTargetPng).digest("hex"),
      viewportPng: path.basename(restoredViewportPath),
      viewportPngSha256: createHash("sha256").update(restoredViewportPng).digest("hex"),
    },
    failures,
    protocolFailures,
  };
  await writeFile(path.join(evidence, "receipt.json"), JSON.stringify(receipt, null, 2));
  console.log(JSON.stringify(receipt, null, 2));
} finally {
  socket?.close();
  if (browser.exitCode === null) {
    const exited = new Promise(resolve => browser.once("exit", resolve));
    browser.kill();
    await Promise.race([exited, delay(2_000)]);
  }
  server.close();
  if (profile.startsWith(evidence + path.sep)) {
    await rm(profile, { recursive: true, force: true, maxRetries: 2 }).catch(() => {});
  }
}
