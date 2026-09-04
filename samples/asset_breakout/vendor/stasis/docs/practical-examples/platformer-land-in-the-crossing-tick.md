# Platformer: Land in the Crossing Tick

**Goal:** if gravity moves the player through the floor boundary, finish the same tick on the floor and grounded.

Apply gravity, move, then resolve the boundary. Landing owns position, velocity, and grounded state together.

```stasis
function tick(): i32 {
    platformer.player_vy += 1;
    platformer.player_y += platformer.player_vy;
    platformer.grounded = false;
    if (platformer.player_y >= platformer.floor_y) {
        platformer.player_y = platformer.floor_y;
        platformer.player_vy = 0;
        platformer.grounded = true;
    }
    return 0;
}
```

```stasis
test `falling through the floor boundary lands in the same tick`(): bool {
    reset_platformer();
    platformer.player_y = 8;
    platformer.player_vy = 2;

    tick();

    return platformer.player_y == platformer.floor_y && platformer.player_vy == 0 && platformer.grounded;
}
```

Full source: [platformer_landing.stasis](../examples/src/platformer_landing.stasis)
