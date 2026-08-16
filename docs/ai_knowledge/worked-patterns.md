# Worked patterns

<!-- tags: worked-example, brickout, pong, data-driven, repair, verification -->

This document combines the repository patterns into small implementation shapes.
The snippets are **pseudocode** unless explicitly marked otherwise; they show
boundaries and invariants, not drop-in Stasis syntax.

## Pattern: brick-breaker tick

Source: `samples/brickout_revenge/`, `samples/headless_scenario/`,
`samples/state_inspection/`, `docs/render_parity_gate.md`.

```text
tick(state, commands):
    commands = normalize_input(commands)
    move_paddle(state, commands, FIXED_DT)
    move_ball(state, FIXED_DT)
    resolve_world_bounds(state)
    resolve_paddle_and_brick_contacts(state)
    update_lives_score_and_wave(state)
    update_cooldowns(state)
    return state

render(state):
    draw_walls(state)
    draw_paddle(state)
    draw_ball(state)
    draw_bricks(state)
    draw_hud(state)
```

The update sequence makes collision, score, and lifecycle changes authoritative;
the renderer consumes their result. A scenario can inspect ball position,
remaining bricks, score, lives, phase, and wave after each selected tick.

## Pattern: Pong-like bounded round

Source: `mobile/android/app/src/main/assets/workshop_sample/src/main.stasis` and
the workshop Pong architecture, harness, and verification documentation.
This is a generic phase design, not the checked-in Workshop Pong state machine;
that sample uses only its own `Playing` and `Finished` phases.

```text
state = { phase: READY, left_score: 0, right_score: 0, ... }

on START in READY:
    reset_round_positions()
    phase = PLAYING

on tick in PLAYING:
    apply_player_commands()
    move_ball()
    resolve_paddles_and_bounds()
    if point_won:
        increment_score()
        phase = READY or GAME_OVER according to the rule
```

Keep the point/round transition in state logic. The HUD is a projection of score
and phase, not a second scoreboard.

## Pattern: data-driven selection

Source: `samples/typed_sprite/`, `samples/state_inspection/`,
`samples/headless_scenario/`.

```text
catalog = table keyed by stable ID
state = { selected_id, query, visible_ids }

update(state, SELECT(id)):
    require catalog contains id
    state.selected_id = id
    state.visible_ids = filter_and_bound(catalog, state.query)

render(state):
    render_rows(state.visible_ids)
    render_selected(catalog[state.selected_id])
```

The stable key prevents selection from depending on row position. A headless
scenario can select, filter, inspect IDs, and compare the rendered projection.

## Pattern: repair from first divergence

```text
replay(trace, initial_state)
for tick in trace:
    expected = fixture[tick]
    actual = update(actual, trace.commands[tick], FIXED_DT)
    if inspect(actual) != expected.state:
        compare_fields(expected.state, inspect(actual))
        fix_transition_or_invariant()
        rerun_from_initial_state()
```

This is **pseudocode**. The repository workflow supplies the concrete scenario,
inspection, parity, and bounded-performance mechanisms. Do not patch the final
frame first; repair the earliest divergent transition.

## Completion checklist

- [ ] Initial state is declared and inspectable.
- [ ] Input, update, and render are separate.
- [ ] Data rows and runtime state use stable references.
- [ ] Timing units/policy and bounded work are explicit.
- [ ] Equality edges have tests.
- [ ] Scenario, state inspection, and render parity agree.
- [ ] Repair evidence identifies the first divergence.
