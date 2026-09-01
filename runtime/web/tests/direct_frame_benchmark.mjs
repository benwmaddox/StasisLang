import { performance } from "node:perf_hooks";

const ITERATIONS = 31;
const WARMUP = 6;
const PRIVATE_QUAD_BYTES = 64;
const SOLID_TEXEL_BYTES_PER_DOMAIN = 16;
const V6_RECT_BATCH_MIN = 64;

function sprite(name, handle, domain, width, height, alpha) {
  return { kind: "sprite", name, handle, domain, width, height, alpha };
}

function rect(name, width, height, alpha) {
  return { kind: "rect", name, width, height, alpha };
}

const A0 = sprite("A", 101, 0, 19, 31, 0.61);
const B0 = sprite("B", 202, 0, 43, 17, 0.73);
const B1 = sprite("B", 202, 1, 43, 17, 0.73);
const C = rect("C", 29, 53, 0.47);

function repeatPattern(name, pattern, repetitions, assumptions) {
  return {
    name,
    exact_pattern: pattern.map((item) => item.name).join("-"),
    repetitions,
    items: Array.from({ length: repetitions }, () => pattern).flat(),
    assumptions
  };
}

const scenarios = [
  repeatPattern("abab_sprites_same_domain", [A0, B0, A0, B0], 1024, {
    logical_images: 2,
    private_binding_domains: 1,
    note: "A and B differ in logical image, size, and alpha but are co-resident"
  }),
  repeatPattern("abab_sprites_forced_second_domain", [A0, B1, A0, B1], 1024, {
    logical_images: 2,
    private_binding_domains: 2,
    note: "B is forced to a second page/domain; exact order requires every page transition"
  }),
  repeatPattern("abcacb_mixed_same_domain", [A0, B0, C, A0, C, B0], 682, {
    logical_images: 2,
    private_binding_domains: 1,
    note: "C is a differently sized translucent solid; no layering or reordering"
  }),
  repeatPattern("abcacb_mixed_forced_second_domain", [A0, B1, C, A0, C, B1], 682, {
    logical_images: 2,
    private_binding_domains: 2,
    note: "B is forced to a second page/domain; page-neutral C stays with the active batch"
  }),
  repeatPattern("alternating_sprite_solid_stress", [A0, C], 4096, {
    logical_images: 1,
    private_binding_domains: 1,
    note: "4096 exact-order translucent sprite/solid pairs"
  })
];

function percentile(values, fraction) {
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * fraction))];
}

function spriteRunCount(items) {
  let runs = 0;
  let previousWasSprite = false;
  for (const item of items) {
    if (item.kind === "sprite" && !previousWasSprite) runs += 1;
    previousWasSprite = item.kind === "sprite";
  }
  return runs;
}

function buildV6Scratch(items) {
  const spriteCount = items.filter((item) => item.kind === "sprite").length;
  const rectCount = items.length - spriteCount;
  const spriteI32 = new Int32Array(spriteCount * 3);
  const spriteF32 = new Float32Array(spriteCount * 8);
  const rectF32 = new Float32Array(rectCount * 8);
  const order = new Int32Array(items.length);
  let spriteIndex = 0;
  let rectIndex = 0;
  for (let itemIndex = 0; itemIndex < items.length; itemIndex += 1) {
    const item = items[itemIndex];
    if (item.kind === "sprite") {
      const i = spriteIndex * 3;
      const f = spriteIndex * 8;
      spriteI32[i] = item.handle;
      spriteI32[i + 2] = Math.round(item.alpha * 255) << 24;
      spriteF32[f] = (itemIndex * 13) % 1280;
      spriteF32[f + 1] = (itemIndex * 7) % 720;
      spriteF32[f + 2] = item.width;
      spriteF32[f + 3] = item.height;
      order[itemIndex] = 2 * 16384 + spriteIndex;
      spriteIndex += 1;
    } else {
      const f = rectIndex * 8;
      rectF32[f] = (itemIndex * 11) % 1280;
      rectF32[f + 1] = (itemIndex * 5) % 720;
      rectF32[f + 2] = item.width;
      rectF32[f + 3] = item.height;
      rectF32[f + 4] = 0.37;
      rectF32[f + 5] = 0.59;
      rectF32[f + 6] = 0.83;
      rectF32[f + 7] = item.alpha;
      order[itemIndex] = 4 * 16384 + rectIndex;
      rectIndex += 1;
    }
  }
  // The removed v6 helper copied both sprite scratch lanes into canonical lanes.
  const canonicalI32 = new Int32Array(spriteI32);
  const canonicalF32 = new Float32Array(spriteF32);
  return canonicalI32[0] ^ canonicalF32[3] ^ rectF32[7] ^ order[order.length - 1];
}

function buildV7DirectGuestFrame(items) {
  const spriteCount = items.filter((item) => item.kind === "sprite").length;
  const rectCount = items.length - spriteCount;
  const runsCount = spriteRunCount(items);
  const spriteI32 = new Int32Array(spriteCount * 3);
  const spriteF32 = new Float32Array(spriteCount * 13);
  const rectF32 = new Float32Array(rectCount * 8);
  const runs = new Int32Array(runsCount * 8);
  const order = new Int32Array(rectCount + runsCount);
  let spriteIndex = 0;
  let rectIndex = 0;
  let runIndex = -1;
  let orderIndex = 0;
  let inSpriteRun = false;
  let constructionHash = 0;
  for (let itemIndex = 0; itemIndex < items.length; itemIndex += 1) {
    const item = items[itemIndex];
    if (item.kind === "sprite") {
      if (!inSpriteRun) {
        runIndex += 1;
        const run = runIndex * 8;
        runs[run] = spriteIndex;
        runs[run + 2] = -1;
        order[orderIndex] = 2 * 16384 + runIndex;
        orderIndex += 1;
      }
      const i = spriteIndex * 3;
      const f = spriteIndex * 13;
      spriteI32[i] = item.handle;
      spriteI32[i + 1] = (Math.round(item.alpha * 255) << 24) | 0x00ffffff;
      spriteF32[f] = (itemIndex * 13) % 1280;
      spriteF32[f + 1] = (itemIndex * 7) % 720;
      spriteF32[f + 2] = item.width;
      spriteF32[f + 3] = item.height;
      spriteF32[f + 8] = item.width * 0.37;
      spriteF32[f + 9] = item.height * 0.61;
      spriteF32[f + 10] = itemIndex % 2 ? -1 : 1;
      spriteF32[f + 11] = 1;
      spriteF32[f + 12] = itemIndex % 360;
      runs[runIndex * 8 + 1] += 1;
      spriteIndex += 1;
      inSpriteRun = true;
    } else {
      const f = rectIndex * 8;
      rectF32[f] = (itemIndex * 11) % 1280;
      rectF32[f + 1] = (itemIndex * 5) % 720;
      rectF32[f + 2] = item.width;
      rectF32[f + 3] = item.height;
      rectF32[f + 4] = 0.37;
      rectF32[f + 5] = 0.59;
      rectF32[f + 6] = 0.83;
      rectF32[f + 7] = item.alpha;
      order[orderIndex] = 4 * 16384 + rectIndex;
      orderIndex += 1;
      rectIndex += 1;
      inSpriteRun = false;
    }
    constructionHash = (constructionHash * 33 + item.width + item.height) | 0;
  }
  return constructionHash ^ spriteI32[0] ^ spriteF32[12] ^ rectF32[7] ^ runs[1] ^ order[order.length - 1];
}

function measure(operation) {
  const samples = [];
  let checksum = 0;
  for (let iteration = 0; iteration < ITERATIONS; iteration += 1) {
    const started = performance.now();
    checksum ^= operation();
    const elapsed = (performance.now() - started) * 1000;
    if (iteration >= WARMUP) samples.push(elapsed);
  }
  return { p50_us: percentile(samples, 0.50), p95_us: percentile(samples, 0.95), checksum };
}

function pipelineBoundaries(items) {
  let boundaries = 0;
  for (let index = 1; index < items.length; index += 1) {
    if (items[index].kind !== items[index - 1].kind) boundaries += 1;
  }
  return boundaries;
}

function atlasPageTransitions(items) {
  const domains = items.filter((item) => item.kind === "sprite").map((item) => item.domain);
  let transitions = 0;
  for (let index = 1; index < domains.length; index += 1) {
    if (domains[index] !== domains[index - 1]) transitions += 1;
  }
  return transitions;
}

function v6Plan(items) {
  let draws = 0;
  for (let index = 0; index < items.length;) {
    if (items[index].kind === "rect") {
      draws += 1;
      index += 1;
      continue;
    }
    let end = index;
    while (end < items.length && items[end].kind === "sprite") end += 1;
    if (end - index < V6_RECT_BATCH_MIN) {
      draws += end - index;
    } else {
      draws += 1;
      for (let cursor = index + 1; cursor < end; cursor += 1) {
        if (items[cursor].domain !== items[cursor - 1].domain) draws += 1;
      }
    }
    index = end;
  }
  return {
    draw_submissions: draws,
    composites: draws,
    atlas_page_transitions: atlasPageTransitions(items),
    sprite_solid_pipeline_boundaries: pipelineBoundaries(items)
  };
}

function v7Plan(items) {
  let draws = 0;
  let activeDomain = null;
  let activeInstances = 0;
  for (let index = 0; index < items.length; index += 1) {
    const item = items[index];
    let domain = item.domain;
    if (item.kind === "rect") {
      if (activeDomain !== null) domain = activeDomain;
      else {
        const nextSprite = items.slice(index + 1).find((candidate) => candidate.kind === "sprite");
        domain = nextSprite?.domain ?? 0;
      }
    }
    if (domain !== activeDomain || activeInstances === 4096) {
      draws += 1;
      activeDomain = domain;
      activeInstances = 0;
    }
    activeInstances += 1;
  }
  return {
    draw_submissions: draws,
    composites: 0,
    atlas_page_transitions: atlasPageTransitions(items),
    sprite_solid_pipeline_boundaries: 0
  };
}

function scenarioEvidence(scenario) {
  const sprites = scenario.items.filter((item) => item.kind === "sprite").length;
  const rectangles = scenario.items.length - sprites;
  const runs = spriteRunCount(scenario.items);
  const beforePlan = v6Plan(scenario.items);
  const afterPlan = v7Plan(scenario.items);
  const domains = new Set(scenario.items.filter((item) => item.kind === "sprite").map((item) => item.domain)).size;
  return {
    name: scenario.name,
    fixture: {
      exact_pattern: scenario.exact_pattern,
      repetitions: scenario.repetitions,
      total_items: scenario.items.length,
      sprites,
      rectangles,
      sprite_runs: runs,
      painter_order_preserved: true,
      application_layering_or_reordering: false,
      ...scenario.assumptions
    },
    measured_cpu: {
      before_v6_guest_construction: measure(() => buildV6Scratch(scenario.items)),
      after_v7_direct_guest_frame_construction: measure(() => buildV7DirectGuestFrame(scenario.items))
    },
    counters: {
      kind: "deterministic_production-layout_counter_model_not_gpu_timing",
      before_v6: {
        canonical_bytes_written: sprites * 44 + rectangles * 32 + scenario.items.length * 4,
        scratch_to_frame_bytes_copied: sprites * 44,
        modeled_host_private_repack_bytes: sprites * PRIVATE_QUAD_BYTES,
        modeled_host_private_upload_bytes: sprites * PRIVATE_QUAD_BYTES,
        ...beforePlan,
        modeled_solid_texel_bytes_per_context_generation: 0
      },
      after_v7: {
        canonical_bytes_written: sprites * 64 + rectangles * 32 + runs * 32 + (runs + rectangles) * 4,
        scratch_to_frame_bytes_copied: 0,
        modeled_host_private_repack_bytes: scenario.items.length * PRIVATE_QUAD_BYTES,
        modeled_host_private_upload_bytes: scenario.items.length * PRIVATE_QUAD_BYTES,
        ...afterPlan,
        modeled_solid_texel_bytes_per_context_generation: domains * SOLID_TEXEL_BYTES_PER_DOMAIN
      }
    }
  };
}

const evidence = {
  schema: "stasis.direct_frame_benchmark.v3",
  platform: `node ${process.version} ${process.platform}/${process.arch}`,
  phase_model: {
    image_and_atlas_preparation: "not timed here; production performs it once per load/resource/density/context generation and the modeled solid-texel bytes are labeled separately",
    static_frame_or_list_construction: "not measured: v7 does not expose sealed persistent replay or patchable slots; remaining #399 scope",
    dynamic_frame_construction: "measured every render; harness allocations do not model production's persistent global canonical lanes",
    host_private_repack_and_upload: "modeled byte counts only; this harness does not execute production planner, WebGL bufferSubData, driver, or GPU work"
  },
  assumptions: {
    exact_source_order: true,
    no_global_sorting: true,
    no_application_layering_or_reordering: true,
    solid_rectangles_use_host_private_material_records: true,
    solid_texel_is_host_owned_and_does_not_consume_guest_atlas_capacity: true
  },
  scenarios: scenarios.map(scenarioEvidence),
  unavailable: ["GPU timing", "device upload timing", "native/Android unified-pipeline timing"]
};

console.log(JSON.stringify(evidence, null, 2));
