# Worked patterns

<!-- tags: worked-example, bounded-state, stable-ids, waves, targeting, verification -->

The compiler-checked project is under [examples/](examples/). These excerpts
come directly from its source and tests. They keep capacity, ordering, and
identity decisions visible.

## Bounded slots and stable IDs

Allocate from a fixed pool in ascending slot order. The returned index is a
stable slot ID for that occupancy lifetime. Removal marks a row inactive, so
surviving rows keep their IDs and the released slot can be reused. An old
reference to the released slot must not be treated as the identity of its next
occupant.

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

The pool's exhausted result is `-1`. A failed allocation must not
advance a schedule or fabricate an ID.

## Query, materialize, then commit

Materialization writes an intent but does not change enemy health or cooldown
state. Commit checks the target slot again before applying damage and starting
the cooldown.

```stasis
function materialize_attack(): void {
    pending_attack.valid = false;
    pending_attack.target_id = -1;
    pending_attack.damage = 0;
    if (state.cooldown_ticks != 0) {
        return;
    }
    let target_id: i32 = select_target();
    if (target_id >= 0) {
        pending_attack.valid = true;
        pending_attack.target_id = target_id;
        pending_attack.damage = tower_rules.damage;
    }
}

function commit_attack(): bool {
    if (!pending_attack.valid) {
        return false;
    }
    let target_id: i32 = pending_attack.target_id;
    if (target_id < 0 || target_id >= ENEMY_CAPACITY || !enemies[target_id].active) {
        pending_attack.valid = false;
        return false;
    }
    enemies[target_id].health -= pending_attack.damage;
    state.cooldown_ticks = tower_rules.cooldown_ticks;
    pending_attack.valid = false;
    return true;
}
```

The bounds and active checks reject an invalid or inactive slot. They do not
detect retirement followed by reuse of the same slot. The example avoids that
case by running materialization and commit consecutively, before cleanup or
allocation. A deferred intent needs a generation or world revision.

## Make system order executable

The step function is a compact rule specification. Its order determines which
state each system observes.

```stasis
function simulation_step(): void {
    advance_cooldown();
    spawn_due_events();
    move_enemies();
    materialize_attack();
    commit_attack();
    remove_defeated();
    state.tick_index += 1;
}
```

Retain the initial state and ordered commands for replay. Compare checkpoints
after selected ticks; the first mismatch narrows repair to one transition.

## Keep rendering a projection

Presentation rebuilds a bounded projection from authoritative rows. The
projection is read-only with respect to gameplay: `render()` writes its render
buffer but does not advance the simulation or alter health, cursors, or IDs.

```stasis
function render(): i32 {
    render_enemy_count = 0;
    for (let slot_id: i32 = 0; slot_id < ENEMY_CAPACITY; slot_id += 1) {
        render_enemies[slot_id].visible = false;
        if (enemies[slot_id].active) {
            let command_index: i32 = render_enemy_count;
            render_enemies[command_index].visible = true;
            render_enemies[command_index].stable_id = slot_id;
            render_enemies[command_index].path_position = enemies[slot_id].path_position;
            render_enemies[command_index].health = enemies[slot_id].health;
            render_enemy_count += 1;
        }
    }
    return 0;
}
```

Use stable order for inspection, rendering, and deterministic tie-breaks unless
presentation has an explicit independent sort key.

## Test the activation boundary

The example rejects unsorted authored rows before publishing activation
metadata.

```stasis
test `unsorted wave data is rejected without activation`(): bool {
    reset_simulation();
    set_wave_event(0, 3, 2);
    set_wave_event(1, 2, 2);
    return !activate_wave(2) && state.wave_count == 0 && state.wave_cursor == 0;
}
```

Starting from reset, this test proves that rejection returns `false` and leaves
`wave_count` and `wave_cursor` at zero. It does not prove that authored rows are
rolled back or that an earlier active schedule is preserved; those guarantees
require staging storage and additional tests.

The example also tests the exact cooldown boundary, lower-slot targeting on a
tie, a cursor retained on capacity exhaustion, released-slot reuse without
renumbering a survivor, unchanged enemy health and cooldown before commit, and
a render projection that leaves the asserted gameplay fields unchanged. Keep
each test at the transition boundary that matters.

## Design influences

These patterns reflect classic single-threaded simulation, data locality,
bounded render storage, stable entity mapping, command-driven replay, and
bottom-up rules with tunable per-kind data. The historical references listed in
the library README are non-normative influences. Current Stasis documentation,
the compiler, and the checked example define the behavior used here.
