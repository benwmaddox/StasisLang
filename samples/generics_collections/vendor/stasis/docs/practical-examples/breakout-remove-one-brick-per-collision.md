# Breakout: Remove One Brick on a Vertical Hit

**Goal:** when a moving ball point enters overlapping bricks vertically, remove one deterministic brick and reflect once.

Move the ball, query the first active point hit by stable slot order, then commit one result in `tick()`.

```stasis
function first_brick_at(x: i32, y: i32): i32 {
    for (let brick_id: i32 = 0; brick_id < BRICK_CAPACITY; brick_id += 1) {
        if (
            bricks[brick_id].active
            && x >= bricks[brick_id].left
            && x <= bricks[brick_id].right
            && y >= bricks[brick_id].top
            && y <= bricks[brick_id].bottom
        ) {
            return brick_id;
        }
    }
    return -1;
}
```

```stasis
function tick(): i32 {
    breakout.ball_x += breakout.ball_dx;
    breakout.ball_y += breakout.ball_dy;
    let hit_id: i32 = first_brick_at(breakout.ball_x, breakout.ball_y);
    if (hit_id >= 0) {
        bricks[hit_id].active = false;
        breakout.ball_dy = -breakout.ball_dy;
    }
    return 0;
}
```

```stasis
test `one tick removes one overlapping brick and reflects once`(): bool {
    reset_breakout();
    for (let brick_id: i32 = 0; brick_id < BRICK_CAPACITY; brick_id += 1) {
        bricks[brick_id].active = true;
        bricks[brick_id].left = 4;
        bricks[brick_id].right = 6;
        bricks[brick_id].top = 4;
        bricks[brick_id].bottom = 6;
    }
    breakout.ball_x = 5;
    breakout.ball_y = 3;
    breakout.ball_dy = 1;

    tick();

    return !bricks[0].active && bricks[1].active && breakout.ball_dy == -1;
}
```

Full source: [breakout_brick.stasis](../examples/src/breakout_brick.stasis)
