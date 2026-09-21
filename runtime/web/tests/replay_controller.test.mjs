import assert from "node:assert/strict";
import test from "node:test";
import {
  ReplayDecodeError,
  ReplayDivergenceError,
  ReplayIdentityError,
  createCanonicalStateHashAdapter,
  createHostFrameSource,
  createReplayController,
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

test("schema-v2 decoder accepts object and JSON input while enforcing strict nested fields", () => {
  const source = document();
  assert.deepEqual(decodeReplay(source), { ...source, identity: { ...source.identity, asset_manifest_sha256: null, controller_schema_version: null } });
  assert.deepEqual(decodeReplay(JSON.stringify(source)), { ...source, identity: { ...source.identity, asset_manifest_sha256: null, controller_schema_version: null } });
  assert.throws(() => decodeReplay({ ...source, unexpected: true }), ReplayDecodeError);
  assert.throws(() => decodeReplay(JSON.stringify({ ...source, identity: { ...source.identity, observed_i32: source.identity.observed_i32.map(field => ({ ...field, extra: 1 })) } })), ReplayDecodeError);
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
