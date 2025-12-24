# Aquarium mini sample plan

This document defines a compact Stasis sample: an aquarium where fish swim, can be fed, and can be interacted with via pointer input. It is meant to exercise deterministic gameplay update, SoA storage, rendering, and (later) audio.

## Goals

- A fun, minimal interactive sample that demonstrates "game structure" in Stasis.
- Uses deterministic update with static global memory (no hidden allocation).
- Uses SoA-friendly entity storage (arrays per field).
- Uses pointer input:
  - Tap/click to drop food.
  - Drag to "stir" water (push fish and pellets slightly).
- Optional: hooks for sound events once audio lands.

## Non-goals (initially)

- Complex AI, breeding, inventory systems, or large asset pipelines.
- High-fidelity physics (simple steering and impulses are enough).

## World and simulation

### Coordinate system

- 2D world in "screen units" matching the viewport pixels, or normalized [0,1] scaled to viewport.
- Pick one and keep it consistent. For easy hit-testing, start with viewport pixels.

### Time step

- Fixed time step update for determinism (e.g., 60 Hz).
- Clamp delta time if host supplies variable dt.

### Deterministic randomness

- Keep a seeded RNG in global state:
  - Used for fish wander and bubble spawning.
  - Seed is constant or derived from a CLI arg.

## Entities and SoA layout

Use fixed-capacity arrays with an `alive` flag or compacted "count" region.

### Fish (capacity e.g., 64)

Parallel arrays:

- `fish_alive[] : bool`
- `fish_x[] : f32`, `fish_y[] : f32`
- `fish_vx[] : f32`, `fish_vy[] : f32`
- `fish_dir[] : f32` (for facing)
- `fish_hunger[] : f32` (0..1)
- `fish_wander_phase[] : f32` (or rng state per fish)

### Food pellets (capacity e.g., 128)

- `pellet_alive[] : bool`
- `pellet_x[] : f32`, `pellet_y[] : f32`
- `pellet_vx[] : f32`, `pellet_vy[] : f32`
- `pellet_age[] : f32`

Removal strategy:

- For simplicity and speed: "swap-remove" compaction per tick for pellets.
- Fish can remain stable-index if desired; or also compact.

## Behaviors

### Fish swim (simple steering)

For each fish:

- Wander:
  - Apply a small steering force based on a slowly changing angle.
- Boundary avoidance:
  - If near edges, steer back in.
- Speed limiting:
  - Clamp velocity to `max_speed`.
- Facing:
  - Set `fish_dir` from velocity (atan2) for rendering.

### Hunger and feeding

- Hunger increases slowly over time up to 1.0.
- If a pellet exists within `seek_radius`, fish steers toward the nearest pellet.
- If within `eat_radius`, pellet is consumed:
  - Remove pellet.
  - Reduce hunger by a fixed amount.
  - Trigger a "chomp" sound event (later).

### Pellets fall and settle

- On spawn: pellet gets small downward velocity.
- Each tick:
  - Apply gravity-like acceleration downward.
  - Apply drag so they settle.
  - Clamp to bottom boundary (y = height - margin).
- Pellets expire after `ttl_seconds`.

### Interaction: stir water (drag impulse)

When pointer is down and moves:

- Compute an impulse centered on pointer position:
  - Affect entities within `stir_radius`.
  - Add small velocity proportional to pointer delta and falloff with distance.
- Keep effect subtle; it should feel like "pushing water" not teleporting.

## Input mapping

Use the unified pointer snapshot model:

- Tap/click (went_down):
  - Spawn one pellet at pointer position.
  - Trigger "plop" sound (later).
- Drag:
  - Apply stir impulses using `dx/dy`.

Optionally allow multi-touch:

- Primary pointer drops food.
- Secondary pointers only stir.

## Rendering plan

Start procedural; add assets later if desired.

### Background

- Simple gradient or solid color with a few bubble particles.
- Optional: animated caustics later (not required).

### Fish and pellets

Procedural shapes:

- Fish: triangle + tail fin (2-3 triangles) rotated by `fish_dir`.
- Pellet: small filled circle or quad.

If SVG assets are preferred later:

- Store under `samples/aquarium/assets/` or `assets_src/`.
- Keep them small and validate via existing SVG validator.

## Audio hooks (future)

Once audio output exists:

- Event sounds:
  - `plop` on pellet spawn.
  - `chomp` on pellet eaten.
- Ambient loop:
  - very low-volume bubbles.

Implementation approach:

- Game emits simple sound events into a fixed-size queue:
  - `SoundEvent { kind, volume, pan }`
- The audio mixer consumes events and mixes into the output buffer.

## Suggested files and structure

- `docs/aquarium-sample-plan.md` (this doc)
- `samples/aquarium.stasis` (main sample)
- Optional data:
  - `samples/aquarium/data/config.json` for tunables (keep deterministic and minimal)

## Milestones

- M1: Implement fish + pellets with deterministic update (no input).
- M2: Add pointer input: tap to drop food, drag to stir.
- M3: Rendering polish (better fish motion, bubbles).
- M4: Add audio hooks once audio task lands.

## Acceptance criteria

- `stasis run samples/aquarium.stasis --graphics` shows fish that swim and seek/eat pellets.
- Pointer input works:
  - Tap drops pellets at correct location.
  - Drag stirs fish/pellets smoothly.
- Behavior is deterministic with a fixed seed.

