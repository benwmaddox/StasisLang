# Underwater Automation Sample (Undersea Research Station)

Design a calm, bioluminescent automation game sample for Stasis where transport paths rise from the ocean floor to the surface. Units ride vertical lanes, pass through transformation modules (drone -> pressurized drone -> deep specialist, etc.), and deliver research payloads while the renderer supports efficient batching and a full-screen GPU water-light effect.

## Experience Goals
- Tone: quiet exploration, soft blues/teals with bioluminescent accents; music-free in this sample, but space left for hum/ambient FX.
- Loop: place lanes and modules near the sea floor; spawn drones at the bottom; watch them ride upward, transform, and deposit samples near the top docks.
- Clarity: depth bands and module icons stay readable with minimal text; allow pause/step for inspection.
- Determinism: fixed-step simulation and seeded RNG; no hidden allocations; all state in a single global struct with SoA arrays.

## Map & Pathing
- Playfield: vertical slice (e.g., 160 tiles wide x 120 tiles tall) with the sea floor at y=0 and surface at y=max_y. Depth bands mark pressure tiers.
- Lanes: discrete vertical paths starting at the bottom edge; optional horizontal connectors every N rows for cross-routing.
- Cells: `CellType` enum (empty, lane, module, obstruction, dock, light). Tile data held in SoA arrays for efficient traversal and rendering.
- Flow: units move only upward on lanes; horizontal connectors only at designated crossbars; docks live near the top to receive outputs.
- Occlusion/light: annotate tiles with a `light_level` (0-255) to modulate glow and post-process intensity.

## Units & Transformations
- Unit pipeline (examples): `drone` (base) -> `pressurized_drone` (mid-depth) -> `abyssal_drone` (deep specialist) -> `research_payload` (final delivered artifact).
- Modules (placed on lanes; each consumes input type and emits output type):
  - Pressure Lock: drone -> pressurized_drone
  - Thermal Brace: pressurized_drone -> abyssal_drone
  - Bio Lab: abyssal_drone -> research_payload
  - Recharger: any -> same type, restores durability (for future wear mechanics)
  - Splitter/Router: duplicates or redirects based on depth band
- Each unit tracks `type`, `progress`, `lane_x`, `y_fixed16` (for sub-tile interpolation), and `cooldown`. Arrays are SoA for cache-friendly stepping and rendering culling.

## Resources & Progression
- Inputs: energy (passive trickle from thermal vents), data samples (generated at deep vents), hull plating (used to build modules).
- Unlocks: mid-depth modules gated by collected data; deep modules gated by a pressure threshold and hull plating.
- Win/loop goal: deliver N research_payload units to the surface; optional secondary goals (map a vent, stabilize light levels).

## Rendering Plan (Efficiency + PostFX)
- Geometry: keep render commands in a compact SoA buffer per frame: lane segments (lines), modules (instanced quads with icon index), drones (instanced quads/triangles). Reuse static vertex buffers; update instance data only when needed.
- Layering: background gradient (depth-based), static lanes, dynamic units, UI overlay. Draw order minimizes state changes.
- Culling: only enqueue instance data for on-screen tiles; cap maximum instances per frame with a predictable limit.
- Full-screen effect: add a post-process pass in `Stasis.Cli` (SDL2 + GL) that renders a fullscreen quad with a caustics shader. Uniforms: `time`, `depth_scale`, `caustics_intensity`, `surface_jitter`, `biolume_color`. Allow toggling strength from Stasis via a built-in setter (named `set_postfx` in this branch). The pass applies after scene draw for minimal overdraw.
- Color palette: cool base (dark teal), soft gradients with lighter shafts; bioluminescent highlights around modules/drones.

## Data Layout in Stasis
- Single global struct `state: GameState` to follow the SoA pattern:
  - `tiles_type[]`, `tiles_light[]`, `tiles_module[]`, `tiles_depth[]`
  - `units_type[]`, `units_x[]`, `units_y_fixed[]`, `units_progress[]`, `units_cooldown[]`, `units_alive[]`
  - `lanes_x[]` (precomputed columns), `crossbars[]` for connector rows
  - `resource_energy`, `resource_data`, `resource_plating`, `delivered_payloads`
  - `time_ms`, `seed`, `postfx_strength`
- Keep counts (`unit_count`, `max_units`) and free-list for unit slots to avoid allocations.

## Simulation Loop
- Init: seed RNG, generate lane columns and depth bands, place starter modules, clear unit arrays, set default postfx strength.
- Tick (fixed dt, e.g., 16ms):
  1) Spawn base drones at bottom spawn pads if capacity allows.
  2) Advance units upward; resolve module interactions when a unit enters a module tile (type change, cooldown).
  3) Handle connectors and routing decisions.
  4) Deliver payloads at surface docks; increment goals/resources.
  5) Update light levels (slow oscillation) and postfx parameters.
  6) Write render buffer (instances) and present.
- Input: pause/resume, place/remove modules, adjust postfx intensity; keep controls minimal for sample.

## UI & Feedback
- Overlay: goal progress (payloads), resources, depth legend, postfx toggle, help key.
- Highlights: when hovering/selecting a tile, tint lane/module; show next transformation in small text.
- Minimal HUD text to stay calm; avoid flashing/clutter.

## Testing & Determinism
- Stasis tests: simulate N ticks with seeded RNG and assert counts (units transformed, payloads delivered, light oscillation bounds). Add failure cases for invalid module placement and routing loops.
- Host tests: headless render path that runs a few frames, validates post-process uniform updates, and checks instance counts stay within caps.
- Performance target: cap units and tiles to keep frame time under 16ms on mid-tier hardware; document limits in the sample.

## Deliverables / Next Steps
- Implement `samples/underwater_automation.stasis` following the above state layout and loop.
- Extend `Stasis.Cli` rendering: instanced quads for tiles/modules/units, and a fullscreen caustics post-process shader with uniform setters exposed as built-ins.
- Add `docs/assets` sketch (optional) for the depth bands and module icons.
- Document controls and commands in `README` once the sample ships; ensure `stasis run samples/underwater_automation.stasis` works with and without headless mode.
