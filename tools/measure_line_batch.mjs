import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const graphics = fs.readFileSync(path.join(root, "src/stdlib/graphics.stasis"), "utf8");
const internal = fs.readFileSync(path.join(root, "src/stdlib/internal/gfx_cmd.stasis"), "utf8");

const report = {
  schema: 1,
  measurement_kind: "deterministic operation and command descriptors (not wall-clock timing)",
  batch_capacity: 512,
  old_packed_api: {
    public_signature: "draw_lines(f32[], count)",
    construction_scalar_writes_per_line: 8,
    count_storage_coupled: false,
    submission_bulk_copies_per_batch: 1,
    canonical_line_calls_per_line: 0,
    order_entries_per_line: 1,
    submission_descriptor: "one memcpy plus one order entry per accepted line",
  },
  line_batch_api: {
    public_signature: "LineBatch.append(Line); LineBatch.draw()",
    construction_scalar_writes_per_line: 8,
    count_storage_coupled: true,
    submission_bulk_copies_per_batch: 0,
    canonical_line_calls_per_line: 1,
    order_entries_per_line: 1,
    submission_descriptor: "one canonical gfx_cmd_line call and order entry per accepted line",
  },
  wire_contract: {
    version: 7,
    geometry_capacity: 10000,
    line_stride_f32: 8,
    shared_with_rectangles: true,
  },
};

const failures = [];
if (!graphics.includes("struct LineBatch")) failures.push("missing LineBatch");
if (graphics.includes("function draw_lines(")) failures.push("packed public draw_lines remains");
if (internal.includes("function gfx_cmd_lines_from(")) failures.push("packed internal copier remains");
if (!internal.includes("const GFX_CMD_VERSION: i32 = 7;")) failures.push("wire version changed");
if (!internal.includes("const GFX_MAX_GEOMETRY: i32 = 10000;")) failures.push("geometry capacity changed");
if (failures.length) {
  throw new Error(failures.join("; "));
}

const output = `${JSON.stringify(report, null, 2)}\n`;
if (process.argv.includes("--check")) {
  const expected = fs.readFileSync(path.join(root, "tools/line_batch_measurement.json"), "utf8");
  if (output !== expected) throw new Error("line batch measurement report is stale");
}
process.stdout.write(output);
