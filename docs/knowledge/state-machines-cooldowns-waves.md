# State machines, cooldowns, and waves

<!-- tags: state-machine, logical-ticks, cooldowns, waves, bounded-work, determinism -->

A state machine is a transition contract: accepted commands, guards, outputs,
and the next state. Keep transitions in simulation code. Presentation may
report state but must not choose it. Restart paths must reset every field that
participates in a transition.

## Make the logical tick authoritative

Use one integer tick index for simulation ordering. All systems in one step
belong to the same logical tick, but each system observes mutations made by the
systems before it. Their declared order defines those boundaries.

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

Here, cooldown readiness is updated before the attack query; due rows are
allocated before movement; the query is materialized before commit; and
defeated rows are released after damage. The order is part of the rules.

Command-driven simulation follows the same shape: collect commands for a
logical tick, apply them in deterministic order, run systems, then record state
or a checkpoint. Replaying the initial state and command stream should produce
the same checkpoints. A mismatch at a known tick identifies a divergence.

## Store transition state explicitly

Cursors, counters, and capacity results are state-machine data, not hidden
control flow.

```stasis
struct SimulationState {
    tick_index: i32;
    wave_count: i32;
    wave_cursor: i32;
    cooldown_ticks: i32;
    capacity_blocked: bool;
    spawned_count: i32;
    defeated_count: i32;
}
```

State invariants should be inspectable before and after each transition:
`0 <= wave_cursor <= wave_count`, nonnegative cooldowns, and stable slot IDs
for each active occupancy lifetime. Test exact boundary values rather than only
a later success.

## Countdown cooldowns

Represent a cooldown as remaining logical ticks. Decrement toward zero and
define whether a system may act on the step that reaches readiness.

```stasis
function advance_cooldown(): void {
    if (state.cooldown_ticks > 0) {
        state.cooldown_ticks -= 1;
    }
}
```

The example queries only when the countdown is zero and writes the configured
cooldown during commit. A successful attack sets two remaining ticks. Because
the decrement runs before the query, the first later step changes `2` to `1`
and cannot attack; the second changes `1` to `0` and may attack.

## Validate and consume sorted wave events

Keep authored events sorted by `spawn_tick` and store a monotonic cursor into
the validated table. Advance the cursor only after successful consumption.

```stasis
function spawn_due_events(): void {
    state.capacity_blocked = false;
    let attempts: i32 = 0;
    for (attempts = 0; attempts < MAX_SPAWNS_PER_STEP && state.wave_cursor < state.wave_count; attempts += 1) {
        let event_index: i32 = state.wave_cursor;
        if (wave_events[event_index].spawn_tick > state.tick_index) {
            return;
        }
        if (allocate_enemy(wave_events[event_index].health) < 0) {
            state.capacity_blocked = true;
            return;
        }
        state.wave_cursor += 1;
    }
}
```

The wave table and spawn attempts are both bounded. If the pool is exhausted,
the pending event remains at the cursor and is retried after capacity is
released. It is not skipped.

## Deterministic targeting

Describe targeting as an ordered priority chain. Apply eligibility checks and
comparisons in the same order for every candidate, then use a stable-ID
tie-break for equal gameplay values.

```stasis
function select_target(): i32 {
    let selected_id: i32 = -1;
    let selected_position: i32 = -1;
    for (let slot_id: i32 = 0; slot_id < ENEMY_CAPACITY; slot_id += 1) {
        if (enemies[slot_id].active && enemies[slot_id].path_position <= tower_rules.range_end) {
            if (enemies[slot_id].path_position > selected_position) {
                selected_id = slot_id;
                selected_position = enemies[slot_id].path_position;
            }
        }
    }
    return selected_id;
}
```

This rule prefers the greatest path position in range. The strict comparison
keeps the first matching slot on a tie; ascending traversal makes the lower
stable ID win. Additional priorities should be explicit and end with the same
stable-ID rule.

## Query, materialize, then commit

The materialization phase reads gameplay rows and writes a bounded intent; it
does not change enemy health or cooldown state. Commit checks that the slot is
in range and still active, applies damage, and records the cooldown. In this
example the two phases are adjacent, with no allocation or removal between
them. The active-slot check cannot detect retirement followed by reuse of the
same slot; a deferred intent needs a generation or world revision as well as
the slot ID.

The complete implementation and boundary tests are under [examples/](examples/).
