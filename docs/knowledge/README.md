# Stasis knowledge library

Small, executable lessons for Stasis developers. The language specification
and compiler diagnostics remain authoritative for syntax and semantics.

## A Little Stasis

Read these in order. Each lesson adds one idea and uses snippets backed by the
compiled example project.

1. [Three entry points](a-little-stasis/01-three-entry-points.md)
2. [State has owners](a-little-stasis/02-state-has-owners.md)
3. [A tick is an ordered recipe](a-little-stasis/03-a-tick-is-an-ordered-recipe.md)
4. [Input crosses a boundary](a-little-stasis/04-input-crosses-a-boundary.md)
5. [Bounded storage is policy](a-little-stasis/05-bounded-storage-is-policy.md)
6. [Query, materialize, commit](a-little-stasis/06-query-materialize-commit.md)
7. [Test systems, not balance numbers](a-little-stasis/07-test-systems-not-balance-numbers.md)
8. [Projection is not authority](a-little-stasis/08-projection-is-not-authority.md)

## Focused references

- [Geometry and collision](geometry-and-collision.md)
- [Semantic edit and validation](semantic-edit-and-validation.md)

## Executable backing

The Stasis snippets come from
[game_patterns.stasis](examples/src/game_patterns.stasis) and
[game_patterns.test.stasis](examples/tests/game_patterns.test.stasis).
Run `stasis format --check`, `stasis check`, and `stasis test` from
`examples/` after changing a snippet or its backing behavior.
