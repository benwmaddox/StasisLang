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

test("the sole WebGL2 runtime keeps loading visible, hides only when ready, and retains failure", () => {
  const source = read("game.js");
  assert.match(source, /const loadingStatus = document\.getElementById\("stasis-loading-status"\)/);
  assert.match(source, /if \(loadingStatus\) loadingStatus\.textContent = message;/);
  assert.match(source, /else loadingBox\.textContent = message;/);
  assert.match(source, /setLoading\("Preparing…", "loading"\)/);
  assert.match(source, /dataset\.hidden = state === "ready" \? "true" : "false"/);
  assert.match(source, /dataset\.failed = state === "failed" \? "true" : "false"/);
  assert.match(source, /setLoading\("", "ready"\)/);
  assert.match(source, /setLoading\(`Unable to start this game\./);
  assert.match(source, /WebGL2 is required by the Stasis Web renderer/);
});
