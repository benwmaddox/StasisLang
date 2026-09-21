import assert from "node:assert/strict";
import test from "node:test";
import {
  ReplayDecodeError,
  ReplayDivergenceError,
  ReplayIdentityError,
  WEB_REPLAY_LIMITS,
  createCanonicalStateHashAdapter,
  createHostFrameSource,
  createReplayController,
  createWasmReplayBridge,
  fetchReplayBytes,
  decodeReplay,
} from "../replay_controller.mjs";

const HASH_A = "a".repeat(64);
const HASH_B = "b".repeat(64);
const HASH_C = "c".repeat(64);

function identity({ i32 = 2, f32 = 2, observedI32 = null, observedF32 = null } = {}) {
  return {
    stasis_version: "0.1.0",
    release_id: "test-release",
    target: "web",
    source_sha256: HASH_A,
    state_layout_sha256: HASH_B,
    compiler_layout_sha256: HASH_C,
    runtime_sha256: HASH_A,
    host_schema_version: 4,
    host_i32_count: i32,
    host_f32_count: f32,
    input_usage_sha256: HASH_B,
    tick_rate_hz: 60,
    hash_scope: "simulation_after_tick",
    determinism_profile: "input_only_no_external_observations",
    observed_i32: (observedI32 || Array.from({ length: i32 }, (_, index) => index)).map((index, slot) => ({ slot, index, path: `input.i32_${index}`, family: "raw" })),
    observed_f32: (observedF32 || Array.from({ length: f32 }, (_, index) => index)).map((index, slot) => ({ slot, index, path: `input.f32_${index}`, family: "raw" })),
  };
}

function document({ total_ticks = 5, segments, checkpoints = [], finalHash = HASH_C, initialHash = HASH_A, observedI32, observedF32 } = {}) {
  const replayIdentity = identity({ observedI32, observedF32 });
  const i32Values = [11, -2].slice(0, replayIdentity.observed_i32.length);
  const f32Bits = [0x7fc00001, 0x80000000].slice(0, replayIdentity.observed_f32.length);
  return {
    schema_version: 2,
    identity: replayIdentity,
    initial_state: { values: [], state_sha256: initialHash },
    initial_input: { i32_values: i32Values, f32_bits: f32Bits },
    segments: segments || [{ tick_gap: 0, run_ticks: total_ticks, i32_changes: [], f32_changes: [] }],
    checkpoints,
    total_ticks,
    final_state: { tick: total_ticks, state_sha256: finalHash },
  };
}

function v3Document(options = {}) {
  const source = document(options);
  const { target, runtime_sha256, ...compatibility } = source.identity;
  return {
    ...source,
    schema_version: 3,
    identity: {
      compatibility,
      producer: { target: "windows-x86_64", runtime_sha256 },
    },
  };
}

function packagedIdentity(source, { target = "wasm32-web", runtime = HASH_C } = {}) {
  const compatibility = source.schema_version === 3
    ? source.identity.compatibility
    : Object.fromEntries(Object.entries(source.identity).filter(([key]) => !["target", "runtime_sha256"].includes(key)));
  return { compatibility, consumer: { target, runtime_sha256: runtime } };
}

test("schema-v2 decoder accepts object and JSON input while enforcing strict nested fields", () => {
  const source = document();
  assert.deepEqual(decodeReplay(source), { ...source, identity: { ...source.identity, asset_manifest_sha256: null, controller_schema_version: null } });
  assert.deepEqual(decodeReplay(JSON.stringify(source)), { ...source, identity: { ...source.identity, asset_manifest_sha256: null, controller_schema_version: null } });
  assert.throws(() => decodeReplay({ ...source, unexpected: true }), ReplayDecodeError);
  assert.throws(() => decodeReplay(JSON.stringify({ ...source, identity: { ...source.identity, observed_i32: source.identity.observed_i32.map(field => ({ ...field, extra: 1 })) } })), ReplayDecodeError);

  const primitiveCollection = {
    ...source,
    initial_state: {
      ...source.initial_state,
      values: [{
        location: { kind: "collection", path: "values", field: "", index: 0 },
        value: { type_name: "i32", bits: "01000000" },
      }],
    },
  };
  assert.equal(decodeReplay(primitiveCollection).initial_state.values[0].location.field, "");
});

test("schema-v3 decoder validates producer provenance and accepts a different Web consumer", () => {
  const source = v3Document({ total_ticks: 1 });
  const decoded = decodeReplay(source);
  assert.equal(decoded.schema_version, 3);
  assert.equal(decoded.identity.producer.target, "windows-x86_64");
  assert.equal(decoded.identity.compatibility.runtime_sha256, undefined);
  assert.doesNotThrow(() => createReplayController(source, {
    packageIdentity: packagedIdentity(source, { target: "wasm32-web", runtime: HASH_B }),
  }));
  assert.throws(
    () => decodeReplay({ ...source, identity: { ...source.identity, producer: { ...source.identity.producer, extra: true } } }),
    ReplayDecodeError,
  );
});

test("schema-v3 compares every portable compatibility field and descriptor", () => {
  const source = v3Document({ total_ticks: 1 });
  const expected = packagedIdentity(source);
  const scalarMutations = {
    stasis_version: "0.1.1",
    release_id: "other-release",
    source_sha256: HASH_B,
    state_layout_sha256: HASH_C,
    compiler_layout_sha256: HASH_A,
    asset_manifest_sha256: HASH_A,
    host_schema_version: 5,
    host_i32_count: 3,
    host_f32_count: 3,
    input_usage_sha256: HASH_C,
    tick_rate_hz: 30,
    hash_scope: "other_scope",
    determinism_profile: "other_profile",
    controller_schema_version: 2,
  };
  for (const [field, value] of Object.entries(scalarMutations)) {
    const mutated = { ...expected, compatibility: { ...expected.compatibility, [field]: value } };
    assert.throws(() => createReplayController(source, { packageIdentity: mutated }), ReplayIdentityError, field);
  }
  for (const field of ["observed_i32", "observed_f32"]) {
    const descriptors = expected.compatibility[field].map((entry, index) => index === 0 ? { ...entry, path: `${entry.path}.changed` } : entry);
    const mutated = { ...expected, compatibility: { ...expected.compatibility, [field]: descriptors } };
    assert.throws(() => createReplayController(source, { packageIdentity: mutated }), ReplayIdentityError, field);
  }
});

test("schema-v2 retains exact target and runtime matching with packaged identity metadata", () => {
  const source = document({ total_ticks: 1 });
  const exact = packagedIdentity(source, { target: source.identity.target, runtime: source.identity.runtime_sha256 });
  assert.doesNotThrow(() => createReplayController(source, { packageIdentity: exact }));
  assert.throws(
    () => createReplayController(source, { packageIdentity: packagedIdentity(source, { target: source.identity.target, runtime: HASH_B }) }),
    error => error instanceof ReplayIdentityError && error.field === "identity.runtime_sha256",
  );
});

test("JSON replay decoding rejects duplicate keys at every nested contract layer", () => {
  const v2 = JSON.stringify(document({ total_ticks: 1 }));
  const v3 = JSON.stringify(v3Document({ total_ticks: 1 }));
  const withStateEntry = JSON.stringify({
    ...document({ total_ticks: 1 }),
    initial_state: {
      values: [{
        location: { kind: "collection", path: "values", field: "", index: 0 },
        value: { type_name: "i32", bits: "01000000" },
      }],
      state_sha256: HASH_A,
    },
  });
  for (const duplicate of [
    v2.replace('"schema_version":2', '"schema_version":2,"schema_version":2'),
    v3.replace('"release_id":"test-release"', '"release_id":"test-release","release_id":"test-release"'),
    v2.replace('"tick_gap":0', '"tick_gap":0,"tick_gap":0'),
    withStateEntry.replace('"kind":"collection"', '"kind":"collection","kind":"collection"'),
    v2.replace('"target":"web"', '"target":"web","\\u0074arget":"web"'),
  ]) {
    assert.throws(
      () => decodeReplay(duplicate),
      error => error instanceof ReplayDecodeError && error.code === "duplicate_field",
    );
  }
});

test("compact RLE applies changes at run starts and preserves values through tick gaps", () => {
  const source = document({
    total_ticks: 5,
    segments: [
      { tick_gap: 0, run_ticks: 2, i32_changes: [{ slot: 0, value: 20 }], f32_changes: [{ slot: 1, bits: 0x3f800000 }] },
      { tick_gap: 1, run_ticks: 2, i32_changes: [{ slot: 1, value: 99 }], f32_changes: [{ slot: 0, bits: 0x00000001 }] },
    ],
  });
  const controller = createReplayController(source);
  const i32 = new Int32Array(2);
  const f32 = new Float32Array(2);
  controller.applyHostFrame(1, i32, f32);
  assert.deepEqual([...i32], [20, -2]);
  assert.deepEqual([...new Uint32Array(f32.buffer)], [0x7fc00001, 0x3f800000]);
  controller.applyHostFrame(2, i32, f32);
  controller.applyHostFrame(3, i32, f32);
  assert.deepEqual([...i32], [20, -2], "gap tick keeps the preceding snapshot");
  controller.applyHostFrame(4, i32, f32);
  assert.deepEqual([...i32], [20, 99]);
  assert.deepEqual([...new Uint32Array(f32.buffer)], [0x00000001, 0x3f800000]);
  controller.applyHostFrame(5, i32, f32);
});

test("replay projection zeroes unobserved HostFrame slots and preserves exact f32 bits", () => {
  const source = document({
    total_ticks: 1,
    observedI32: [1],
    observedF32: [0],
    segments: [{ tick_gap: 0, run_ticks: 1, i32_changes: [{ slot: 0, value: 7 }], f32_changes: [{ slot: 0, bits: 0x7fc12345 }] }],
  });
  const controller = createReplayController(source);
  const i32 = new Int32Array([99, 99]);
  const f32 = new Float32Array(2);
  new Uint32Array(f32.buffer).set([0xdeadbeef, 0xdeadbeef]);
  controller.applyHostFrame(1, i32, f32);
  assert.deepEqual([...i32], [0, 7]);
  assert.deepEqual([...new Uint32Array(f32.buffer)], [0x7fc12345, 0], "NaN payload and zero default remain bit exact");
});

test("host frame source keeps live and replay modes explicit", () => {
  const calls = [];
  const live = createHostFrameSource({ live: (timestamp, tick) => calls.push(["live", timestamp, tick]) });
  live.write(4, null, null, 80);
  assert.equal(live.mode, "live");
  assert.deepEqual(calls, [["live", 80, 4]]);
  const controller = createReplayController(document({ total_ticks: 1 }), { hashAdapter: () => HASH_C });
  const replay = createHostFrameSource({ replay: controller });
  replay.write(1, new Int32Array(2), new Float32Array(2));
  assert.equal(replay.mode, "replay");
});

test("runTick uses one ordinary tick/render sequence and ignores physical input", () => {
  const controller = createReplayController(document({ total_ticks: 1 }), { hashAdapter: () => HASH_C });
  const i32 = new Int32Array([77, 88]);
  const f32 = new Float32Array([4, 5]);
  const calls = [];
  controller.runTick({
    tick: 1,
    hostI32: i32,
    hostF32: f32,
    tickFn: () => { calls.push(["tick", [...i32], [...f32]]); return 0; },
    renderFn: () => { calls.push("render"); return 0; },
  });
  assert.equal(calls.length, 2);
  assert.equal(calls[0][0], "tick");
  assert.deepEqual(calls[0][1], [11, -2], "replay input is the recorded snapshot, not physical input");
  assert.equal(calls[1], "render");
});

test("identity mismatch is rejected before playback", () => {
  assert.throws(() => createReplayController(document(), { identity: { target: "android" } }), ReplayIdentityError);
  assert.throws(() => createReplayController(document({ total_ticks: 1 }), { identity: { host_i32_count: 3 } }), ReplayIdentityError);
});

test("replay fetch requires same-origin streaming and enforces the byte bound before reading", async () => {
  await assert.rejects(
    fetchReplayBytes("https://elsewhere.invalid/replay.json", { baseUrl: "https://game.test/index.html", fetchImpl: () => { throw new Error("must not fetch"); } }),
    /package origin/,
  );
  let reads = 0;
  const response = {
    ok: true,
    status: 200,
    headers: { get: () => String(WEB_REPLAY_LIMITS.maxFileBytes + 1) },
    body: { getReader: () => ({ read: async () => { reads += 1; return { done: true }; } }) },
  };
  await assert.rejects(
    fetchReplayBytes("/large.json", { baseUrl: "https://game.test/index.html", fetchImpl: async () => response }),
    error => error instanceof ReplayDecodeError && error.code === "file_too_large",
  );
  assert.equal(reads, 0);

  const repeatedChunk = new Uint8Array(1024 * 1024);
  let streamed = 0;
  let cancelled = false;
  const oversizedReader = {
    read: async () => ({ done: false, value: (streamed += 1, repeatedChunk) }),
    cancel: async () => { cancelled = true; },
    releaseLock() {},
  };
  await assert.rejects(
    fetchReplayBytes("/streamed-large.json", {
      baseUrl: "https://game.test/index.html",
      fetchImpl: async () => ({ ok: true, status: 200, headers: { get: () => null }, body: { getReader: () => oversizedReader } }),
    }),
    error => error instanceof ReplayDecodeError && error.code === "file_too_large",
  );
  assert.equal(streamed, WEB_REPLAY_LIMITS.maxFileBytes / repeatedChunk.byteLength + 1);
  assert.equal(cancelled, true);

  const chunks = [new Uint8Array([1, 2]), new Uint8Array([3])];
  const reader = {
    read: async () => chunks.length ? { done: false, value: chunks.shift() } : { done: true },
    releaseLock() {},
  };
  const bytes = await fetchReplayBytes("replay.json", {
    baseUrl: "https://game.test/play/index.html",
    fetchImpl: async () => ({ ok: true, status: 200, headers: { get: () => null }, body: { getReader: () => reader } }),
  });
  assert.deepEqual([...bytes], [1, 2, 3]);
});

const wasmStateDescriptor = requiredBytes => ({
  schema: "stasis.replay_state_snapshot.v2",
  abi_version: 2,
  support: "canonical_bytes",
  byte_order: "little_endian",
  hash_scope: "simulation_after_tick",
  required_bytes: requiredBytes,
  entries: requiredBytes === 0 ? [] : [{
    kind: "scalar", path: "score", field: "", storage_type: "i32",
    offset: 0, element_count: 1, element_bytes: 4,
  }],
  unsupported_paths: [],
  size_operation: "stasis_replay_state_snapshot_size",
  write_operation: "stasis_replay_state_snapshot_write",
  restore_operation: "stasis_replay_state_snapshot_restore",
});

test("Wasm replay bridge rejects descriptor/ABI size drift before using memory", () => {
  const memory = new WebAssembly.Memory({ initial: 1 });
  assert.throws(() => createWasmReplayBridge({
    descriptor: wasmStateDescriptor(4),
    exports: {
      memory,
      stasis_replay_state_snapshot_size: () => 8,
      stasis_replay_state_snapshot_write: () => 4,
      stasis_replay_state_snapshot_restore: () => 4,
    },
  }), /does not match descriptor/);
});

test("Wasm replay bridge reserves a stable tail and roundtrips exact snapshot bytes", () => {
  const memory = new WebAssembly.Memory({ initial: 1 });
  let state = new Uint8Array([1, 0, 0, 0]);
  const bridge = createWasmReplayBridge({
    descriptor: wasmStateDescriptor(4),
    exports: {
      memory,
      stasis_replay_state_snapshot_size: () => 4,
      stasis_replay_state_snapshot_write: (pointer, bytes) => {
        new Uint8Array(memory.buffer, pointer, bytes).set(state);
        return bytes;
      },
      stasis_replay_state_snapshot_restore: (pointer, bytes) => {
        state = new Uint8Array(memory.buffer, pointer, bytes).slice();
        return bytes;
      },
    },
  });
  assert.equal(bridge.scratchPointer, 65_536);
  assert.deepEqual([...bridge.readSnapshotBytes()], [1, 0, 0, 0]);
  const firstHash = bridge.hashAdapter.hashState();
  assert.equal(bridge.writeSnapshotBytes(new Uint8Array([2, 0, 0, 0])), 4);
  assert.deepEqual([...state], [2, 0, 0, 0]);
  assert.notEqual(bridge.hashAdapter.hashState(), firstHash);
});

test("Wasm replay bridge preserves zero-byte ABI calls and rejects bad return codes", () => {
  const memory = new WebAssembly.Memory({ initial: 1 });
  const calls = [];
  const zero = createWasmReplayBridge({
    descriptor: wasmStateDescriptor(0),
    exports: {
      memory,
      stasis_replay_state_snapshot_size: () => 0,
      stasis_replay_state_snapshot_write: (...args) => { calls.push(["write", ...args]); return 0; },
      stasis_replay_state_snapshot_restore: (...args) => { calls.push(["restore", ...args]); return 0; },
    },
  });
  assert.deepEqual([...zero.readSnapshotBytes()], []);
  assert.equal(zero.writeSnapshotBytes(new Uint8Array()), 0);
  assert.deepEqual(calls, [["write", 0, 0], ["restore", 0, 0]]);

  const bad = createWasmReplayBridge({
    descriptor: wasmStateDescriptor(4),
    exports: {
      memory: new WebAssembly.Memory({ initial: 1 }),
      stasis_replay_state_snapshot_size: () => 4,
      stasis_replay_state_snapshot_write: () => -1,
      stasis_replay_state_snapshot_restore: () => 4,
    },
  });
  assert.throws(() => bad.readSnapshotBytes(), /snapshot write returned -1/);
});

test("checkpoint and final verification report only the first bounded divergence", () => {
  const source = document({
    total_ticks: 256,
    checkpoints: [{ tick: 256, state_sha256: HASH_B }],
    finalHash: HASH_B,
  });
  let hashCalls = 0;
  const adapter = createCanonicalStateHashAdapter(({ phase, tick }) => {
    hashCalls += 1;
    return phase === "initial" ? HASH_A : (tick === 256 ? HASH_C : HASH_A);
  });
  const controller = createReplayController(source, { hashAdapter: adapter });
  controller.initialize();
  const i32 = new Int32Array(2);
  const f32 = new Float32Array(2);
  for (let tick = 1; tick <= 256; tick += 1) controller.applyHostFrame(tick, i32, f32);
  assert.throws(() => controller.verifyTick(256), error => {
    assert.ok(error instanceof ReplayDivergenceError);
    assert.equal(error.tick, 256);
    assert.equal(error.intervalStart, 1);
    assert.match(error.message, /within ticks 1..=256/);
    assert.equal(error.message.length <= 512, true);
    return true;
  });
  assert.equal(hashCalls, 2);
  assert.throws(() => controller.verifyTick(256), ReplayDivergenceError, "first divergence is sticky");
});

test("malformed RLE and checkpoint data are rejected before allocation-heavy playback", () => {
  const source = document();
  assert.throws(() => decodeReplay({ ...source, segments: [{ ...source.segments[0], tick_gap: 1 }] }), /tick_gap/);
  assert.throws(() => decodeReplay({ ...source, segments: [{ ...source.segments[0], i32_changes: [{ slot: 0, value: 11 }] }] }), /redundant/);
  assert.throws(() => decodeReplay({ ...source, final_state: { tick: 4, state_sha256: HASH_A } }), /final_state.tick/);
});
