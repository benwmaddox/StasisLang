// Run against a freshly packaged development samples/generics_collections Web bundle.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { spawn } from "node:child_process";
import { readFile, writeFile, mkdir, mkdtemp, rm } from "node:fs/promises";
import { createServer } from "node:http";
import path from "node:path";

const bundle = path.resolve(process.argv[2] || "samples/generics_collections/build/web");
const evidence = path.resolve(process.argv[3] || "target/generics-collections-browser");
await mkdir(evidence, { recursive: true });
const profile = await mkdtemp(path.join(evidence, "chrome-"));
const browserPath = process.env.STASIS_BROWSER_EXECUTABLE
  || "C:/Program Files/Google/Chrome/Application/chrome.exe";
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
async function until(action) {
  const deadline = Date.now() + 20_000;
  while (Date.now() < deadline) {
    try {
      const value = await action();
      if (value) return value;
    } catch {}
    await delay(50);
  }
  throw new Error("generics browser acceptance condition timed out");
}

let socket;
try {
  const port = await until(async () => (
    await readFile(path.join(profile, "DevToolsActivePort"), "utf8")
  ).split("\n")[0]);
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
    window.genericsProof = { failures: [], imports: [], exports: [], guest: null };
    const originalInstantiate = WebAssembly.instantiate;
    WebAssembly.instantiate = async function(bytes, imports) {
      const module = await WebAssembly.compile(bytes);
      genericsProof.imports = WebAssembly.Module.imports(module);
      genericsProof.exports = WebAssembly.Module.exports(module);
      const result = await originalInstantiate(bytes, imports);
      genericsProof.guest = (result.instance || result).exports;
      return result;
    };
    addEventListener('error', event => genericsProof.failures.push(event.message));
    addEventListener('unhandledrejection', event => genericsProof.failures.push(String(event.reason)));
  ` });
  await call("Page.navigate", { url: `http://127.0.0.1:${server.address().port}/` });
  await until(() => evaluate(
    "document.body.dataset.ready === 'true' && Number(document.body.dataset.frames) >= 2",
  ));

  assert.equal(await evaluate("document.body.dataset.mainResult"), "0");
  assert.equal(await evaluate("document.body.dataset.backend"), "WebGL2");
  assert.equal(await evaluate("document.body.dataset.rectangles"), "1");
  assert.ok(Number(await evaluate("document.body.dataset.drawCalls")) >= 1);
  assert.deepEqual(await evaluate("genericsProof.failures"), []);
  assert.deepEqual(await evaluate("genericsProof.imports"), []);
  assert.deepEqual(protocolFailures, []);
  assert.equal(await evaluate("genericsProof.guest.tick()"), 0);
  assert.equal(await evaluate(`(() => {
    const hash = STASIS_GAME.globals.generics_collections_digest_value.hash;
    return genericsProof.guest.__stasis_global_get_i32(hash);
  })()`), 507);
  const bounds = await evaluate(`(() => {
    const e = genericsProof.guest;
    const hash = STASIS_GAME.globals.web_bounds_probe_index.hash;
    const trapped = index => {
      e.__stasis_global_set_i32(hash, index);
      try { e.tick(); return false; }
      catch (error) { return error instanceof WebAssembly.RuntimeError; }
      finally { e.__stasis_global_set_i32(hash, 0); }
    };
    return { low: trapped(-1), high: trapped(2) };
  })()`);
  assert.deepEqual(bounds, { low: true, high: true });

  const screenshot = await call("Page.captureScreenshot", { format: "png", fromSurface: true });
  const screenshotBytes = Buffer.from(screenshot.data, "base64");
  assert.ok(screenshotBytes.length > 1_000, "browser screenshot is unexpectedly empty");
  const screenshotUrl = JSON.stringify(`data:image/png;base64,${screenshot.data}`);
  const pixels = await evaluate(`(async () => {
    const bitmap = await createImageBitmap(await (await fetch(${screenshotUrl})).blob());
    const canvas = document.createElement("canvas");
    canvas.width = bitmap.width;
    canvas.height = bitmap.height;
    const context = canvas.getContext("2d", { willReadFrequently: true });
    context.drawImage(bitmap, 0, 0);
    const data = context.getImageData(0, 0, canvas.width, canvas.height).data;
    let backgroundPixels = 0;
    let accentPixels = 0;
    const near = (actual, expected) => Math.abs(actual - expected) <= 5;
    for (let offset = 0; offset < data.length; offset += 4) {
      if (near(data[offset], 10) && near(data[offset + 1], 20) && near(data[offset + 2], 41)) {
        backgroundPixels += 1;
      }
      if (near(data[offset], 41) && near(data[offset + 1], 184) && near(data[offset + 2], 133)) {
        accentPixels += 1;
      }
    }
    bitmap.close();
    return {
      width: canvas.width,
      height: canvas.height,
      totalPixels: canvas.width * canvas.height,
      backgroundPixels,
      accentPixels,
    };
  })()`);
  assert.ok(
    pixels.backgroundPixels >= pixels.totalPixels * 0.2,
    `authored background color is missing from frame: ${JSON.stringify(pixels)}`,
  );
  assert.ok(
    pixels.accentPixels >= pixels.totalPixels * 0.01,
    `authored accent rectangle is missing from frame: ${JSON.stringify(pixels)}`,
  );
  await writeFile(path.join(evidence, "browser.png"), screenshotBytes);

  const hashes = {};
  for (const file of ["game.js", "game.wasm", "stasis_provenance.json"]) {
    hashes[file] = createHash("sha256")
      .update(await readFile(path.join(bundle, file))).digest("hex");
  }
  await delay(100);
  const pageFailures = await evaluate("genericsProof.failures");
  assert.deepEqual(pageFailures, []);
  assert.deepEqual(protocolFailures, []);
  const receipt = {
    schema: "stasis.generics_collections_browser_acceptance.v1",
    browser: await call("Browser.getVersion"),
    mainResult: 0,
    stateDigest: 507,
    bounds,
    frame: {
      backend: "WebGL2",
      frames: Number(await evaluate("document.body.dataset.frames")),
      rectangles: Number(await evaluate("document.body.dataset.rectangles")),
      drawCalls: Number(await evaluate("document.body.dataset.drawCalls")),
      pixels,
      screenshotBytes: screenshotBytes.length,
      screenshotSha256: createHash("sha256").update(screenshotBytes).digest("hex"),
    },
    imports: await evaluate("genericsProof.imports"),
    exports: await evaluate("genericsProof.exports.map(entry => entry.name)"),
    failures: [...pageFailures, ...protocolFailures],
    hashes,
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
