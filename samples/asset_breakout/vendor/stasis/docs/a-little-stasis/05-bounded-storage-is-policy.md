# 05. Bounded Storage Is Policy

What does a fixed slot decide when storage fills and an old occupant leaves?

A bounded array makes capacity, allocation order, and reuse visible to the simulation. An inactive slot is available; an active slot has one current owner.

## Allocate by stable slot order

The first free slot wins. A full scan returns `-1`, so the caller must handle capacity explicitly.

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

## Reuse is an ownership transition

Removal releases a slot. The next allocation may reuse its ID, while a survivor keeps its own state.

```stasis
test `allocation reuses the lowest released slot`(): bool {
    reset_simulation();
    let released_id: i32 = allocate_enemy(1);
    let survivor_id: i32 = allocate_enemy(4);
    enemies[released_id].health = 0;
    remove_defeated();

    let reused_id: i32 = allocate_enemy(6);

    return reused_id == released_id && enemies[survivor_id].active;
}
```

**Keep:** capacity and slot reuse are state transitions, not hidden collection behavior.

