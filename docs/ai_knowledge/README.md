# Stasis knowledge library

<!-- tags: stasis, lifecycle, validation, deterministic-games, data-driven-design, retrieval-index -->

Permanent, repository-local notes for building and checking Stasis programs. The
documents are organized by the boundary being designed: source and lifecycle,
semantic editing, validation, data, and the fixed-tick 2D game patterns that use
those ideas.

## Routes

| Need | Read | Core question |
| --- | --- | --- |
| Understand a Stasis program | [stasis-language-and-lifecycle.md](stasis-language-and-lifecycle.md) | What is state, what advances it, and what renders it? |
| Change a workspace safely | [semantic-edit-and-validation.md](semantic-edit-and-validation.md) | How can an edit be atomic, inspectable, and recoverable? |
| Prove behavior and repair failures | [deterministic-tests-and-repair.md](deterministic-tests-and-repair.md) | Which invariants and hashes make a result reproducible? |
| Model configurable products | [data-driven-apps.md](data-driven-apps.md) | Which behavior belongs in tables/configuration/state? |
| Build a stable simulation | [fixed-tick-game-loop.md](fixed-tick-game-loop.md) | How do input, update, and render stay separated? |
| Implement 2D geometry | [geometry-and-collision.md](geometry-and-collision.md) | Which inequalities and coordinate conventions decide contact? |
| Coordinate gameplay modes | [state-machines-cooldowns-waves.md](state-machines-cooldowns-waves.md) | How do phases, timers, and waves compose? |
| Find an end-to-end pattern | [worked-patterns.md](worked-patterns.md) | What does a small implementation and repair loop look like? |

## Scope and source rule

These notes are derived from the repository's canonical workflow, workshop,
renderer, sample, scenario, inspection, performance, parity, typed-sprite, and
Pong documentation. Repository-relative paths are cited in each document. Code
marked **Stasis excerpt** is intended to be compilable repository syntax when
the cited source supports that claim; code marked **pseudocode** is a design
sketch and must be translated before compilation.

## Suggested reading order

1. [stasis-language-and-lifecycle.md](stasis-language-and-lifecycle.md)
2. [semantic-edit-and-validation.md](semantic-edit-and-validation.md)
3. [deterministic-tests-and-repair.md](deterministic-tests-and-repair.md)
4. Choose [data-driven-apps.md](data-driven-apps.md) or [fixed-tick-game-loop.md](fixed-tick-game-loop.md).
5. For games, continue through [geometry-and-collision.md](geometry-and-collision.md),
   [state-machines-cooldowns-waves.md](state-machines-cooldowns-waves.md), and
   [worked-patterns.md](worked-patterns.md).
