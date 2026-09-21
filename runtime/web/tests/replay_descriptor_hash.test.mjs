import assert from "node:assert/strict";
import crypto from "node:crypto";
import test from "node:test";
import { ReplayDecodeError, createDescriptorStateHashAdapter } from "../replay_controller.mjs";

const typeTag = Object.freeze({ i32: 1, f32: 2, f64: 3, bool: 4, u8: 5, u16: 6, u32: 7 });
const width = Object.freeze({ i32: 4, f32: 4, f64: 8, bool: 1, u8: 1, u16: 2, u32: 4 });

function descriptor(entries) {
  let offset = 0;
  const normalized = entries.map(entry => {
    const result = { ...entry, offset, element_bytes: width[entry.storage_type] };
    offset += result.element_count * result.element_bytes;
    return result;
  });
  return {
    schema: "stasis.replay_state_snapshot.v1",
    abi_version: 1,
    support: "canonical_bytes",
    byte_order: "little_endian",
    hash_scope: "simulation_after_tick",
    required_bytes: offset,
    entries: normalized,
    unsupported_paths: [],
    size_operation: "stasis_replay_state_snapshot_size",
    write_operation: "stasis_replay_state_snapshot_write",
  };
}

function oracle(entries, bytesByEntry) {
  const hash = crypto.createHash("sha256");
  hash.update(Buffer.from("stasis.simulation-state.v1\0", "utf8"));
  for (const entry of entries) {
    const bytes = bytesByEntry.get(`${entry.path}\0${entry.field}`);
    for (let index = 0; index < entry.element_count; index += 1) {
      const label = entry.field === "" ? entry.path : `${entry.path}[${index}].${entry.field}`;
      const labelBytes = Buffer.from(label, "utf8");
      const length = Buffer.alloc(8);
      length.writeBigUInt64LE(BigInt(labelBytes.length));
      hash.update(length);
      hash.update(labelBytes);
      hash.update(Buffer.from([typeTag[entry.storage_type]]));
      hash.update(bytes.subarray(index * entry.element_bytes, (index + 1) * entry.element_bytes));
    }
  }
  return hash.digest("hex");
}

test("descriptor state hash matches an independent SHA-256 oracle for every scalar type and collection labels", () => {
  const entries = [
    { path: "enabled", field: "", storage_type: "bool", element_count: 1 },
    { path: "score", field: "", storage_type: "f64", element_count: 1 },
    { path: "small", field: "", storage_type: "u8", element_count: 1 },
    { path: "tiny", field: "", storage_type: "u16", element_count: 1 },
    { path: "total", field: "", storage_type: "u32", element_count: 1 },
    { path: "actors", field: "value", storage_type: "i32", element_count: 2 },
    { path: "ratio", field: "f32", storage_type: "f32", element_count: 1 },
  ];
  // Descriptor entries are compiler-canonical: sorted scalars first, then
  // collections sorted by path and field, with offsets following that order.
  const snapshot = descriptor(entries);
  const bytesByEntry = new Map([
    ["actors\0value", Uint8Array.from([0x78, 0x56, 0x34, 0x12, 0xff, 0xff, 0xff, 0xff])],
    ["enabled\0", Uint8Array.from([1])],
    ["ratio\0f32", Uint8Array.from([0x45, 0x23, 0xc1, 0x7f])], // f32 NaN payload 0x7fc12345
    ["score\0", Uint8Array.from([0x42, 0x00, 0x00, 0x00, 0x00, 0x00, 0xf8, 0x7f])], // f64 NaN payload
    ["small\0", Uint8Array.from([0xab])],
    ["tiny\0", Uint8Array.from([0x34, 0x12])],
    ["total\0", Uint8Array.from([0x89, 0xab, 0xcd, 0xef])],
  ]);
  const adapter = createDescriptorStateHashAdapter({
    descriptor: snapshot,
    readEntryBytes: entry => bytesByEntry.get(`${entry.path}\0${entry.field}`),
  });
  assert.equal(adapter.hashState(), oracle(snapshot.entries, bytesByEntry));
});

test("descriptor state hash rejects unsupported, non-contiguous, and incorrectly typed descriptors", () => {
  const valid = descriptor([{ path: "value", field: "", storage_type: "i32", element_count: 1 }]);
  assert.throws(() => createDescriptorStateHashAdapter({ descriptor: { ...valid, support: "descriptor_only" }, readEntryBytes: () => new Uint8Array(4) }), /canonical_bytes/);
  assert.throws(() => createDescriptorStateHashAdapter({ descriptor: { ...valid, required_bytes: 8 }, readEntryBytes: () => new Uint8Array(4) }), /required_bytes/);
  assert.throws(() => createDescriptorStateHashAdapter({ descriptor: { ...valid, entries: [{ ...valid.entries[0], element_bytes: 2 }] }, readEntryBytes: () => new Uint8Array(4) }), /element_bytes/);
  assert.throws(() => createDescriptorStateHashAdapter({ descriptor: { ...valid, entries: [{ ...valid.entries[0], offset: 1 }] }, readEntryBytes: () => new Uint8Array(4) }), /contiguous/);
  assert.throws(() => createDescriptorStateHashAdapter({ descriptor: { ...valid, entries: [{ ...valid.entries[0], element_count: 2 }] }, readEntryBytes: () => new Uint8Array(8) }), /scalar/);
  assert.throws(() => createDescriptorStateHashAdapter({ descriptor: { ...valid, unsupported_paths: ["opaque"] }, readEntryBytes: () => new Uint8Array(4) }), /unsupported paths/);
  const collectionThenScalar = descriptor([
    { path: "items", field: "value", storage_type: "i32", element_count: 1 },
    { path: "score", field: "", storage_type: "i32", element_count: 1 },
  ]);
  assert.throws(() => createDescriptorStateHashAdapter({ descriptor: collectionThenScalar, readEntryBytes: () => new Uint8Array(4) }), /precede collections/);
  const descendingCollections = descriptor([
    { path: "z", field: "value", storage_type: "i32", element_count: 1 },
    { path: "a", field: "value", storage_type: "i32", element_count: 1 },
  ]);
  assert.throws(() => createDescriptorStateHashAdapter({ descriptor: descendingCollections, readEntryBytes: () => new Uint8Array(4) }), /sorted unique/);
});

test("zero-capacity collection lanes are valid and contribute no labeled values", () => {
  const snapshot = descriptor([
    { path: "score", field: "", storage_type: "i32", element_count: 1 },
    { path: "empty", field: "values", storage_type: "i32", element_count: 0 },
  ]);
  const bytesByEntry = new Map([
    ["score\0", Uint8Array.from([7, 0, 0, 0])],
    ["empty\0values", new Uint8Array(0)],
  ]);
  const adapter = createDescriptorStateHashAdapter({
    descriptor: snapshot,
    readEntryBytes: entry => bytesByEntry.get(`${entry.path}\0${entry.field}`),
  });
  assert.equal(adapter.hashState(), oracle(snapshot.entries, bytesByEntry));
});

test("descriptor reader must synchronously return exactly the advertised bytes", () => {
  const valid = descriptor([{ path: "value", field: "", storage_type: "i32", element_count: 1 }]);
  const missing = createDescriptorStateHashAdapter({ descriptor: valid, readEntryBytes: () => new Uint8Array(3) });
  assert.throws(() => missing.hashState(), ReplayDecodeError);
  const extra = createDescriptorStateHashAdapter({ descriptor: valid, readEntryBytes: () => new Uint8Array(5) });
  assert.throws(() => extra.hashState(), ReplayDecodeError);
  const asyncReader = createDescriptorStateHashAdapter({ descriptor: valid, readEntryBytes: () => Promise.resolve(new Uint8Array(4)) });
  assert.throws(() => asyncReader.hashState(), /synchronous/);
  const invalidBool = descriptor([{ path: "enabled", field: "", storage_type: "bool", element_count: 1 }]);
  const boolAdapter = createDescriptorStateHashAdapter({ descriptor: invalidBool, readEntryBytes: () => Uint8Array.from([2]) });
  assert.throws(() => boolAdapter.hashState(), /bool value/);
});
