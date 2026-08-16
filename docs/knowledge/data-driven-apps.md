# Data-driven applications

<!-- tags: data-driven, bounded-state, stable-ids, validation, commands, determinism -->

Separate authored definitions from runtime facts. Definitions describe what may
exist; runtime state describes what currently exists; systems transform that
state in a declared order. This keeps the same rules usable from a headless
test, a tool, or a renderer.

## Data boundaries

| Kind | Purpose | Owner |
| --- | --- | --- |
| Definition | Per-kind rules, limits, costs, ranges, and schedules | Loader and validator |
| Runtime state | Active rows, health, progress, selection, and counters | Simulation systems |
| Command | An ordered request to change runtime state | Command intake |
| Projection | Bounded presentation data rebuilt from gameplay state | Render/projection system |

Do not make a projection authoritative. Rendering may rebuild projection
buffers, but it must remain read-only with respect to gameplay state.

When definitions can change during play, load them into separate bounded
staging storage, validate the complete set, and publish them at a defined
boundary. The compact example below writes its wave rows before activation; it
is suitable for setup after reset. Add staging storage before using the same
shape for live replacement.

## Make capacity part of the design

Fixed arrays make storage limits and exhaustion behavior inspectable. Explicit
loop and event budgets bound the work performed in one logical tick.

```stasis
const ENEMY_CAPACITY: i32 = 4;
const WAVE_CAPACITY: i32 = 6;
const MAX_SPAWNS_PER_STEP: i32 = 2;
```

When a pool is full, return a controlled failure or retain the pending event.
Do not silently grow an unbounded collection inside a deterministic system.

## Use stable IDs for bounded rows

An array index can be a stable slot ID while that slot remains active. The
first-free-slot policy gives deterministic allocation and a natural tie-break
order. Removing a row does not renumber surviving rows.

```stasis
function allocate_enemy(health: i32): i32 {
    for (let slot_id: i32 = 0; slot_id < ENEMY_CAPACITY; slot_id += 1) {
        if (!enemies[slot_id].active) {
            enemies[slot_id].active = true;
            enemies[slot_id].health = health;
            enemies[slot_id].path_position = 0;
            state.spawned_count += 1;
            return slot_id;
        }
    }
    return -1;
}
```

The exhausted result is `-1`. Callers must handle it explicitly; a failed
allocation must not advance a schedule or fabricate an ID. Reusing an inactive
slot starts a new occupancy lifetime, so an old reference to that slot is no
longer an entity identity. If references can survive removal and reuse, add a
generation or another identity field and validate it with the slot ID.

## Validate authored data before activation

An authored schedule is a complete data set, not a collection of independent
edits. The example requires nonnegative ticks, positive health, and
nondecreasing event order before publishing the wave.

```stasis
function wave_is_valid(count: i32): bool {
    if (count < 0 || count > WAVE_CAPACITY) {
        return false;
    }
    for (let index: i32 = 0; index < count; index += 1) {
        if (wave_events[index].spawn_tick < 0 || wave_events[index].health <= 0) {
            return false;
        }
        if (index > 0 && wave_events[index].spawn_tick < wave_events[index - 1].spawn_tick) {
            return false;
        }
    }
    return true;
}

function activate_wave(count: i32): bool {
    if (!wave_is_valid(count)) {
        return false;
    }
    state.wave_count = count;
    state.wave_cursor = 0;
    state.capacity_blocked = false;
    return true;
}
```

On rejection, this function leaves `wave_count`, `wave_cursor`, and
`capacity_blocked` unchanged. It does not restore rows previously written by
`set_wave_event`. A live data loader needs separate staging storage if rejected
input must leave both rows and activation metadata unchanged.

## Tune data; keep rules first-principles

Prefer small records with one clear meaning. If two kinds differ in movement,
health, or targeting, store those choices as data and keep the transformation
explicit. Start with the elementary rule, expose fields that need tuning, and
add an abstraction only after repeated behavior is understood.

Commands should enter at a known boundary and retain their order. For replay or
divergence diagnosis, keep the initial state, ordered commands, and selected
state checkpoints. The first mismatching checkpoint identifies the earliest
useful transition to inspect.

## Checklist

- Declare capacity beside each bounded collection.
- Define each slot ID's lifetime and invalid-ID result; add generations when
  references can outlive slot reuse.
- Validate rows, ranges, ordering, and references before activation.
- Keep mutable counters out of authored definitions.
- Apply commands at a defined boundary.
- Make capacity exhaustion observable and testable.
- Keep projections read-only and rebuildable from authoritative state.
- Compare checkpoints to locate the first divergent transition.

The complete checked example is under [examples/](examples/).
