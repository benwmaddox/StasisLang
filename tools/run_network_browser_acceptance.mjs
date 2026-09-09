import assert from "node:assert/strict";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import path from "node:path";
import { spawn } from "node:child_process";
import { encodeVideo } from "./network_browser_video.mjs";

class Cdp {
  constructor(url) {
    this.nextId = 1;
    this.pending = new Map();
    this.socket = new WebSocket(url);
    this.ready = new Promise((resolve, reject) => {
      this.socket.addEventListener("open", resolve, { once: true });
      this.socket.addEventListener("error", reject, { once: true });
    });
    this.socket.addEventListener("message", event => {
      const message = JSON.parse(event.data);
      const pending = this.pending.get(message.id);
      if (!pending) return;
      this.pending.delete(message.id);
      if (message.error) pending.reject(new Error(message.error.message));
      else pending.resolve(message.result);
    });
    this.socket.addEventListener("close", event => {
      for (const pending of this.pending.values()) pending.reject(new Error(`browser debugging connection closed (${event.code}: ${event.reason})`));
      this.pending.clear();
    });
  }
  async call(method, params = {}) {
    await this.ready;
    const id = this.nextId++;
    const result = new Promise((resolve, reject) => this.pending.set(id, { resolve, reject }));
    this.socket.send(JSON.stringify({ id, method, params }));
    return result;
  }
  close() { this.socket.close(); }
}

const SECRET = "a17c9e240d6b3f8152a48c70e93db6f1a17c9e240d6b3f8152a48c70e93db6f1";
const hostExecutable = process.env.STASIS_NETWORK_HOST_EXECUTABLE;
assert.ok(hostExecutable, "STASIS_NETWORK_HOST_EXECUTABLE is required");
const browserExecutable = process.env.STASIS_BROWSER_EXECUTABLE || defaultBrowser();
assert.ok(browserExecutable, "STASIS_BROWSER_EXECUTABLE is required on this platform");

const evidenceRoot = path.resolve(process.env.STASIS_NETWORK_EVIDENCE_DIR || "target/network-browser-acceptance");
await rm(evidenceRoot, { recursive: true, force: true });
await mkdir(evidenceRoot, { recursive: true });
const scratchRoot = path.resolve("target/network-browser-scratch");
await mkdir(scratchRoot, { recursive: true });
const scratch = await mkdtemp(path.join(scratchRoot, "run-"));
const readyFile = path.join(scratch, "ready.txt");
const profile = path.join(scratch, "chrome-profile");

const host = spawn(hostExecutable, ["--ready-file", readyFile], { stdio: ["ignore", "pipe", "pipe"] });
let hostStdout = "";
let hostStderr = "";
host.stdout.on("data", chunk => { hostStdout += chunk; });
host.stderr.on("data", chunk => { hostStderr += chunk; });

let browser;
let cdp;
let browserStderr = "";
let frame = 0;
try {
  const port = Number(await waitForFile(readyFile, 15_000));
  assert.ok(Number.isInteger(port) && port > 0 && port <= 65535, "host returned an invalid port");
  const debugPort = await reserveDebugPort();
  browser = spawn(browserExecutable, [
    "--headless=new", "--no-sandbox", "--disable-gpu-sandbox", "--use-angle=swiftshader", "--enable-unsafe-swiftshader",
    "--no-first-run", "--no-default-browser-check",
    "--remote-allow-origins=*",
    `--remote-debugging-port=${debugPort}`, `--user-data-dir=${profile}`, "about:blank",
  ], { stdio: ["ignore", "pipe", "pipe"] });
  browser.stderr.on("data", chunk => { browserStderr += chunk; });
  const version = await waitForJson(`http://127.0.0.1:${debugPort}/json/version`, 15_000);
  const pageInfo = await fetch(`http://127.0.0.1:${debugPort}/json/new?about%3Ablank`, { method: "PUT" }).then(checkResponse).then(r => r.json());
  cdp = new Cdp(pageInfo.webSocketDebuggerUrl);
  await cdp.ready;
  await cdp.call("Page.enable");
  await cdp.call("Runtime.enable");
  await cdp.call("Page.navigate", { url: `http://127.0.0.1:${port}/#secret=${SECRET}` });
  await waitForEvaluation(cdp, "Boolean(window.__STASIS_CHARACTERIZATION__)", 15_000);
  await waitForEvaluation(cdp, "Boolean(window.__STASIS_CHARACTERIZATION__.networkTestMemory())", 15_000);
  await captureStage(cdp, "Browser runtime ready");

  assert.equal(await evaluate(cdp, "window.__STASIS_CHARACTERIZATION__.networkConnect()"), 0);
  await waitForEvaluation(cdp, "window.__STASIS_CHARACTERIZATION__.networkClient.state === 1", 10_000);
  const join = await receive(cdp);
  assert.deepEqual(join, { kind: "join_ack", seat: 0 });
  await captureStage(cdp, "Joined native host · ACK received");

  await send(cdp, { kind: "guest_command", command: "move", sequence: 1 });
  const snapshot = await receive(cdp);
  assert.deepEqual(snapshot, { kind: "snapshot", tick: 42, world: { players: 1 } });
  await captureStage(cdp, "Authoritative snapshot · tick 42");
  const command = await receive(cdp);
  assert.deepEqual(command, { kind: "native_command", command: "wave" });
  await send(cdp, { kind: "command_ack", command: "wave" });
  await captureStage(cdp, "Two-way command acknowledged");

  assert.equal(await evaluate(cdp, "window.__STASIS_CHARACTERIZATION__.networkCheckpoint(0, 1)"), 0);
  await evaluate(cdp, "window.__STASIS_CHARACTERIZATION__.networkClient.socket.close(); true");
  await waitForEvaluation(cdp, "window.__STASIS_CHARACTERIZATION__.networkClient.state === 0", 10_000);
  assert.equal(await evaluate(cdp, "window.__STASIS_CHARACTERIZATION__.networkConnect()"), 0);
  await waitForEvaluation(cdp, "window.__STASIS_CHARACTERIZATION__.networkClient.state === 1", 10_000);
  const reconnect = await receive(cdp);
  assert.deepEqual(reconnect, { kind: "reconnect_ack", resumed: true });
  await captureStage(cdp, "Reconnected · session resumed");

  const screenshot = await cdp.call("Page.captureScreenshot", { format: "png" });
  const screenshotBytes = Buffer.from(screenshot.data, "base64");
  assert.equal(screenshotBytes.includes(Buffer.from(SECRET)), false, "screenshot contains pairing secret");
  const visibleText = await evaluate(cdp, "document.body.innerText");
  assert.equal(visibleText.includes(SECRET), false, "visible browser content contains pairing secret");
  assert.doesNotMatch(visibleText, /stasis-resume-v1\.[0-9a-f]{32}/i, "visible browser content contains resume credential");
  await writeFile(path.join(evidenceRoot, "browser.png"), screenshotBytes);
  await encodeVideo(evidenceRoot);
  await send(cdp, { kind: "acceptance_complete" });

  const hostResult = await waitForExit(host, 10_000);
  assert.equal(hostResult.code, 0, `host failed: ${hostStderr}`);
  assert.match(hostStdout, /^BROWSER_NETWORK_ACCEPTANCE_OK\s*$/);
  const captured = `${hostStdout}\n${hostStderr}`;
  assert.equal(captured.includes(SECRET), false, "host output contains pairing secret");
  assert.doesNotMatch(captured, /stasis-resume-v1\.[0-9a-f]{32}/i, "host output contains resume credential");
  assert.equal(browserStderr.includes(SECRET), false, "browser output contains pairing secret");
  assert.doesNotMatch(browserStderr, /stasis-resume-v1\.[0-9a-f]{32}/i, "browser output contains resume credential");
  const evidence = {
    schema: "stasis.network_browser_acceptance.v1",
    browser: version.Browser,
    assertions: ["join_ack", "snapshot", "guest_to_native_command", "native_to_guest_command", "resume_same_handle", "secret_safe_capture"],
  };
  await writeFile(path.join(evidenceRoot, "result.json"), `${JSON.stringify(evidence, null, 2)}\n`);
  process.stdout.write("BROWSER_NETWORK_ACCEPTANCE_OK\n");
} catch (error) {
  if (cdp) {
    const pageStatus = await evaluate(cdp, `JSON.stringify({ loading: document.getElementById("stasis-loading")?.textContent, error: document.getElementById("stasis-error")?.textContent })`).catch(() => "");
    if (pageStatus) process.stderr.write(`page status: ${pageStatus.replaceAll(SECRET, "[REDACTED]")}\n`);
  }
  if (browserStderr) process.stderr.write(browserStderr.replaceAll(SECRET, "[REDACTED]"));
  if (hostStderr) process.stderr.write(hostStderr.replaceAll(SECRET, "[REDACTED]"));
  throw error;
} finally {
  cdp?.close();
  await terminate(browser);
  await terminate(host);
  await rm(scratch, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
}

function defaultBrowser() {
  if (process.platform !== "win32") return "";
  const roots = [process.env.PROGRAMFILES, process.env["PROGRAMFILES(X86)"]].filter(Boolean);
  for (const root of roots) {
    for (const relative of ["Google/Chrome/Application/chrome.exe", "Microsoft/Edge/Application/msedge.exe"]) {
      const candidate = path.join(root, relative);
      try { if (requireStat(candidate)) return candidate; } catch { /* try next */ }
    }
  }
  return "";
}

function requireStat(candidate) {
  return process.getBuiltinModule("node:fs").statSync(candidate).isFile();
}

async function reserveDebugPort() {
  const net = await import("node:net");
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const port = server.address().port;
      server.close(error => error ? reject(error) : resolve(port));
    });
  });
}

async function waitForFile(file, timeout) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    try { return await readFile(file, "utf8"); } catch { await delay(25); }
  }
  throw new Error("native host did not become ready");
}

async function waitForJson(url, timeout) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    try { return await fetch(url).then(checkResponse).then(r => r.json()); } catch { await delay(50); }
  }
  throw new Error("browser debugging endpoint did not become ready");
}

function checkResponse(response) {
  if (!response.ok) throw new Error(`HTTP ${response.status}`);
  return response;
}

async function waitForEvaluation(cdp, expression, timeout) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    if (await evaluate(cdp, expression).catch(() => false)) return;
    await delay(25);
  }
  throw new Error(`browser condition timed out: ${expression}`);
}

async function evaluate(cdp, expression) {
  const result = await cdp.call("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true });
  if (result.exceptionDetails) throw new Error(result.exceptionDetails.text);
  return result.result.value;
}

async function send(cdp, value) {
  const json = JSON.stringify(value);
  const expression = `(() => { const api = window.__STASIS_CHARACTERIZATION__; const bytes = new TextEncoder().encode(${JSON.stringify(json)}); const memory = api.networkTestMemory(); new Uint8Array(memory.buffer, 0, bytes.length).set(bytes); return api.networkSend(7, bytes.length); })()`;
  assert.equal(await evaluate(cdp, expression), 0);
}

async function receive(cdp) {
  const expression = `(() => { const api = window.__STASIS_CHARACTERIZATION__; const memory = api.networkTestMemory(); const length = api.networkPoll(7, 65536); if (length <= 0) return null; return new TextDecoder().decode(new Uint8Array(memory.buffer, 0, length)); })()`;
  const deadline = Date.now() + 10_000;
  while (Date.now() < deadline) {
    const value = await evaluate(cdp, expression);
    if (value !== null) return JSON.parse(value);
    await delay(10);
  }
  throw new Error("browser did not receive network payload");
}

function waitForExit(child, timeout) {
  if (child.exitCode !== null) return Promise.resolve({ code: child.exitCode });
  return Promise.race([
    new Promise(resolve => child.once("exit", code => resolve({ code }))),
    delay(timeout).then(() => { throw new Error("native host did not exit"); }),
  ]);
}

function delay(ms) { return new Promise(resolve => setTimeout(resolve, ms)); }

async function captureStage(cdp, label) {
  await evaluate(cdp, `(() => { const item = document.createElement("li"); item.textContent = ${JSON.stringify(label)}; document.getElementById("acceptance-progress").append(item); return true; })()`);
  const capture = await cdp.call("Page.captureScreenshot", { format: "png" });
  frame += 1;
  await writeFile(path.join(evidenceRoot, `frame-${String(frame).padStart(2, "0")}.png`), Buffer.from(capture.data, "base64"));
}

async function terminate(child) {
  if (!child || child.exitCode !== null) return;
  child.kill();
  await Promise.race([
    new Promise(resolve => child.once("exit", resolve)),
    delay(2_000),
  ]);
}
