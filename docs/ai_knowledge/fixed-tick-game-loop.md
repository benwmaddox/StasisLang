# Fixed-tick game loop

<!-- tags: games, fixed-tick, input, update, render, determinism, performance -->

## Recommended fixed-step order

Fixed-step simulation is a useful design option when repeatability matters.
Rendering may occur at a different rate, but it must not change authoritative
state. Stasis does not require this policy: existing games may instead consume
a bounded wall-clock delta, expressed explicitly in milliseconds or derived
seconds.

```text
sample input -> queue commands -> run bounded fixed updates -> render snapshot
```

This is **pseudocode**. The exact host/runtime API belongs to the workshop and
renderer sources.

Source: `docs/android_workshop_game_architecture_recommendations.md`,
`docs/android_workshop_codex_harness.md`, `samples/bounded_performance/`,
`samples/headless_scenario/`, `samples/brickout_revenge/`.

## Fixed-step invariants

- Every fixed-step update receives the same `dt`.
- Input is converted to commands at a defined point in the tick.
- Collision and timers are evaluated during update, not during render.
- A frame can be skipped or redrawn without changing simulation state.
- Catch-up work has a maximum per host frame.
- Entity creation/removal is safe for the current iteration.

## Pseudocode loop

```text
accumulator += elapsed_host_time
steps = 0
while accumulator >= FIXED_DT and steps < MAX_STEPS:
    commands = consume_input_for_tick()
    state = update(state, commands, FIXED_DT)
    accumulator -= FIXED_DT
    steps += 1
render(state)
```

The loop is **pseudocode**, not a compilable Stasis excerpt. `MAX_STEPS` is a
bounded-work guard: when the host falls behind, the program must have an
explicit policy for the leftover time (for example, retain it or report a
diagnostic), rather than silently running unlimited updates.

## Input policy

Separate edge events from held controls. An edge event such as “press” should be
consumed once; a held direction may contribute every tick while active. Record
the policy in the scenario so replay uses the same command sequence.

## Timer policy

Choose one declared timer unit, such as fixed ticks or milliseconds. Decrement
once per update according to the selected timing policy. If the timer contract
is nonnegative, clamp at zero. Trigger on a named crossing such as `previous >
0` and `next == 0`, avoiding repeated trigger behavior.

## Verification

Use headless scenarios for exact tick counts and state expectations; current
scenario execution does not call `render()`. Validate the rendered path
separately with render-parity evidence. Check bounded performance separately so
a visually correct loop cannot hide excessive catch-up or allocation. If an
input replay facility is used, preserve its exact command sequence as evidence.

Source: `samples/headless_scenario/`, `samples/state_inspection/`,
`docs/render_parity_gate.md`, `samples/bounded_performance/`.
