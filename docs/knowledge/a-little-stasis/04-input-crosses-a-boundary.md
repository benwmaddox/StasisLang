# 04. Input Crosses a Boundary

How does a request become part of the next tick without changing live rules early?

An input has two owners in sequence: the boundary validates and queues it, then the simulation applies and consumes it.

## Validate, then enqueue

Invalid values and a full queue return `false` before storage. The count owns the next write position.

```stasis
function submit_tower_rules(damage: i32, range_end: i32, cooldown_ticks: i32): bool {
    if (damage <= 0 || range_end < 0 || cooldown_ticks < 0) {
        return false;
    }
    if (pending_input_count >= INPUT_CAPACITY) {
        return false;
    }
    let input_index: i32 = pending_input_count;
    pending_inputs[input_index].damage = damage;
    pending_inputs[input_index].range_end = range_end;
    pending_inputs[input_index].cooldown_ticks = cooldown_ticks;
    pending_input_count += 1;
    return true;
}
```

## Apply, then consume

Accepted entries apply in order. Clearing the count makes the queue empty; old array cells are no longer owned by it.

```stasis
function apply_pending_inputs(): void {
    for (let input_index: i32 = 0; input_index < pending_input_count; input_index += 1) {
        tower_rules.damage = pending_inputs[input_index].damage;
        tower_rules.range_end = pending_inputs[input_index].range_end;
        tower_rules.cooldown_ticks = pending_inputs[input_index].cooldown_ticks;
    }
    pending_input_count = 0;
}
```

## Test the boundary

The values are fixtures. These tests protect rejection and once-only consumption.

```stasis
test `invalid input never enters the queue`(): bool {
    reset_simulation();

    return !submit_tower_rules(3, -1, 2) && pending_input_count == 0;
}
```

```stasis
test `applied input is consumed once`(): bool {
    reset_simulation();
    submit_tower_rules(3, 7, 1);
    apply_pending_inputs();
    let applied_damage: i32 = tower_rules.damage;
    pending_inputs[0].damage = applied_damage + 1;

    apply_pending_inputs();

    return pending_input_count == 0 && tower_rules.damage == applied_damage;
}
```

**Keep:** input becomes live only through validation, queue ownership, and one ordered consume.

