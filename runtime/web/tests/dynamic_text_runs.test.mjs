import test from "node:test";
import assert from "node:assert/strict";
import { loadRuntime } from "./asset_paths.test.mjs";

const ASSET_STATE_NONE = 0;
const ASSET_STATE_LOADING = 2;
const ASSET_STATE_LOADED = 3;
const ASSET_STATE_FAILED = 4;

test("web dynamic text replacement reuses a bounded handle and ignores stale font work", async () => {
  const game = {
    memory: {
      "run.font": { hash: 101, handle: 1101, offset: 0, length: 1, stride: 4, type_id: 1 },
      "run.handle": { hash: 102, handle: 1102, offset: 4, length: 1, stride: 4, type_id: 1 },
      "run.width": { hash: 103, handle: 1103, offset: 8, length: 1, stride: 4, type_id: 2 },
      "run.height": { hash: 104, handle: 1104, offset: 12, length: 1, stride: 4, type_id: 2 },
      dynamic_text: { hash: 202, handle: 1202, offset: 32, length: 32, stride: 1, type_id: 5, byte_backed: true },
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
      assert.equal(env.stasis_jit_text_run_replace_from(101, 0, 1, font, 1202), 1);
      assert.equal(view.getInt32(4, true), stableHandle);
      dynamicBytes.set(new TextEncoder().encode("OK"));
      dynamicTextLength = 2;
      assert.equal(env.stasis_jit_text_run_replace_from(101, 0, 1, font, 1202), 1);
      assert.equal(view.getInt32(4, true), stableHandle);
      const before = [view.getInt32(0, true), view.getInt32(4, true), view.getFloat32(8, true), view.getFloat32(12, true)];
      dynamicBytes.set([0xc3, 0x28]);
      dynamicTextLength = 2;
      assert.equal(env.stasis_jit_text_run_replace_from(101, 0, 1, font, 1202), 0);
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
      "run.font": { hash: 101, handle: 1101, offset: 0, length: 1, stride: 4, type_id: 1 },
      "run.handle": { hash: 102, handle: 1102, offset: 4, length: 1, stride: 4, type_id: 1 },
      "run.width": { hash: 103, handle: 1103, offset: 8, length: 1, stride: 4, type_id: 2 },
      "run.height": { hash: 104, handle: 1104, offset: 12, length: 1, stride: 4, type_id: 2 },
      dynamic_text: { hash: 202, handle: 1202, offset: 32, length: 32, stride: 1, type_id: 5, byte_backed: true },
    },
    globals: {
      "dynamic_text.length": { hash: 203, type_id: 1 },
    },
    views: {
      "101": { font: "run.font", handle: "run.handle", width: "run.width", height: "run.height" },
    },
    strings: { "1": "assets/font.ttf", "1202": "literal collision" },
  };
  const encoded = new TextEncoder().encode("buffer wins");
  let currentLength = encoded.length;
  const result = await loadRuntime(game, {
    globalGetI32: hash => hash === 203 ? currentLength : 0,
    main: (env, memory) => {
      const dynamicBytes = new Uint8Array(memory.buffer, 32, 32);
      dynamicBytes.set(encoded);
      const font = env.load_font(1, 20);
      assert.equal(env.stasis_jit_text_run_replace_from(101, 0, 1, font, 1202), 1);
      const view = new DataView(memory.buffer);
      const before = [view.getInt32(0, true), view.getInt32(4, true), view.getFloat32(8, true), view.getFloat32(12, true)];
      dynamicBytes.set([0xc3, 0x28]);
      currentLength = 2;
      assert.equal(env.stasis_jit_text_run_replace_from(101, 0, 1, font, 1202), 0);
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
      "run.font": { hash: 101, handle: 1101, offset: 0, length: 1, stride: 4, type_id: 1 },
      "run.handle": { hash: 102, handle: 1102, offset: 4, length: 1, stride: 4, type_id: 1 },
      "run.width": { hash: 103, handle: 1103, offset: 8, length: 1, stride: 4, type_id: 2 },
      "run.height": { hash: 104, handle: 1104, offset: 12, length: 1, stride: 4, type_id: 2 },
      dynamic_text: { hash: 202, handle: 1202, offset: 32, length: 32, stride: 1, type_id: 5, byte_backed: true },
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
      assert.equal(env.stasis_jit_text_run_replace_from(101, 0, 1, replacementFont, 1202), 1);
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

test("web released font handles stay invalid and the allocator never aliases them", async () => {
  const game = {
    memory: {},
    globals: {},
    views: {},
    strings: { "1": "assets/font.ttf", "2": "released" },
  };
  const releases = new Map();
  let first = 0;
  let second = 0;
  const result = await loadRuntime(game, {
    fontLoad: font => new Promise(resolve => releases.set(font.source, () => resolve(font))),
    main: env => {
      first = env.load_font(1, 20);
      assert.ok(first > 0);
      assert.ok(env.stasis_jit_gfx_cache_text(first, 2) > 0);
      env.stasis_jit_gfx_release_font(first);
      assert.equal(env.stasis_jit_gfx_cache_text(first, 2), 0);
      assert.equal(env.stasis_jit_measure_text(first, 2), 0);
      for (let index = 0; index < 4100; index += 1) {
        game.strings["2"] = `released-${index}`;
        assert.equal(env.stasis_jit_gfx_cache_text(first, 2), 0);
      }
      game.strings["1"] = "assets/replacement.ttf";
      second = env.load_font(1, 20);
      assert.ok(second > first);
      assert.ok(env.stasis_jit_gfx_cache_text(second, 2) > 0);
    },
  });
  assert.equal(releases.size, 2);
  releases.get("url(assets/replacement.ttf)")();
  await result.runtimePromise;
  assert.equal(result.addedFonts.length, 1);
  releases.get("url(assets/font.ttf)")();
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(result.addedFonts.length, 1);
});

test("web post-bootstrap font status gates prepared text until calibration", async () => {
  const game = {
    memory: {
      gfx_cmd_i32: { offset: 0, length: 67888 },
      gfx_cmd_f32: { offset: 300000, length: 146564 },
    },
    strings: { "1": "assets/post-main.ttf", "2": "calibrated" },
  };
  let fontHandle = 0;
  let textHandle = 0;
  let tickCount = 0;
  let releaseFont;
  const result = await loadRuntime(game, {
    memoryPages: 16,
    fontLoad: font => new Promise(resolve => { releaseFont = () => resolve(font); }),
    tick: env => {
      if (tickCount++ === 0) {
        fontHandle = env.load_font(1, 20);
        textHandle = env.stasis_jit_gfx_cache_text(fontHandle, 2);
        assert.equal(env.font_status(fontHandle), ASSET_STATE_LOADING);
      }
    },
    render: (_env, memory) => {
      const i32 = new Int32Array(memory.buffer, 0, 67888);
      const f32 = new Float32Array(memory.buffer, 300000, 146564);
      i32[0] = 1196967473;
      i32[1] = 7;
      i32[2] = 0;
      i32[3] = 0;
      i32[4] = 0;
      i32[7] = 1;
      i32[9] = 0;
      i32[22] = 0;
      i32[24] = 0;
      i32[27] = 0;
      i32[29] = 0;
      i32[12320] = fontHandle;
      i32[12321] = -textHandle;
      i32[12322] = 0;
      f32[133252] = 0;
      f32[133253] = 0;
      f32[133254] = 1;
      f32[133255] = 1;
      f32[133256] = 1;
      f32[133257] = 1;
    },
    measureText: ({ value }) => value === "Mg"
      ? { width: 500, fontBoundingBoxAscent: 800, fontBoundingBoxDescent: 200 }
      : { width: 42, actualBoundingBoxDescent: 2 },
  });
  await result.runtimePromise;

  result.runFrame();
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(typeof releaseFont, "function");
  assert.notEqual(result.env.font_status(fontHandle), ASSET_STATE_LOADED);
  assert.equal(result.measurements.some(({ value }) => value === "calibrated"), false);

  releaseFont();
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(result.env.font_status(fontHandle), ASSET_STATE_LOADED);
  assert.deepEqual(result.measurements.map(({ value }) => value), ["Mg"]);

  result.runFrame();
  assert.equal(result.measurements.at(-1).value, "calibrated");
  assert.equal(result.addedFonts.length, 1);
});

test("web post-bootstrap font rejection becomes a settled failure and clears pending runs", async () => {
  const game = {
    memory: {
      "run.font": { hash: 101, handle: 1101, offset: 0, length: 1, stride: 4, type_id: 1 },
      "run.handle": { hash: 102, handle: 1102, offset: 4, length: 1, stride: 4, type_id: 1 },
      "run.width": { hash: 103, handle: 1103, offset: 8, length: 1, stride: 4, type_id: 2 },
      "run.height": { hash: 104, handle: 1104, offset: 12, length: 1, stride: 4, type_id: 2 },
    },
    views: {
      "101": { font: "run.font", handle: "run.handle", width: "run.width", height: "run.height" },
    },
    strings: { "1": "assets/rejected-post-main.ttf", "2": "rejected" },
  };
  let fontHandle = 0;
  let rejectFont;
  let tickCount = 0;
  let unhandled = 0;
  const onUnhandled = () => { unhandled += 1; };
  process.on("unhandledRejection", onUnhandled);
  try {
    const result = await loadRuntime(game, {
      fontLoad: () => new Promise((_resolve, reject) => { rejectFont = reject; }),
      tick: env => {
        if (tickCount++ === 0) {
          fontHandle = env.load_font(1, 20);
          assert.equal(env.stasis_jit_text_run_load_from(101, 0, 1, fontHandle, 2), 1);
        }
      },
    });
    await result.runtimePromise;
    result.runFrame();
    await new Promise(resolve => setImmediate(resolve));
    assert.equal(typeof rejectFont, "function");
    assert.notEqual(result.env.font_status(fontHandle), ASSET_STATE_LOADED);
    rejectFont(new Error("post-main font rejected"));
    await new Promise(resolve => setImmediate(resolve));
    await new Promise(resolve => setImmediate(resolve));

    const view = new DataView(result.memory.buffer);
    assert.equal(result.env.font_status(fontHandle), ASSET_STATE_FAILED);
    assert.equal(result.document.body.dataset.fontStatus, "failed");
    assert.equal(view.getFloat32(8, true), 0);
    assert.equal(view.getFloat32(12, true), 0);
    assert.equal(result.addedFonts.length, 0);

    assert.equal(result.env.stasis_jit_text_run_load_from(101, 0, 1, fontHandle, 2), 1);
    assert.equal(view.getFloat32(8, true), 0);
    assert.equal(view.getFloat32(12, true), 0);
    assert.equal(result.env.stasis_jit_text_run_replace_from(101, 0, 1, fontHandle, 2), 1);
    assert.equal(view.getFloat32(8, true), 0);
    assert.equal(view.getFloat32(12, true), 0);
  } finally {
    process.off("unhandledRejection", onUnhandled);
  }
  assert.equal(unhandled, 0);
});

test("web release during a post-bootstrap FontFace load ignores late completion", async () => {
  const game = {
    memory: {},
    strings: { "1": "assets/released-post-main.ttf" },
  };
  let fontHandle = 0;
  let releaseFont;
  let tickCount = 0;
  const result = await loadRuntime(game, {
    fontLoad: font => new Promise(resolve => { releaseFont = () => resolve(font); }),
    tick: env => {
      if (tickCount++ === 0) fontHandle = env.load_font(1, 20);
    },
  });
  await result.runtimePromise;
  result.runFrame();
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(typeof releaseFont, "function");
  assert.notEqual(result.env.font_status(fontHandle), ASSET_STATE_LOADED);

  result.env.stasis_jit_gfx_release_font(fontHandle);
  assert.equal(result.env.font_status(fontHandle), ASSET_STATE_NONE);
  releaseFont();
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(result.env.font_status(fontHandle), ASSET_STATE_NONE);
  assert.equal(result.addedFonts.length, 0);
  assert.equal(result.document.body.dataset.fontStatus, undefined);
});

test("web calibration failure restores Canvas state and settles the font failed", async () => {
  const game = {
    memory: {},
    strings: { "1": "assets/bad-metrics.ttf" },
  };
  let fontHandle = 0;
  let tickCount = 0;
  const result = await loadRuntime(game, {
    tick: env => {
      if (tickCount++ === 0) fontHandle = env.load_font(1, 20);
    },
    measureText: ({ value }) => {
      if (value === "Mg") throw new Error("invalid font metrics");
      return { width: 20, actualBoundingBoxDescent: 2 };
    },
  });
  await result.runtimePromise;
  const before = { ...result.contextState };
  result.runFrame();
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));

  assert.equal(result.env.font_status(fontHandle), ASSET_STATE_FAILED);
  assert.equal(result.document.body.dataset.fontStatus, "failed");
  assert.equal(result.addedFonts.length, 0);
  assert.equal(result.contextState.saves - before.saves, 1);
  assert.equal(result.contextState.restores - before.restores, 1);
});

test("web pending run measurement failure restores both Canvas contexts", async () => {
  const game = {
    memory: {
      "run.font": { hash: 101, handle: 1101, offset: 0, length: 1, stride: 4, type_id: 1 },
      "run.handle": { hash: 102, handle: 1102, offset: 4, length: 1, stride: 4, type_id: 1 },
      "run.width": { hash: 103, handle: 1103, offset: 8, length: 1, stride: 4, type_id: 2 },
      "run.height": { hash: 104, handle: 1104, offset: 12, length: 1, stride: 4, type_id: 2 },
    },
    views: {
      "101": { font: "run.font", handle: "run.handle", width: "run.width", height: "run.height" },
    },
    strings: { "1": "assets/late-metrics.ttf", "2": "throws" },
  };
  let fontHandle = 0;
  let resolveFont;
  let tickCount = 0;
  const result = await loadRuntime(game, {
    fontLoad: font => new Promise(resolve => { resolveFont = () => resolve(font); }),
    tick: env => {
      if (tickCount++ === 0) {
        fontHandle = env.load_font(1, 20);
        assert.equal(env.stasis_jit_text_run_load_from(101, 0, 1, fontHandle, 2), 1);
      }
    },
    measureText: ({ value }) => {
      if (value === "throws") throw new Error("run metrics unavailable");
      return { width: 500, fontBoundingBoxAscent: 800, fontBoundingBoxDescent: 200 };
    },
  });
  await result.runtimePromise;
  result.runFrame();
  await new Promise(resolve => setImmediate(resolve));
  const before = { ...result.contextState };
  resolveFont();
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));

  const view = new DataView(result.memory.buffer);
  assert.equal(result.env.font_status(fontHandle), ASSET_STATE_FAILED);
  assert.equal(view.getFloat32(8, true), 0);
  assert.equal(view.getFloat32(12, true), 0);
  assert.equal(result.contextState.saves - before.saves, 2);
  assert.equal(result.contextState.restores - before.restores, 2);
});
