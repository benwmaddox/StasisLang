import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { mkdir, mkdtemp, open, readFile, rm } from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { startBrowserWithRetry, waitForBrowserEndpoint } from "../network_browser_startup.mjs";

const fixture = `
  const http = require('node:http');
  const fs = require('node:fs');
  const path = require('node:path');
  const [profile, mode] = process.argv.slice(1);
  const trace = (phase, details = {}) => process.stderr.write(JSON.stringify({ phase, pid: process.pid, ...details }) + '\\n');
  trace('start', { mode });
  if (mode === 'exit') process.exit(23);
  http.createServer((request, response) => {
    trace('request', { url: request.url });
    if (mode === 'hang') {
      fs.writeFileSync(path.join(profile, 'request-received'), '1');
      return;
    }
    response.end(JSON.stringify({ Browser: 'startup-fixture' }), () => trace('response', { status: response.statusCode }));
  }).listen(0, '127.0.0.1', function () {
    const portFile = path.join(profile, 'DevToolsActivePort');
    trace('listen', { address: this.address().address, port: this.address().port });
    fs.writeFileSync(portFile, 'incomplete');
    trace('port-incomplete');
    setTimeout(() => {
      const port = this.address().port;
      fs.writeFileSync(portFile, port + '\\n/devtools/browser/fixture');
      trace('port-written', { port });
    }, 50);
  });
`;

const maxCapturedStderrBytes = 4 * 1024;
const maxRenderedChildDiagnosticBytes = 6 * 1024;

function captureFixtureStderr(child) {
  let text = "";
  let byteLength = 0;
  let truncated = false;
  child.stderr.setEncoding("utf8");
  child.stderr.on("data", chunk => {
    if (byteLength >= maxCapturedStderrBytes) {
      truncated = true;
      return;
    }
    const bytes = Buffer.from(chunk, "utf8");
    const retained = bytes.subarray(0, maxCapturedStderrBytes - byteLength);
    text += retained.toString("utf8");
    byteLength += retained.length;
    truncated ||= retained.length < bytes.length;
  });
  return () => ({ text, truncated, byteLength });
}

async function readPortFileBounded(profile) {
  let handle;
  try {
    handle = await open(path.join(profile, "DevToolsActivePort"), "r");
    const bytes = Buffer.alloc(257);
    const { bytesRead } = await handle.read(bytes, 0, bytes.length, 0);
    return {
      state: "present",
      value: bytes.subarray(0, Math.min(bytesRead, 256)).toString("utf8"),
      truncated: bytesRead > 256,
    };
  } catch (error) {
    return error.code === "ENOENT"
      ? { state: "missing" }
      : { state: "read-error", code: String(error.code || "unknown").slice(0, 64) };
  } finally {
    await handle?.close().catch(() => {});
  }
}

async function diagnoseFailedChildren(t, children, startedAt, error) {
  const snapshots = await Promise.all(children.map(async ({ child, profile, stderrSnapshot }, index) => ({
    index: index + 1,
    pid: child.pid ?? null,
    exitCode: child.exitCode,
    signalCode: child.signalCode,
    elapsedMs: Math.round(performance.now() - startedAt),
    portFile: await readPortFileBounded(profile),
    stderr: stderrSnapshot(),
    failure: String(error?.message || error).slice(0, 256),
  })));

  for (const snapshot of snapshots) {
    let diagnostic = JSON.stringify(snapshot);
    while (Buffer.byteLength(diagnostic, "utf8") > maxRenderedChildDiagnosticBytes && snapshot.stderr.text.length > 0) {
      snapshot.stderr.text = snapshot.stderr.text.slice(0, Math.floor(snapshot.stderr.text.length / 2));
      snapshot.stderr.truncated = true;
      snapshot.stderr.byteLength = Buffer.byteLength(snapshot.stderr.text, "utf8");
      diagnostic = JSON.stringify(snapshot);
    }
    t.diagnostic(`startup fixture failure child ${snapshot.index}: ${diagnostic}`);
  }
}

async function startFixture(t, mode) {
  const root = path.resolve("target/network-browser-startup-tests");
  await mkdir(root, { recursive: true });
  const profile = await mkdtemp(path.join(root, "case-"));
  const child = spawn(process.execPath, ["-e", fixture, profile, mode], { stdio: ["ignore", "ignore", "pipe"] });
  const stderrSnapshot = captureFixtureStderr(child);
  t.after(async () => {
    if (child.exitCode === null && child.signalCode === null) {
      const closed = once(child, "close");
      child.kill();
      await closed;
    }
    await rm(profile, { recursive: true, force: true });
  });
  return { child, profile, stderrSnapshot };
}

test("concurrent children publish their own allocated debugging ports", { timeout: 10_000 }, async t => {
  const startedAt = performance.now();
  const first = await startFixture(t, "ready");
  const second = await startFixture(t, "ready");
  let endpoints;
  try {
    endpoints = await Promise.all([first, second].map(({ child, profile }) =>
      waitForBrowserEndpoint(child, profile, 5_000)));
  } catch (error) {
    try {
      await diagnoseFailedChildren(t, [first, second], startedAt, error);
    } catch (diagnosticError) {
      try {
        t.diagnostic(`startup fixture diagnostics unavailable: ${String(diagnosticError?.message || diagnosticError).slice(0, 256)}`);
      } catch { /* Preserve the original endpoint error if the reporter also fails. */ }
    }
    throw error;
  }
  assert.notEqual(endpoints[0].port, endpoints[1].port);
  for (const endpoint of endpoints) assert.equal(endpoint.version.Browser, "startup-fixture");
});

test("browser exit fails before the startup deadline", { timeout: 5_000 }, async t => {
  const { child, profile } = await startFixture(t, "exit");
  await assert.rejects(waitForBrowserEndpoint(child, profile, 10_000), /browser exited/);
});

test("a hanging debugging HTTP endpoint cannot exceed the startup bound", { timeout: 5_000 }, async t => {
  const { child, profile } = await startFixture(t, "hang");
  const start = performance.now();
  await assert.rejects(waitForBrowserEndpoint(child, profile, 1_000), /startup bound/);
  assert.ok(performance.now() - start < 3_000);
  assert.equal(await readFile(path.join(profile, "request-received"), "utf8"), "1");
});

test("a launch error is reported without an unhandled child error", { timeout: 5_000 }, async () => {
  const child = spawn(path.resolve("target/nonexistent-browser-startup-fixture.exe"), [], { stdio: "ignore" });
  await assert.rejects(waitForBrowserEndpoint(child, "", 10_000), /could not be launched/);
});

test("a hung browser is stopped before a clean-profile retry", { timeout: 5_000 }, async t => {
  const first = await startFixture(t, "hang");
  const second = await startFixture(t, "ready");
  const launches = [first, second];
  const stopped = [];
  const result = await startBrowserWithRetry(
    attempt => ({ browser: launches[attempt - 1].child, profile: launches[attempt - 1].profile }),
    async browser => {
      stopped.push(browser);
      if (browser.exitCode === null && browser.signalCode === null) {
        const closed = once(browser, "close");
        browser.kill();
        await closed;
      }
    },
    { attempts: 2, timeout: 500 },
  );
  assert.equal(stopped.length, 1);
  assert.equal(stopped[0], first.child);
  assert.equal(result.browser, second.child);
  assert.equal(result.version.Browser, "startup-fixture");
});
