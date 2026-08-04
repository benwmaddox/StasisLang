import { strict as assert } from "node:assert";
import { test } from "node:test";
import { displayRuntimeValue, isLiveResponse } from "./protocol";
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
