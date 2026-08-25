# Stasis AI Edit Architecture Recommendations

Concise guidance for AI-assisted edits to Stasis game code, informed by Handmade Hero-style explicit runtime ownership, Age of Empires II-style deterministic simulation, and the current Stasis hot-reload workflow.

For the project-neutral input, tick, state, and render model behind these concise
recommendations, see [Stasis project architecture](project_architecture.md).

1. Keep host assumptions out of Stasis game edits. Stasis code should own simulation state, game rules, and render commands; the host supplies input, timing, storage, and a surface.
2. Treat `tick()` as the deterministic simulation step. It should orchestrate systems in a stable order, not hide gameplay in render or host callbacks.
3. Keep `render()` as a projection of current state into render commands. Do not mutate gameplay state from render except temporary render buffers.
4. Put persistent gameplay state in explicit Stasis globals or structs. Prefer plain, inspectable fields over implicit host state or derived global tick tricks.
5. Use lifecycle-local state for entity/event timing. If behavior depends on time since an entity, encounter, projectile, effect, resource, objective, or mode was created/entered, add a local counter/state field, reset it on creation/entry, and increment it during tick.
6. Prefer feature-owned data and functions. Actor changes belong with actor state/functions; resource or objective changes belong with their state/functions; cross-feature rules belong in `systems/*.stasis`.
7. As features grow, split toward files named for durable gameplay concepts, such as actors, projectiles, abilities, resources, objectives, camera, score, encounters, and `systems/<system>.stasis`.
8. Preserve hot reload when possible. Prefer function-body changes and tuning constants for iteration; warn when struct/global layout changes require `ResetRequired`.
9. Use command/event-style functions for lifecycle boundaries. Examples: `spawn_actor()`, `reset_encounter()`, `award_resource(kind, amount)`, `start_phase(mode)`, `fire_projectile()`. Creation/reset/entry functions should initialize all lifecycle state.
10. Make AI edits inspect the full feature path before writing: state definition, creation/reset path, per-tick update path, render path, and tests/simulated input path.
11. Keep mobile input abstracted through Stasis `Input` globals and helper functions so the same game logic can later run on desktop, web, or published Android.
12. Add testable invariants for behavior changes. The AI should be able to set input/state, run ticks, and inspect state/render output to verify the change.
13. Avoid broad rewrites during AI edits. Make the smallest structural change that gives the feature a clear owner and keeps the game compiling/running.
14. Prefer data-oriented clarity over deep abstractions: arrays, IDs, counters, and explicit update loops are easier to hot reload, inspect, and port.
15. Keep performance visible. New systems should avoid per-tick allocation/object churn and should fit the 60 fps budget shown by the workshop overlay.
