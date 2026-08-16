# State machines, cooldowns, and waves

<!-- tags: state-machine, phases, cooldowns, waves, transitions, games, timers -->

## Explicit phases

Represent mutually exclusive gameplay modes as a finite state machine. Typical
phases include `ready`, `playing`, `paused`, `cleared`, and `game_over`; the exact
names should match the product.

| Phase | Accepts | Transition examples |
| --- | --- | --- |
| Ready | Start | Playing |
| Playing | Movement, fire, collision results | Paused, Cleared, Game over |
| Paused | Resume, quit | Playing, Ready |
| Cleared | Advance, restart | Ready or Playing |
| Game over | Restart | Ready |

The table is a design table, not a compilable Stasis excerpt. Source:
`samples/brickout_revenge/`, `samples/headless_scenario/`, and the workshop Pong
architecture/verification documentation.

## Transition discipline

```text
phase + command + facts -> (next phase, effects)
```

**Pseudocode:**

```text
if phase == PLAYING and lives == 0:
    phase = GAME_OVER
elif phase == PLAYING and remaining_targets == 0:
    phase = CLEARED
```

Centralize transitions so each phase has one owner. Entering a phase may reset a
timer or spawn a wave; rendering should display phase, not decide it.

## Cooldowns

Store cooldowns in one explicit integer unit, such as ticks or milliseconds.
For a cooldown with a nonnegative contract, enforce:

```text
0 <= cooldown <= cooldown_limit
```

On each update, decrement according to the declared timing policy. Clamp toward
zero when nonnegative cooldowns are part of the design. A command can fire only
when the value is ready; a successful fire sets it to the configured interval.
Define whether the fire update itself consumes time and test that boundary.

Source: `docs/android_workshop_codex_harness.md`, `samples/brickout_revenge/`,
`samples/bounded_performance/`.

## Waves

Represent a wave as data plus progress state: wave ID, spawn definition, total,
spawned, remaining, and optional delay/cooldown. Bound active entities and spawn
work per tick. A transition should be explicit:

```text
spawn_delay == 0 and spawned < total -> spawn bounded batch
remaining == 0 and spawned == total -> mark wave cleared
```

The above is **pseudocode**. Keep wave definitions in a table/configuration and
wave progress in runtime state. This makes balancing data-driven while keeping
the lifecycle deterministic.

## Verification checklist

- Every phase has allowed commands and terminal behavior.
- Every transition has a named trigger.
- Cooldown bounds and ready values are explicit and tested.
- Wave spawning has per-tick and total bounds.
- Restart returns all phase, timer, entity, and score fields to the declared
  initial state.
- Headless scenarios cover phase and timer boundaries.
