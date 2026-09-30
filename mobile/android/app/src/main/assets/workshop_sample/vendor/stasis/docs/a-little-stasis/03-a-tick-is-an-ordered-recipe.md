# 03. A tick is an ordered recipe

If each system works alone, can the game still be wrong?

Yes. The order in which systems observe and change state is part of the rule. Write that order in one small recipe:

```stasis
function tick(): i32 {
    apply_pending_inputs();
    advance_cooldown();
    spawn_due_events();
    move_enemies();
    materialize_attack();
    commit_attack();
    remove_defeated();
    state.tick_index += 1;
    return 0;
}
```

This test observes two dependencies after one public tick: the queued range is
live before targeting, and movement also runs before targeting. The recipe
states the finer ordering between input and movement.

```stasis
test `public tick applies input and movement before targeting`(): bool {
    reset_simulation();
    let target_id: i32 = allocate_enemy(5);
    let health_before: i32 = enemies[target_id].health;
    let position_before: i32 = enemies[target_id].path_position;
    submit_tower_rules(4, 0, 3);

    tick();

    if (state.tick_index != 1 || pending_input_count != 0 || tower_rules.range_end != 0) {
        return false;
    }
    return enemies[target_id].path_position == position_before + 1 && enemies[target_id].health == health_before && !pending_attack.valid;
}
```

The unchanged health is a fixture consequence of the no-target path. The
durable assertions are the observable dependencies, not an ordering the test
cannot distinguish.

**Keep:** system order is behavior, so test interactions at the tick boundary.
