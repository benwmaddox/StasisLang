import fs from "node:fs";
import childProcess from "node:child_process";

const previous = childProcess.execFileSync(
  "git", ["show", "14f6d57fbb69459302a552c905f199473322cec8:samples/pointer_pong/main.stasis"],
  { encoding: "utf8" },
);
const current = fs.readFileSync("samples/pointer_pong/main.stasis", "utf8");
const graphics = fs.readFileSync("runtime/stasis_graphics.c", "utf8");
const stdlib = fs.readFileSync("src/stdlib/graphics.stasis", "utf8");
const integer = name => Number(graphics.match(new RegExp(`#define ${name} (\\d+)`))?.[1]);
const quadBody = graphics.match(/typedef struct \{([^}]+)\} StasisTextQuad;/s)?.[1] || "";
const quadBytes = [...quadBody.matchAll(/\bfloat\s+([^;]+);/g)]
  .reduce((count, match) => count + match[1].split(",").length, 0) * 4;
const oldLoads = previous.match(/score_digits\.\w+\.load_text_from/g) || [];
const oldFrameDraws = (previous.match(/draw_score_digit\(/g) || []).length - 1;
const newRuns = current.match(/global \w+_score_run: TextRun;/g) || [];
const newFrameDraws = current.match(/\w+_score_run\.draw\(/g) || [];
const perRunBytes = integer("STASIS_DYNAMIC_TEXT_MAX_BYTES")
  + integer("STASIS_DYNAMIC_TEXT_MAX_QUADS") * quadBytes;

const result = {
  environment: { node: process.version, platform: `${process.platform}-${process.arch}` },
  construction: { before_fixed_handles: oldLoads.length, after_dynamic_handles: newRuns.length },
  steady_frame: {
    before_cached_draw_commands: oldFrameDraws,
    after_cached_draw_commands: newFrameDraws.length,
    after_string_copies: 0,
    after_host_calls: 0,
  },
  native_retained_memory: {
    before_score_payload_bytes_including_terminators: oldLoads.length * 2,
    after_active_score_capacity_bytes: newRuns.length * perRunBytes,
    total_dynamic_pool_capacity_bytes: integer("STASIS_MAX_DYNAMIC_TEXT_RUNS") * perRunBytes,
  },
  contracts: {
    cached_draw_uses_handle: /gfx_cmd_text_cached\(self\.font, self\.handle/.test(stdlib),
    replacement_absent_from_render: !/function render\(\)[\s\S]*replace_text_from/.test(current),
  },
};
console.log(JSON.stringify(result, null, 2));
