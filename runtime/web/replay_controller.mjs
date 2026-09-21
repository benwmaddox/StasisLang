// Portable compact-replay controller for packaged Web hosts.
//
// This module deliberately contains no DOM or WebAssembly assumptions. The
// Web template can use the controller as the single HostFrame source while
// retaining its ordinary tick -> render loop, and tests can provide a tiny
// fake hash adapter. The JSON shape is the schema-v2 shape emitted by the
// desktop recorder in apps/stasis/src/record_replay.rs.

export const WEB_REPLAY_LIMITS = Object.freeze({
  schemaVersion: 2,
  maxFileBytes: 256 * 1024 * 1024,
  maxTicks: 1_000_000,
  maxHostValues: 4_096,
  maxCheckpoints: 4_096,
  maxTextBytes: 4_096,
  maxChangesPerSegment: 8_192,
  checkpointInterval: 256,
  maxDiagnosticBytes: 512,
  hostSchemaVersion: 4,
});

const HASH_RE = /^[0-9a-f]{64}$/;
const HASH_SCOPE = "simulation_after_tick";
const OPTIONAL = Symbol("optional");

export class ReplayDecodeError extends Error {
  constructor(message, code = "invalid_replay") {
    super(message);
    this.name = "ReplayDecodeError";
    this.code = code;
    this.stasisReplayError = true;
  }
}

export class ReplayIdentityError extends ReplayDecodeError {
  constructor(field, expected, actual) {
    super(`replay identity mismatch for ${field}: expected ${String(expected)}, found ${String(actual)}`, "identity_mismatch");
    this.name = "ReplayIdentityError";
    this.field = field;
    this.expected = expected;
    this.actual = actual;
  }
}

export class ReplayDivergenceError extends Error {
  constructor(message, details = {}) {
    super(message);
    this.name = "ReplayDivergenceError";
    this.code = "replay_diverged";
    this.stasisReplayError = true;
    Object.assign(this, details);
  }
}

const fail = (message, code) => { throw new ReplayDecodeError(message, code); };

const isObject = value => value !== null && typeof value === "object" && !Array.isArray(value);
const isPlainObject = value => isObject(value)
  && (Object.getPrototypeOf(value) === Object.prototype || Object.getPrototypeOf(value) === null);

const ownKeys = value => Object.keys(value);
const checkKeys = (value, required, optional = []) => {
  if (!isPlainObject(value)) fail("replay value must be a plain object", "invalid_object");
  const allowed = new Set([...required, ...optional]);
  for (const key of ownKeys(value)) {
    if (!allowed.has(key)) fail(`replay object contains unknown field ${JSON.stringify(key)}`, "unknown_field");
  }
  for (const key of required) {
    if (!Object.prototype.hasOwnProperty.call(value, key)) fail(`replay object is missing field ${JSON.stringify(key)}`, "missing_field");
  }
};

const textBytes = value => {
  if (typeof TextEncoder === "function") return new TextEncoder().encode(value).byteLength;
  return value.length;
};

const text = (value, label, { hash = false, optional = false } = {}) => {
  if (optional && value === null) return null;
  if (typeof value !== "string" || value.length === 0) fail(`${label} must be non-empty text`, "invalid_text");
  if (textBytes(value) > WEB_REPLAY_LIMITS.maxTextBytes) fail(`${label} exceeds ${WEB_REPLAY_LIMITS.maxTextBytes} bytes`, "text_too_large");
  if (hash && !HASH_RE.test(value)) fail(`${label} must be lowercase 64-hex`, "invalid_hash");
  return value;
};

const integer = (value, label, min, max) => {
  if (!Number.isSafeInteger(value) || value < min || value > max) fail(`${label} must be an integer in ${min}..${max}`, "invalid_integer");
  return value;
};

const array = (value, label, max) => {
  if (!Array.isArray(value)) fail(`${label} must be an array`, "invalid_array");
  if (value.length > max) fail(`${label} exceeds ${max} entries`, "array_too_large");
  return value;
};

const cloneArray = value => value.slice();

function decodeBytes(source) {
  if (typeof source === "string") {
    if (textBytes(source) > WEB_REPLAY_LIMITS.maxFileBytes) fail("replay JSON exceeds the 256 MiB limit", "file_too_large");
    return source;
  }
  if (source instanceof ArrayBuffer) {
    if (source.byteLength > WEB_REPLAY_LIMITS.maxFileBytes) fail("replay bytes exceed the 256 MiB limit", "file_too_large");
    return new TextDecoder("utf-8", { fatal: true }).decode(new Uint8Array(source));
  }
  if (ArrayBuffer.isView(source)) {
    if (source.byteLength > WEB_REPLAY_LIMITS.maxFileBytes) fail("replay bytes exceed the 256 MiB limit", "file_too_large");
    return new TextDecoder("utf-8", { fatal: true }).decode(new Uint8Array(source.buffer, source.byteOffset, source.byteLength));
  }
  return source;
}

const parseRoot = source => {
  const bytes = decodeBytes(source);
  if (typeof bytes === "string") {
    try { return JSON.parse(bytes); } catch (error) { fail(`failed to parse replay JSON: ${error.message}`, "invalid_json"); }
  }
  return bytes;
};

function decodeIdentity(value) {
  checkKeys(value,
    ["stasis_version", "release_id", "target", "source_sha256", "state_layout_sha256",
      "compiler_layout_sha256", "runtime_sha256", "host_schema_version", "host_i32_count",
      "host_f32_count", "input_usage_sha256", "tick_rate_hz", "hash_scope",
      "determinism_profile", "observed_i32", "observed_f32"],
    ["asset_manifest_sha256", "controller_schema_version"]);
  const identity = {
    stasis_version: text(value.stasis_version, "identity stasis_version"),
    release_id: text(value.release_id, "identity release_id"),
    target: text(value.target, "identity target"),
    source_sha256: text(value.source_sha256, "identity source_sha256", { hash: true }),
    state_layout_sha256: text(value.state_layout_sha256, "identity state_layout_sha256", { hash: true }),
    compiler_layout_sha256: text(value.compiler_layout_sha256, "identity compiler_layout_sha256", { hash: true }),
    runtime_sha256: text(value.runtime_sha256, "identity runtime_sha256", { hash: true }),
    asset_manifest_sha256: value.asset_manifest_sha256 === undefined || value.asset_manifest_sha256 === null
      ? null : text(value.asset_manifest_sha256, "identity asset_manifest_sha256", { hash: true }),
    host_schema_version: integer(value.host_schema_version, "identity host_schema_version", 1, Number.MAX_SAFE_INTEGER),
    host_i32_count: integer(value.host_i32_count, "identity host_i32_count", 1, WEB_REPLAY_LIMITS.maxHostValues),
    host_f32_count: integer(value.host_f32_count, "identity host_f32_count", 1, WEB_REPLAY_LIMITS.maxHostValues),
    input_usage_sha256: text(value.input_usage_sha256, "identity input_usage_sha256", { hash: true }),
    tick_rate_hz: integer(value.tick_rate_hz, "identity tick_rate_hz", 1, Number.MAX_SAFE_INTEGER),
    hash_scope: value.hash_scope,
    determinism_profile: text(value.determinism_profile, "identity determinism_profile"),
    controller_schema_version: value.controller_schema_version === undefined || value.controller_schema_version === null
      ? null : integer(value.controller_schema_version, "identity controller_schema_version", 1, Number.MAX_SAFE_INTEGER),
    observed_i32: [], observed_f32: [],
  };
  if (identity.hash_scope !== HASH_SCOPE) fail(`identity hash_scope must be ${HASH_SCOPE}`, "invalid_hash_scope");
  for (const [key, output] of [["observed_i32", identity.observed_i32], ["observed_f32", identity.observed_f32]]) {
    const fields = array(value[key], `identity ${key}`, WEB_REPLAY_LIMITS.maxHostValues);
    let previousIndex = -1;
    fields.forEach((field, index) => {
      checkKeys(field, ["slot", "index", "path", "family"]);
      const slot = integer(field.slot, `${key}[${index}].slot`, 0, WEB_REPLAY_LIMITS.maxHostValues - 1);
      const fieldIndex = integer(field.index, `${key}[${index}].index`, 0, identity[key === "observed_i32" ? "host_i32_count" : "host_f32_count"] - 1);
      if (slot !== index) fail(`${key}[${index}] slot is not canonical position ${index}`, "noncanonical_descriptor");
      if (fieldIndex <= previousIndex) fail(`${key} descriptors must be sorted and unique`, "noncanonical_descriptor");
      previousIndex = fieldIndex;
      output.push({ slot, index: fieldIndex, path: text(field.path, `${key}[${index}].path`), family: text(field.family, `${key}[${index}].family`) });
    });
  }
  return identity;
}

function decodeScalar(value, entryIndex) {
  checkKeys(value, ["type_name", "bits"]);
  const typeName = text(value.type_name, `initial state entry ${entryIndex} type_name`);
  const widths = { i32: 8, f32: 8, u32: 8, f64: 16, bool: 2, u8: 2, u16: 4 };
  const width = widths[typeName];
  if (!width) fail(`initial state entry ${entryIndex} has unsupported scalar type ${JSON.stringify(typeName)}`, "invalid_scalar");
  const bits = text(value.bits, `initial state entry ${entryIndex} bits`);
  if (bits.length !== width || !/^[0-9a-f]+$/.test(bits)) fail(`initial state entry ${entryIndex} bits has invalid width or encoding`, "invalid_scalar");
  const parsed = Number.parseInt(bits, 16);
  if ((typeName === "bool" && parsed > 1) || (typeName === "u8" && parsed > 0xff) || (typeName === "u16" && parsed > 0xffff)) {
    fail(`initial state entry ${entryIndex} ${typeName} value is out of range`, "invalid_scalar");
  }
  return { type_name: typeName, bits };
}

function decodeInitialState(value) {
  checkKeys(value, ["values", "state_sha256"]);
  const values = array(value.values, "initial_state.values", WEB_REPLAY_LIMITS.maxTicks).map((entry, index) => {
    checkKeys(entry, ["location", "value"]);
    checkKeys(entry.location, ["kind"], ["path", "field", "index"]);
    let location;
    if (entry.location.kind === "scalar") {
      checkKeys(entry.location, ["kind", "path"]);
      location = { kind: "scalar", path: text(entry.location.path, `initial state entry ${index} scalar path`) };
    } else if (entry.location.kind === "collection") {
      checkKeys(entry.location, ["kind", "path", "field", "index"]);
      location = {
        kind: "collection",
        path: text(entry.location.path, `initial state entry ${index} collection path`),
        field: text(entry.location.field, `initial state entry ${index} collection field`),
        index: integer(entry.location.index, `initial state entry ${index} collection index`, 0, 0x7fffffff),
      };
    } else fail(`initial state entry ${index} has unknown location kind`, "invalid_location");
    return { location, value: decodeScalar(entry.value, index) };
  });
  return { values, state_sha256: text(value.state_sha256, "initial_state.state_sha256", { hash: true }) };
}

const decodeI32Change = (change, label, count) => {
  checkKeys(change, ["slot", "value"]);
  return { slot: integer(change.slot, `${label}.slot`, 0, count - 1), value: integer(change.value, `${label}.value`, -0x80000000, 0x7fffffff) };
};
const decodeF32Change = (change, label, count) => {
  checkKeys(change, ["slot", "bits"]);
  return { slot: integer(change.slot, `${label}.slot`, 0, count - 1), bits: integer(change.bits, `${label}.bits`, 0, 0xffffffff) };
};

function decodeCompactDocument(root) {
  checkKeys(root, ["schema_version", "identity", "initial_state", "initial_input", "segments", "checkpoints", "total_ticks", "final_state"]);
  if (root.schema_version !== WEB_REPLAY_LIMITS.schemaVersion) fail(`unsupported compact replay schema ${root.schema_version} (expected 2)`, "unsupported_schema");
  const identity = decodeIdentity(root.identity);
  const initial_state = decodeInitialState(root.initial_state);
  checkKeys(root.initial_input, ["i32_values", "f32_bits"]);
  const initialInputI32 = array(root.initial_input.i32_values, "initial_input.i32_values", WEB_REPLAY_LIMITS.maxHostValues);
  const initialInputF32 = array(root.initial_input.f32_bits, "initial_input.f32_bits", WEB_REPLAY_LIMITS.maxHostValues);
  if (initialInputI32.length !== identity.observed_i32.length || initialInputF32.length !== identity.observed_f32.length) {
    fail("initial_input lengths must match observed field descriptors", "input_dimensions");
  }
  const initial_input = {
    i32_values: initialInputI32.map((value, index) => integer(value, `initial_input.i32_values[${index}]`, -0x80000000, 0x7fffffff)),
    f32_bits: initialInputF32.map((value, index) => integer(value, `initial_input.f32_bits[${index}]`, 0, 0xffffffff)),
  };
  const total_ticks = integer(root.total_ticks, "total_ticks", 1, WEB_REPLAY_LIMITS.maxTicks);
  const segments = array(root.segments, "segments", WEB_REPLAY_LIMITS.maxTicks);
  if (segments.length === 0) fail("compact replay must contain at least one input segment", "empty_segments");
  const snapshot = { i32_values: cloneArray(initial_input.i32_values), f32_bits: cloneArray(initial_input.f32_bits) };
  let cursor = 0;
  const decodedSegments = segments.map((segment, segmentIndex) => {
    checkKeys(segment, ["tick_gap", "run_ticks", "i32_changes", "f32_changes"]);
    const tick_gap = integer(segment.tick_gap, `segments[${segmentIndex}].tick_gap`, 0, WEB_REPLAY_LIMITS.maxTicks);
    const run_ticks = integer(segment.run_ticks, `segments[${segmentIndex}].run_ticks`, 1, WEB_REPLAY_LIMITS.maxTicks);
    if (segmentIndex === 0 && tick_gap !== 0) fail("first compact replay segment must have tick_gap=0", "invalid_tick_gap");
    const i32Changes = array(segment.i32_changes, `segments[${segmentIndex}].i32_changes`, WEB_REPLAY_LIMITS.maxChangesPerSegment);
    const f32Changes = array(segment.f32_changes, `segments[${segmentIndex}].f32_changes`, WEB_REPLAY_LIMITS.maxChangesPerSegment);
    if (i32Changes.length + f32Changes.length > WEB_REPLAY_LIMITS.maxChangesPerSegment) fail(`segment ${segmentIndex} has too many changes`, "changes_too_large");
    let previous = -1;
    const i32_changes = i32Changes.map((change, changeIndex) => {
      const decoded = decodeI32Change(change, `segments[${segmentIndex}].i32_changes[${changeIndex}]`, initial_input.i32_values.length);
      if (decoded.slot <= previous) fail(`segment ${segmentIndex} i32 changes must be sorted and unique`, "noncanonical_change");
      if (snapshot.i32_values[decoded.slot] === decoded.value) fail(`segment ${segmentIndex} i32 change at slot ${decoded.slot} is redundant`, "redundant_change");
      previous = decoded.slot;
      return decoded;
    });
    previous = -1;
    const f32_changes = f32Changes.map((change, changeIndex) => {
      const decoded = decodeF32Change(change, `segments[${segmentIndex}].f32_changes[${changeIndex}]`, initial_input.f32_bits.length);
      if (decoded.slot <= previous) fail(`segment ${segmentIndex} f32 changes must be sorted and unique`, "noncanonical_change");
      if (snapshot.f32_bits[decoded.slot] === decoded.bits) fail(`segment ${segmentIndex} f32 change at slot ${decoded.slot} is redundant`, "redundant_change");
      previous = decoded.slot;
      return decoded;
    });
    const gapEnd = cursor + tick_gap;
    const runEnd = gapEnd + run_ticks;
    if (!Number.isSafeInteger(gapEnd) || !Number.isSafeInteger(runEnd) || runEnd > total_ticks) fail(`segment ${segmentIndex} exceeds total_ticks`, "segment_coverage");
    i32_changes.forEach(change => { snapshot.i32_values[change.slot] = change.value; });
    f32_changes.forEach(change => { snapshot.f32_bits[change.slot] = change.bits; });
    cursor = runEnd;
    return { tick_gap, run_ticks, i32_changes, f32_changes };
  });
  if (cursor !== total_ticks) fail(`compact replay segment coverage ends at tick ${cursor}, expected ${total_ticks}`, "segment_coverage");
  const checkpoints = array(root.checkpoints, "checkpoints", WEB_REPLAY_LIMITS.maxCheckpoints).map((checkpoint, index) => {
    checkKeys(checkpoint, ["tick", "state_sha256"]);
    const tick = integer(checkpoint.tick, `checkpoints[${index}].tick`, 1, total_ticks);
    const expected = (index + 1) * WEB_REPLAY_LIMITS.checkpointInterval;
    if (tick !== expected) fail(`checkpoint ${index} must be at tick ${expected}, found ${tick}`, "checkpoint_sequence");
    return { tick, state_sha256: text(checkpoint.state_sha256, `checkpoints[${index}].state_sha256`, { hash: true }) };
  });
  if (checkpoints.length !== Math.floor(total_ticks / WEB_REPLAY_LIMITS.checkpointInterval)) fail(`compact replay must contain exactly ${Math.floor(total_ticks / WEB_REPLAY_LIMITS.checkpointInterval)} checkpoints`, "checkpoint_count");
  checkKeys(root.final_state, ["tick", "state_sha256"]);
  const final_state = { tick: integer(root.final_state.tick, "final_state.tick", total_ticks, total_ticks), state_sha256: text(root.final_state.state_sha256, "final_state.state_sha256", { hash: true }) };
  if (checkpoints.at(-1)?.tick === total_ticks && checkpoints.at(-1).state_sha256 !== final_state.state_sha256) fail("final checkpoint does not match final state", "checkpoint_final_mismatch");
  return { schema_version: 2, identity, initial_state, initial_input, segments: decodedSegments, checkpoints, total_ticks, final_state };
}

export function decodeReplay(source) {
  const root = parseRoot(source);
  if (!isPlainObject(root)) fail("replay document must be a plain object", "invalid_document");
  return decodeCompactDocument(root);
}

export async function decodeReplayInput(source) {
  if (source && typeof source.text === "function" && !(source instanceof String)) {
    const value = await source.text();
    return decodeReplay(value);
  }
  if (source && typeof source.arrayBuffer === "function") return decodeReplay(await source.arrayBuffer());
  return decodeReplay(source);
}

const normalizeHash = value => {
  if (typeof value === "string") {
    if (!HASH_RE.test(value)) throw new Error("canonical state hash ABI returned a non-sha256 value");
    return value;
  }
  let bytes = null;
  if (value instanceof ArrayBuffer) bytes = new Uint8Array(value);
  else if (ArrayBuffer.isView(value)) bytes = new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
  else if (Array.isArray(value) && value.every(entry => Number.isInteger(entry) && entry >= 0 && entry <= 255)) bytes = Uint8Array.from(value);
  if (!bytes || bytes.length !== 32) throw new Error("canonical state hash ABI must return 64-hex text or 32 bytes");
  return Array.from(bytes, byte => byte.toString(16).padStart(2, "0")).join("");
};

export function createCanonicalStateHashAdapter(hashState) {
  const fn = typeof hashState === "function" ? hashState : hashState?.hashState;
  if (typeof fn !== "function") throw new TypeError("a canonical state hash adapter requires hashState()");
  return Object.freeze({ hashState: (...args) => normalizeHash(fn(...args)) });
}

const compareIdentity = (actual, expected) => {
  if (!expected) return;
  const keys = ["stasis_version", "release_id", "target", "source_sha256", "state_layout_sha256", "compiler_layout_sha256", "runtime_sha256", "asset_manifest_sha256", "host_schema_version", "host_i32_count", "host_f32_count", "input_usage_sha256", "tick_rate_hz", "hash_scope", "determinism_profile", "controller_schema_version"];
  for (const key of keys) {
    if (expected[key] !== undefined && expected[key] !== actual[key]) throw new ReplayIdentityError(`identity.${key}`, expected[key], actual[key]);
  }
  if (expected.observed_i32 !== undefined && JSON.stringify(expected.observed_i32) !== JSON.stringify(actual.observed_i32)) throw new ReplayIdentityError("identity.observed_i32", expected.observed_i32, actual.observed_i32);
  if (expected.observed_f32 !== undefined && JSON.stringify(expected.observed_f32) !== JSON.stringify(actual.observed_f32)) throw new ReplayIdentityError("identity.observed_f32", expected.observed_f32, actual.observed_f32);
};

const truncateDiagnostic = message => {
  const value = String(message);
  return value.length > WEB_REPLAY_LIMITS.maxDiagnosticBytes ? value.slice(0, WEB_REPLAY_LIMITS.maxDiagnosticBytes) : value;
};

function resetCursor(document) {
  return {
    snapshot: { i32_values: cloneArray(document.initial_input.i32_values), f32_bits: cloneArray(document.initial_input.f32_bits) },
    segmentIndex: 0,
    segmentCursor: 0,
    segmentApplied: false,
    nextTick: 1,
    nextCheckpoint: 0,
    lastVerifiedTick: 0,
    firstDivergence: null,
    completed: false,
  };
}

export function createReplayController(source, options = {}) {
  // Always normalize and validate caller input. A plain object that merely
  // resembles a decoded document must not bypass bounds or canonical checks.
  const document = decodeReplay(source);
  compareIdentity(document.identity, options.identity ?? options.packageIdentity);
  const hashAdapter = options.hashAdapter ? createCanonicalStateHashAdapter(options.hashAdapter) : null;
  let cursor = resetCursor(document);
  const applyChangesForTick = tick => {
    while (true) {
      const segment = document.segments[cursor.segmentIndex];
      if (!segment) throw new ReplayDecodeError(`compact replay has no segment for tick ${tick}`, "segment_coverage");
      const gapEnd = cursor.segmentCursor + segment.tick_gap;
      if (tick <= gapEnd) return;
      const runEnd = gapEnd + segment.run_ticks;
      if (tick <= runEnd) {
        if (!cursor.segmentApplied) {
          for (const change of segment.i32_changes) cursor.snapshot.i32_values[change.slot] = change.value;
          for (const change of segment.f32_changes) cursor.snapshot.f32_bits[change.slot] = change.bits;
          cursor.segmentApplied = true;
        }
        return;
      }
      cursor.segmentIndex += 1;
      cursor.segmentCursor = runEnd;
      cursor.segmentApplied = false;
    }
  };
  const writeF32Bits = (target, index, bits) => {
    if (target instanceof Float32Array) {
      new Uint32Array(target.buffer, target.byteOffset, target.length)[index] = bits;
      return;
    }
    target[index] = new DataView(new Uint32Array([bits]).buffer).getFloat32(0, true);
  };
  const applyHostFrame = (tick, hostI32, hostF32) => {
    if (!(hostI32 instanceof Int32Array) || !(hostF32 instanceof Float32Array)) throw new TypeError("replay HostFrame requires Int32Array and Float32Array targets");
    if (hostI32.length !== document.identity.host_i32_count || hostF32.length !== document.identity.host_f32_count) throw new ReplayDecodeError("active HostFrame dimensions do not match replay", "input_dimensions");
    if (tick !== cursor.nextTick) throw new ReplayDecodeError(`compact replay tick sequence mismatch: expected ${cursor.nextTick}, found ${tick}`, "tick_sequence");
    applyChangesForTick(tick);
    hostI32.fill(0);
    hostF32.fill(0);
    for (const field of document.identity.observed_i32) hostI32[field.index] = cursor.snapshot.i32_values[field.slot];
    for (const field of document.identity.observed_f32) writeF32Bits(hostF32, field.index, cursor.snapshot.f32_bits[field.slot]);
    cursor.nextTick += 1;
    return { tick, mode: "replay" };
  };
  const expectedAtTick = tick => {
    if (tick === document.final_state.tick) return document.final_state.state_sha256;
    const checkpoint = document.checkpoints[cursor.nextCheckpoint];
    return checkpoint?.tick === tick ? checkpoint.state_sha256 : null;
  };
  const verifyTick = (tick, context = undefined) => {
    if (cursor.firstDivergence) throw new ReplayDivergenceError(cursor.firstDivergence, { diagnostic: cursor.firstDivergence });
    const expected = expectedAtTick(tick);
    if (!expected) return { verified: false, tick };
    if (tick !== cursor.nextTick - 1 || tick < 1) throw new ReplayDecodeError(`compact replay verification sequence mismatch: expected ${cursor.nextTick - 1}, found ${tick}`, "tick_sequence");
    const actual = hashAdapter?.hashState({ tick, phase: "simulation_after_tick", context });
    if (!actual) throw new Error(`replay checkpoint ${tick} requires a canonical state hash adapter`);
    if (actual !== expected) {
      const intervalStart = cursor.lastVerifiedTick + 1;
      const diagnostic = truncateDiagnostic(`replay diverged within ticks ${intervalStart}..=${tick}; detected at checkpoint ${tick}: expected state ${expected}, found ${actual}`);
      cursor.firstDivergence = diagnostic;
      throw new ReplayDivergenceError(diagnostic, { tick, expected, actual, intervalStart, diagnostic });
    }
    if (tick === document.final_state.tick) cursor.completed = true;
    if (cursor.nextCheckpoint < document.checkpoints.length && document.checkpoints[cursor.nextCheckpoint].tick === tick) cursor.nextCheckpoint += 1;
    cursor.lastVerifiedTick = tick;
    return { verified: true, tick, final: tick === document.final_state.tick };
  };
  const initialize = context => {
    if (context?.restoreInitialState) context.restoreInitialState(document.initial_state, document);
    if (hashAdapter) {
      const actual = hashAdapter.hashState({ tick: 0, phase: "initial", context });
      if (actual !== document.initial_state.state_sha256) throw new ReplayDivergenceError(`replay initial state mismatch: expected ${document.initial_state.state_sha256}, found ${actual}`, { tick: 0, expected: document.initial_state.state_sha256, actual });
    }
    return { initialized: true };
  };
  const reset = context => { cursor = resetCursor(document); if (context?.restoreInitialState) return initialize(context); return { initialized: false }; };
  const runTick = ({ tick, hostI32, hostF32, tickFn, renderFn, context } = {}) => {
    applyHostFrame(tick, hostI32, hostF32);
    const tickResult = tickFn();
    const verification = verifyTick(tick, context);
    const renderResult = renderFn();
    return { tickResult, renderResult, verification };
  };
  return Object.freeze({
    mode: "replay",
    document,
    frameCount: document.total_ticks,
    initialize,
    reset,
    applyHostFrame,
    verifyTick,
    runTick,
    get nextTick() { return cursor.nextTick; },
    get lastVerifiedTick() { return cursor.lastVerifiedTick; },
    get firstDivergence() { return cursor.firstDivergence; },
    get completed() { return cursor.completed; },
  });
}

export function createHostFrameSource({ replay = null, live } = {}) {
  if (replay) return Object.freeze({ mode: "replay", write: (tick, hostI32, hostF32) => replay.applyHostFrame(tick, hostI32, hostF32) });
  if (typeof live !== "function") throw new TypeError("live HostFrame source requires writeLive()");
  return Object.freeze({ mode: "live", write: (tick, hostI32, hostF32, timestamp) => live(timestamp, tick, hostI32, hostF32) });
}
