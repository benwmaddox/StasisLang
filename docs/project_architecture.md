# A practical structure for Stasis games

This is a useful default for a Stasis game. Start with it, keep it simple, and
split it into more files only when the game grows.

```text
host input -> game intent -> tick systems -> game state -> render commands
```

The important boundaries are straightforward:

- Refresh one caller-owned `HostFrame` once per tick, then translate its input into game actions.
- During play, change gameplay state only from tick logic.
- Run gameplay systems in a visible, deliberate order.
- Make `render()` draw the state that already exists.
- Keep every queue, array, and per-tick workload bounded.

These rules prevent common game bugs: repeated clicks at high simulation speed,
preview and execution disagreeing, render-dependent behavior, hidden update
order, and hot reload leaving old cached data behind.

## Start with one obvious game loop

A contributor should be able to open `main.stasis` and quickly find four entry
points:

- `main()` creates the window, loads required assets, and initializes the game.
- `tick()` reads input and advances the game.
- `render()` draws the current state.
- `on_code_swap()` repairs or invalidates state after a live edit.

Keep `tick()` short enough to read as the game's update schedule:

```stasis
function tick(): i32 {
    host_frame.refresh();
    if (host_frame.quit_requested) { return 1; }

    bind_input();
    handle_screen_actions();

    let step: i32 = 0;
    let step_count: i32 = simulation_steps_this_tick();
    for (step = 0; step < step_count; step = step + 1) {
        simulation_step();
    }

    prepare_world_cache();
    update_effect_timers();
    return 0;
}
```

This shape supports menus, pause, slow motion, fast-forward, replay, and tests
without creating separate game-rule paths.

## Keep the game in one root state

Use one global root record by default. It makes persistent state easy to inspect
and gives live code replacement one clear layout to preserve.

```stasis
const MAX_ACTORS: i32 = 128;
const MAX_INTENTS: i32 = 8;

enum Screen {
    Menu,
    Playing,
    Paused,
    Result,
}

enum IntentKind {
    None,
    Start,
    Move,
    UseAction,
    Pause,
    Resume,
}

struct Actor {
    active: bool;
    x_milli: i32;
    y_milli: i32;
    hp: i32;
}

struct Intent {
    kind: IntentKind;
    actor_index: i32;
    target_x: i32;
    target_y: i32;
}

struct GameWorld {
    actors: Actor[128];
    actor_count: i32;
    tick: i32;
    revision: i32;
}

struct TickInput {
    intents: Intent[8];
    intent_count: i32;
    overflowed: bool;
}

struct UiState {
    screen: Screen;
    selected_actor: i32;
    effect_ticks: i32;
}

struct WorldCache {
    world_revision: i32;
    dirty: bool;
    occupancy: i32[256];
}

struct Resources {
    actor_sprite: Sprite;
    font: i32;
}

struct AppState {
    world: GameWorld;
    input: TickInput;
    ui: UiState;
    cache: WorldCache;
    resources: Resources;
}

global game: AppState;
global host_frame: HostFrame;
```

The groups answer practical questions:

- `world`: What must be saved or replayed to reproduce the game?
- `input`: What is the player trying to do during this host tick?
- `ui`: What screen, selection, or short-lived effect is visible?
- `cache`: What lookup data can be rebuilt from the world?
- `resources`: Which host-owned sprite, font, or audio handles are loaded?

Small games can keep these fields flat. Add a nested record when it clarifies
ownership or lifetime, not just to make the root record shorter.

## Translate input into game actions

Input code should turn keys and pointers into a small list of typed intents. It
should not move actors, spend currency, or decide whether an action is legal.

```stasis
function clear_input(): void {
    game.input.intent_count = 0;
    game.input.overflowed = false;
}

function push_intent(kind: IntentKind, actor_index: i32,
    target_x: i32, target_y: i32): bool {
    if (game.input.intent_count >= MAX_INTENTS) {
        // Keep the earliest inputs. Later inputs are rejected predictably.
        game.input.overflowed = true;
        return false;
    }

    let index: i32 = game.input.intent_count;
    game.input.intents[index].kind = kind;
    game.input.intents[index].actor_index = actor_index;
    game.input.intents[index].target_x = target_x;
    game.input.intents[index].target_y = target_y;
    game.input.intent_count = index + 1;
    return true;
}

function bind_input(): void {
    clear_input();

    if (host_frame.pointer_count > 0 && host_frame.pointers[0].went_up) {
        push_intent(
            IntentKind.Move,
            game.ui.selected_actor,
            f32_to_i32(host_frame.pointers[0].x_logical),
            f32_to_i32(host_frame.pointers[0].y_logical)
        );
    }
}
```

Bind input once per host tick. If fast-forward runs four simulation steps, the
same click must not be applied four times.

Use edge input for actions such as click, jump, or pause. Use held input for
continuous actions such as steering. Replays, bots, and tests should feed the
same intent path as physical controls.

## Make simulation order visible

`simulation_step()` is the rules schedule for the game. Its call order is part
of the design:

```stasis
function simulation_step(): void {
    if (game.ui.screen != Screen.Playing) { return; }

    game.world.tick = game.world.tick + 1;
    update_statuses();
    apply_player_actions();
    move_actors();
    resolve_collisions();
    update_objectives();
    remove_destroyed_actors();
    game.world.revision = game.world.revision + 1;
}
```

Here, collisions see the new positions, objectives see resolved collisions, and
destroyed actors remain available until the final cleanup pass. Reordering the
calls changes the rules.

Prefer systems that own one decision for all relevant entities:

- movement changes positions;
- collision resolves contact;
- spawning activates and fully initializes entities;
- objectives decide victory or failure;
- cleanup retires destroyed entities.

This is usually easier to debug than one large `update_actor()` function that
hides movement, combat, and cleanup behind per-entity dispatch. Iterate fixed
arrays in stable index order. If equal choices use the first index, make that a
documented rule and test it.

## Separate checking from changing

Names should make side effects easy to spot:

- `can_*`, `is_*`, and `find_*` answer questions without changing the game;
- `update_*`, `apply_*`, `spawn_*`, and `destroy_*` change game state;
- `prepare_*` and `rebuild_*` update rebuildable lookup data;
- `draw_*` only emits render commands.

Do not implement a query by changing the real world, reading the result, and
then trying to restore the old values. That approach becomes fragile as the
game grows.

When an action needs a preview, AI score, replay record, and final execution,
calculate it once:

```text
Intent -> query_action -> ActionOutcome -> apply_action
```

`ActionOutcome` should hold the ordered targets, costs, movement, effects, and
failure reason. Preview draws it; execution applies it. Include the world
revision so an outcome can be recalculated if the game advances before the
player confirms it.

## Rebuild cached data in one place

Navigation grids, occupancy maps, spatial indexes, and similar data are useful,
but they should have one clear rebuild point:

```stasis
function prepare_world_cache(): void {
    if (!game.cache.dirty &&
        game.cache.world_revision == game.world.revision) {
        return;
    }

    rebuild_occupancy();
    rebuild_navigation();
    game.cache.world_revision = game.world.revision;
    game.cache.dirty = false;
}
```

World-changing systems mark the data dirty or advance the world revision.
Readers do not quietly rebuild it for themselves. This keeps expensive work out
of draw calls and makes stale-cache bugs easier to locate.

## Render the state; do not finish the rules

By the time `render()` starts, the visible game should already be decided:

```stasis
function render(): i32 {
    begin_frame();
    clear(0.03, 0.04, 0.07, 1.0);

    if (game.ui.screen == Screen.Menu) {
        draw_menu();
    } else {
        draw_world();
        draw_effects();
        draw_hud();
    }

    end_frame();
    return 0;
}
```

Rendering may calculate local positions and emit commands. It should not:

- advance time or animation counters;
- accept actions or spend resources;
- decide collisions, targets, damage, or victory;
- rebuild gameplay caches;
- load required assets on demand;
- use sprite size or physical display size to change game rules.

Update animation timers in a named tick system. Share logical rectangles between
layout, hit testing, and drawing so touch targets cannot drift away from their
buttons. Keep interpolation local to rendering; the next tick continues from
game state, not from a rounded draw position.

## Use enums for screens and modes

If only one state can be active, use an enum:

```stasis
enum Screen {
    Menu,
    Playing,
    Paused,
    Result,
}

enum MatchPhase {
    Preparing,
    Active,
    Won,
    Lost,
}
```

A collection of `show_menu`, `paused`, `show_result`, and `modal_open` booleans
usually hides precedence rules in unrelated `if` statements. One enum makes the
valid states and transitions obvious. Input and rendering should read the same
screen value.

## Keep live edits safe

Live code replacement can preserve a record while changing the algorithm that
created its cached data. Invalidate that data in `on_code_swap()`:

```stasis
function on_code_swap(): void {
    update_layout();
    game.cache.dirty = true;
    game.cache.world_revision = -1;
}
```

Use this hook to repair compatible invariants, refresh layout, and invalidate
caches. Do not grant items, spawn enemies, simulate missed ticks, or silently
restart the game. Those are gameplay events, not code-swap repairs.

## Split files when the game needs it

A small game can stay in one source file:

```text
src/main.stasis
tests/game.test.stasis
```

For a larger game, split along the same boundaries:

```text
src/
  main.stasis          lifecycle and update schedule
  game.stasis          stable import facade
  game/
    model.stasis       records, enums, constants, root state
    input.stasis       host input -> game intent
    rules.stasis       queries and value calculations
    systems/           movement, collision, objectives, progression
    view/              world drawing, screens, HUD, assets
    platform/          persistence and other host effects
tests/
  rules.test.stasis
  simulation.test.stasis
  input.test.stasis
  render.test.stasis
```

Stasis files share one project symbol graph; folders communicate ownership but
do not create namespaces. Keep names clear and imports stable. Do not add an ECS,
event bus, or service container until a real game feature is simpler with it.

## Test the boundaries that protect the game

Alongside normal rule tests, cover a few architecture-level cases:

- The same initial state and intents produce the same result.
- A click is consumed once even when fast-forward runs several steps.
- Pause input works without advancing the simulation tick.
- A rejected action leaves the world unchanged.
- Preview and execution produce the same ordered outcome.
- Exact transition ticks and stable tie-breaking order are covered.
- Calling `render()` leaves `AppState` unchanged.
- `on_code_swap()` invalidates caches produced by changeable algorithms.
- Full-capacity loops stay inside their arrays and work budget.

Framebuffer captures prove that the state is drawn correctly. State tests prove
that the game rules are correct. Use both for important player-visible behavior.

## Review a change in seven questions

Before merging a game change, answer:

1. Which player action or world event starts it?
2. Where is its persistent state?
3. What is the maximum work it can create in one tick?
4. Where does it run in the update schedule?
5. Does preview, AI, replay, and execution use the same rule result?
6. Does rendering only display the result?
7. Which focused test or capture proves it?

The structure can remain in one file or grow into focused systems. The useful
part stays the same: bind input once, update explicit state in a visible order,
and render the result without changing the game.

## Realtime network controls

Realtime games use the bounded contract in
[`realtime_networking.md`](realtime_networking.md). Raw control changes are
scheduled for future authoritative simulation ticks, submitted independently
of the tick loop, and applied in stable order at their exact due tick. Held
state persists and neutral release is explicit. Missing, delayed, duplicated,
reordered, stale, conflicting, too-far, and malformed transitions have
deterministic admission outcomes; a missing packet never stalls simulation or
rendering. Conflicting pending variants quarantine their shared identity so
arrival order cannot change the result.

The production transport carries the module's versioned control payload inside
its existing message envelope. Host-authoritative games correct clients with
snapshots after due-time loss, while deterministic-peer games must recover a
lost transition before due and require matching rates, integer state
transitions, and replay hashes. Rendering may interpolate completed states
only. Turn-based games keep their existing command path and do not use the
realtime control API.

Stasis guests import `src/stdlib/realtime_controls.stasis`. Its externs map to
stable `stasis_realtime_*` native symbols for AOT and explicit JIT adapters in
`stasis_dynload`; the host retains ownership of queues, clocks, snapshots, and
replay storage. Guests can build/submit bounded RTC1 payloads, apply scalar-array
snapshot corrections, attach authoritative hashes, and inspect resync state;
game-token restoration and replay callbacks stay in Rust. Guest reads return
the last completed control state and never advance simulation or presentation
time. The guest ABI intentionally bounds ticks and epochs to `i32::MAX`.

## World-space simulation and camera presentation

Games whose world is larger than the logical display use the shared
`src/stdlib/world_camera.stasis` presentation helper. Authoritative gameplay
always stores and updates world-space values. Render code derives an
inclusively clamped camera from a completed followed point, converts world
coordinates to the logical viewport, and emits an ordered clip pair. The
camera and render interpolation history do not feed collision, scoring,
snapshots, replay, or networking.

Small worlds center in the viewport. Large worlds scroll at the immediately
adjacent interior value and remain fixed at exact or exterior clamp values.
Hosts and guests project the same supplied completed state through the same
module rather than synchronizing camera state. Large maps use its bounded
half-open tile range, which deterministically coarsens its effective tile size
instead of truncating visible coverage, and its density-aware 64 MiB residency
contract, never a full-map texture. The current host renders at the completed
post-tick phase; a separate deterministic probe models two presentation phases
per tick until host scheduling consumes the declared presentation rate.
Detailed semantics, formulas, measured overdraw, and evidence are in
[`world_camera_viewport.md`](world_camera_viewport.md).
