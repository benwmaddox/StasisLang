# Stasis knowledge library

<!-- tags: stasis, lifecycle, validation, deterministic-games, data-driven-design -->

Repository-local guidance for Stasis developers. The library covers program
lifecycle, semantic changes, deterministic validation, data-driven design, and
small bounded game simulations. It is a design and implementation reference;
the language specification and compiler diagnostics remain authoritative for
Stasis syntax and semantics.

## Routes

| Need | Read | Core question |
| --- | --- | --- |
| Understand a Stasis program | [stasis-language-and-lifecycle.md](stasis-language-and-lifecycle.md) | Where do state, commands, simulation, and rendering meet? |
| Change a workspace safely | [semantic-edit-and-validation.md](semantic-edit-and-validation.md) | How can an edit be atomic, inspectable, and recoverable? |
| Prove behavior and repair failures | [deterministic-tests-and-repair.md](deterministic-tests-and-repair.md) | Which invariants make a result reproducible? |
| Model configurable products | [data-driven-apps.md](data-driven-apps.md) | Which behavior belongs in data, state, and systems? |
| Build a logical-tick simulation | [fixed-tick-game-loop.md](fixed-tick-game-loop.md) | How do commands, ordered updates, and rendering stay separate? |
| Implement 2D geometry | [geometry-and-collision.md](geometry-and-collision.md) | Which inequalities and coordinate conventions decide contact? |
| Coordinate gameplay modes | [state-machines-cooldowns-waves.md](state-machines-cooldowns-waves.md) | How do phases, cooldowns, and waves compose? |
| Find an end-to-end pattern | [worked-patterns.md](worked-patterns.md) | What does a small implementation and repair loop look like? |

## Scope and executable examples

The library is intentionally compact. Each document explains a boundary or
invariant, gives repository references, and points to a focused implementation
pattern. It does not prescribe one architecture for every Stasis program.

The checked example at [examples/src/game_patterns.stasis](examples/src/game_patterns.stasis)
and [examples/tests/game_patterns.test.stasis](examples/tests/game_patterns.test.stasis)
is the executable source of truth for the Stasis snippets in this library.
The example demonstrates bounded slots, integer logical ticks, ordered systems,
deterministic selection, query/materialize/commit separation, a bounded wave
cursor, and a read-only gameplay render projection. Run `stasis check` and
`stasis test` from `examples/` when changing the example or reviewing
documentation.

Prefer concepts that can be stated as invariants and tested at exact logical
ticks. Keep authored definitions, mutable simulation state, commands, and
render data distinct when that separation makes ownership or validation clear.

## Suggested reading order

1. [stasis-language-and-lifecycle.md](stasis-language-and-lifecycle.md)
2. [semantic-edit-and-validation.md](semantic-edit-and-validation.md)
3. [deterministic-tests-and-repair.md](deterministic-tests-and-repair.md)
4. [data-driven-apps.md](data-driven-apps.md) and [fixed-tick-game-loop.md](fixed-tick-game-loop.md)
5. For 2D games, continue through [geometry-and-collision.md](geometry-and-collision.md),
   [state-machines-cooldowns-waves.md](state-machines-cooldowns-waves.md), and
   [worked-patterns.md](worked-patterns.md).

## Further reading and design influences

These sources influence the organization and tradeoffs described here. They
are external design material, not normative Stasis semantics.

- Handmade Hero, [Day 26: Introduction to Game Architecture](https://guide.handmadehero.org/code/day026/), for explicit input, update, and render boundaries.
- Handmade Hero, [Day 54: Removing the Dormant Entity Concept](https://guide.handmadehero.org/code/day054/), for entity removal and iteration tradeoffs.
- Handmade Hero, [Day 63: Simulation Regions](https://guide.handmadehero.org/code/day063/), for separating active simulation from stored entity data.
- Handmade Hero, [Day 64: Mapping Entity Indexes to Pointers](https://guide.handmadehero.org/code/day064/), for stable identity and lookup indirection.
- Handmade Hero, [Day 69: Pairwise Collision Rules](https://guide.handmadehero.org/code/day069/), for explicit data-driven interaction rules.
- Handmade Hero, [Day 78: Multiple Collision Volumes](https://guide.handmadehero.org/code/day078/), for separating an entity's position from its collision shapes.
- Handmade Hero, [Day 88: Push Buffer Rendering](https://guide.handmadehero.org/code/day088/), and [Day 229: Sorting Render Elements](https://guide.handmadehero.org/code/day229/), for bounded render data and explicit ordering.
- Mike Acton, [Data-Oriented Design and C++](https://www.youtube.com/watch?v=rX0ItVEVjHc), for designing around data transformations and actual access patterns.
- Richard Fabian, [Data-Oriented Design](https://dataorienteddesign.com/dodbook.pdf), for the broader design tradeoffs behind data-oriented systems.
- Bob Nystrom, [Data Locality](https://gameprogrammingpatterns.com/data-locality.html), for organizing hot data around the systems that process it.
- Mark Terrano and Paul Bettner, [1500 Archers on a 28.8: Network Programming in Age of Empires and Beyond](https://www.gamedeveloper.com/programming/1500-archers-on-a-28-8-network-programming-in-age-of-empires-and-beyond), for synchronized simulations driven by identical ordered commands.
- Chris Sawyer, [RollerCoaster Tycoon interviews](https://coasterbuzz.com/Content/rollercoaster-tycoon-chris-sawyer-interview), for a historical account of the game's design, first-principles ride physics, and tightly integrated development.
