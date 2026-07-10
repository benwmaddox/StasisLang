# Android Exploration Tutorial Design

## Product Role

The exploration tutorial is the default project bundled by the general Stasis Workshop. It is not the identity of the Workshop application and is not implicitly packaged into unrelated release builds. Pong remains a second bundled template and the current game-specific Android release target.

The tutorial should feel like a small game immediately:

1. Tap a visible place in the world.
2. The character walks toward it without overshooting.
3. Walking near a collectible picks it up.
4. The HUD and world respond, exposing progress and inventory.
5. Editing a small data value or system produces an understandable hot reload.

## Learning Goals

The project teaches that game state is data transformed by ordered systems. It deliberately avoids a hidden object graph, per-entity heap allocation, inheritance, frame-rate-dependent movement, and behavior spread across callbacks.

A learner should be able to answer:

- Where is position data stored?
- Which system changes it?
- In what order do input, movement, collection, and rendering run?
- What makes an entity ID stable?
- Which edits are safe fast reloads versus layout resets?
- How can the same state run in JIT, AOT, desktop, and Android?

## World and Interaction

The initial map is a bounded open garden larger than the phone viewport. It contains a player, several visually distinct collectible keepsakes, landmarks, and a goal marker. Direct movement is intentionally introduced before pathfinding.

- Touch coordinates map through the camera to world coordinates.
- A tap writes a destination component for the player.
- Each fixed tick moves along the normalized direction using integer/fixed-point world units and clamps the final step to the remaining distance.
- The character stops inside a small arrival radius and clears its destination-active flag.
- Collection checks run after movement. A collectible inside the pickup radius becomes inactive and increments the matching inventory count exactly once.
- The camera follows with deterministic bounded smoothing and clamps to the world rectangle.
- The render pass emits background/landmark, collectible, destination marker, player, and HUD commands in stable layer order.

Later lessons can add a bounded navigation grid, obstacles, NPC movement, item use, and audio without replacing the data layout learned in the first lesson.

## Data-Oriented Layout

Use bounded global structure-of-arrays storage. Slot index is the iteration key; public entity ID is `slot + 1`, reserving `0` as invalid. The first tutorial uses deterministic startup allocation and no slot reuse. A later spawning lesson can add generations and a deterministic lowest-free-slot allocator.

Suggested constants:

```text
MAX_ENTITIES = 128
MAX_ITEM_KINDS = 8
WORLD_UNITS_SCALE = 100
```

Core component columns:

| Column | Type | Meaning |
|---|---:|---|
| `entity_alive[128]` | `bool` | Slot participates in systems |
| `entity_kind[128]` | `i32` | Player, collectible, landmark, or marker |
| `position_x100[128]` | `i32` | Fixed-point world X |
| `position_y100[128]` | `i32` | Fixed-point world Y |
| `sprite_handle[128]` | `i32` | Shared manifest/runtime asset handle |
| `render_layer[128]` | `i32` | Stable render ordering |
| `destination_active[128]` | `bool` | Movement target exists |
| `destination_x100[128]` | `i32` | Target X |
| `destination_y100[128]` | `i32` | Target Y |
| `move_speed_x100[128]` | `i32` | Distance per fixed tick |
| `collectible_kind[128]` | `i32` | Inventory category or zero |
| `collectible_active[128]` | `bool` | Still present in world |
| `pickup_radius_x100[128]` | `i32` | Squared-distance collection input |

Singleton data remains explicit rather than masquerading as entities:

- input snapshot and previous-touch state;
- camera position and viewport size;
- player entity ID;
- inventory count array by item kind;
- collected/total counters and tutorial message state;
- deterministic tick counter.

No system owns a private copy of authoritative component data.

## Deterministic System Schedule

`tick()` calls systems in this exact order:

1. `input_target_system` detects a new press edge and converts screen to clamped world coordinates.
2. `movement_system` iterates slots ascending and advances active destinations by fixed tick distance.
3. `collection_system` iterates collectible slots ascending, performs squared-distance checks, deactivates each item once, and emits a bounded collection event.
4. `inventory_system` consumes collection events in emission order and updates counts/tutorial progress.
5. `camera_follow_system` follows the accepted player position and clamps to world bounds.
6. `tutorial_progress_system` derives the next instructional message from data.
7. `render_extract_system` writes a bounded render-command snapshot; rendering itself does not mutate gameplay.

The schedule is visible in `src/main.stasis` and each system lives in one correspondingly named file. All loops use ascending bounded indices. Gameplay never uses wall-clock `dt`, random device state, or unordered containers.

## Project Layout and Teaching Order

```text
src/main.stasis                 # lifecycle and visible system schedule
src/config.stasis               # capacities, fixed-point values, world bounds
src/components.stasis           # SoA declarations and entity-kind constants
src/world_data.stasis           # deterministic starting entities/items
src/input.stasis                # touch snapshot -> world target
src/systems/movement.stasis
src/systems/collection.stasis
src/systems/inventory.stasis
src/systems/camera.stasis
src/systems/tutorial.stasis
src/systems/render_extract.stasis
src/assets.stasis               # stable manifest IDs/handles
tests/*.test.stasis
assets/manifest.json
assets/images/*
assets/audio/*                  # optional later lesson
```

Onboarding reveals the project in layers:

1. Change movement speed and observe a body-only hot reload.
2. Move one starting collectible by editing world data.
3. Change pickup radius and verify a Stasis test.
4. Add an item kind by changing bounded data and accepting a reset-required layout edit.
5. Add a system only after the learner understands the schedule.

Pong remains available in the template selector as the smaller example for collision, difficulty curves, tests, and release packaging.

Current implementation note: the first playable keeps executable declarations in `src/main.stasis` because the Workshop JIT and Android AOT paths do not yet agree on project-relative import bases. The planned files above are present as lesson maps, but splitting declarations into them would currently make one host pass while another fails. This is a shared project-import limitation, not a reason to add an Android-only compiler route; the files should become executable modules only after project-relative imports are fixed and tested across host JIT, AOT, and Android.

## Required Stasis Tests

- A new press edge sets the player destination; a held touch does not resubmit it every tick.
- Screen-to-world mapping respects camera offset and clamps to bounds.
- Movement converges and never overshoots on horizontal, vertical, diagonal, near-target, and zero-distance cases.
- Identical input/tick sequences produce identical positions and inventory.
- Collection occurs at the boundary radius, occurs once, and follows ascending entity order when several items overlap.
- Inactive/dead/out-of-capacity entities do not participate.
- Inventory counts and collected/total progress remain consistent.
- Camera following never exposes space outside world bounds.
- Render extraction is stable, bounded, layered, and contains no gameplay mutation.
- A representative end-to-end executable test runs input through tick and verifies movement, collection, inventory, and render state.

## Runtime and Packaging Acceptance

- The same project and `assets/manifest.json` resolve identical handles on host JIT, host AOT, Android Workshop JIT, and a game-specific exploration release when one is intentionally built.
- Workshop first launch selects the exploration project but also lists Pong without copying one project's mutable state into the other.
- Touch automation selects several destinations and proves the character and camera move; a pickup changes both world visibility and inventory.
- Hot reload preserves component state for body-only edits and explains reset-required layout changes.
- Capacity overflow, missing assets, corrupt manifests, and render-command overflow fail deterministically.
- The tutorial stays within the 60 Hz frame budget on the minimum supported arm64 device with no per-tick object allocation in the Stasis logic.

## Deliberate Follow-On Lessons

- Deterministic grid navigation using bounded arrays and a fixed neighbor order.
- NPC intents as another input column consumed by the same movement system.
- Sprite hot reload and fallback rendering through the shared asset manifest.
- Collection audio events through the shared bounded mixer/event queue.
- Save data as a versioned projection of durable component columns, not a serialized object graph.
