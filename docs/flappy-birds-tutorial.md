# Flappy Birds In Stasis (A Tiny, Deterministic Clone)

Welcome! We are going to build a clean, deterministic Flappy Birds clone with Stasis and the standard library. The core loop stays tiny and testable, and the visuals are driven by a couple of sprites.

## The Stasis approach (quick overview)

Stasis is built around a few core ideas that shape this tutorial:

- Static global memory only: all game state lives in globals.
- AoS in code, SoA in memory: struct fields lower to parallel arrays for fast, predictable access.
- Deterministic logic: no hidden allocation or implicit copies.
- Tests are first class: small functions stay testable, even inside games.
- Backends: Cranelift is fast for iteration; LLVM is for release builds.

We will lean on two modules:

- `stdlib` for core helpers
- `game` for AABB collision

You will explicitly import both (nothing is auto-included).

---

## 1) Create The Assets

We will use the `.stv` sprite format so you can hot-reload visuals. Create these files:

`assets_src/flappy-birds/bird.stv`

```
stv 1
size 16 12
rgba 1 0.9 0.2 1
rect 2 3 12 6
rgba 0.95 0.7 0.15 1
circle 12 6 2
rgba 0 0 0 1
rect 10 4 2 2
```

`assets_src/flappy-birds/pipe.stv`

```
stv 1
size 24 64
rgba 0.2 0.9 0.3 1
rect 0 0 24 64
rgba 0.15 0.75 0.25 1
rect 2 4 20 56
rgba 0.1 0.6 0.2 1
rect 0 0 24 6
```

---

## 2) Core Gameplay Module

Create `examples/flappy_birds_core.stasis`. This file is pure logic: no rendering and no input. It is safe to import from tests.

```
import "../src/stdlib/stdlib.stasis";
import "../src/stdlib/game.stasis";

const BIRD_X: f32 = 20.0;
const BIRD_W: f32 = 8.0;
const BIRD_H: f32 = 6.0;
const GRAVITY: f32 = 0.25;
const FLAP_V: f32 = 3.5;

const PIPE_W: f32 = 12.0;
const GAP_HALF: f32 = 20.0;
const PIPE_SPACING: f32 = 60.0;
const PIPE_SPEED: f32 = 1.5;

const WORLD_W: f32 = 200.0;
const WORLD_H: f32 = 120.0;

global bird_y: f32;
global bird_vy: f32;
global score: i32;
global pipe_x: f32[3];
global pipe_gap_y: f32[3];
global rng_state: i32;

function rand_next(): i32 {
    rng_state = rng_state * 1103515245 + 12345;
    return rng_state;
}

function rand_gap_y(): f32 {
    let r: i32 = rand_next();
    let v: i32 = r % 1000;
    if (v < 0) { v = -v; }
    let vf: f32 = v;
    let range: f32 = WORLD_H - 2.0 * GAP_HALF - 10.0;
    let base: f32 = 5.0 + GAP_HALF;
    return base + range * (vf / 1000.0);
}

function reset() {
    bird_y = WORLD_H * 0.5;
    bird_vy = 0.0;
    score = 0;
    rng_state = 1;

    let i: i32 = 0;
    for (i = 0; i < 3; i = i + 1) {
        pipe_x[i] = WORLD_W + (PIPE_SPACING * i);
        pipe_gap_y[i] = rand_gap_y();
    }
}

function flap() {
    bird_vy = -FLAP_V;
}

function update_bird() {
    bird_vy = bird_vy + GRAVITY;
    bird_y = bird_y + bird_vy;
}

function update_pipes() {
    let i: i32 = 0;
    for (i = 0; i < 3; i = i + 1) {
        pipe_x[i] = pipe_x[i] - PIPE_SPEED;
        if (pipe_x[i] < -PIPE_W) {
            pipe_x[i] = pipe_x[i] + (PIPE_SPACING * 3.0);
            pipe_gap_y[i] = rand_gap_y();
            score = score + 1;
        }
    }
}

function check_collision(): bool {
    if (bird_y < 0.0) { return true; }
    if (bird_y + BIRD_H > WORLD_H) { return true; }

    let bird_min_x: f32 = BIRD_X;
    let bird_min_y: f32 = bird_y;
    let bird_max_x: f32 = BIRD_X + BIRD_W;
    let bird_max_y: f32 = bird_y + BIRD_H;

    let i: i32 = 0;
    for (i = 0; i < 3; i = i + 1) {
        let px: f32 = pipe_x[i];
        let gap: f32 = pipe_gap_y[i];

        let top_min_x: f32 = px;
        let top_min_y: f32 = 0.0;
        let top_max_x: f32 = px + PIPE_W;
        let top_max_y: f32 = gap - GAP_HALF;

        let bot_min_x: f32 = px;
        let bot_min_y: f32 = gap + GAP_HALF;
        let bot_max_x: f32 = px + PIPE_W;
        let bot_max_y: f32 = WORLD_H;

        if (game_aabb_intersects(bird_min_x, bird_min_y, bird_max_x, bird_max_y, top_min_x, top_min_y, top_max_x, top_max_y)) {
            return true;
        }
        if (game_aabb_intersects(bird_min_x, bird_min_y, bird_max_x, bird_max_y, bot_min_x, bot_min_y, bot_max_x, bot_max_y)) {
            return true;
        }
    }

    return false;
}

function step(input_flap: bool): bool {
    if (input_flap) {
        flap();
    }
    update_bird();
    update_pipes();
    return !check_collision();
}
```

---

## 3) Visual Main File (Sprites)

Create `examples/flappy_birds.stasis`. This file adds input and graphics using sprites.

```
import "../src/stdlib/stdlib.stasis";
import "flappy_birds_core.stasis";

const SCREEN_W: i32 = 400;
const SCREEN_H: i32 = 240;
const SCALE: f32 = 2.0;

const BIRD_SPRITE_W: f32 = 16.0;
const BIRD_SPRITE_H: f32 = 12.0;
const PIPE_SPRITE_W: f32 = 24.0;
const PIPE_SPRITE_H: f32 = 64.0;

const KEY_SPACE: i32 = 44;

global spr_bird: i32;
global spr_pipe: i32;

function init_assets() {
    spr_bird = gfx_load_sprite("assets_src/flappy-birds/bird.stv");
    spr_pipe = gfx_load_sprite("assets_src/flappy-birds/pipe.stv");
}

function to_screen_x(x: f32): f32 {
    return x * SCALE;
}

function to_screen_y(y: f32): f32 {
    return y * SCALE;
}

function draw_pipe(px: f32, gap: f32) {
    let pipe_scale_x: f32 = (PIPE_W * SCALE) / PIPE_SPRITE_W;

    let top_h: f32 = gap - GAP_HALF;
    if (top_h > 0.5) {
        let sy_top: f32 = (top_h * SCALE) / PIPE_SPRITE_H;
        let cx_top: f32 = to_screen_x(px + PIPE_W * 0.5);
        let cy_top: f32 = to_screen_y(top_h * 0.5);
        gfx_draw_sprite(spr_pipe, cx_top, cy_top, pipe_scale_x, sy_top, 0.0, 1.0, 1.0, 1.0, 1.0);
    }

    let bottom_start: f32 = gap + GAP_HALF;
    let bottom_h: f32 = WORLD_H - bottom_start;
    if (bottom_h > 0.5) {
        let sy_bottom: f32 = (bottom_h * SCALE) / PIPE_SPRITE_H;
        let cx_bottom: f32 = to_screen_x(px + PIPE_W * 0.5);
        let cy_bottom: f32 = to_screen_y(bottom_start + bottom_h * 0.5);
        gfx_draw_sprite(spr_pipe, cx_bottom, cy_bottom, pipe_scale_x, sy_bottom, 0.0, 1.0, 1.0, 1.0, 1.0);
    }
}

function draw_frame() {
    begin_frame();
    clear(0.05, 0.07, 0.12, 1.0);

    gfx_poll_reload(spr_bird);
    gfx_poll_reload(spr_pipe);

    let i: i32 = 0;
    for (i = 0; i < 3; i = i + 1) {
        draw_pipe(pipe_x[i], pipe_gap_y[i]);
    }

    let bird_scale_x: f32 = (BIRD_W * SCALE) / BIRD_SPRITE_W;
    let bird_scale_y: f32 = (BIRD_H * SCALE) / BIRD_SPRITE_H;
    let bird_cx: f32 = to_screen_x(BIRD_X + BIRD_W * 0.5);
    let bird_cy: f32 = to_screen_y(bird_y + BIRD_H * 0.5);
    gfx_draw_sprite(spr_bird, bird_cx, bird_cy, bird_scale_x, bird_scale_y, 0.0, 1.0, 1.0, 1.0, 1.0);

    end_frame();
}

function main(): i32 {
    if (!init_window(SCREEN_W, SCREEN_H, "Stasis Flappy")) {
        return 1;
    }

    init_assets();
    reset();

    while (!should_quit()) {
        let do_flap: bool = is_key_down(KEY_SPACE);
        if (!step(do_flap)) {
            reset();
        }
        draw_frame();
        sleep_ms(16);
    }
    return 0;
}
```

---

## 4) Tests Live Separately

Create `tests/flappy_birds.stasis` so test builds do not depend on the graphics runtime:

```
import "flappy_birds_core.stasis";

test `collision with top pipe`() {
    reset();
    bird_y = 10.0;
    pipe_x[0] = BIRD_X;
    pipe_gap_y[0] = 60.0;
    return check_collision();
}

test `bird survives in gap`() {
    reset();
    bird_y = 60.0;
    pipe_x[0] = BIRD_X;
    pipe_gap_y[0] = 60.0;
    return !check_collision();
}

test `pipes move left`() {
    reset();
    let start: f32 = pipe_x[0];
    step(false);
    return pipe_x[0] == start - PIPE_SPEED;
}
```

---

## 5) Run The Tests

From the repo root:

```
Stasis.Cliin\Debug
et9.0\Stasis.Cli.exe test testslappy_birds.stasis --backend cranelift
```

---

## 6) Run With Visuals

Build the graphics runtime and launch:

```
runtimeuild.bat
dotnet run --project Stasis.Cli -- run examples/flappy_birds.stasis --backend cranelift --graphics --graphics-lib runtimeuild\Release\stasis_graphics.dll
```

---

## 7) Next Fun Improvements

- Add score text and sound
- Scale difficulty by increasing `PIPE_SPEED`
- Add a scrolling background sprite

You now have a clean, deterministic Flappy Birds core with hot-reloadable visuals.
