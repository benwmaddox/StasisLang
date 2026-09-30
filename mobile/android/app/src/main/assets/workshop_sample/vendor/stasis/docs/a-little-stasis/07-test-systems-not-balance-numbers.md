# 07. Test systems, not balance numbers

How do you find the first wrong system when a fixture value changes?

Use a short testing ladder. Start with one query, then one direct action, then the public tick boundary, and finally a short trace. Assert ownership, ordering, gates, and interactions. Health and damage values are fixtures; a relational change can still prove that an attack happened.

First, isolate a query. This test protects the targeting tie-break without testing combat balance.

```stasis
test `target ties choose the lower stable slot id`(): bool {
    reset_simulation();
    let first_id: i32 = allocate_enemy(5);
    let second_id: i32 = allocate_enemy(5);
    enemies[first_id].path_position = 4;
    enemies[second_id].path_position = enemies[first_id].path_position;

    return first_id < second_id && select_target() == first_id;
}
```

Next, test one gate directly. The cooldown blocks materialization until the
system advances it to ready.

```stasis
test `cooldown gates attack materialization until ready`(): bool {
    reset_simulation();
    let target_id: i32 = allocate_enemy(5);
    state.cooldown_ticks = 1;

    materialize_attack();
    let blocked: bool = !pending_attack.valid;
    advance_cooldown();
    materialize_attack();

    return blocked && pending_attack.valid && pending_attack.target_id == target_id;
}
```

Then cross the public boundary with the interaction test from
[A tick is an ordered recipe](03-a-tick-is-an-ordered-recipe.md). Do not repeat
it here; add the next layer.

Finally, checkpoint a few ticks. Stop at the first failed checkpoint: the earliest difference narrows the search to the systems before that observation.

```stasis
test `short tick trace reveals the first divergent system`(): bool {
    reset_simulation();
    let target_id: i32 = allocate_enemy(9);
    submit_tower_rules(2, 8, 2);
    let damage: i32 = pending_inputs[0].damage;
    let cooldown: i32 = pending_inputs[0].cooldown_ticks;
    let initial_health: i32 = enemies[target_id].health;

    tick();
    if (
        state.tick_index != 1
        || enemies[target_id].path_position != 1
        || enemies[target_id].health != initial_health - damage
        || state.cooldown_ticks != cooldown
    ) {
        return false;
    }

    let health_after_attack: i32 = enemies[target_id].health;
    tick();
    if (
        state.tick_index != 2
        || enemies[target_id].path_position != 2
        || enemies[target_id].health != health_after_attack
        || state.cooldown_ticks != cooldown - 1
    ) {
        return false;
    }

    tick();
    return state.tick_index == 3 && enemies[target_id].path_position == 3 && enemies[target_id].health == health_after_attack - damage && state.cooldown_ticks == cooldown;
}
```

**Keep:** Assert system contracts and interactions; do not turn current balance numbers into permanent rules.
