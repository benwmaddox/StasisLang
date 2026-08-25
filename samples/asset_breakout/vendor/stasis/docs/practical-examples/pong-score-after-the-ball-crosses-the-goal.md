# Pong: Score After the Ball Crosses the Goal

**Goal:** award one point, reset the ball, and send it back into play.

Move first. Resolve either goal once. Keep scoring and reset in the same `tick()`.

```stasis
function tick(): i32 {
    pong.ball_x += pong.ball_dx;
    if (pong.ball_x < COURT_LEFT) {
        pong.right_score += 1;
        pong.ball_x = COURT_CENTER;
        pong.ball_dx = 1;
    } else if (pong.ball_x > COURT_RIGHT) {
        pong.left_score += 1;
        pong.ball_x = COURT_CENTER;
        pong.ball_dx = -1;
    }
    return 0;
}
```

Test the whole transition, not the chosen court dimensions.

```stasis
test `crossing the right goal scores once and resets play`(): bool {
    reset_pong();
    pong.left_score = 4;
    pong.right_score = 7;
    pong.ball_x = COURT_RIGHT;
    pong.ball_dx = 1;

    tick();
    if (pong.left_score != 5 || pong.right_score != 7 || pong.ball_x != COURT_CENTER || pong.ball_dx != -1) {
        return false;
    }

    tick();

    return pong.left_score == 5 && pong.right_score == 7 && pong.ball_x == COURT_CENTER - 1 && pong.ball_dx == -1;
}
```

Full source: [pong_goal.stasis](../examples/src/pong_goal.stasis)

