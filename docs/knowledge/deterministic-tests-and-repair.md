# Deterministic tests and repair

<!-- tags: deterministic, logical-ticks, tests, bounded-work, repair, tracing -->

## Deterministic contract

For a fixed program, initial state, rule data, command sequence, and exact
logical tick count, the authoritative simulation state should be reproducible.
Use integer tick state for cooldowns, movement, wave schedules, objectives, and
other gameplay rules. Rendering projects that state; it does not advance or
repair the simulation.

Reset every fixture explicitly. Configure authored data in the fixture, execute
named ticks, and inspect the fields that define the invariant. A test should
identify the first transition that matters rather than asserting only the
final screen.

## The executable example

The bundled example is a small bounded simulation. Its tests are executable
evidence for the concepts in this library:

- `logical ticks drive exact wave transitions` checks wave cursors at exact
  tick boundaries.
- `system order lets a due enemy move and receive damage` checks the declared
  system sequence.
- `spawn cursor advances by a bounded amount` and `capacity exhaustion
  preserves the pending event` check bounded work and capacity behavior.
- `cooldown becomes ready on its exact boundary` checks countdown semantics.
- `removal keeps surviving stable ids and permits reuse` checks lifecycle
  stability.
- `attack query does not mutate gameplay before commit` separates decision
  materialization from mutation.
- `render projection preserves authoritative gameplay` checks read-only
  presentation.
- `unsorted wave data is rejected without activation` checks data validation
  before activation.

The exact transition test is the smallest useful model for tick-driven rules:

```stasis
test `logical ticks drive exact wave transitions`(): bool {
    reset_simulation();
    configure_example_wave();
    simulation_step();
    if (state.tick_index != 1 || state.wave_cursor != 2 || state.spawned_count != 2) {
        return false;
    }
    simulation_step();
    if (state.tick_index != 2 || state.wave_cursor != 2) {
        return false;
    }
    simulation_step();
    return state.tick_index == 3 && state.wave_cursor == 3;
}
```

The order test makes a cross-system dependency observable:

```stasis
test `system order lets a due enemy move and receive damage`(): bool {
    reset_simulation();
    set_wave_event(0, 0, 3);
    activate_wave(1);
    simulation_step();
    return enemies[0].active && enemies[0].path_position == 1 && enemies[0].health == 1;
}
```

## Bounded collections and work

Capacity is part of the behavior contract. Test both successful allocation and
the full-capacity path. A rejected spawn must leave its event pending so a
later tick can retry or report the blocked condition.

```stasis
test `capacity exhaustion preserves the pending event`(): bool {
    reset_simulation();
    for (let index: i32 = 0; index < ENEMY_CAPACITY; index += 1) {
        allocate_enemy(1);
    }
    set_wave_event(0, 0, 3);
    activate_wave(1);
    spawn_due_events();
    return state.capacity_blocked && state.wave_cursor == 0 && state.spawned_count == ENEMY_CAPACITY;
}
```

Also test that a producer advances by its declared per-tick limit and does not
turn a large schedule into unbounded work. The example keeps a monotonic cursor
over sorted wave data and allocates the first available stable slot.

## Boundaries, order, and lifecycle

Countdown rules need a test at the exact ready boundary, not only a test for a
nonzero value:

```stasis
test `cooldown becomes ready on its exact boundary`(): bool {
    reset_simulation();
    let target_id: i32 = allocate_enemy(9);
    materialize_attack();
    if (!commit_attack() || state.cooldown_ticks != 2 || enemies[target_id].health != 7) {
        return false;
    }
    advance_cooldown();
    materialize_attack();
    if (pending_attack.valid || state.cooldown_ticks != 1) {
        return false;
    }
    advance_cooldown();
    materialize_attack();
    return pending_attack.valid && pending_attack.target_id == target_id;
}
```

Lifecycle tests should prove that a removed slot is inactive, a surviving slot
keeps its ID and state, and allocation reuses only a known free slot:

```stasis
test `removal keeps surviving stable ids and permits reuse`(): bool {
    reset_simulation();
    let removed_id: i32 = allocate_enemy(1);
    let survivor_id: i32 = allocate_enemy(4);
    enemies[removed_id].health = 0;
    remove_defeated();
    if (enemies[removed_id].active || !enemies[survivor_id].active || survivor_id != 1) {
        return false;
    }
    let reused_id: i32 = allocate_enemy(6);
    return reused_id == removed_id && enemies[survivor_id].health == 4 && active_enemy_count() == 2;
}
```

## Query, commit, and render tests

Materialize a decision without mutating authoritative state, then commit the
same decision and inspect the mutation:

```stasis
test `attack query does not mutate gameplay before commit`(): bool {
    reset_simulation();
    let target_id: i32 = allocate_enemy(5);
    enemies[target_id].path_position = 3;
    materialize_attack();
    if (!pending_attack.valid || pending_attack.target_id != target_id) {
        return false;
    }
    if (enemies[target_id].health != 5 || state.cooldown_ticks != 0) {
        return false;
    }
    return commit_attack() && enemies[target_id].health == 3 && state.cooldown_ticks == 2;
}
```

Rendering is a projection boundary. It may populate render data, but it must
not advance the tick, move the wave cursor, change health, or reorder stable
gameplay IDs:

```stasis
test `render projection preserves authoritative gameplay`(): bool {
    reset_simulation();
    let first_id: i32 = allocate_enemy(5);
    let second_id: i32 = allocate_enemy(7);
    enemies[first_id].path_position = 2;
    enemies[second_id].path_position = 6;
    let tick_before: i32 = state.tick_index;
    let cursor_before: i32 = state.wave_cursor;
    render();
    return state.tick_index == tick_before && state.wave_cursor == cursor_before && enemies[first_id].health == 5 && enemies[second_id].health == 7 && render_enemy_count == 2 && render_enemies[0].stable_id == first_id && render_enemies[1].stable_id == second_id;
}
```

## Repair loop

1. Reset to the same initial state and command sequence.
2. Find the first logical tick where inspected state diverges.
3. Compare the transition inputs, active IDs, cursors, and authoritative fields
   at that tick.
4. Check system order and the boundary condition before changing arithmetic.
5. Fix the smallest violated invariant.
6. Add or preserve a focused regression test.
7. Run the focused test, then the bounded-work, render-projection, and complete
   project test sets.

For long deterministic scenarios, a canonical state digest or checksum can
shorten comparisons when the harness provides one. Keep the digest definition
explicit and retain field-level evidence for a useful diagnosis. A trace of
named state transitions identifies the first divergence more effectively than
comparing only final output.

The original [Age of Empires architecture paper](https://www.gamedeveloper.com/programming/1500-archers-on-a-28-8-network-programming-in-age-of-empires-and-beyond)
is a conceptual reference for synchronous simulation, checksums, and tracing
the first divergence. It does not imply that Stasis is a networked simulation.

Other non-normative design influences include [Handmade Hero Day 26](https://guide.handmadehero.org/code/day026/),
[Handmade Hero Day 63](https://guide.handmadehero.org/code/day063/), [Handmade Hero Day 88](https://guide.handmadehero.org/code/day088/),
[Data-Oriented Design](https://dataorienteddesign.com/dodbook.pdf), and
[Data Locality](https://gameprogrammingpatterns.com/data-locality.html).
