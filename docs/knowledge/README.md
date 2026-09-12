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

## Practical examples

Each page solves one system problem for one style of game.

- [Pong: score after the ball crosses the goal](practical-examples/pong-score-after-the-ball-crosses-the-goal.md)
- [Breakout: remove one brick on a vertical hit](practical-examples/breakout-remove-one-brick-per-collision.md)
- [Platformer: land in the crossing tick](practical-examples/platformer-land-in-the-crossing-tick.md)
- [Snake: reject a reverse turn](practical-examples/snake-reject-a-reverse-turn.md)

## Focused references

- [Geometry and collision](geometry-and-collision.md)
- [Loading screens around asset IO](loading-screens.md)
- [Semantic edit and validation](semantic-edit-and-validation.md)

## Executable backing

The Stasis snippets come from the source and test files under `examples/`.
Keep both the source documentation and `vendor/stasis` immutable. Copy the
example project to a separate workspace under `build`, then initialize its
own vendor snapshot before running it. From a generated project root:

```powershell
New-Item -ItemType Directory -Force build/knowledge-examples
Copy-Item -Recurse vendor/stasis/docs/examples/* build/knowledge-examples
stasis --workspace build/knowledge-examples vendor update
stasis --workspace build/knowledge-examples format --check
stasis --workspace build/knowledge-examples check
stasis --workspace build/knowledge-examples test
```

```sh
mkdir -p build/knowledge-examples
cp -R vendor/stasis/docs/examples/. build/knowledge-examples/
stasis --workspace build/knowledge-examples vendor update
stasis --workspace build/knowledge-examples format --check
stasis --workspace build/knowledge-examples check
stasis --workspace build/knowledge-examples test
```

In a Stasis checkout, use `docs/knowledge/examples` as the copy source instead.
The example uses public `/vendor/stasis/stdlib/` imports, which remain inside
the shipped package during vendor validation. `vendor update` installs the
selected toolchain into the copied workspace's `vendor/stasis`, outside the
original fingerprinted snapshot. Run these commands only in the copy: installing
a vendor inside the source documentation would contaminate future packages.
