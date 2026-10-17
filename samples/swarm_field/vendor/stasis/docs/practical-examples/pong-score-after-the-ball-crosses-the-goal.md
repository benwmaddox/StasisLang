# Pong: Score After the Ball Crosses the Goal

**Goal:** award one point, reset the ball, and send it back into play.

Court bounds are runtime settings on the Pong state, with defaults on their
owning record. Move first, then resolve either goal once in `tick()`.

```stasis
function tick(): i32 {
    pong.ball_x += pong.ball_dx;
    if (pong.ball_x < pong.settings.court_left) {
        pong.right_score += 1;
        pong.ball_x = court_center();
        pong.ball_dx = 1;
    } else if (pong.ball_x > pong.settings.court_right) {
        pong.left_score += 1;
        pong.ball_x = court_center();
        pong.ball_dx = -1;
    }
    return 0;
}
```

Use a non-default court to test the transition rather than baking the sample's
default dimensions into the assertion. `reset_pong()` resets play and preserves
the settings, so a restart does not discard the configured bounds.

```stasis
test `crossing the right goal scores once and resets play`(): bool {
    if (pong.settings.court_left != 0 || pong.settings.court_right != 100) {
        return false;
    }
    pong.settings.court_left = 10;
    pong.settings.court_right = 50;
    reset_pong();
    if (pong.settings.court_left != 10 || pong.settings.court_right != 50) {
        return false;
    }
    pong.left_score = 4;
    pong.right_score = 7;
    pong.ball_x = pong.settings.court_right;
    pong.ball_dx = 1;

    tick();
    let center: i32 =(pong.settings.court_left + pong.settings.court_right) / 2;
    if (pong.left_score != 5 || pong.right_score != 7 || pong.ball_x != center || pong.ball_dx != -1) {
        return false;
    }

    tick();

    return pong.left_score == 5 && pong.right_score == 7 && pong.ball_x == center - 1 && pong.ball_dx == -1;
}
```

Full source: [pong_goal.stasis](../examples/src/pong_goal.stasis)
