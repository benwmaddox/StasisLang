const { test, expect } = require("@playwright/test");
const http = require("http");
const fs = require("fs");
const path = require("path");
const { spawnSync } = require("child_process");

function repoRoot() {
  return path.resolve(__dirname, "..", "..");
}

function contentType(p) {
  if (p.endsWith(".html")) return "text/html; charset=utf-8";
  if (p.endsWith(".js")) return "text/javascript; charset=utf-8";
  if (p.endsWith(".wasm")) return "application/wasm";
  if (p.endsWith(".svg")) return "image/svg+xml";
  if (p.endsWith(".json")) return "application/json; charset=utf-8";
  if (p.endsWith(".css")) return "text/css; charset=utf-8";
  return "application/octet-stream";
}

function startStaticServer(root) {
  const rootResolved = path.resolve(root);
  const server = http.createServer((req, res) => {
    let urlPath = String(req.url || "/").split("?")[0];
    try {
      urlPath = decodeURIComponent(urlPath);
    } catch {
      // ignore
    }
    if (urlPath.endsWith("/")) urlPath += "index.html";

    const fsPath = path.resolve(path.join(rootResolved, "." + urlPath));
    if (!fsPath.startsWith(rootResolved)) {
      res.writeHead(403);
      res.end("forbidden");
      return;
    }

    fs.readFile(fsPath, (err, data) => {
      if (err) {
        res.writeHead(404);
        res.end("not found");
        return;
      }
      res.writeHead(200, { "Content-Type": contentType(fsPath) });
      res.end(data);
    });
  });

  return new Promise((resolve, reject) => {
    server.on("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const addr = server.address();
      resolve({ server, port: addr.port });
    });
  });
}

test("brickout wasm starts and ticks without errors", async ({ page }) => {
  const root = repoRoot();

  const build = spawnSync(
    "powershell",
    ["-ExecutionPolicy", "Bypass", "-File", path.join(root, "scripts", "build-wasm-brickout.ps1")],
    { cwd: root, encoding: "utf8" }
  );
  expect(build.status, `build failed:\n${build.stdout}\n${build.stderr}`).toBe(0);

  const wasmPath = path.join(
    root,
    "examples",
    "wasm",
    "brickout_revenge_v1",
    "brickout_revenge_v1.wasm"
  );
  expect(fs.existsSync(wasmPath), "wasm output missing").toBeTruthy();

  const { server, port } = await startStaticServer(root);
  const baseURL = `http://127.0.0.1:${port}`;

  const errors = [];
  page.on("pageerror", (e) => errors.push(`pageerror: ${String(e && e.stack ? e.stack : e)}`));
  page.on("console", (msg) => {
    if (msg.type() === "error") errors.push(`console.error: ${msg.text()}`);
  });

  try {
    await page.goto(`${baseURL}/examples/wasm/brickout_revenge_v1/`, { waitUntil: "load" });
    await page.getByRole("button", { name: "Start" }).click();

    await expect(page.locator("#log")).toContainText("main() -> 0", { timeout: 20_000 });
    await page.waitForFunction(() => window.__stasisWasmHost && window.__stasisWasmHost.tickCount >= 20, null, {
      timeout: 20_000
    });

    expect(errors, `browser errors:\n${errors.join("\n")}`).toEqual([]);
  } finally {
    await new Promise((r) => server.close(r));
  }
});

