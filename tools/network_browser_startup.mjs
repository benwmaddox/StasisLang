import { readFile } from "node:fs/promises";
import path from "node:path";
import { setTimeout as delay } from "node:timers/promises";

export async function waitForBrowserEndpoint(browser, profile, timeout) {
  const deadline = performance.now() + timeout;
  let launchFailed = false;
  const onError = () => { launchFailed = true; };
  browser.once("error", onError);
  try {
    while (performance.now() < deadline) {
      if (launchFailed) throw new Error("browser could not be launched");
      if (browser.exitCode !== null || browser.signalCode !== null) {
        throw new Error("browser exited before its debugging endpoint became ready");
      }
      try {
        const activePort = await readFile(path.join(profile, "DevToolsActivePort"), "utf8");
        const portLine = activePort.split(/\r?\n/)[0];
        const port = Number(portLine);
        if (/^\d+$/.test(portLine) && port > 0 && port <= 65535) {
          const remaining = Math.floor(deadline - performance.now());
          if (remaining <= 0) break;
          const response = await fetch(`http://127.0.0.1:${port}/json/version`, {
            signal: AbortSignal.timeout(Math.min(500, remaining)),
          });
          if (response.ok) return { port, version: await response.json() };
        }
      } catch { /* Chromium may still be publishing the file or starting HTTP. */ }
      await delay(Math.min(50, Math.max(0, deadline - performance.now())));
    }
    throw new Error("browser debugging endpoint did not become ready within the startup bound");
  } finally {
    browser.removeListener("error", onError);
  }
}
