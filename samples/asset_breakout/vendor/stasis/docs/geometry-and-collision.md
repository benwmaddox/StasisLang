# Geometry and collision

<!-- tags: 2d, geometry, collision, shapes, contact, deterministic-order -->

Collision reads simulation positions and shapes. Rendering projects that data.
Choose the shape and contact policy that express the game rule.

## Coordinate contract

Declare origin, axis directions, units, numeric range, and whether positions
mean centers, corners, or cells. Keep collision shape separate from visible
art. Normalize rectangles to `left <= right` and `top <= bottom`; reject or
repair invalid authored shapes at the data boundary.

| Policy | Boundary | Typical use |
| --- | --- | --- |
| Inclusive contact | Equal edges count | Walls and support surfaces |
| Strict overlap | Positive area/penetration required | Damage and triggers |
| Cell occupancy | Same canonical cell | Grid and board rules |

Test equality and the nearest value on either side. Detection identifies the
contact; game rules choose blocking, damage, bouncing, triggering, or no response.
For each pair, define symmetry, response ownership, and events per tick.

## Query, materialize, commit

1. Query active positions and shapes in a bounded, deterministic order.
2. Materialize bounded intents with stable IDs and decision data.
3. Commit in the declared order, rechecking that referenced objects are active.

Queries preserve health, cooldowns, occupancy, and lifecycle state. Choose
explicit tie-breakers for equal candidates. Define what later queries see after
removal, and avoid compaction while pending work depends on old indexes.
The [compiled example](examples/src/game_patterns.stasis) demonstrates this
flow with attacks; its [tests](examples/tests/game_patterns.test.stasis) cover
ordering, target ties, removal, and slot reuse.

## Verify boundaries

- Test separated, touching, overlap, containment, and corner cases.
- Test negative movement, world edges, and pairs with different responses.
- Test ties, full intent capacity, and removal during a pass.
- Check that query/materialize and render projection preserve gameplay state.
