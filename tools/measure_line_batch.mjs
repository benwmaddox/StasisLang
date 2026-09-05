import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const baselineCommit = "eb4393e5353fa8e70424dae2b783bd155777493a";
const graphicsPath = "src/stdlib/graphics.stasis";
const internalPath = "src/stdlib/internal/gfx_cmd.stasis";

const readCurrent = (relativePath) => fs.readFileSync(path.join(root, relativePath), "utf8");
const readBaseline = (relativePath) =>
  execFileSync(
    "git",
    ["-c", `safe.directory=${root.replaceAll("\\", "/")}`, "show", `${baselineCommit}:${relativePath}`],
    { cwd: root, encoding: "utf8" },
  );
const escapeRegex = (value) => value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

function extractBlock(source, start, label) {
  const open = source.indexOf("{", start);
  if (open < 0) throw new Error(`${label}: missing opening brace`);
  let depth = 0;
  for (let index = open; index < source.length; index += 1) {
    if (source[index] === "{") depth += 1;
    if (source[index] === "}") depth -= 1;
    if (depth === 0) return source.slice(open + 1, index);
  }
  throw new Error(`${label}: unterminated block`);
}

function extractFunction(source, name) {
  const pattern = new RegExp(
    `function\\s+(?:@[^\\s(]+(?:\\([^)]*\\))?\\s+)*${escapeRegex(name)}\\s*\\(`,
  );
  const match = pattern.exec(source);
  if (!match) throw new Error(`missing function ${name}`);
  return extractBlock(source, match.index, `function ${name}`);
}

function extractStruct(source, name) {
  const match = new RegExp(`struct\\s+${escapeRegex(name)}\\s*\\{`).exec(source);
  if (!match) throw new Error(`missing struct ${name}`);
  return extractBlock(source, match.index, `struct ${name}`);
}

function intConstant(source, name) {
  const match = new RegExp(`const\\s+${escapeRegex(name)}:\\s*i32\\s*=\\s*(\\d+)\\s*;`).exec(source);
  if (!match) throw new Error(`missing integer constant ${name}`);
  return Number.parseInt(match[1], 10);
}

const matches = (source, pattern) => [...source.matchAll(pattern)];

function assertEqual(actual, expected, label) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(`${label}: expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`);
  }
}

const graphics = readCurrent(graphicsPath);
const internal = readCurrent(internalPath);
const oldGraphics = readBaseline(graphicsPath);
const oldInternal = readBaseline(internalPath);
const expectedLanes = ["x1", "y1", "x2", "y2", "r", "g", "b", "a"];

const batchStruct = extractStruct(graphics, "LineBatch");
const laneFields = matches(batchStruct, /\b(\w+):\s*f32\[(\d+)\]\s*;/g).map((match) => ({
  name: match[1],
  extent: Number.parseInt(match[2], 10),
}));
assertEqual(laneFields.map((field) => field.name), expectedLanes, "LineBatch lane fields");
const laneExtents = [...new Set(laneFields.map((field) => field.extent))];
if (laneExtents.length !== 1) throw new Error(`LineBatch lane extents differ: ${laneExtents}`);
const batchCapacity = laneExtents[0];

const append = extractFunction(graphics, "append");
const appendLine = extractFunction(graphics, "append_line");
const draw = extractFunction(graphics, "draw");
const appendStores = matches(append, /self\.(\w+)\[self\.count\]\s*=\s*line\.(\w+)\s*;/g);
assertEqual(appendStores.map((match) => match[1]), expectedLanes, "append(Line) destination lanes");
assertEqual(appendStores.map((match) => match[2]), expectedLanes, "append(Line) source fields");
const appendLineStores = matches(appendLine, /self\.(\w+)\[self\.count\]\s*=\s*(\w+)\s*;/g);
assertEqual(appendLineStores.map((match) => match[1]), expectedLanes, "append_line destination lanes");
assertEqual(appendLineStores.map((match) => match[2]), expectedLanes, "append_line source arguments");
const drawLoads = matches(draw, /self\.(\w+)\[i\]/g).map((match) => match[1]);
assertEqual(drawLoads, expectedLanes, "LineBatch.draw lane loads");
const drawCanonicalCalls = matches(draw, /gfx_cmd_line\s*\(/g).length;
const drawBulkCopies = matches(draw, /sys_memcpy_f32\s*\(/g).length;
if (drawCanonicalCalls !== 1 || drawBulkCopies !== 0) {
  throw new Error("LineBatch.draw must contain one canonical gfx_cmd_line call site");
}

const canonicalLine = extractFunction(internal, "gfx_cmd_line");
const descriptorStores = matches(
  canonicalLine,
  /gfx_cmd_f32\[base\s*\+\s*(\d+)\]\s*=\s*(\w+)\s*;/g,
).map((match) => ({ offset: Number.parseInt(match[1], 10), lane: match[2] }));
assertEqual(descriptorStores.map((store) => store.offset), [0, 1, 2, 3, 4, 5, 6, 7], "gfx_cmd_line descriptor offsets");
assertEqual(descriptorStores.map((store) => store.lane), expectedLanes, "gfx_cmd_line descriptor lanes");
const canonicalOrderAppends = matches(canonicalLine, /gfx_cmd_append_order\(GFX_ORDER_LINE,\s*i\)/g).length;
if (canonicalOrderAppends !== 1) throw new Error("gfx_cmd_line order append changed");

const oldPackedPublic = extractFunction(oldGraphics, "draw_lines");
if (!/function\s+draw_lines\(lines:\s*f32\[\],\s*count:\s*i32\):\s*void/.test(oldGraphics)) {
  throw new Error("baseline packed public signature changed");
}
if (!/gfx_cmd_lines_from\(lines,\s*count\)\s*;/.test(oldPackedPublic)) {
  throw new Error("baseline draw_lines no longer delegates to gfx_cmd_lines_from");
}
if (/function\s+draw_lines\s*\(/.test(graphics) || /function\s+gfx_cmd_lines_from\s*\(/.test(internal)) {
  throw new Error("packed line API remains in the current implementation");
}
const oldPackedInternal = extractFunction(oldInternal, "gfx_cmd_lines_from");
const oldBulkCopies = matches(oldPackedInternal, /sys_memcpy_f32\s*\(/g).length;
const oldCanonicalCalls = matches(oldPackedInternal, /gfx_cmd_line\s*\(/g).length;
const oldOrderAppends = matches(
  oldPackedInternal,
  /gfx_cmd_append_order\(GFX_ORDER_LINE,\s*start\s*\+\s*order_i\)/g,
).length;
if (oldBulkCopies !== 1 || oldOrderAppends !== 1 || oldCanonicalCalls !== 0) {
  throw new Error("baseline packed submission implementation changed");
}

const schemaVersion = intConstant(internal, "GFX_CMD_VERSION");
const geometryCapacity = intConstant(internal, "GFX_MAX_GEOMETRY");
const lineStride = intConstant(internal, "GFX_LINE_STRIDE_F32");
assertEqual(lineStride, expectedLanes.length, "current line stride/lane count");
assertEqual(intConstant(oldInternal, "GFX_CMD_VERSION"), schemaVersion, "baseline/current schema");
assertEqual(intConstant(oldInternal, "GFX_MAX_GEOMETRY"), geometryCapacity, "baseline/current geometry capacity");
assertEqual(intConstant(oldInternal, "GFX_LINE_STRIDE_F32"), lineStride, "baseline/current line stride");

const report = {
  schema: 2,
  measurement_kind: "source-derived deterministic static operation counts (not wall-clock timing)",
  baseline_commit: baselineCommit,
  line_lanes: expectedLanes,
  batch_capacity: batchCapacity,
  old_packed_api: {
    public_signature: "draw_lines(f32[], count)",
    packed_input_f32_per_line: lineStride,
    submission_bulk_copies_per_batch: oldBulkCopies,
    descriptor_f32_per_line: lineStride,
    order_append_call_sites_in_submission_loop: oldOrderAppends,
    canonical_line_calls_per_line: oldCanonicalCalls,
  },
  line_batch_api: {
    public_signatures: "LineBatch.append(Line); LineBatch.append_line(...); LineBatch.draw()",
    append_typed_line_field_reads_per_line: appendStores.length,
    append_typed_line_lane_stores_per_line: appendStores.length,
    append_line_scalar_lane_stores_per_line: appendLineStores.length,
    draw_lane_loads_per_line: drawLoads.length,
    submission_bulk_copies_per_batch: drawBulkCopies,
    canonical_line_calls_per_line: drawCanonicalCalls,
    canonical_descriptor_f32_stores_per_line: descriptorStores.length,
    canonical_order_appends_per_line: canonicalOrderAppends,
  },
  wire_contract: {
    version: schemaVersion,
    geometry_capacity: geometryCapacity,
    line_stride_f32: lineStride,
    shared_with_rectangles: /GFX_I_RECT_COUNT/.test(canonicalLine),
  },
};

if (!report.wire_contract.shared_with_rectangles) {
  throw new Error("gfx_cmd_line no longer accounts for rectangle capacity");
}

const output = `${JSON.stringify(report, null, 2)}\n`;
if (process.argv.includes("--check")) {
  const expected = fs.readFileSync(path.join(root, "tools/line_batch_measurement.json"), "utf8");
  if (output !== expected) throw new Error("line batch measurement report is stale");
}
process.stdout.write(output);
