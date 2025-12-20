# Flappy Birds In Stasis (A Tiny, Deterministic Clone)

Welcome! We are going to build a clean, deterministic Flappy Birds clone with Stasis and the standard library. It is a small loop with pure math, global state, and testable collision checks. The result is simple, fast, and great for iteration.

We will lean on two modules:

- `stdlib` for core helpers
- `game` for AABB collision

You will explicitly import both (nothing is auto-included).

---

## 1) Create The Source File

Create `examples/flappy_birds.stasis` with the full code below.

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

## 2) How The Loop Works

We split the loop into tiny, testable pieces:

- `update_bird()` applies gravity and moves the bird
- `update_pipes()` scrolls pipes and respawns them
- `check_collision()` uses `game_aabb_intersects` on top/bottom pipes
- `step(input_flap)` runs one frame and returns alive/dead

This mirrors the classic game flow, but it stays deterministic and easily tested.

---

## 3) Run The Tests

From the repo root:

```
Stasis.Cli\bin\Debug\net9.0\Stasis.Cli.exe test examples\flappy_birds.stasis --backend cranelift
```

You should see three passing tests. That means the core physics and collision logic are solid.

---

## 4) Add A Simple Render Loop (Optional)

If you want visuals, call `step()` in a loop and wire in the graphics runtime (SDL bindings). The same logic still holds; you just draw rectangles for bird and pipes.

Minimal idea:

- Bird rectangle at `(BIRD_X, bird_y)`
- Pipe rectangles for top and bottom
- Call `step(is_key_down(...))` each frame

---

## 5) Next Fun Improvements

- Add score text and sound
- Scale difficulty by increasing `PIPE_SPEED`
- Use `game_aabb_sweep_resolve` for smoother responses

You now have a clean, deterministic Flappy Birds core. That is an excellent base to build from.
