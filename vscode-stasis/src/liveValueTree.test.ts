import { strict as assert } from "node:assert";
import { test } from "node:test";
import { buildLiveValueTree } from "./liveValueTree";
import { LiveCollection, LiveValue } from "./protocol";

const values: LiveValue[] = [
  { path: "score", staticType: "i32", value: { type: "i32", value: 7 }, tick: 4, watched: false },
  { path: "state.level", staticType: "i32", value: { type: "i32", value: 2 }, tick: 4, watched: false },
  { path: "state.enemies.length", staticType: "i32", value: { type: "i32", value: 2 }, tick: 4, watched: false },
];

const enemies: LiveCollection = {
  path: "state.enemies",
  elementShape: "Enemy",
  capacity: 2,
  activeCount: 2,
  fields: [
    { field: "Active", staticType: "bool" },
    { field: "hp", staticType: "i32" },
    { field: "speed", staticType: "f32" },
  ],
  rows: [
    { index: 0, values: { Active: true, hp: 10, speed: 1.5 } },
    { index: 1, values: { Active: false, hp: 20, speed: 2.5 } },
  ],
  rowsTruncated: false,
  tick: 4,
};

test("live values form a globals-first hierarchy", () => {
  const tree = buildLiveValueTree(values, [enemies], new Set());
  assert.deepEqual(tree.map((node) => node.label), ["score", "state"]);
  const state = tree[1]!;
  assert.deepEqual(state.children.map((node) => node.label), ["enemies", "level"]);
  assert.deepEqual(state.children[0]!.children.map((node) => node.label), ["[0]", "length"]);
  assert.deepEqual(state.children[0]!.children[0]!.children.map((node) => node.label), ["Active", "hp", "speed"]);
});

test("table mode flattens each struct element into one row", () => {
  const tree = buildLiveValueTree(values, [enemies], new Set(["state.enemies"]));
  const collection = tree[1]!.children[0]!;
  assert.deepEqual(collection.children.map((node) => node.label), ["[0]", "length"]);
  assert.equal(collection.children[0]!.children.length, 0);
});

test("inactive row filtering can be disabled", () => {
  const tree = buildLiveValueTree(values, [enemies], new Set(), false);
  assert.deepEqual(tree[1]!.children[0]!.children.map((node) => node.label), ["[0]", "[1]", "length"]);
});
