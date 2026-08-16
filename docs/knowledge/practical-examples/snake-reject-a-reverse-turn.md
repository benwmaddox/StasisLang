# Snake: Reject a Reverse Turn

**Goal:** consume a queued turn without allowing a 180-degree direction change.

Compare the queued direction with the current direction at the start of `tick()`. Move only after the direction is settled.

```stasis
function tick(): i32 {
    if (snake.has_queued_turn) {
        let reverses_x: bool = snake.queued_x == -snake.direction_x;
        let reverses_y: bool = snake.queued_y == -snake.direction_y;
        if (!reverses_x && !reverses_y) {
            snake.direction_x = snake.queued_x;
            snake.direction_y = snake.queued_y;
        }
        snake.has_queued_turn = false;
    }
    snake.head_x += snake.direction_x;
    snake.head_y += snake.direction_y;
    return 0;
}
```

```stasis
test `a reverse turn is consumed without reversing movement`(): bool {
    reset_snake();
    let x_before: i32 = snake.head_x;
    if (!queue_turn(-1, 0)) {
        return false;
    }

    tick();

    return snake.head_x == x_before + 1 && snake.direction_x == 1 && !snake.has_queued_turn;
}
```

Full source: [snake_turn.stasis](../examples/src/snake_turn.stasis)

