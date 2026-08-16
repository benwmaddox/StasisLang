# Stasis language and lifecycle

<!-- tags: stasis, source, state, tick, render, lifecycle, references -->

## Model

Treat a running program as a state transition with a render projection:

```text
input/events + current state --tick--> next state --render--> visible output
```

The state is the durable fact of the program. A renderer should expose that
state; it should not become the source of game or application rules. Keep input,
mutation, and drawing at distinct boundaries so a headless scenario can exercise
the same transition without a display.

Source: `docs/agent_workflow.md`, `docs/live_cli_workspace.md`,
`docs/android_workshop_game_architecture_recommendations.md`,
`docs/android_workshop_codex_harness.md`.

## Three boundaries

| Boundary | Responsibility | Useful check |
| --- | --- | --- |
| Input | Decode events/commands into domain intent | Same command sequence gives same result |
| Update | Apply rules once for one logical tick | No draw call changes authoritative state |
| Render | Project state to sprites/text/geometry | Render parity agrees with inspected state |

Keep a small, explicit state surface. For a game this can include positions,
velocities, lives, score, phase, timers, and wave index. For an application it
can include records, selection, filters, and pending edits. Derived display data
may be recomputed from authoritative state.

## References and identity

When a value must be shared across systems, use a stable identifier/reference
rather than copying an entire object into every consumer. Keep hash domains
explicit: semantic edits use a source hash, headless scenarios can report a
simulation-state hash, and render parity uses trace/capture evidence. These are
not interchangeable. Likewise, label the scope of each identity: record ID,
asset/sprite ID, source symbol, or scenario step.

Source: `docs/semantic_edit_protocol.md`, `docs/spec.md`,
`samples/state_inspection/`, `docs/render_parity_gate.md`,
`samples/typed_sprite/`.

## Lifecycle checklist

- Define the initial state and its serialization shape.
- Define the command/event vocabulary accepted by update.
- Advance time through one canonical tick operation.
- Make rendering a read-only projection of current state.
- Expose enough state for headless inspection.
- Specify completion, failure, and reset transitions.
- Record the source path for any example used in a test or workshop.

## Common failure

If a visual test passes while state inspection disagrees, the likely defect is a
boundary leak: rendering has hidden a state error, or the inspected state is not
the state that the renderer consumes. Repair by making the transition explicit,
then compare state and render output at the same tick.

**Pseudocode:**

```text
state = initial_state()
for command in commands:
    state = update(state, command)
    assert inspect(state) == expected_inspection(command)
    frame = render(state)
    assert frame == expected_frame(command)
```

Source: `samples/headless_scenario/`, `samples/state_inspection/`,
`docs/render_parity_gate.md`.
