import test from "node:test";
import assert from "node:assert/strict";
import { loadRuntime } from "./asset_paths.test.mjs";

test("web dynamic text replacement reuses a bounded handle and ignores stale font work", async () => {
  const game = {
    memory: {
      "run.font": { offset: 0, length: 1, stride: 4, type_id: 1 },
      "run.handle": { offset: 4, length: 1, stride: 4, type_id: 1 },
      "run.width": { offset: 8, length: 1, stride: 4, type_id: 2 },
      "run.height": { offset: 12, length: 1, stride: 4, type_id: 2 },
      dynamic_text: { hash: 202, offset: 32, length: 32, stride: 1, type_id: 5, byte_backed: true },
    },
    globals: {
      "dynamic_text.length": { hash: 203, type_id: 1 },
    },
    views: {
      "101": { font: "run.font", handle: "run.handle", width: "run.width", height: "run.height" },
    },
    strings: { "1": "assets/font.ttf", "2": "fixed" },
  };
  let stableHandle = 0;
  let dynamicTextLength = 0;
  const result = await loadRuntime(game, {
    globalGetI32: hash => hash === 203 ? dynamicTextLength : 0,
    main: (env, memory) => {
      const font = env.load_font(1, 20);
      const fixed = env.stasis_jit_gfx_cache_text(font, 2);
      assert.equal(env.stasis_jit_gfx_cache_text(font, 2), fixed);
      assert.equal(env.stasis_jit_text_run_load_from(101, 0, 1, font, 2), 1);
      for (let value = 0; value < 5000; value += 1) {
        game.strings["2"] = `score ${value}`;
        assert.equal(env.stasis_jit_text_run_replace_from(101, 0, 1, font, 2), 1);
      }
      const view = new DataView(memory.buffer);
      stableHandle = view.getInt32(4, true);
      assert.notEqual(stableHandle, fixed);
      const dynamicBytes = new Uint8Array(memory.buffer, 32, 32);
      const localized = new TextEncoder().encode("Punktzahl 7");
      dynamicBytes.set(localized);
      dynamicTextLength = localized.length;
      assert.equal(env.stasis_jit_text_run_replace_from(101, 0, 1, font, 202), 1);
      assert.equal(view.getInt32(4, true), stableHandle);
      dynamicBytes.set(new TextEncoder().encode("OK"));
      dynamicTextLength = 2;
      assert.equal(env.stasis_jit_text_run_replace_from(101, 0, 1, font, 202), 1);
      assert.equal(view.getInt32(4, true), stableHandle);
      const before = [view.getInt32(0, true), view.getInt32(4, true), view.getFloat32(8, true), view.getFloat32(12, true)];
      dynamicBytes.set([0xc3, 0x28]);
      dynamicTextLength = 2;
      assert.equal(env.stasis_jit_text_run_replace_from(101, 0, 1, font, 202), 0);
      assert.deepEqual(
        [view.getInt32(0, true), view.getInt32(4, true), view.getFloat32(8, true), view.getFloat32(12, true)],
        before,
      );
      game.strings["2"] = "x".repeat(4097);
      assert.equal(env.stasis_jit_text_run_replace_from(101, 0, 1, font, 2), 0);
      assert.deepEqual(
        [view.getInt32(0, true), view.getInt32(4, true), view.getFloat32(8, true), view.getFloat32(12, true)],
        before,
      );
    },
    measureText: ({ font, value }) => font.startsWith("1000px")
      ? { width: 500, fontBoundingBoxAscent: 800, fontBoundingBoxDescent: 200 }
      : { width: value === "Punktzahl 7" ? 91 : value.length * 7, actualBoundingBoxDescent: 2 },
  });
  const { memory } = result;
  await result.runtimePromise;
  const view = new DataView(memory.buffer);
  assert.equal(view.getInt32(4, true), stableHandle);
  assert.equal(view.getFloat32(8, true), 14);
  assert.equal(view.getFloat32(12, true), 18);
  assert.ok(result.measurements.some(({ value }) => value === "OK"));
  assert.ok(result.measurements.every(({ value }) => value !== "OKnktzahl 7"));
});

test("web dynamic text buffer wins over a colliding string literal handle", async () => {
  const game = {
    memory: {
      "run.font": { offset: 0, length: 1, stride: 4, type_id: 1 },
      "run.handle": { offset: 4, length: 1, stride: 4, type_id: 1 },
      "run.width": { offset: 8, length: 1, stride: 4, type_id: 2 },
      "run.height": { offset: 12, length: 1, stride: 4, type_id: 2 },
      dynamic_text: { hash: 202, offset: 32, length: 32, stride: 1, type_id: 5, byte_backed: true },
    },
    globals: {
      "dynamic_text.length": { hash: 203, type_id: 1 },
    },
    views: {
      "101": { font: "run.font", handle: "run.handle", width: "run.width", height: "run.height" },
    },
    strings: { "1": "assets/font.ttf", "202": "literal collision" },
  };
  const encoded = new TextEncoder().encode("buffer wins");
  let currentLength = encoded.length;
  const result = await loadRuntime(game, {
    globalGetI32: hash => hash === 203 ? currentLength : 0,
    main: (env, memory) => {
      const dynamicBytes = new Uint8Array(memory.buffer, 32, 32);
      dynamicBytes.set(encoded);
      const font = env.load_font(1, 20);
      assert.equal(env.stasis_jit_text_run_replace_from(101, 0, 1, font, 202), 1);
      const view = new DataView(memory.buffer);
      const before = [view.getInt32(0, true), view.getInt32(4, true), view.getFloat32(8, true), view.getFloat32(12, true)];
      dynamicBytes.set([0xc3, 0x28]);
      currentLength = 2;
      assert.equal(env.stasis_jit_text_run_replace_from(101, 0, 1, font, 202), 0);
      assert.deepEqual(
        [view.getInt32(0, true), view.getInt32(4, true), view.getFloat32(8, true), view.getFloat32(12, true)],
        before,
      );
    },
  });
  await result.runtimePromise;
  assert.ok(result.measurements.some(({ value }) => value === "buffer wins"));
  assert.ok(result.measurements.every(({ value }) => value !== "literal collision"));
});

test("web stale immutable font readiness cannot overwrite a replacement receiver", async () => {
  const game = {
    memory: {
      "run.font": { offset: 0, length: 1, stride: 4, type_id: 1 },
      "run.handle": { offset: 4, length: 1, stride: 4, type_id: 1 },
      "run.width": { offset: 8, length: 1, stride: 4, type_id: 2 },
      "run.height": { offset: 12, length: 1, stride: 4, type_id: 2 },
      dynamic_text: { hash: 202, offset: 32, length: 32, stride: 1, type_id: 5, byte_backed: true },
    },
    globals: {
      "dynamic_text.length": { hash: 203, type_id: 1 },
    },
    views: {
      "101": { font: "run.font", handle: "run.handle", width: "run.width", height: "run.height" },
    },
    strings: {
      "1": "assets/font-b.ttf",
      "2": "assets/font-a.ttf",
      "3": "stale immutable",
    },
  };
  const releases = new Map();
  const replacement = new TextEncoder().encode("replacement B");
  let replacementFont = 0;
  let replacementHandle = 0;
  const result = await loadRuntime(game, {
    globalGetI32: hash => hash === 203 ? replacement.length : 0,
    fontLoad: font => new Promise(resolve => releases.set(font.source, () => resolve(font))),
    main: (env, memory) => {
      replacementFont = env.load_font(1, 30);
      const staleFont = env.load_font(2, 20);
      assert.equal(env.stasis_jit_text_run_load_from(101, 0, 1, staleFont, 3), 1);
      new Uint8Array(memory.buffer, 32, 32).set(replacement);
      assert.equal(env.stasis_jit_text_run_replace_from(101, 0, 1, replacementFont, 202), 1);
      replacementHandle = new DataView(memory.buffer).getInt32(4, true);
    },
    measureText: ({ font, value }) => font.startsWith("1000px")
      ? { width: 500, fontBoundingBoxAscent: 800, fontBoundingBoxDescent: 200 }
      : {
          width: font.includes("stasis-font-1") ? 222 : 111,
          actualBoundingBoxDescent: value.length > 0 ? 2 : 0,
        },
  });
  assert.equal(releases.size, 2);
  releases.get("url(assets/font-b.ttf)")();
  await new Promise(resolve => setImmediate(resolve));
  releases.get("url(assets/font-a.ttf)")();
  await result.runtimePromise;

  const view = new DataView(result.memory.buffer);
  assert.equal(view.getInt32(0, true), replacementFont);
  assert.equal(view.getInt32(4, true), replacementHandle);
  assert.equal(view.getFloat32(8, true), 222);
});
