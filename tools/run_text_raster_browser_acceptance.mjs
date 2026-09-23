// Real Canvas2D/WebGL2 host integration; the guest ABI fixture does not compile Stasis.
// Usage: node tools/run_text_raster_browser_acceptance.mjs <evidence-dir> <baseline-ref>
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { execFileSync, spawn } from "node:child_process";
import { readFile, writeFile, mkdir, mkdtemp, rm } from "node:fs/promises";
import { createServer } from "node:http";
import path from "node:path";

const baseline = process.argv[3];
if (!baseline) throw new Error("Expected evidence directory and baseline Git ref");
const evidence = path.resolve(process.argv[2]);
await mkdir(evidence, { recursive: true });
const sources = {
  before: execFileSync("git", ["show", `${baseline}:runtime/web/game.js`]),
  after: await readFile("runtime/web/game.js"),
};
const font = await readFile("apps/stasis/assets/gauntlet-font/Basic-Regular.ttf");
const html = variant => `<!doctype html><meta charset="utf-8">
<style>body{margin:0;background:#17212b}canvas{width:1280px;height:360px;display:block}</style>
<canvas id="stasis-canvas" width="640" height="360"></canvas><pre id="stasis-error"></pre>
<script>
window.proof = { fills: [], quads: [] };
const fill = CanvasRenderingContext2D.prototype.fillText;
CanvasRenderingContext2D.prototype.fillText = function(text, x, y) {
  const transform = this.getTransform();
  proof.fills.push({ text, width: this.canvas.width, height: this.canvas.height, x, y, scale: transform.a });
  return fill.call(this, text, x, y);
};
const upload = WebGL2RenderingContext.prototype.bufferSubData;
WebGL2RenderingContext.prototype.bufferSubData = function(...args) {
  if (args[2] instanceof Float32Array && args[4] === 16) {
    proof.quads.push(Array.from(args[2].subarray(args[3], args[3] + 4)));
  }
  return upload.apply(this, args);
};
const memory = new WebAssembly.Memory({ initial: 16 });
const ints = new Int32Array(memory.buffer, 0, 67888);
const floats = new Float32Array(memory.buffer, 300000, 146564);
let env, fonts = [], runs = [];
window.STASIS_GAME = {
  collectionViewAbiVersion: 2,
  memory: {
    gfx_cmd_i32: { offset: 0, length: 67888 }, gfx_cmd_f32: { offset: 300000, length: 146564 },
    host_i32: { offset: 900000, length: 768 }, host_f32: { offset: 903072, length: 64 }
  },
  strings: { 1: 'font.ttf', 2: 'Small text: baseline gyp 012345', 3: 'Physical pixels' }, assets: {}
};
WebAssembly.instantiate = async (_bytes, imports) => {
  env = imports.env;
  return { instance: { exports: {
    memory, __stasis_collection_view_abi_version: new WebAssembly.Global({ value: 'i32' }, 2),
    main() {
      fonts = [env.load_font(1, 12), env.load_font(1, 30)];
      runs = fonts.map((font, index) => env.stasis_jit_gfx_cache_text(font, index + 2));
      return 0;
    },
    tick() { return 0; },
    render() {
      env.web_begin_frame(23, 33, 43);
      ints[0] = 1196967473; ints[1] = 7; ints[7] = 2;
      for (let index = 0; index < 2; index++) {
        const meta = 12320 + index * 3;
        ints[meta] = fonts[index]; ints[meta + 1] = -runs[index]; ints[meta + 2] = 0;
        floats.set([12, 15 + index * 45, 1, 1, 1, 1], 133252 + index * 6);
      }
      return 0;
    }
  } } };
};
</script><script src="/${variant}.js"></script>`;
const server = createServer((request, response) => {
  const route = new URL(request.url, "http://localhost").pathname;
  if (route === "/font.ttf") response.end(font);
  else if (route === "/before.js" || route === "/after.js") {
    response.setHeader("Content-Type", "text/javascript");
    response.end(sources[route.slice(1, -3)]);
  } else if (route === "/before" || route === "/after") {
    response.setHeader("Content-Type", "text/html");
    response.end(html(route.slice(1)));
  } else response.end(Buffer.alloc(0));
});
await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
const profile = await mkdtemp(path.join(evidence, "chrome-"));
const browser = spawn(process.env.STASIS_BROWSER_EXECUTABLE
  || "C:/Program Files/Google/Chrome/Application/chrome.exe", [
  "--headless=new", "--no-first-run", "--no-default-browser-check", "--use-angle=swiftshader",
  "--enable-unsafe-swiftshader", "--remote-debugging-port=0", `--user-data-dir=${profile}`, "about:blank"
], { stdio: "ignore" });
const delay = milliseconds => new Promise(resolve => setTimeout(resolve, milliseconds));
async function until(action) {
  const deadline = Date.now() + 20000;
  while (Date.now() < deadline) {
    const value = await action().catch(() => null);
    if (value) return value;
    await delay(50);
  }
  throw new Error("browser acceptance timeout");
}
let socket;
try {
  const port = await until(async () => (
    await readFile(path.join(profile, "DevToolsActivePort"), "utf8")
  ).split("\n")[0]);
  const pages = await fetch(`http://127.0.0.1:${port}/json/list`).then(response => response.json());
  socket = new WebSocket(pages.find(page => page.type === "page").webSocketDebuggerUrl);
  await new Promise(resolve => socket.addEventListener("open", resolve, { once: true }));
  let id = 0;
  const pending = new Map();
  socket.addEventListener("message", event => {
    const message = JSON.parse(event.data);
    pending.get(message.id)?.(message);
  });
  const call = (method, params = {}) => new Promise((resolve, reject) => {
    const key = ++id;
    const timer = setTimeout(() => {
      pending.delete(key);
      reject(new Error(`${method} timeout`));
    }, 20000);
    pending.set(key, message => {
      clearTimeout(timer);
      pending.delete(key);
      message.error ? reject(new Error(message.error.message)) : resolve(message.result);
    });
    socket.send(JSON.stringify({ id: key, method, params }));
  });
  const evaluate = async expression => {
    const result = await call("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true });
    if (result.exceptionDetails) throw new Error(JSON.stringify(result.exceptionDetails));
    return result.result.value;
  };
  await call("Emulation.setDeviceMetricsOverride", { width: 1280, height: 420, deviceScaleFactor: 1, mobile: false });
  const receipts = {
    browser: await call("Browser.getVersion"), baseline,
    sourceSha256: Object.fromEntries(Object.entries(sources)
      .map(([variant, bytes]) => [variant, createHash("sha256").update(bytes).digest("hex")])),
    qualification: "Real browser host renderer with synthetic guest ABI; not packaged-game or physical-device acceptance"
  };
  for (const variant of ["before", "after"]) {
    await call("Page.navigate", { url: `http://127.0.0.1:${server.address().port}/${variant}` });
    await until(() => evaluate("document.body.dataset.ready === 'true' && proof.quads.length >= 2"));
    receipts[variant] = await evaluate("({ fills: proof.fills, quads: proof.quads.slice(-2), display: { ...document.body.dataset } })");
    const screenshot = await call("Page.captureScreenshot", { format: "png" });
    await writeFile(path.join(evidence, `${variant}.png`), Buffer.from(screenshot.data, "base64"));
  }
  assert.deepEqual(receipts.after.quads, receipts.before.quads, "logical placement and bounds remain stable");
  assert.equal(receipts.after.display.textRasterScale, "2");
  for (let index = 0; index < 2; index++) {
    assert.equal(receipts.after.fills[index].width, receipts.before.fills[index].width * 2);
    assert.equal(receipts.after.fills[index].height, receipts.before.fills[index].height * 2);
    assert.equal(receipts.after.fills[index].y, receipts.before.fills[index].y);
  }
  await writeFile(path.join(evidence, "receipt.json"), JSON.stringify(receipts, null, 2));
  console.log(`Text raster acceptance passed. Evidence: ${evidence}`);
} finally {
  socket?.close();
  if (browser.exitCode === null) {
    const exited = new Promise(resolve => browser.once("exit", resolve));
    browser.kill();
    await Promise.race([exited, delay(2000)]);
  }
  server.close();
  if (path.resolve(profile).startsWith(evidence + path.sep)) {
    await rm(profile, { recursive: true, force: true, maxRetries: 2 }).catch(() => {});
  }
}
