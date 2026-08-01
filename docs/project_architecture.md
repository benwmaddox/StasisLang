# Stasis project architecture

This guide proposes a default structure for games and interactive simulations
written in Stasis. It is a set of suggestions, not a framework requirement. A
small project can keep the whole shape in one file. A larger project can split
the same responsibilities into focused files as real ownership seams appear.

The central flow is:

```text
host input snapshot
    -> bounded input model
    -> typed application intent
    -> ordered tick logic
    -> explicit prepared state
    -> render commands
```

Information should move forward through this flow. Rendering does not create
gameplay facts, and host callbacks do not mutate the world behind the tick
schedule.

## The default lifecycle contract

A Stasis project should make these entry point responsibilities obvious:

- `main()` initializes persistent state and requests host resources.
- `tick()` binds one host input snapshot, resolves application intent, and
  advances zero or more deterministic simulation steps.
- `render()` reads prepared state and emits render commands.
- `on_code_swap()` repairs compatible invariants and invalidates derived state
  whose algorithm may have changed.

From those entry points, a contributor should be able to answer:

1. What state persists between ticks?
2. Which fields determine simulation results?
3. How much input and simulation work can one host tick create?
4. In what exact order do systems mutate state?
5. Which values are derived, and where are they rebuilt?
6. Can rendering run without changing application state?
7. What must be repaired after a live code swap?

The goal is not immutable state. Stasis state is deliberately mutable. The goal
is visible ownership, bounded cost, and predictable mutation order.

## One root state, several kinds of truth

One global root state is a useful default. It is an inspectable ledger of
persistent memory and a clear layout boundary for live code replacement. The
root can still separate fields by authority and lifetime.

```stasis
const MAX_ACTORS: i32 = 128;
const MAX_INTENTS_PER_TICK: i32 = 8;

enum AppScreen { Menu, Playing, Paused, Result }
enum IntentKind { None, Start, Move, UseAction, Pause, Resume }

struct Actor {
    active: bool;
    x_milli: i32;
    y_milli: i32;
    hp: i32;
}

struct PlayerIntent {
    kind: IntentKind;
    actor_index: i32;
    target_x: i32;
    target_y: i32;
}

struct SimulationState {
    actors: Actor[128];
    actor_count: i32;
    tick: i32;
    revision: i32;
}

struct BoundInput {
    intents: PlayerIntent[8];
    intent_count: i32;
    overflowed: bool;
}

struct InteractionState {
    screen: AppScreen;
    selected_actor: i32;
}

struct PresentationState {
    effect_ticks: i32;
    effect_kind: i32;
}

struct DerivedState {
    prepared_revision: i32;
    dirty: bool;
    occupancy: i32[256];
    queue: i32[256];
}

struct ResourceState {
    actor_sprite: Sprite;
    font: i32;
}

struct AppState {
    simulation: SimulationState;
    input: BoundInput;
    interaction: InteractionState;
    presentation: PresentationState;
    derived: DerivedState;
    resources: ResourceState;
}

global app: AppState;
```

These categories remain useful even if a small project keeps their fields flat:

- Simulation state is authoritative. The same initial state and accepted intent
  stream should reproduce the same results.
- Bound input is the current host snapshot translated into project concepts. It
  is cleared and rebound each host tick.
- Interaction state describes screens, selection, focus, and pending actions. It
  can change while simulation is paused.
- Presentation state contains explicit visual or audio effect lifetime that must
  survive between frames. It must not decide combat or physics.
- Derived state contains rebuildable indexes, navigation fields, broad-phase
  data, and scratch buffers.
- Resource state contains host handles such as sprites, fonts, and audio. A
  resource handle must not decide a simulation result.

Projects with durable progression can add `ProfileState`. Editors may add
bounded `DocumentState` or `ScratchState`. Add a category when it expresses a
real lifetime or authority distinction, not merely to shorten a file.

## Bind input once per host tick

The host supplies one stable input snapshot for a tick. Bind that snapshot to a
small project-owned model before gameplay systems use it.

"Bound input" means both:

1. Host keys, pointers, coordinates, and edges are translated into logical
   project concepts.
2. The amount of input work one host tick can create has a fixed capacity and an
   explicit overflow rule.

```stasis
function clear_bound_input(): void {
    app.input.intent_count = 0;
    app.input.overflowed = false;
}

function push_intent(kind: IntentKind, actor_index: i32,
    target_x: i32, target_y: i32): bool {
    if (app.input.intent_count >= MAX_INTENTS_PER_TICK) {
        // Keep the earliest intents and reject later intents deterministically.
        app.input.overflowed = true;
        return false;
    }

    let index: i32 = app.input.intent_count;
    app.input.intents[index].kind = kind;
    app.input.intents[index].actor_index = actor_index;
    app.input.intents[index].target_x = target_x;
    app.input.intents[index].target_y = target_y;
    app.input.intent_count = index + 1;
    return true;
}

function bind_input(): void {
    clear_bound_input();
    if (input_pointer_count() > 0 && input_pointer_went_up(0)) {
        let logical_x: i32 = f32_to_i32(input_pointer_x_logical(0));
        let logical_y: i32 = f32_to_i32(input_pointer_y_logical(0));
        push_intent(IntentKind.Move, app.interaction.selected_actor,
            logical_x, logical_y);
    }
}
```

The input boundary may answer "which logical button contains this pointer?" It
should not answer "is this move legal?" or directly change health, inventory,
score, or world position. Those are tick logic decisions.

Use edge input for one-shot intent and held input for continuous intent. Keep
that distinction visible when the project needs it. Do not allow an OS callback
to mutate simulation at an arbitrary point in a frame.

Replay, automation, and tests should feed the same bound input or typed intent
boundary. They should not call gameplay shortcuts that physical input cannot
reach.

### Host ticks and simulation steps are different

One host tick may advance zero simulation steps while paused, one step at normal
speed, or several steps at accelerated speed. Bind and resolve input once,
outside the simulation step loop:

```stasis
function tick(): i32 {
    if (should_quit()) { return 1; }

    bind_input();
    resolve_application_intents();

    let step: i32 = 0;
    let step_count: i32 = effective_simulation_steps();
    for (step = 0; step < step_count; step = step + 1) {
        simulation_step();
    }

    prepare_derived_state();
    update_presentation_state();
    persist_profile_if_needed();
    return 0;
}
```

Putting `bind_input()` inside the speed loop would repeat a single pointer or key
edge and make controls depend on simulation speed.

Application intent and simulation time are separate. Menu navigation, pause,
editor tools, and result screens still need input while the simulation tick does
not advance. Resolve application-level intent before deciding how many
simulation steps to run.

## Keep tick logic as an explicit schedule

The simulation step is the project's mutation schedule. Keep the top-level
function short enough that its order can be reviewed without opening every
system.

```stasis
function simulation_step(): void {
    if (!simulation_can_advance()) { return; }

    app.simulation.tick = app.simulation.tick + 1;
    update_statuses();
    update_player_actions();
    update_actors();
    resolve_collisions();
    update_objectives();
    finalize_destroyed_entities();
    app.simulation.revision = app.simulation.revision + 1;
}
```

Order is behavior. This schedule states that statuses run before movement,
collisions see new positions, objectives observe resolved collisions, and entity
destruction is finalized last. Reordering those calls changes the rules even if
their bodies stay the same.

### Organize systems by owned decision

A useful system owns one meaningful transition across all relevant entities:

- movement owns position advancement;
- collision owns contact resolution;
- spawning owns entity activation and complete initialization;
- objectives own victory and completion transitions;
- navigation owns route preparation;
- presentation timing owns effect lifetime.

Avoid a generic per-entity update that hides movement, combat, spawning, and
destruction order behind dispatch. Fixed arrays and explicit system passes make
the rules and maximum work easier to inspect.

For small bounded populations, stable integer indices are useful identities.
Iterate them in stable order. If order affects targeting, collision tie breaks,
or resource claims, document that order as a rule and test it.

### Separate queries from mutations

Function vocabulary should communicate mutation expectations:

- `can_*`, `is_*`, `find_*`, and value-returning rule functions are queries;
- `update_*` advances one scheduled system;
- `apply_*`, `commit_*`, `spawn_*`, and `destroy_*` mutate authoritative state;
- `prepare_*` and `rebuild_*` mutate only named derived state;
- `draw_*` emits render commands without mutating application state.

A hypothetical query should build its proposed world in bounded scratch. Avoid
save, mutate, calculate, and restore transactions against authoritative state.
Rollback lists become incomplete as state grows and are especially dangerous
when preview, rendering, automation, or AI scoring calls the query.

### Materialize consequential outcomes

When an action is previewed, scored, rejected, replayed, or confirmed later,
give it typed input and one bounded result:

```text
PlayerIntent -> query_action -> ActionOutcome -> commit_action
```

`ActionOutcome` should contain ordered targets, costs, movement, effects, and a
failure reason. Preview reads it. Commit applies it after a final revision check.
This prevents the view, automation, and execution from each implementing the
same rule differently.

If the world can advance while an outcome is visible, store the simulation tick
or revision that produced it. Rebuild or reject stale outcomes explicitly.

### Give derived data one owner

Derived caches should declare their dependency on authoritative state:

```stasis
function mark_derived_dirty(): void {
    app.derived.dirty = true;
}

function prepare_derived_state(): void {
    if (!app.derived.dirty &&
        app.derived.prepared_revision == app.simulation.revision) { return; }

    rebuild_occupancy();
    rebuild_navigation();
    app.derived.prepared_revision = app.simulation.revision;
    app.derived.dirty = false;
}
```

Authoritative mutations mark or advance the dependency revision. One preparation
boundary rebuilds the cache. Readers do not lazily repair it.

## Render prepared state

Rendering reads simulation, interaction, presentation, derived, and resource
state and emits host render commands. It may calculate local layout or
interpolation values, but those values do not flow back into gameplay.

```stasis
function render(): i32 {
    begin_frame();
    clear(0.03, 0.04, 0.07, 1.0);

    if (app.interaction.screen == AppScreen.Menu) {
        draw_menu();
    } else if (app.interaction.screen == AppScreen.Result) {
        draw_world();
        draw_result();
    } else {
        draw_world();
        draw_interaction();
        draw_hud();
    }

    end_frame();
    return 0;
}
```

The render contract is:

- do not advance simulation time;
- do not spend resources or accept actions;
- do not create targets, collisions, or objective facts;
- do not rebuild authoritative or derived caches;
- do not load required resources lazily;
- do not use physical display size to alter logical gameplay geometry;
- do not make simulation decisions from sprite dimensions or resource handles.

Writing the host render command buffers is the intended output of `render()`.
Changing project state as a side effect of drawing is not.

It is fine to derive a draw position from two integer simulation positions and a
fixed-point progress value. Keep interpolation local. The next tick continues
from simulation state, not a rendered float.

Presentation animation should derive from an explicit simulation or
presentation tick, or advance in a named application-tick system. Avoid mutable
animation state inside `draw_*` functions.

Layout and hit testing should share named logical rectangles or computed layout
state. Duplicating coordinates between input and draw code makes controls drift
away from their touch targets.

## Use enums for state machines

Use enums for mutually exclusive states:

```stasis
enum AppScreen { Menu, Playing, Paused, Result }
enum RunPhase { Preparing, Active, Won, Lost }
enum InteractionKind { None, Selected, ActionPending }
```

Use independent fields only for facts that may truthfully coexist. A screen,
pause flag, modal flag, and result flag interpreted by ordered conditionals
usually form an implicit state machine and should become an enum.

Dispatch once at application boundaries. Input and render should interpret the
same screen state instead of maintaining separate precedence rules.

## Treat live code swap as an invariant boundary

A compatible edit can change an algorithm without changing persistent layout.
Derived values produced by the previous code can therefore be structurally valid
but semantically stale.

```stasis
function on_code_swap(): void {
    update_layout();
    app.derived.dirty = true;
    app.derived.prepared_revision = -1;
}
```

Use `on_code_swap()` to:

- recompute layout tied to host display state;
- invalidate navigation, indexes, or other derived caches;
- clamp or repair a newly documented invariant when compatible migration needs
  it.

Do not use it to simulate missed ticks, grant resources, spawn entities, or
silently restart the run. Live editing is a transaction between code versions,
not a gameplay event. A layout-incompatible edit should follow the normal reset
or migration contract instead of disguising the incompatibility.

## Suggested source structure

Stasis imports contribute declarations to one project-wide symbol graph. File
layout communicates ownership to people and tools; it is not a namespace or an
access-control mechanism. Use stable naming protocols and a small import facade
to preserve the intended boundaries.

### Small project

```text
src/
  main.stasis       lifecycle, state, rules, tick, and render
tests/
  game.test.stasis
```

Keep a small project together until there is a real ownership seam. File count
is not architecture.

### Growing project

```text
src/
  main.stasis             host lifecycle only
  project.stasis          stable import facade
  project/
    model.stasis          enums, records, constants, and root state
    input.stasis          host snapshot -> bounded intent
    rules.stasis          deterministic queries and effective values
    systems/
      movement.stasis
      collision.stasis
      objectives.stasis
      progression.stasis
    view/
      primitives.stasis
      world.stasis
      screens.stasis
      assets.stasis
    platform/
      persistence.stasis
tests/
  rules.test.stasis
  simulation.test.stasis
  input.test.stasis
  render.test.stasis
```

`main.stasis` makes the lifecycle visible. `project.stasis` keeps imports stable
while implementation files move. `model.stasis` declares facts but not update
order. `systems/` are feature slices that own transitions, not one file per
struct. `view/` reads prepared state. `platform/` isolates storage or other host
effects that should not enter deterministic simulation.

Do not build a general ECS, callback graph, service container, or event bus just
to imitate another engine. Add indirection only when a concrete feature needs a
bounded policy seam that plain data and explicit dispatch cannot express clearly.

## Data and policy

Stable tunable specifications belong in fixed records and arrays. JSON or CSV
binding can provide live authoring while packaged data supplies the same bounded
schema in builds.

Systems should ask narrow queries for effective values instead of combining
mode, difficulty, upgrades, and base values independently. Use one documented
order, for example:

```text
base specification
    -> mode replacement
    -> additive modifier
    -> percentage modifier
    -> clamp
```

Keep policy identity small and typed. Prefer an enum selected at one boundary to
many feature booleans checked throughout systems.

## Architecture contract tests

Test architectural properties alongside project rules:

- identical initial state and identical bounded input produce identical
  simulation state;
- a one-shot input edge is consumed once when several simulation steps run;
- paused application input can change interaction state without advancing the
  simulation tick;
- input overflow follows the documented deterministic policy;
- rejected intents leave authoritative simulation state unchanged;
- queries leave authoritative simulation state unchanged;
- previewed and committed outcomes agree in target order and effects;
- system boundaries behave correctly at the exact transition tick;
- stable index and neighbor tie orders have explicit cases;
- render leaves application state unchanged;
- required resources fail before interactive rendering instead of producing a
  partial project;
- `on_code_swap()` invalidates every derived cache whose implementation may
  change;
- maximum-capacity loops stay inside their arrays and work budget.

Deterministic framebuffer captures are useful evidence that input, tick state,
and rendering describe the same visible result. They complement state tests;
they do not replace them.

## Common boundary violations

Pause and reconsider the design when code starts to require:

- a render function that calls a mutating legality or cache-building query;
- input code that directly changes health, score, position, or inventory;
- wall-clock time in an authoritative simulation rule;
- a query implemented as save, mutate, calculate, and restore;
- several booleans whose precedence defines one hidden screen or mode;
- separate preview, automation, replay, and execution versions of one outcome;
- resource availability or sprite dimensions deciding simulation behavior;
- per-frame allocation or unbounded event growth hidden behind a helper;
- a reset that saves unrelated fields, clears the root, then restores them;
- a live-swap hook that invents gameplay state instead of repairing invariants.

These are usually signs that intent, state lifetime, scratch, outcome, or system
ownership has not been made explicit.

## Default review checklist

Before merging a nontrivial Stasis project change, state:

- Mapping: which input or world behavior is represented and where its state
  lives.
- Authority: which fields determine the result and which are derived or
  presentational.
- Bound: the maximum entities, intents, events, targets, or work for one tick.
- Order: where the transition appears in the explicit tick schedule.
- Projection: how rendering communicates the result without creating it.
- Extension: where one adjacent requirement fits without a fake action,
  duplicate path, or implicit state machine.
- Evidence: which focused state test, trace, replay, or capture proves the
  prediction.

The default can remain in one file or grow into focused slices. The essential
shape stays the same: bind and bound input once, mutate explicit state through an
ordered tick, and render prepared state without feeding presentation back into
simulation.
