import test from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { mkdtemp, rm } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import path from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { startBrowserWithRetry } from "../../../tools/network_browser_startup.mjs";

const requestedBrowser = process.env.STASIS_BROWSER_EXECUTABLE;
const browser = requestedBrowser
  || (process.platform === "win32" ? "C:/Program Files/Google/Chrome/Application/chrome.exe"
    : process.platform === "darwin" ? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
      : ["/usr/bin/google-chrome", "/usr/bin/chromium", "/usr/bin/chromium-browser"].find(existsSync));
const browserAvailable = browser && existsSync(browser);
const template = readFileSync(new URL("../index.html", import.meta.url), "utf8");
const game = template
  .replaceAll("__STASIS_LOADING_FONT_FACE__", "")
  .replaceAll("__STASIS_GAME_TITLE__", "Stasis embed fit")
  .replaceAll("__STASIS_LOGICAL_WIDTH__", "1600")
  .replaceAll("__STASIS_LOGICAL_HEIGHT__", "720")
  .replaceAll("__STASIS_PERFORMANCE_HUD_STYLE__", "")
  .replaceAll("__STASIS_PERFORMANCE_HUD__", "")
  .replaceAll("__STASIS_GAME_SCRIPT__", "data:text/javascript,");
const embed = '<!doctype html><html><body style="margin:0"><iframe id="game" src="/index.html" style="display:block;width:1200px;height:800px;border:0"></iframe></body></html>';

test("real Chrome fits 1600x720 in an embedded iframe after resize", {
  skip: browserAvailable || requestedBrowser ? false
    : "Chrome unavailable; set STASIS_BROWSER_EXECUTABLE to run the browser fit test",
  timeout: 150_000
}, async () => {
  assert.ok(browserAvailable, `Chrome executable missing: ${browser}`);
  const server = createServer((request, response) => {
    const pathname = new URL(request.url, "http://localhost").pathname;
    if (pathname === "/embed.html" || pathname === "/index.html") {
      response.setHeader("Content-Type", "text/html; charset=utf-8");
      response.end(pathname === "/embed.html" ? embed : game);
    } else {
      response.writeHead(404).end();
    }
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  const profile = await mkdtemp(path.join(tmpdir(), "stasis-embed-fit-"));
  let chrome;
  let socket;
  try {
    const started = await startBrowserWithRetry(attempt => {
      const candidateProfile = path.join(profile, `chrome-${attempt}`);
      const candidate = spawn(browser, [
        "--headless=new", "--no-sandbox", "--disable-gpu-sandbox",
        "--no-first-run", "--no-default-browser-check", "--remote-debugging-port=0",
        `--user-data-dir=${candidateProfile}`, "about:blank"
      ], { stdio: "ignore", windowsHide: true });
      return { browser: candidate, profile: candidateProfile };
    }, candidate => {
      candidate.kill();
    }, { attempts: 2, timeout: 60_000 });
    chrome = started.browser;
    const { port } = started;
    const pages = await fetch(`http://127.0.0.1:${port}/json/list`).then(response => response.json());
    const page = pages.find(candidate => candidate.type === "page");
    assert.ok(page, "Chrome exposes a page target");
    socket = new WebSocket(page.webSocketDebuggerUrl);
    await new Promise((resolve, reject) => {
      socket.addEventListener("open", resolve, { once: true });
      socket.addEventListener("error", reject, { once: true });
    });

    let nextId = 0;
    const pending = new Map();
    socket.addEventListener("message", event => {
      const message = JSON.parse(event.data);
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
      }, 10_000);
      pending.set(id, message => {
        clearTimeout(timer);
        message.error ? reject(new Error(message.error.message)) : resolve(message.result);
      });
      socket.send(JSON.stringify({ id, method, params }));
    });
    const evaluate = async expression => {
      const result = await call("Runtime.evaluate", {
        expression, returnByValue: true, awaitPromise: true
      });
      if (result.exceptionDetails) throw new Error(result.exceptionDetails.text);
      return result.result.value;
    };
    const measure = () => evaluate(`(() => {
      const frame = document.getElementById("game");
      const doc = frame?.contentDocument;
      const canvas = doc?.getElementById("stasis-canvas");
      if (!canvas || !frame.contentWindow.STASIS_AVAILABLE_VIEWPORT) return null;
      const shell = canvas.parentElement.getBoundingClientRect();
      const surface = canvas.getBoundingClientRect();
      return {
        frame: [frame.clientWidth, frame.clientHeight],
        shell: [shell.width, shell.height],
        canvas: [surface.width, surface.height],
        logical: [canvas.dataset.logicalWidth, canvas.dataset.logicalHeight],
        available: frame.contentWindow.STASIS_AVAILABLE_VIEWPORT
      };
    })()`);
    const until = async predicate => {
      for (let attempt = 0; attempt < 200; attempt++) {
        const value = await measure();
        if (value && predicate(value)) return value;
        await delay(25);
      }
      throw new Error("embedded frame did not refit within 5 seconds");
    };

    await call("Page.navigate", {
      url: `http://127.0.0.1:${server.address().port}/embed.html`
    });
    const initial = await until(value => value.shell[0] === 1200);
    assert.deepEqual(initial.logical, ["1600", "720"]);
    assert.deepEqual(initial.frame, [1200, 800]);
    assert.deepEqual(initial.shell, [1200, 540]);
    assert.deepEqual(initial.canvas, initial.shell);
    assert.deepEqual(initial.available, { width: 1200, height: 800 });

    await evaluate('document.getElementById("game").style.cssText = "display:block;width:1920px;height:1080px;border:0"');
    const desktop = await until(value => value.shell[0] === 1920);
    assert.deepEqual(desktop.frame, [1920, 1080]);
    assert.deepEqual(desktop.shell, [1920, 864]);
    assert.deepEqual(desktop.canvas, desktop.shell);
    assert.deepEqual(desktop.available, { width: 1920, height: 1080 });

    await evaluate('document.getElementById("game").style.cssText = "display:block;width:844px;height:390px;border:0"');
    const landscape = await until(value => value.shell[0] === 844);
    assert.deepEqual(landscape.frame, [844, 390]);
    assert.ok(Math.abs(landscape.shell[1] - 379.8) < 0.1);
    assert.deepEqual(landscape.canvas, landscape.shell);
    assert.deepEqual(landscape.available, { width: 844, height: 390 });
  } finally {
    socket?.close();
    chrome?.kill();
    await new Promise(resolve => server.close(resolve));
    await rm(profile, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
  }
});
