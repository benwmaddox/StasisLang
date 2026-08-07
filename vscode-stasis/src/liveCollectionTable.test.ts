import { strict as assert } from "node:assert";
import { test } from "node:test";
import { buildLiveCollectionTableModel } from "./liveCollectionTable";
import { LiveCollection } from "./protocol";

const enemies: LiveCollection = {
  path: "state.enemies",
  elementShape: "Enemy",
  capacity: 4,
  activeCount: 2,
  fields: [
    { field: "active", staticType: "bool" },
    { field: "hp", staticType: "i32" },
    { field: "name", staticType: "string" },
  ],
  rows: [
    { index: 0, values: { active: true, hp: 10, name: "rook" } },
    { index: 1, values: { active: false, hp: 20, name: "pawn" } },
  ],
  rowsTruncated: false,
  tick: 12,
};

test("collection table model provides aligned columns and cells", () => {
  const model = buildLiveCollectionTableModel(enemies, false);

  assert.deepEqual(model.columns, [
    { key: "active", label: "active", staticType: "bool" },
    { key: "hp", label: "hp", staticType: "i32" },
    { key: "name", label: "name", staticType: "string" },
  ]);
  assert.deepEqual(model.rows, [
    { index: 0, cells: ["true", "10", "rook"] },
    { index: 1, cells: ["false", "20", "pawn"] },
  ]);
});

test("collection table model applies the configured active filter", () => {
  const model = buildLiveCollectionTableModel(enemies, true);

  assert.equal(model.filtered, true);
  assert.deepEqual(model.rows.map((row) => row.index), [0]);
});
