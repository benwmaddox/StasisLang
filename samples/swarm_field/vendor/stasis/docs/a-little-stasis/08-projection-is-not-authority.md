# 08. Projection is not authority

What may rendering change, and what must remain untouched?

Rendering copies authoritative records into its own bounded projection. It may rebuild presentation data, but it must not advance the tick or mutate gameplay state.

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

Test the authority boundary separately. Rendering must preserve the simulation values it reads.

```stasis
test `render preserves authoritative gameplay state`(): bool {
    reset_simulation();
    let enemy_id: i32 = allocate_enemy(5);
    enemies[enemy_id].path_position = 3;
    let tick_before: i32 = state.tick_index;
    let health_before: i32 = enemies[enemy_id].health;
    let position_before: i32 = enemies[enemy_id].path_position;

    render();

    return state.tick_index == tick_before && enemies[enemy_id].health == health_before && enemies[enemy_id].path_position == position_before && enemies[enemy_id].active;
}
```

Then test the projection itself. Stable slot order and copied fields are the contract here, not a particular health value.

```stasis
test `render projects active slots in stable order`(): bool {
    reset_simulation();
    let first_id: i32 = allocate_enemy(5);
    let second_id: i32 = allocate_enemy(7);
    enemies[first_id].path_position = 2;
    enemies[second_id].path_position = 6;

    render();

    if (render_enemy_count != 2) {
        return false;
    }
    let first_matches: bool = render_enemies[0].stable_id == first_id && render_enemies[0].path_position == enemies[first_id].path_position;
    let second_matches: bool = render_enemies[1].stable_id == second_id && render_enemies[1].path_position == enemies[second_id].path_position;
    return first_matches && second_matches;
}
```

**Keep:** Rendering is a repeatable projection of gameplay state, never an owner of gameplay state.
