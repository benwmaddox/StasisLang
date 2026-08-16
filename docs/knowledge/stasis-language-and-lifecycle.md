# Stasis language and lifecycle

<!-- tags: stasis, source, state, commands, tick, render, lifecycle -->

## Lifecycle model

A useful Stasis program architecture has authoritative state, a transition
boundary, and a render projection. Commands enter the program, an ordered
simulation step changes authoritative state, and rendering reads that state
into presentation data. Keep those responsibilities visible even when the
implementation is small. These ownership rules are project invariants, not
restrictions enforced by the language.

The checked example exposes one public transition operation. Its `tick()`
advances exactly one `simulation_step()` and then returns:

```stasis
function tick(): i32 {
    simulation_step();
    return 0;
}
```

This is a logical tick, not a presentation instruction. A caller can issue
ticks for a headless test, a replay, or an interactive session and get the same
state transitions for the same command sequence.

## Three boundaries

| Boundary | Responsibility | Invariant |
| --- | --- | --- |
| Input and commands | Decode external intent, validate it, and apply it at a defined simulation boundary | The same ordered commands produce the same state |
| Simulation | Run the systems for one logical tick in a declared order | Only simulation code changes gameplay state |
| Render | Copy authoritative state into presentation data | Rendering does not change gameplay state |

The example does not need an external command queue to demonstrate the
boundaries. `set_wave_event()` writes candidate schedule data, and
`activate_wave()` validates it before activation. A larger program can add a
command collection without changing the ownership rule: command handling feeds
the simulation, and rendering remains a consumer.

## Authoritative state and identity

Keep durable rules in explicit state. The example stores the current logical
tick, wave cursor, cooldown, capacity status, and counters in
`SimulationState`. Enemy slots hold mutable gameplay data. A slot index is a
stable identity while that slot is active; removal leaves other active slots in
place, and allocation reuses the lowest available slot deterministically.

Separate authored definitions from runtime state when they have different
ownership. `WaveEvent` and `TowerRules` describe configured behavior. The
`enemies` array and `SimulationState` change during simulation. Render records
are a projection and are not a second authority.

## Ordered tick transition

`tick()` is the lifecycle entry point; `simulation_step()` makes its ordered
state transition visible:

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

The order is observable. A due spawn is present before movement, so a newly
spawned enemy moves during that step. Target selection produces an attack intent
without changing health. Commit applies the intent and starts the cooldown.
Removal runs after damage, and the tick index advances only after all systems
complete. If a different order is correct for a game, declare and test that
order rather than relying on incidental function calls.

## Rendering is a projection

The example copies active enemies into compact render records in slot order.
The operation clears presentation visibility, writes the projection, and does
not update `state`, `enemies`, or wave data:

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

The matching test checks both sides of the boundary:

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

## Lifecycle checklist

- Define initial state and its bounded storage shape.
- Define command or event data and the boundary at which it is consumed.
- Make one operation the canonical logical-tick transition.
- Write simulation systems in an explicit order.
- Keep render data derived and read-only with respect to gameplay.
- Specify reset, completion, failure, and capacity-exhaustion transitions.
- Test exact state transitions and the render projection independently.

Useful repository references include `docs/agent_workflow.md`,
`docs/live_cli_workspace.md`, `docs/semantic_edit_protocol.md`,
`docs/render_parity_gate.md`, `samples/headless_scenario/`, and
`samples/state_inspection/`.
