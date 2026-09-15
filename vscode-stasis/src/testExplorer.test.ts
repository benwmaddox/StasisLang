import { strict as assert } from "node:assert";
import { test } from "node:test";
import { hasDuplicateTestLabel, matchPassedTest, validTestSourceOffsets } from "./testExplorer";

test("Test Explorer matches the CLI's full test path and label identity", () => {
  const filePath = "/workspace/project/tests/one.test.stasis";
  const passed = [
    "/workspace/project/tests/two.test.stasis :: same name",
    "/workspace/project/tests/one.test.stasis :: same name",
  ];

  assert.equal(matchPassedTest(passed, filePath, "same name"), "passed");
  assert.equal(
    matchPassedTest(passed, "/workspace/project/tests/missing.test.stasis", "same name"),
    "not_found",
  );
});

test("Test Explorer refuses ambiguous duplicate full identities", () => {
  assert.equal(hasDuplicateTestLabel(["same name", "same name"], "same name"), true);
  assert.equal(hasDuplicateTestLabel(["same name", "other name"], "same name"), false);
  assert.equal(
    matchPassedTest(
      [
        "/workspace/project/tests/one.test.stasis :: same name",
        "/workspace/project/tests/one.test.stasis :: same name",
      ],
      "/workspace/project/tests/one.test.stasis",
      "same name",
    ),
    "ambiguous",
  );
});

test("Test Explorer resolves valid UTF-8 byte spans to string offsets", () => {
  const source = "αβ\n";
  assert.deepEqual(validTestSourceOffsets(source, { start: 2, end: 4 }), { start: 1, end: 2 });
  assert.deepEqual(validTestSourceOffsets(source, { start: 0, end: 5 }), { start: 0, end: 3 });
});

test("Test Explorer skips malformed UTF-8 source span metadata", () => {
  const source = "αβ\n";
  for (const span of [
    { start: -1, end: 2 },
    { start: 3, end: 2 },
    { start: 1, end: 4 },
    { start: 0, end: 1 },
    { start: 0.5, end: 2 },
    { start: Number.NaN, end: 2 },
    { start: 0, end: 6 },
  ]) {
    assert.equal(validTestSourceOffsets(source, span), undefined, JSON.stringify(span));
  }
});
