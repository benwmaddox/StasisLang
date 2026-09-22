import test from "node:test";
import assert from "node:assert/strict";
import { loadRuntime } from "./asset_paths.test.mjs";

const encoder = new TextEncoder();

test("web UTF-8 literal metadata feeds host adapters without copied payloads", async () => {
  const fontPath = "assets/font.ttf";
  const spritePath = "assets/sprite.svg";
  const copiedText = "éclair";
  const fontBytes = encoder.encode(fontPath);
  const spriteBytes = encoder.encode(spritePath);
  const copiedBytes = encoder.encode(copiedText);
  const game = {
    memory: {
      destination: { hash: 901, handle: 9001, offset: 256, length: 16, stride: 1, type_id: 5, byte_backed: true },
    },
    stringLiteralTableVersion: 1,
    stringLiteralTable: {
      "11": { offset: 0, byte_length: fontBytes.length },
      "22": { offset: 64, byte_length: spriteBytes.length },
      "33": { offset: 96, byte_length: copiedBytes.length },
      "44": { offset: 65536, byte_length: 0 },
    },
    strings: {},
    assets: {},
  };
  let font = 0;
  let sprite = 0;
  const result = await loadRuntime(game, {
    main: (env, memory) => {
      const bytes = new Uint8Array(memory.buffer);
      bytes.set(fontBytes, 0);
      bytes.set(spriteBytes, 64);
      bytes.set(copiedBytes, 96);
      font = env.load_font(11, 20);
      sprite = env.gfx_load_sprite(22);
      assert.equal(env.measure_text(font, 33), copiedText.length * 7);
      assert.equal(env.measure_text(font, 44), 0);
      assert.ok(env.stasis_jit_gfx_cache_text(font, 33) > 0);
      env.sys_memcpy_u8(9001, 0, 33, 0, copiedBytes.length);
    },
  });
  await result.runtimePromise;

  assert.ok(font > 0);
  assert.ok(sprite > 0);
  assert.deepEqual(
    Array.from(new Uint8Array(result.memory.buffer, 256, copiedBytes.length)),
    Array.from(copiedBytes),
  );
  assert.deepEqual(result.imageSources, [spritePath]);
  assert.ok(result.fontSources.includes(`url(${fontPath})`));
});

test("web literal metadata rejects invalid or malformed UTF-8 without compatibility fallback", async () => {
  const fontPath = "assets/font.ttf";
  const fontBytes = encoder.encode(fontPath);
  const game = {
    memory: {
      destination: { hash: 902, handle: 9002, offset: 256, length: 8, stride: 1, type_id: 5, byte_backed: true },
    },
    stringLiteralTableVersion: 1,
    stringLiteralTable: {
      "1": { offset: 0, byte_length: fontBytes.length },
      "2": { offset: 64, byte_length: 2 },
      "3": { offset: -1, byte_length: 2 },
      "4": { offset: 65536, byte_length: 0 },
      "5": { offset: "64", byte_length: 2 },
    },
    strings: { "2": "fallback", "3": "fallback", "5": "fallback" },
  };
  const result = await loadRuntime(game, {
    main: (env, memory) => {
      const bytes = new Uint8Array(memory.buffer);
      bytes.set(fontBytes, 0);
      bytes.set([0xc3, 0x28], 64);
      const font = env.load_font(1, 20);
      for (const handle of [2, 3, 5]) {
        assert.equal(env.measure_text(font, handle), 0);
        assert.equal(env.stasis_jit_gfx_cache_text(font, handle), 0);
        env.sys_memcpy_u8(9002, 0, handle, 0, 2);
        assert.deepEqual(Array.from(bytes.slice(256, 258)), [0, 0]);
      }
      assert.equal(env.measure_text(font, 4), 0, "empty literal at memory end is valid");
    },
  });
  await result.runtimePromise;
});

test("web literal cache observes memory.grow and decodes the new buffer", async () => {
  const fontPath = "assets/font.ttf";
  const fontBytes = encoder.encode(fontPath);
  const game = {
    memory: {},
    stringLiteralTableVersion: 1,
    stringLiteralTable: {
      "1": { offset: 0, byte_length: fontBytes.length },
      "2": { offset: 64, byte_length: 2 },
    },
    strings: {},
  };
  let firstWidth = 0;
  let secondWidth = 0;
  const result = await loadRuntime(game, {
    main: (env, memory) => {
      const bytes = new Uint8Array(memory.buffer);
      bytes.set(fontBytes, 0);
      bytes.set(encoder.encode("aa"), 64);
      const font = env.load_font(1, 20);
      firstWidth = env.measure_text(font, 2);
      memory.grow(1);
      new Uint8Array(memory.buffer).set(encoder.encode("é"), 64);
      secondWidth = env.measure_text(font, 2);
    },
  });
  await result.runtimePromise;

  assert.equal(firstWidth, 14);
  assert.equal(secondWidth, 7);
  assert.deepEqual(
    result.measurements
      .filter(({ value }) => value === "aa" || value === "é")
      .map(({ value }) => value),
    ["aa", "é"],
  );
});
