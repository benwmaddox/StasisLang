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
