import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";

const root = new URL("../", import.meta.url);
const read = name => fs.readFileSync(new URL(name, root), "utf8");

test("web package template exposes an immediate accessible loading status", () => {
  const html = read("index.html");
  assert.match(html, /id="stasis-loading"/);
  assert.match(html, /role="status"/);
  assert.match(html, /aria-live="polite"/);
  assert.match(html, /id="stasis-loading-title">__STASIS_GAME_TITLE__<\/h1>/);
  assert.match(html, /id="stasis-loading-status">Preparing…<\/div>/);
  assert.match(html, /position: fixed; inset: 0;/);
  assert.match(html, /body \{[^}]*overflow: hidden;/);
  assert.match(html, /data-hidden="true"/);
});

test("web runtimes keep loading visible, hide only when ready, and retain failure", () => {
  for (const name of ["game.js", "game_minimal.js"]) {
    const source = read(name);
    assert.match(source, /const loadingStatus = document\.getElementById\("stasis-loading-status"\)/);
    assert.match(source, /if \(loadingStatus\) loadingStatus\.textContent = message;/);
    assert.match(source, /else loadingBox\.textContent = message;/);
    assert.match(source, /setLoading\("Preparing…", "loading"\)/);
    assert.match(source, /dataset\.hidden = state === "ready" \? "true" : "false"/);
    assert.match(source, /dataset\.failed = state === "failed" \? "true" : "false"/);
    assert.match(source, /setLoading\("", "ready"\)/);
    assert.match(source, /setLoading\(`Unable to start this game\./);
    assert.match(source, /, "failed"\)/);
  }
});
