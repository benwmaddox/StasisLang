# 06. Query, Materialize, Commit

How can a system decide an action without mutating the world while it is still deciding?

A query selects a stable ID. Materialization records an intent. Commit validates that intent and owns the state change.

## Query and materialize

`select_target()` reads active slots and applies a deterministic priority. The materializer clears stale intent before recording the selected action.

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
```

## Commit once

Commit rechecks the slot, changes health, starts the cooldown, and consumes the intent. A rejected or repeated commit changes nothing.

```stasis
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

## Test the transition

The values are fixtures. This test protects commit ownership and once-only consumption.

```stasis
test `attack commit consumes one intent once`(): bool {
    reset_simulation();
    let target_id: i32 = allocate_enemy(5);
    pending_attack.valid = true;
    pending_attack.target_id = target_id;
    pending_attack.damage = 3;
    let health_before: i32 = enemies[target_id].health;
    let damage: i32 = pending_attack.damage;

    let first_commit: bool = commit_attack();
    let health_after: i32 = enemies[target_id].health;
    let second_commit: bool = commit_attack();

    return first_commit && !second_commit && health_after == health_before - damage && enemies[target_id].health == health_after;
}
```

**Keep:** query what may happen, materialize one intent, and let one commit own the transition.
