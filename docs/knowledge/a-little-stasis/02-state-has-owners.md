# 02. State has owners

Which code is allowed to change each value?

Give each kind of state its own record. Live rules and pending requests may contain the same fields, but they have different owners and different times at which they may change.

```stasis
struct TowerRules {
    damage: i32;
    range_end: i32;
    cooldown_ticks: i32;
}

struct TowerRuleInput {
    damage: i32;
    range_end: i32;
    cooldown_ticks: i32;
}
```

Simulation progression and presentation data have different owners as well. The render record is a copy for display, not another authority over an enemy slot.

```stasis
struct RenderEnemy {
    visible: bool;
    stable_id: i32;
    path_position: i32;
    health: i32;
}

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

Queueing an input must not silently update live rules. This small test stops before application so that ownership, not the fixture values, is the assertion.

```stasis
test `queued input does not mutate live rules`(): bool {
    reset_simulation();
    let damage_before: i32 = tower_rules.damage;
    let range_before: i32 = tower_rules.range_end;
    let cooldown_before: i32 = tower_rules.cooldown_ticks;

    if (!submit_tower_rules(4, 6, 3)) {
        return false;
    }

    return pending_input_count == 1 && tower_rules.damage == damage_before && tower_rules.range_end == range_before && tower_rules.cooldown_ticks == cooldown_before;
}
```

**Keep:** a value crosses from input to live state only through the owner of that transition.
