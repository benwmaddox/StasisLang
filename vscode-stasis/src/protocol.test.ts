import { strict as assert } from "node:assert";
import { test } from "node:test";
import {
  byteOffsetToStringOffset,
  displayRuntimeValue,
  isLiveResponse,
  JsonLineDecoder,
  stringOffsetToByteOffset,
} from "./protocol";

test("JSONL decoder preserves split responses", () => {
  const decoder = new JsonLineDecoder();
  assert.deepEqual(decoder.push('{"schema_version":1,"request_'), []);
  assert.deepEqual(decoder.push('id":7,"tick":2,"ok":true,"kind":"paused"}\r\n'), [
    { schema_version: 1, request_id: 7, tick: 2, ok: true, kind: "paused" },
  ]);
  assert.deepEqual(decoder.finish(), []);
});

test("live response validation rejects unrelated stdout", () => {
  assert.equal(
    isLiveResponse({ schema_version: 1, request_id: 1, tick: 0, ok: true, kind: "status" }),
    true,
  );
  assert.equal(isLiveResponse({ kind: "status" }), false);
});

test("runtime values display scalars without protocol wrappers", () => {
  assert.equal(displayRuntimeValue({ type: "i32", value: 42 }), "42");
  assert.equal(displayRuntimeValue({ hp: 4, active: true }), '{"hp":4,"active":true}');
});

test("completion offsets translate between VS Code strings and UTF-8", () => {
  const source = "// 🐴\nscore";
  const stringOffset = source.indexOf("score") + 3;
  const byteOffset = stringOffsetToByteOffset(source, stringOffset);
  assert.equal(byteOffsetToStringOffset(source, byteOffset), stringOffset);
});
