import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { mkdir, mkdtemp, readFile, rm } from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { startBrowserWithRetry, waitForBrowserEndpoint } from "../network_browser_startup.mjs";

const fixture = `
  const http = require('node:http');
  const fs = require('node:fs');
  const path = require('node:path');
  const [profile, mode] = process.argv.slice(1);
  if (mode === 'exit') process.exit(23);
  http.createServer((request, response) => {
    if (mode === 'hang') {
      fs.writeFileSync(path.join(profile, 'request-received'), '1');
      return;
    }
    response.end(JSON.stringify({ Browser: 'startup-fixture' }));
  }).listen(0, '127.0.0.1', function () {
    const portFile = path.join(profile, 'DevToolsActivePort');
    fs.writeFileSync(portFile, 'incomplete');
    setTimeout(() => fs.writeFileSync(portFile, this.address().port + '\\n/devtools/browser/fixture'), 50);
  });
`;

async function startFixture(t, mode) {
  const root = path.resolve("target/network-browser-startup-tests");
  await mkdir(root, { recursive: true });
  const profile = await mkdtemp(path.join(root, "case-"));
  const child = spawn(process.execPath, ["-e", fixture, profile, mode], { stdio: "ignore" });
  t.after(async () => {
    if (child.exitCode === null && child.signalCode === null) {
      const closed = once(child, "close");
      child.kill();
      await closed;
    }
    await rm(profile, { recursive: true, force: true });
  });
  return { child, profile };
}

test("concurrent children publish their own allocated debugging ports", { timeout: 10_000 }, async t => {
  const first = await startFixture(t, "ready");
  const second = await startFixture(t, "ready");
  const endpoints = await Promise.all([first, second].map(({ child, profile }) =>
    waitForBrowserEndpoint(child, profile, 5_000)));
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
