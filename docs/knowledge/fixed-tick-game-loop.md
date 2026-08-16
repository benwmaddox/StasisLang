# Logical-tick game loop

<!-- tags: games, logical-tick, input, commands, update, render, determinism, bounded-work -->

## Contract

Use an integer logical tick as the authority for gameplay transitions. The
example's public `tick()` operation advances exactly one simulation step. A
simulation does not need a presentation-driven timing policy to remain
repeatable: callers, tests, and replays can issue the same sequence of ticks
and commands.

The small example has three explicit phases:

1. Input or setup produces validated command/configuration data.
2. `simulation_step()` applies systems in a fixed order and increments the
   logical tick once.
3. `render()` projects current state into presentation records without
   changing gameplay.

The transition entry point is deliberately small:

```stasis
function tick(): i32 {
    simulation_step();
    return 0;
}
```

## Ordered systems

The order inside one step is part of the game rules:

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

The consequences are exact and testable:

- Cooldowns are reduced before an attack is considered.
- Events due at the current `state.tick_index` are spawned before movement.
- Movement runs over the active bounded slot set.
- Target selection materializes an `AttackIntent`; it does not mutate health.
- Commit validates the target, applies damage, and sets the cooldown.
- Defeated slots become inactive after damage, and their slot indices can be
  reused deterministically.
- `state.tick_index` changes once, at the end of the step.

Changing this order changes behavior. Treat it as a contract and cover any
ordering decision with an exact transition test.

## Input and command discipline

Represent external intent as data with a declared application point. Validate
commands before they can mutate authoritative state. Consume edge-like commands
once, preserve the order of commands with equal priority, and make repeated
commands harmless or explicitly invalid according to the domain rules.

The example uses wave events as preloaded data rather than an interactive input
queue. `wave_is_valid()` rejects invalid ordering and `activate_wave()` changes
the active schedule only after validation. This is the same boundary discipline
used for interactive commands: invalid input does not partially activate a new
rule set.

## Exact logical transitions

The bounded wave cursor traverses a schedule with two events at logical tick
zero, one at tick two, and one at tick four. The first three steps are asserted
directly:

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

This test defines the transition precisely: the first call processes the two
events at tick zero and leaves the state at tick one; the second call does not
release the event scheduled for tick two; the third call does. There is no
implicit extra step.

Cooldowns use the same integer boundary. A cooldown set to two is not ready
after one call to `advance_cooldown()`, and is ready after the second call:

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

## Bounded work and stable slots

Declare capacities beside the data they bound. Iterate with explicit limits,
scan slots in a deterministic order, and report exhaustion as state. The wave
cursor remains on a pending event when no slot is available; a later step can
retry after a slot is released. A bounded spawn count also prevents one step
from consuming an unbounded schedule.

A stable slot ID identifies one active occupant without shifting when another
slot is removed. After removal, the vacated ID may identify a later occupant;
stale references must not cross that reuse boundary. The next allocation
reuses the lowest available slot:

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

## Query, materialize, commit, and render

Separate a decision from its mutation when a rule needs inspection, preview,
or a deterministic validation point. In the example, `select_target()` queries
the current state, `materialize_attack()` records the selected target and damage
in `pending_attack`, and `commit_attack()` is the only one of those operations
that changes target health and starts the cooldown. The test asserts that
materializing the attack leaves gameplay unchanged:

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

Rendering remains a read-only projection. Use render records or a bounded
command collection when that makes presentation ordering explicit; do not let
rendering choose targets, apply damage, advance a cooldown, or remove state.

## Verification checklist

- One call to `tick()` means one call to `simulation_step()`.
- Every state-changing system has a declared position in the step order.
- Commands and schedules are validated before activation.
- Integer tick and cooldown boundaries have exact tests.
- Capacity exhaustion leaves pending work observable and recoverable.
- Stable slot IDs do not shift while occupied; reuse is an explicit identity
  boundary.
- Query and materialization functions do not mutate authoritative gameplay.
- Render projection preserves authoritative state.

Useful repository references include `samples/headless_scenario/`,
`samples/state_inspection/`, `samples/bounded_performance/`, and
`docs/render_parity_gate.md`.
