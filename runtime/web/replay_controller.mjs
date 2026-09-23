// Portable compact-replay controller for packaged Web hosts.
//
// This module deliberately contains no DOM or WebAssembly assumptions. The
// Web template can use the controller as the single HostFrame source while
// retaining its ordinary tick -> render loop, and tests can provide a tiny
// fake hash adapter. Schema-v2 remains supported for exact target/runtime
// playback; schema-v3 adds a portable compatibility contract and keeps
// producer/consumer details as audit provenance.

export const WEB_REPLAY_LIMITS = Object.freeze({
  schemaVersion: 3,
  legacySchemaVersion: 2,
  // JSON text, UTF-16 decoding, and the parsed object graph coexist briefly.
  // Keep the browser ceiling below the native 256 MiB file cap to avoid a
  // several-hundred-megabyte transient allocation on constrained devices.
  maxFileBytes: 64 * 1024 * 1024,
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

export async function fetchReplayBytes(source, { fetchImpl = globalThis.fetch, baseUrl = globalThis.location?.href } = {}) {
  if (typeof fetchImpl !== "function") throw new TypeError("replay loading requires fetch()");
  if (!baseUrl) throw new ReplayDecodeError("replay loading requires a document URL", "invalid_replay_url");
  let url;
  try { url = new URL(source, baseUrl); } catch {
    throw new ReplayDecodeError("stasis-replay is not a valid URL", "invalid_replay_url");
  }
  const base = new URL(baseUrl);
  if (url.origin !== base.origin) throw new ReplayDecodeError("stasis-replay must use the package origin", "cross_origin_replay");
  const response = await fetchImpl(url.href, { credentials: "same-origin", cache: "no-store" });
  if (!response?.ok) throw new ReplayDecodeError(`failed to load replay: HTTP ${response?.status ?? "unknown"}`, "replay_fetch_failed");
  const declared = response.headers?.get?.("content-length");
  if (declared !== null && declared !== undefined) {
    const length = Number(declared);
    if (!Number.isSafeInteger(length) || length < 0 || length > WEB_REPLAY_LIMITS.maxFileBytes) {
      throw new ReplayDecodeError("replay response exceeds the 64 MiB Web limit", "file_too_large");
    }
  }
  if (!response.body?.getReader) throw new ReplayDecodeError("replay response is not streamable", "replay_stream_required");
  const reader = response.body.getReader();
  const chunks = [];
  let total = 0;
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      if (!(value instanceof Uint8Array)) throw new ReplayDecodeError("replay response returned invalid bytes", "replay_fetch_failed");
      total += value.byteLength;
      if (total > WEB_REPLAY_LIMITS.maxFileBytes) {
        await reader.cancel();
        throw new ReplayDecodeError("replay response exceeds the 64 MiB Web limit", "file_too_large");
      }
      chunks.push(value);
    }
  } finally {
    reader.releaseLock?.();
  }
  const bytes = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
  return bytes;
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

const text = (value, label, { hash = false, optional = false, allowEmpty = false } = {}) => {
  if (optional && value === null) return null;
  if (typeof value !== "string" || (!allowEmpty && value.length === 0)) fail(`${label} must be non-empty text`, "invalid_text");
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
    if (textBytes(source) > WEB_REPLAY_LIMITS.maxFileBytes) fail("replay JSON exceeds the 64 MiB Web limit", "file_too_large");
    return source;
  }
  if (source instanceof ArrayBuffer) {
    if (source.byteLength > WEB_REPLAY_LIMITS.maxFileBytes) fail("replay bytes exceed the 64 MiB Web limit", "file_too_large");
    return new TextDecoder("utf-8", { fatal: true }).decode(new Uint8Array(source));
  }
  if (ArrayBuffer.isView(source)) {
    if (source.byteLength > WEB_REPLAY_LIMITS.maxFileBytes) fail("replay bytes exceed the 64 MiB Web limit", "file_too_large");
    return new TextDecoder("utf-8", { fatal: true }).decode(new Uint8Array(source.buffer, source.byteOffset, source.byteLength));
  }
  return source;
}

// JSON.parse discards all but the last value for a duplicate object key. Scan
// the bounded source first so the Web decoder keeps the same strict contract
// as the native serde visitor instead of accepting an ambiguous replay.
function rejectDuplicateJsonKeys(source) {
  let offset = 0;
  const skipSpace = () => { while (/\s/.test(source[offset] || "")) offset += 1; };
  const parseString = () => {
    const start = offset;
    offset += 1;
    while (offset < source.length) {
      const char = source[offset++];
      if (char === '"') {
        try { return JSON.parse(source.slice(start, offset)); } catch { fail("replay JSON contains an invalid string", "invalid_json"); }
      }
      if (char === "\\") {
        if (source[offset] === "u") offset += 5;
        else offset += 1;
      } else if (char.charCodeAt(0) < 0x20) fail("replay JSON contains an invalid string", "invalid_json");
    }
    fail("replay JSON contains an unterminated string", "invalid_json");
  };
  const parseValue = depth => {
    if (depth > 64) fail("replay JSON nesting exceeds 64 levels", "json_too_deep");
    skipSpace();
    const char = source[offset];
    if (char === "{") {
      offset += 1;
      const keys = new Set();
      skipSpace();
      if (source[offset] === "}") { offset += 1; return; }
      while (true) {
        skipSpace();
        if (source[offset] !== '"') fail("replay JSON object key must be a string", "invalid_json");
        const key = parseString();
        if (keys.has(key)) fail(`replay JSON contains duplicate field ${JSON.stringify(key)}`, "duplicate_field");
        keys.add(key);
        skipSpace();
        if (source[offset++] !== ":") fail("replay JSON object key is missing ':'", "invalid_json");
        parseValue(depth + 1);
        skipSpace();
        const separator = source[offset++];
        if (separator === "}") return;
        if (separator !== ",") fail("replay JSON object is malformed", "invalid_json");
      }
    }
    if (char === "[") {
      offset += 1;
      skipSpace();
      if (source[offset] === "]") { offset += 1; return; }
      while (true) {
        parseValue(depth + 1);
        skipSpace();
        const separator = source[offset++];
        if (separator === "]") return;
        if (separator !== ",") fail("replay JSON array is malformed", "invalid_json");
      }
    }
    if (char === '"') { parseString(); return; }
    const tail = source.slice(offset);
    const literal = /^(?:true|false|null)/.exec(tail)?.[0]
      || /^-?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?/.exec(tail)?.[0];
    if (!literal) fail("replay JSON contains an invalid value", "invalid_json");
    offset += literal.length;
  };
  parseValue(0);
  skipSpace();
  if (offset !== source.length) fail("replay JSON contains trailing data", "invalid_json");
}

const parseRoot = source => {
  const bytes = decodeBytes(source);
  if (typeof bytes === "string") {
    rejectDuplicateJsonKeys(bytes);
    try { return JSON.parse(bytes); } catch (error) { fail(`failed to parse replay JSON: ${error.message}`, "invalid_json"); }
  }
  return bytes;
};

function decodeIdentityV2(value) {
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

function decodeCompatibility(value) {
  checkKeys(value,
    ["stasis_version", "release_id", "source_sha256", "state_layout_sha256",
      "compiler_layout_sha256", "host_schema_version", "host_i32_count",
      "host_f32_count", "input_usage_sha256", "tick_rate_hz", "hash_scope",
      "determinism_profile", "observed_i32", "observed_f32"],
    ["asset_manifest_sha256", "controller_schema_version"]);
  const compatibility = {
    stasis_version: text(value.stasis_version, "compatibility stasis_version"),
    release_id: text(value.release_id, "compatibility release_id"),
    source_sha256: text(value.source_sha256, "compatibility source_sha256", { hash: true }),
    state_layout_sha256: text(value.state_layout_sha256, "compatibility state_layout_sha256", { hash: true }),
    compiler_layout_sha256: text(value.compiler_layout_sha256, "compatibility compiler_layout_sha256", { hash: true }),
    asset_manifest_sha256: value.asset_manifest_sha256 === undefined || value.asset_manifest_sha256 === null
      ? null : text(value.asset_manifest_sha256, "compatibility asset_manifest_sha256", { hash: true }),
    host_schema_version: integer(value.host_schema_version, "compatibility host_schema_version", 1, Number.MAX_SAFE_INTEGER),
    host_i32_count: integer(value.host_i32_count, "compatibility host_i32_count", 1, WEB_REPLAY_LIMITS.maxHostValues),
    host_f32_count: integer(value.host_f32_count, "compatibility host_f32_count", 1, WEB_REPLAY_LIMITS.maxHostValues),
    input_usage_sha256: text(value.input_usage_sha256, "compatibility input_usage_sha256", { hash: true }),
    tick_rate_hz: integer(value.tick_rate_hz, "compatibility tick_rate_hz", 1, Number.MAX_SAFE_INTEGER),
    hash_scope: value.hash_scope,
    determinism_profile: text(value.determinism_profile, "compatibility determinism_profile"),
    controller_schema_version: value.controller_schema_version === undefined || value.controller_schema_version === null
      ? null : integer(value.controller_schema_version, "compatibility controller_schema_version", 1, Number.MAX_SAFE_INTEGER),
    observed_i32: [], observed_f32: [],
  };
  if (compatibility.hash_scope !== HASH_SCOPE) fail(`compatibility hash_scope must be ${HASH_SCOPE}`, "invalid_hash_scope");
  for (const [key, output] of [["observed_i32", compatibility.observed_i32], ["observed_f32", compatibility.observed_f32]]) {
    const fields = array(value[key], `compatibility ${key}`, WEB_REPLAY_LIMITS.maxHostValues);
    let previousIndex = -1;
    fields.forEach((field, index) => {
      checkKeys(field, ["slot", "index", "path", "family"]);
      const slot = integer(field.slot, `${key}[${index}].slot`, 0, WEB_REPLAY_LIMITS.maxHostValues - 1);
      const fieldIndex = integer(field.index, `${key}[${index}].index`, 0, compatibility[key === "observed_i32" ? "host_i32_count" : "host_f32_count"] - 1);
      if (slot !== index) fail(`${key}[${index}] slot is not canonical position ${index}`, "noncanonical_descriptor");
      if (fieldIndex <= previousIndex) fail(`${key} descriptors must be sorted and unique`, "noncanonical_descriptor");
      previousIndex = fieldIndex;
      output.push({ slot, index: fieldIndex, path: text(field.path, `${key}[${index}].path`), family: text(field.family, `${key}[${index}].family`) });
    });
  }
  return compatibility;
}

function decodeProducer(value, label) {
  checkKeys(value, ["target", "runtime_sha256"]);
  return {
    target: text(value.target, `${label} target`),
    runtime_sha256: text(value.runtime_sha256, `${label} runtime_sha256`, { hash: true }),
  };
}

function decodeIdentityV3(value) {
  checkKeys(value, ["compatibility", "producer"]);
  return {
    compatibility: decodeCompatibility(value.compatibility),
    producer: decodeProducer(value.producer, "producer"),
  };
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
        field: text(entry.location.field, `initial state entry ${index} collection field`, { allowEmpty: true }),
        index: integer(entry.location.index, `initial state entry ${index} collection index`, 0, 0x7fffffff),
      };
    } else fail(`initial state entry ${index} has unknown location kind`, "invalid_location");
    return { location, value: decodeScalar(entry.value, index) };
  });
  return { values, state_sha256: text(value.state_sha256, "initial_state.state_sha256", { hash: true }) };
}

const compareUtf8 = (left, right) => {
  const leftBytes = utf8(left);
  const rightBytes = utf8(right);
  const sharedLength = Math.min(leftBytes.length, rightBytes.length);
  for (let index = 0; index < sharedLength; index += 1) {
    if (leftBytes[index] !== rightBytes[index]) return leftBytes[index] - rightBytes[index];
  }
  return leftBytes.length - rightBytes.length;
};

function validateCanonicalInitialState(initialState) {
  let previous = null;
  initialState.values.forEach((entry, index) => {
    if (/^0+$/.test(entry.value.bits)) {
      fail(`initial state entry ${index} explicitly encodes a default value`, "noncanonical_initial_state");
    }
    if (previous) {
      let order = previous.location.kind === entry.location.kind
        ? compareUtf8(previous.location.path, entry.location.path)
        : previous.location.kind === "scalar" ? -1 : 1;
      if (order === 0 && entry.location.kind === "collection") {
        order = compareUtf8(previous.location.field, entry.location.field);
        if (order === 0) order = previous.location.index - entry.location.index;
      }
      if (order >= 0) {
        fail(`initial state entries must be sorted and unique; entry ${index} is not canonical`, "noncanonical_initial_state");
      }
    }
    previous = entry;
  });
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
  if (root.schema_version !== WEB_REPLAY_LIMITS.legacySchemaVersion && root.schema_version !== WEB_REPLAY_LIMITS.schemaVersion) {
    fail(`unsupported compact replay schema ${root.schema_version} (expected 2 or ${WEB_REPLAY_LIMITS.schemaVersion})`, "unsupported_schema");
  }
  const v3 = root.schema_version === WEB_REPLAY_LIMITS.schemaVersion;
  const identity = v3 ? decodeIdentityV3(root.identity) : decodeIdentityV2(root.identity);
  const compatibility = v3 ? identity.compatibility : identity;
  const initial_state = decodeInitialState(root.initial_state);
  if (v3) validateCanonicalInitialState(initial_state);
  checkKeys(root.initial_input, ["i32_values", "f32_bits"]);
  const initialInputI32 = array(root.initial_input.i32_values, "initial_input.i32_values", WEB_REPLAY_LIMITS.maxHostValues);
  const initialInputF32 = array(root.initial_input.f32_bits, "initial_input.f32_bits", WEB_REPLAY_LIMITS.maxHostValues);
  if (initialInputI32.length !== compatibility.observed_i32.length || initialInputF32.length !== compatibility.observed_f32.length) {
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
  const normalizedIdentity = v3
    ? { ...compatibility, compatibility, producer: identity.producer }
    : identity;
  return { schema_version: root.schema_version, identity: normalizedIdentity, initial_state, initial_input, segments: decodedSegments, checkpoints, total_ticks, final_state };
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

// This is intentionally local instead of using SubtleCrypto: verification is
// part of the synchronous tick boundary, and SubtleCrypto would make the
// guest loop asynchronous. The implementation only accepts bounded input
// from the descriptor below, so the incremental buffer never grows with the
// state snapshot.
const SHA256_K = new Uint32Array([
  0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
  0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
  0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
  0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
  0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
  0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
  0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
  0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
]);

const shaRight = (value, bits) => value >>> bits;
const shaRotate = (value, bits) => (value >>> bits) | (value << (32 - bits));

class IncrementalSha256 {
  constructor() {
    this.state = new Uint32Array([0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19]);
    this.buffer = new Uint8Array(64);
    this.bufferLength = 0;
    this.bytesHashed = 0;
    this.finished = false;
    this.schedule = new Uint32Array(64);
  }

  update(source) {
    if (this.finished) throw new Error("SHA-256 digest already finalized");
    const bytes = source instanceof Uint8Array
      ? source : new Uint8Array(source.buffer, source.byteOffset, source.byteLength);
    this.bytesHashed += bytes.byteLength;
    let offset = 0;
    if (this.bufferLength !== 0) {
      const copied = Math.min(64 - this.bufferLength, bytes.length);
      this.buffer.set(bytes.subarray(0, copied), this.bufferLength);
      this.bufferLength += copied;
      offset += copied;
      if (this.bufferLength === 64) {
        this.compress(this.buffer);
        this.bufferLength = 0;
      }
    }
    while (offset + 64 <= bytes.length) {
      this.compress(bytes.subarray(offset, offset + 64));
      offset += 64;
    }
    if (offset < bytes.length) {
      this.buffer.set(bytes.subarray(offset), 0);
      this.bufferLength = bytes.length - offset;
    }
    return this;
  }

  compress(block) {
    const w = this.schedule;
    for (let index = 0; index < 16; index += 1) {
      const base = index * 4;
      w[index] = ((block[base] << 24) | (block[base + 1] << 16) | (block[base + 2] << 8) | block[base + 3]) >>> 0;
    }
    for (let index = 16; index < 64; index += 1) {
      const x = w[index - 15];
      const y = w[index - 2];
      const sigma0 = shaRotate(x, 7) ^ shaRotate(x, 18) ^ shaRight(x, 3);
      const sigma1 = shaRotate(y, 17) ^ shaRotate(y, 19) ^ shaRight(y, 10);
      w[index] = (w[index - 16] + sigma0 + w[index - 7] + sigma1) >>> 0;
    }
    let [a, b, c, d, e, f, g, h] = this.state;
    for (let index = 0; index < 64; index += 1) {
      const sigma1 = shaRotate(e, 6) ^ shaRotate(e, 11) ^ shaRotate(e, 25);
      const choose = (e & f) ^ (~e & g);
      const temp1 = (h + sigma1 + choose + SHA256_K[index] + w[index]) >>> 0;
      const sigma0 = shaRotate(a, 2) ^ shaRotate(a, 13) ^ shaRotate(a, 22);
      const majority = (a & b) ^ (a & c) ^ (b & c);
      const temp2 = (sigma0 + majority) >>> 0;
      h = g; g = f; f = e; e = (d + temp1) >>> 0;
      d = c; c = b; b = a; a = (temp1 + temp2) >>> 0;
    }
    this.state[0] = (this.state[0] + a) >>> 0;
    this.state[1] = (this.state[1] + b) >>> 0;
    this.state[2] = (this.state[2] + c) >>> 0;
    this.state[3] = (this.state[3] + d) >>> 0;
    this.state[4] = (this.state[4] + e) >>> 0;
    this.state[5] = (this.state[5] + f) >>> 0;
    this.state[6] = (this.state[6] + g) >>> 0;
    this.state[7] = (this.state[7] + h) >>> 0;
  }

  digest() {
    if (this.finished) throw new Error("SHA-256 digest already finalized");
    const bitLength = this.bytesHashed * 8;
    this.buffer[this.bufferLength] = 0x80;
    this.buffer.fill(0, this.bufferLength + 1);
    if (this.bufferLength >= 56) {
      this.compress(this.buffer);
      this.buffer.fill(0);
    }
    // The bounded descriptor size is far below 2^53 bits; split explicitly
    // so the length encoding does not depend on BigInt support.
    const high = Math.floor(bitLength / 0x100000000);
    const low = bitLength >>> 0;
    this.buffer[56] = (high >>> 24) & 0xff;
    this.buffer[57] = (high >>> 16) & 0xff;
    this.buffer[58] = (high >>> 8) & 0xff;
    this.buffer[59] = high & 0xff;
    this.buffer[60] = (low >>> 24) & 0xff;
    this.buffer[61] = (low >>> 16) & 0xff;
    this.buffer[62] = (low >>> 8) & 0xff;
    this.buffer[63] = low & 0xff;
    this.compress(this.buffer);
    this.finished = true;
    const result = new Uint8Array(32);
    for (let index = 0; index < this.state.length; index += 1) {
      const value = this.state[index];
      result[index * 4] = value >>> 24;
      result[index * 4 + 1] = value >>> 16;
      result[index * 4 + 2] = value >>> 8;
      result[index * 4 + 3] = value;
    }
    return result;
  }
}

const utf8 = value => new TextEncoder().encode(value);
const u64Le = value => {
  const bytes = new Uint8Array(8);
  let remaining = value;
  for (let index = 0; index < 8; index += 1) {
    bytes[index] = remaining % 256;
    remaining = Math.floor(remaining / 256);
  }
  return bytes;
};

const STATE_TYPE_BYTES = Object.freeze({ bool: 1, u8: 1, u16: 2, i32: 4, f32: 4, u32: 4, f64: 8 });
const STATE_TYPE_TAG = Object.freeze({ i32: 1, f32: 2, f64: 3, bool: 4, u8: 5, u16: 6, u32: 7 });
const MAX_STATE_SNAPSHOT_BYTES = WEB_REPLAY_LIMITS.maxFileBytes;
const MAX_STATE_ENTRIES = WEB_REPLAY_LIMITS.maxTicks;

function decodeStateDescriptor(descriptor) {
  checkKeys(descriptor, ["schema", "abi_version", "support", "byte_order", "hash_scope", "required_bytes", "entries", "unsupported_paths", "size_operation", "write_operation", "restore_operation"]);
  if (descriptor.schema !== "stasis.replay_state_snapshot.v2") fail("replay state descriptor schema is unsupported", "invalid_state_descriptor");
  if (descriptor.abi_version !== 2) fail("replay state descriptor ABI version is unsupported", "invalid_state_descriptor");
  if (descriptor.support !== "canonical_bytes") fail("replay state descriptor does not advertise canonical_bytes support", "unsupported_state_descriptor");
  if (descriptor.byte_order !== "little_endian") fail("replay state descriptor byte_order must be little_endian", "invalid_state_descriptor");
  if (descriptor.hash_scope !== HASH_SCOPE) fail(`replay state descriptor hash_scope must be ${HASH_SCOPE}`, "invalid_state_descriptor");
  const requiredBytes = integer(descriptor.required_bytes, "replay state descriptor required_bytes", 0, MAX_STATE_SNAPSHOT_BYTES);
  const entries = array(descriptor.entries, "replay state descriptor entries", MAX_STATE_ENTRIES);
  const unsupported = array(descriptor.unsupported_paths, "replay state descriptor unsupported_paths", MAX_STATE_ENTRIES);
  if (unsupported.length !== 0) fail("replay state descriptor contains unsupported paths", "unsupported_state_descriptor");
  if (descriptor.size_operation !== "stasis_replay_state_snapshot_size" || descriptor.write_operation !== "stasis_replay_state_snapshot_write" || descriptor.restore_operation !== "stasis_replay_state_snapshot_restore") {
    fail("replay state descriptor operation names are not canonical", "invalid_state_descriptor");
  }
  let expectedOffset = 0;
  let previousScalarPath = null;
  let previousCollectionPath = null;
  let previousCollectionField = null;
  let sawCollection = false;
  const normalized = entries.map((entry, index) => {
    checkKeys(entry, ["kind", "path", "field", "storage_type", "offset", "element_count", "element_bytes"]);
    const kind = entry.kind;
    if (kind !== "scalar" && kind !== "collection") fail(`state descriptor entry ${index} has invalid kind`, "invalid_state_descriptor");
    const path = text(entry.path, `state descriptor entry ${index} path`);
    const field = entry.field === "" ? "" : text(entry.field, `state descriptor entry ${index} field`);
    const storageType = text(entry.storage_type, `state descriptor entry ${index} storage_type`);
    const width = STATE_TYPE_BYTES[storageType];
    if (!width || STATE_TYPE_TAG[storageType] === undefined) fail(`state descriptor entry ${index} has unsupported type ${storageType}`, "unsupported_state_type");
    const offset = integer(entry.offset, `state descriptor entry ${index} offset`, 0, MAX_STATE_SNAPSHOT_BYTES);
    const count = integer(entry.element_count, `state descriptor entry ${index} element_count`, kind === "scalar" ? 1 : 0, MAX_STATE_ENTRIES);
    const elementBytes = integer(entry.element_bytes, `state descriptor entry ${index} element_bytes`, 1, 8);
    if (elementBytes !== width) fail(`state descriptor entry ${index} element_bytes does not match ${storageType}`, "state_width_mismatch");
    if (kind === "scalar" && (field !== "" || count !== 1)) fail(`state descriptor scalar entry ${index} must have an empty field and element_count=1`, "state_count_mismatch");
    if (kind === "scalar") {
      if (sawCollection || (previousScalarPath !== null && path <= previousScalarPath)) {
        fail("state descriptor scalar entries must precede collections and have sorted unique paths", "state_order_mismatch");
      }
      previousScalarPath = path;
    } else {
      sawCollection = true;
      if (previousCollectionPath !== null
          && (path < previousCollectionPath || (path === previousCollectionPath && field <= previousCollectionField))) {
        fail("state descriptor collection entries must have sorted unique paths and fields", "state_order_mismatch");
      }
      previousCollectionPath = path;
      previousCollectionField = field;
    }
    if (offset !== expectedOffset) fail(`state descriptor entry ${index} is not contiguous at offset ${expectedOffset}`, "state_offset_mismatch");
    const byteLength = count * width;
    const end = offset + byteLength;
    if (!Number.isSafeInteger(end) || end > requiredBytes) fail(`state descriptor entry ${index} exceeds required_bytes`, "state_offset_mismatch");
    expectedOffset = end;
    return Object.freeze({ kind, path, field, storage_type: storageType, offset, element_count: count, element_bytes: width });
  });
  if (expectedOffset !== requiredBytes) fail(`state descriptor required_bytes ${requiredBytes} does not equal contiguous entries ${expectedOffset}`, "state_offset_mismatch");
  return Object.freeze({ ...descriptor, required_bytes: requiredBytes, entries: Object.freeze(normalized), unsupported_paths: [] });
}

const exactStateBytes = (value, expected, label) => {
  if (value && typeof value.then === "function") throw new TypeError(`${label} reader must be synchronous`);
  let bytes;
  if (value instanceof ArrayBuffer) bytes = new Uint8Array(value);
  else if (ArrayBuffer.isView(value)) bytes = new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
  else throw new TypeError(`${label} reader must return an ArrayBuffer or typed byte view`);
  if (bytes.byteLength !== expected) throw new ReplayDecodeError(`${label} reader returned ${bytes.byteLength} bytes; expected exactly ${expected}`, "state_bytes_mismatch");
  return bytes;
};

const validateStateValue = (type, bytes, label) => {
  if (type === "bool" && bytes[0] > 1) throw new ReplayDecodeError(`${label} bool value must be 0 or 1`, "invalid_state_value");
};

export function createDescriptorStateHashAdapter({ descriptor, readSnapshotBytes, readEntryBytes } = {}) {
  const hasSnapshotReader = typeof readSnapshotBytes === "function";
  const hasEntryReader = typeof readEntryBytes === "function";
  if (hasSnapshotReader === hasEntryReader) {
    throw new TypeError("descriptor state hash adapter requires exactly one of readSnapshotBytes() or readEntryBytes(entry)");
  }
  const normalized = decodeStateDescriptor(descriptor);
  return Object.freeze({
    descriptor: normalized,
    hashState: () => {
      const hasher = new IncrementalSha256();
      hasher.update(utf8("stasis.simulation-state.v1\0"));
      const snapshot = hasSnapshotReader
        ? exactStateBytes(readSnapshotBytes(), normalized.required_bytes, "state snapshot")
        : null;
      for (const entry of normalized.entries) {
        const width = entry.element_bytes;
        const byteLength = entry.element_count * width;
        const bytes = snapshot
          ? snapshot.subarray(entry.offset, entry.offset + byteLength)
          : exactStateBytes(readEntryBytes(entry), byteLength, `state descriptor entry ${entry.path}${entry.field ? `.${entry.field}` : ""}`);
        for (let index = 0; index < entry.element_count; index += 1) {
          const label = entry.kind === "scalar" ? entry.path : `${entry.path}[${index}].${entry.field}`;
          const value = bytes.subarray(index * width, (index + 1) * width);
          validateStateValue(entry.storage_type, value, label);
          hasher.update(u64Le(utf8(label).byteLength));
          hasher.update(utf8(label));
          hasher.update(Uint8Array.of(STATE_TYPE_TAG[entry.storage_type]));
          hasher.update(value);
        }
      }
      return normalizeHash(hasher.digest());
    },
  });
}

const stateLocationKey = location => location.kind === "scalar"
  ? `scalar\u0000${location.path}`
  : `collection\u0000${location.path}\u0000${location.field}`;

const scalarBitsLe = (value, expectedType, label) => {
  if (!isPlainObject(value) || value.type_name !== expectedType || typeof value.bits !== "string") {
    throw new ReplayDecodeError(`${label} type does not match descriptor type ${expectedType}`, "state_type_mismatch");
  }
  const width = STATE_TYPE_BYTES[expectedType];
  if (value.bits.length !== width * 2 || !/^[0-9a-f]+$/.test(value.bits)) {
    throw new ReplayDecodeError(`${label} bits do not match descriptor width ${width}`, "state_width_mismatch");
  }
  const bytes = new Uint8Array(width);
  for (let index = 0; index < width; index += 1) {
    const source = value.bits.length - (index + 1) * 2;
    bytes[index] = Number.parseInt(value.bits.slice(source, source + 2), 16);
  }
  validateStateValue(expectedType, bytes, label);
  return bytes;
};

export function createDescriptorInitialStateRestorer({ descriptor, writeSnapshotBytes } = {}) {
  if (typeof writeSnapshotBytes !== "function") throw new TypeError("descriptor initial-state restorer requires writeSnapshotBytes(bytes)");
  const normalized = decodeStateDescriptor(descriptor);
  const locations = new Map(normalized.entries.map((entry, order) => [
    entry.kind === "scalar" ? `scalar\u0000${entry.path}` : `collection\u0000${entry.path}\u0000${entry.field}`,
    { entry, order },
  ]));
  return Object.freeze({
    descriptor: normalized,
    restoreInitialState: initialState => {
      if (!isPlainObject(initialState) || !Array.isArray(initialState.values)) {
        throw new ReplayDecodeError("initial state restorer requires decoded initial_state.values", "invalid_initial_state");
      }
      const snapshot = new Uint8Array(normalized.required_bytes);
      let previousOrder = -1;
      let previousElementIndex = -1;
      initialState.values.forEach((stateEntry, index) => {
        if (!isPlainObject(stateEntry) || !isPlainObject(stateEntry.location)) {
          throw new ReplayDecodeError(`initial state entry ${index} is invalid`, "invalid_initial_state");
        }
        const location = locations.get(stateLocationKey(stateEntry.location));
        if (!location) {
          throw new ReplayDecodeError(`initial state entry ${index} is absent from the replay state descriptor`, "state_location_mismatch");
        }
        const { entry: descriptorEntry, order } = location;
        const elementIndex = stateEntry.location.kind === "scalar" ? 0 : stateEntry.location.index;
        if (!Number.isSafeInteger(elementIndex) || elementIndex < 0 || elementIndex >= descriptorEntry.element_count) {
          throw new ReplayDecodeError(`initial state entry ${index} index is outside descriptor bounds`, "state_location_mismatch");
        }
        if (order < previousOrder || (order === previousOrder && elementIndex <= previousElementIndex)) {
          throw new ReplayDecodeError(`initial state entries must be sorted and unique; entry ${index} is not canonical`, "noncanonical_initial_state");
        }
        const bytes = scalarBitsLe(stateEntry.value, descriptorEntry.storage_type, `initial state entry ${index}`);
        if (bytes.every(byte => byte === 0)) {
          throw new ReplayDecodeError(`initial state entry ${index} explicitly encodes a default value`, "noncanonical_initial_state");
        }
        snapshot.set(bytes, descriptorEntry.offset + elementIndex * descriptorEntry.element_bytes);
        previousOrder = order;
        previousElementIndex = elementIndex;
      });
      const written = writeSnapshotBytes(snapshot);
      if (written && typeof written.then === "function") throw new TypeError("initial state snapshot writer must be synchronous");
      if (written !== normalized.required_bytes) {
        throw new ReplayDecodeError(`initial state snapshot writer wrote ${String(written)} bytes; expected ${normalized.required_bytes}`, "state_bytes_mismatch");
      }
      return { restored: true, bytes: written };
    },
  });
}

export function createWasmReplayBridge({ exports, descriptor } = {}) {
  const normalized = decodeStateDescriptor(descriptor);
  const memory = exports?.memory;
  const size = exports?.stasis_replay_state_snapshot_size;
  const write = exports?.stasis_replay_state_snapshot_write;
  const restore = exports?.stasis_replay_state_snapshot_restore;
  if (!(memory instanceof WebAssembly.Memory) || typeof size !== "function" || typeof write !== "function" || typeof restore !== "function") {
    throw new ReplayDecodeError("game Wasm does not expose the canonical replay snapshot ABI", "missing_snapshot_abi");
  }
  const required = size();
  if (!Number.isSafeInteger(required) || required < 0 || required !== normalized.required_bytes) {
    throw new ReplayDecodeError(`snapshot ABI size ${String(required)} does not match descriptor ${normalized.required_bytes}`, "state_bytes_mismatch");
  }
  let pointer = 0;
  if (required > 0) {
    // Stasis Wasm has a compiler-fixed linear-memory layout and no guest heap or
    // guest memory.grow instruction. Pages appended before main are therefore
    // outside every authored address and remain a stable host-owned scratch tail.
    pointer = memory.buffer.byteLength;
    if (!Number.isSafeInteger(pointer) || pointer + required > 0x7fff_ffff) {
      throw new ReplayDecodeError("replay snapshot scratch memory exceeds Wasm32 bounds", "state_bytes_mismatch");
    }
    const pages = Math.ceil(required / 65_536);
    try { memory.grow(pages); } catch {
      throw new ReplayDecodeError("unable to allocate replay snapshot scratch memory", "state_allocation_failed");
    }
  }
  const exactResult = (operation, result) => {
    if (result !== required) throw new ReplayDecodeError(`${operation} returned ${String(result)}; expected ${required}`, "state_bytes_mismatch");
  };
  const readSnapshotBytes = () => {
    exactResult("snapshot write", write(pointer, required));
    return new Uint8Array(memory.buffer, pointer, required).slice();
  };
  const writeSnapshotBytes = bytes => {
    const exact = exactStateBytes(bytes, required, "state snapshot restore");
    if (required > 0) new Uint8Array(memory.buffer, pointer, required).set(exact);
    exactResult("snapshot restore", restore(pointer, required));
    return required;
  };
  return Object.freeze({
    descriptor: normalized,
    requiredBytes: required,
    scratchPointer: pointer,
    readSnapshotBytes,
    writeSnapshotBytes,
    hashAdapter: createDescriptorStateHashAdapter({ descriptor: normalized, readSnapshotBytes }),
    initialStateRestorer: createDescriptorInitialStateRestorer({ descriptor: normalized, writeSnapshotBytes }),
  });
}

export function createCanonicalStateHashAdapter(hashState) {
  const fn = typeof hashState === "function" ? hashState : hashState?.hashState;
  if (typeof fn !== "function") throw new TypeError("a canonical state hash adapter requires hashState()");
  return Object.freeze({ hashState: (...args) => normalizeHash(fn(...args)) });
}

const observedDescriptorsEqual = (expected, actual) => Array.isArray(expected)
  && Array.isArray(actual)
  && expected.length === actual.length
  && expected.every((entry, index) => {
    const found = actual[index];
    return entry !== null
      && typeof entry === "object"
      && found !== null
      && typeof found === "object"
      && entry.slot === found.slot
      && entry.index === found.index
      && entry.path === found.path
      && entry.family === found.family;
  });

const compareIdentity = (actual, expected, schemaVersion) => {
  if (!expected) return;
  if (schemaVersion === WEB_REPLAY_LIMITS.schemaVersion && expected.compatibility) {
    const expectedCompatibility = expected.compatibility;
    const actualCompatibility = actual.compatibility || actual;
    const keys = ["stasis_version", "release_id", "source_sha256", "state_layout_sha256", "compiler_layout_sha256", "asset_manifest_sha256", "host_schema_version", "host_i32_count", "host_f32_count", "input_usage_sha256", "tick_rate_hz", "hash_scope", "determinism_profile", "controller_schema_version"];
    for (const key of keys) {
      if (expectedCompatibility[key] !== undefined && expectedCompatibility[key] !== actualCompatibility[key]) throw new ReplayIdentityError(`compatibility.${key}`, expectedCompatibility[key], actualCompatibility[key]);
    }
    if (expectedCompatibility.observed_i32 !== undefined && !observedDescriptorsEqual(expectedCompatibility.observed_i32, actualCompatibility.observed_i32)) throw new ReplayIdentityError("compatibility.observed_i32", expectedCompatibility.observed_i32, actualCompatibility.observed_i32);
    if (expectedCompatibility.observed_f32 !== undefined && !observedDescriptorsEqual(expectedCompatibility.observed_f32, actualCompatibility.observed_f32)) throw new ReplayIdentityError("compatibility.observed_f32", expectedCompatibility.observed_f32, actualCompatibility.observed_f32);
    return;
  }
  const exactExpected = expected.compatibility
    ? { ...expected.compatibility, target: expected.consumer?.target, runtime_sha256: expected.consumer?.runtime_sha256 }
    : expected;
  const keys = ["stasis_version", "release_id", "target", "source_sha256", "state_layout_sha256", "compiler_layout_sha256", "runtime_sha256", "asset_manifest_sha256", "host_schema_version", "host_i32_count", "host_f32_count", "input_usage_sha256", "tick_rate_hz", "hash_scope", "determinism_profile", "controller_schema_version"];
  for (const key of keys) {
    if (exactExpected[key] !== undefined && exactExpected[key] !== actual[key]) throw new ReplayIdentityError(`identity.${key}`, exactExpected[key], actual[key]);
  }
  if (exactExpected.observed_i32 !== undefined && !observedDescriptorsEqual(exactExpected.observed_i32, actual.observed_i32)) throw new ReplayIdentityError("identity.observed_i32", exactExpected.observed_i32, actual.observed_i32);
  if (exactExpected.observed_f32 !== undefined && !observedDescriptorsEqual(exactExpected.observed_f32, actual.observed_f32)) throw new ReplayIdentityError("identity.observed_f32", exactExpected.observed_f32, actual.observed_f32);
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
  compareIdentity(document.identity, options.identity ?? options.packageIdentity, document.schema_version);
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
