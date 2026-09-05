# Geometry and collision

<!-- tags: 2d, geometry, collision, shapes, contact, deterministic-order -->

Geometry is authoritative data plus an explicit collision policy. Rendering may
show a sprite, tile, or primitive, but collision should read the simulation's
position and shape data directly.

Stasis does not prescribe one collision API. The bundled example demonstrates
stable IDs and query/materialize/commit with attacks; the geometric policies on
this page are general design guidance rather than claims about example code.

## Coordinate and shape contract

Declare one coordinate contract for the simulation:

- origin and positive-axis directions;
- units and permitted numeric range;
- whether positions identify a center, corner, or cell;
- whether a rectangle is represented by edges or by position plus extent;
- whether coordinates are integer cells or fixed-point values.

Keep position separate from collision shape. A moving object can have a visual
position, a collision center, and one or more collision volumes without making
the renderer part of the collision query. A shape may be smaller than the
visible art, and a multi-part object may need several volumes. Name that choice
in the game rules.

Normalize shape data before testing. For an axis-aligned rectangle, keep
`left <= right` and `top <= bottom`. Reject or repair invalid authored shapes
at the data boundary; do not let every collision caller invent its own repair.

## Contact versus overlap

Choose equality deliberately and use the policy that matches the game rule.

| Policy | Boundary condition | Typical use |
| --- | --- | --- |
| Inclusive contact | Equal edges count as a hit | Solid walls, support surfaces, blocking volumes |
| Strict overlap | Positive area or penetration is required | Damage areas, trigger regions, non-solid proximity |
| Cell occupancy | The same canonical cell is occupied | Grid movement, placement, board rules |

Do not mix policies accidentally between walls, units, projectiles, and
triggers. Give a predicate a policy-specific name and test equality plus the
nearest value on both sides of the boundary.

Separate detection from response. A contact can block movement, apply damage,
open a trigger, bounce an object, or do nothing. The shape test should identify
the geometric fact; a pair policy should choose the gameplay consequence.

## Pair policy data

For systems with several object kinds, express pair behavior as data or as a
small set of explicit policy branches:

| Pair | Query rule | Commit rule |
| --- | --- | --- |
| mover / wall | Test the mover volume against the wall volume | Clamp or separate, then constrain movement |
| projectile / target | Test the projectile volume against the target volume | Apply one damage event and retire the projectile if required |
| unit / trigger | Test the unit volume against the trigger volume | Emit one bounded trigger event |
| unit / unit | Use the declared contact policy | Block, overlap, or ignore according to the rule table |

The policy table should state whether a pair is symmetric, which side owns the
response, and whether repeated contact in one tick is one event or many. Do
not infer these answers from declaration order or render order.

## Query, materialize, commit

Use three visible phases when collision can mutate state:

1. Query active positions and shapes in a bounded, deterministic order.
2. Materialize a bounded list of contact or attack intents containing stable
   object IDs and the data needed for the decision.
3. Commit intents in the declared order. Recheck that referenced objects are
   still active before applying damage, separation, scoring, or removal.

Queries must not change health, cooldowns, occupancy, or lifecycle flags.
Materialized intents make the decision inspectable and prevent a later
mutation from silently changing which pair was selected. Commit code owns
mutation and may invalidate later intents explicitly.

## Stable order and removal

Use stable numeric IDs for bounded slots. Scan slots in ascending ID order, or
define another total order in the rules. When candidates tie on range,
priority, path position, or contact side, apply the documented tie-breaker
instead of relying on container or pointer order.

Do not compact an array while a later loop still depends on its indexes unless
the compaction rule is part of the contract. An active flag with deterministic
first-free allocation preserves surviving IDs and makes reuse testable. If
removal is committed during a pass, decide whether later queries see the
removed object and keep that decision consistent across systems.

## Verification checklist

- Test separated, touching, slight-overlap, containment, and corner cases.
- Test negative movement and each world boundary.
- Test a pair with no response and a pair with a response.
- Test equal-priority candidates and verify the stable tie-breaker.
- Test one contact that defeats an object and verify no later system acts on it.
- Test that query/materialize does not mutate authoritative state.
- Test that render projection may write presentation data without changing
  authoritative gameplay state.

These are design influences, not additional Stasis semantics:

- [Handmade Hero Day 26: Introduction to Game Architecture](https://guide.handmadehero.org/code/day026/)
- [Handmade Hero Day 54: Removing the Dormant Entity Concept](https://guide.handmadehero.org/code/day054/)
- [Handmade Hero Day 63: Simulation Regions](https://guide.handmadehero.org/code/day063/)
- [Handmade Hero Day 64: Mapping Entity Indexes to Pointers](https://guide.handmadehero.org/code/day064/)
- [Handmade Hero Day 69: Pairwise Collision Rules](https://guide.handmadehero.org/code/day069/)
- [Handmade Hero Day 78: Multiple Collision Volumes](https://guide.handmadehero.org/code/day078/)
- [Mike Acton, Data-Oriented C++](https://www.youtube.com/watch?v=rX0ItVEVjHc)
- [Richard Fabian, Data-Oriented Design](https://dataorienteddesign.com/dodbook.pdf)
- [Data Locality](https://gameprogrammingpatterns.com/data-locality.html)
