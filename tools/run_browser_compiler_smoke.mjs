import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { spawn } from "node:child_process";
import { access, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import path from "node:path";

const repository = path.resolve(import.meta.dirname, "..");
const compilerWasm = path.resolve(process.argv[2] || path.join(
  repository,
  "build/codex-cargo-target/wasm32-unknown-unknown/release/stasis_browser_compiler.wasm",
));
const evidence = path.resolve(process.argv[3] || path.join(repository, "target/browser-compiler-smoke"));
const webRoot = path.join(repository, "crates/stasis_browser_compiler/web");
const browserPath = process.env.STASIS_BROWSER_EXECUTABLE
  || (process.platform === "win32"
    ? "C:/Program Files/Google/Chrome/Application/chrome.exe"
    : "google-chrome");

await access(compilerWasm);
const coreBytes = await readFile(compilerWasm);
const coreModule = await WebAssembly.compile(coreBytes);
const coreImports = WebAssembly.Module.imports(coreModule);
const coreExports = WebAssembly.Module.exports(coreModule);
const requiredCoreExports = [
  "memory",
  "stasis_browser_compiler_abi_version",
  "stasis_compile_error_len",
  "stasis_compile_error_ptr",
  "stasis_compile_fixed_sample",
  "stasis_compile_output_len",
  "stasis_compile_output_ptr",
];
assert.deepEqual(coreImports, [], "compiler core must not import host capabilities");
for (const name of requiredCoreExports) {
  assert.ok(coreExports.some(item => item.name === name), `compiler core is missing export ${name}`);
}
await mkdir(evidence, { recursive: true });
const profile = await mkdtemp(path.join(evidence, "chrome-"));
const requests = [];
const routes = new Map([
  ["/", { path: path.join(webRoot, "smoke.html"), type: "text/html; charset=utf-8" }],
  ["/smoke.js", { path: path.join(webRoot, "smoke.js"), type: "text/javascript; charset=utf-8" }],
  ["/compiler_worker.js", { path: path.join(webRoot, "compiler_worker.js"), type: "text/javascript; charset=utf-8" }],
  ["/compiler_core.wasm", { path: compilerWasm, type: "application/wasm" }],
]);
const server = createServer(async (request, response) => {
  const requestPath = new URL(request.url, "http://localhost").pathname;
  requests.push({ method: request.method, path: requestPath });
  const route = routes.get(requestPath);
  if (!route || request.method !== "GET") {
    response.writeHead(404).end();
    return;
  }
  try {
    response.setHeader("Content-Type", route.type);
    response.setHeader("Cache-Control", "no-store");
    response.setHeader(
      "Content-Security-Policy",
      "default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; worker-src 'self'; connect-src 'self'",
    );
    response.end(await readFile(route.path));
  } catch (error) {
    response.writeHead(500).end(String(error));
  }
});
await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));

const browser = spawn(browserPath, [
  "--headless=new",
  "--no-sandbox",
  "--disable-gpu-sandbox",
  "--no-first-run",
  "--no-default-browser-check",
  "--remote-debugging-port=0",
  `--user-data-dir=${profile}`,
  "about:blank",
], { stdio: "ignore" });
const delay = milliseconds => new Promise(resolve => setTimeout(resolve, milliseconds));
async function until(action, description) {
  const deadline = Date.now() + 120_000;
  while (Date.now() < deadline) {
    try {
      const value = await action();
      if (value) return value;
    } catch {}
    await delay(50);
  }
  throw new Error(`${description} timed out`);
}

let socket;
try {
  const debugPort = await until(async () => (
    await readFile(path.join(profile, "DevToolsActivePort"), "utf8")
  ).split("\n")[0], "Chrome startup");
  const pages = await fetch(`http://127.0.0.1:${debugPort}/json/list`).then(response => response.json());
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
      protocolFailures.push(message.params.args
        .map(argument => argument.value || argument.description)
        .join(" "));
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
    }, 120_000);
    pending.set(id, message => {
      clearTimeout(timer);
      message.error ? reject(new Error(message.error.message)) : resolve(message.result);
    });
    socket.send(JSON.stringify({ id, method, params }));
  });
  const evaluate = async expression => {
    const result = await call("Runtime.evaluate", {
      expression,
      returnByValue: true,
      awaitPromise: true,
    });
    if (result.exceptionDetails) {
      throw new Error(JSON.stringify(result.exceptionDetails));
    }
    return result.result.value;
  };

  await call("Page.enable");
  await call("Runtime.enable");
  await call("Page.navigate", { url: `http://127.0.0.1:${server.address().port}/` });
  const readyState = await until(async () => {
    const state = await evaluate(`({
      ready: document.body?.dataset.ready,
      fatal: document.body?.dataset.fatal,
    })`);
    return state.fatal || state.ready === "true" ? state : null;
  }, "browser compiler Worker readiness");
  if (readyState.fatal) throw new Error(readyState.fatal);

  const requestsAtReady = requests.length;
  const result = await evaluate("window.runBrowserCompilerSmoke()");
  await delay(250);
  const requestsAfterReady = requests.slice(requestsAtReady);

  assert.equal(result.mainResult, 720);
  assert.ok(result.byteLength > 8, "compiled game.wasm is unexpectedly small");
  assert.deepEqual(result.imports, []);
  assert.ok(result.exports.some(item => item.name === "main" && item.kind === "function"));
  assert.match(result.sha256, /^[0-9a-f]{64}$/);
  assert.deepEqual(protocolFailures, []);
  assert.deepEqual(
    requestsAfterReady,
    [],
    `application-server requests occurred after Worker ready: ${JSON.stringify(requestsAfterReady)}`,
  );
  assert.deepEqual(
    requests.map(request => request.path).sort(),
    ["/", "/compiler_core.wasm", "/compiler_worker.js", "/smoke.js"].sort(),
  );

  const receipt = {
    schema: "stasis.browser_compiler_smoke.v1",
    browser: await call("Browser.getVersion"),
    compiler: {
      abiVersion: Number(await evaluate("document.body.dataset.abiVersion")),
      byteLength: coreBytes.length,
      sha256: createHash("sha256").update(coreBytes).digest("hex"),
      imports: coreImports,
      exports: coreExports,
    },
    game: result,
    network: {
      readyBoundaryRequestCount: requestsAtReady,
      requests,
      requestsAfterReady,
    },
    failures: protocolFailures,
  };
  await writeFile(path.join(evidence, "receipt.json"), `${JSON.stringify(receipt, null, 2)}\n`);
  console.log(JSON.stringify(receipt, null, 2));
} finally {
  socket?.close();
  if (browser.exitCode === null) {
    const exited = new Promise(resolve => browser.once("exit", resolve));
    browser.kill();
    await Promise.race([exited, delay(2_000)]);
  }
  await new Promise(resolve => server.close(resolve));
  if (profile.startsWith(evidence + path.sep)) {
    await rm(profile, { recursive: true, force: true, maxRetries: 2 }).catch(() => {});
  }
}
